use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use libwebrtc::prelude::VideoBuffer as _;
use libwebrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use libwebrtc::video_source::VideoResolution;
use libwebrtc::video_source::native::NativeVideoSource;
use libwebrtc::video_stream::native::NativeVideoStream;
use libwebrtc::video_track::RtcVideoTrack;
use parking_lot::Mutex;

use super::LIVE_PREVIEW_KEY;
use crate::runtime;
use crate::video::VideoFrameStore;

pub(super) const CANVAS_WIDTH: u32 = 1920;
pub(super) const CANVAS_HEIGHT: u32 = 1080;
pub(super) const CANVAS_FPS: u32 = 30;
const PREVIEW_WIDTH: u32 = 640;
const PREVIEW_HEIGHT: u32 = 360;
const PREVIEW_EVERY_N_TICKS: u64 = 3;
const PIP_MARGIN: u32 = 32;
const STUDIO_BLACK_Y: u8 = 16;
const STUDIO_NEUTRAL_UV: u8 = 128;

#[derive(Default)]
pub(super) struct SlotState {
    frame: Option<I420Buffer>,
    epoch: u64,
}

impl SlotState {
    fn store(&mut self, frame: I420Buffer) -> Option<I420Buffer> {
        self.frame.replace(frame)
    }

    fn clear(&mut self) -> Option<I420Buffer> {
        self.epoch = self.epoch.wrapping_add(1);
        self.frame.take()
    }

    fn take(&mut self) -> Option<(I420Buffer, u64)> {
        self.frame.take().map(|frame| (frame, self.epoch))
    }

    fn put_back(&mut self, frame: I420Buffer, taken_epoch: u64) -> Option<I420Buffer> {
        if self.frame.is_none() && self.epoch == taken_epoch {
            self.frame = Some(frame);
            None
        } else {
            Some(frame)
        }
    }
}

pub(super) type FrameSlot = Arc<Mutex<SlotState>>;

pub(super) fn clear_slot(slot: &FrameSlot) {
    let prev = slot.lock().clear();
    drop(prev);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

pub(super) struct Compositor {
    screen_slot: FrameSlot,
    camera_slot: FrameSlot,
    stop: Arc<AtomicBool>,
}

impl Compositor {
    pub fn start(frame_store: Arc<VideoFrameStore>) -> (Self, NativeVideoSource) {
        let output = NativeVideoSource::new(
            VideoResolution {
                width: CANVAS_WIDTH,
                height: CANVAS_HEIGHT,
            },
            false,
        );
        let screen_slot: FrameSlot = Arc::new(Mutex::new(SlotState::default()));
        let camera_slot: FrameSlot = Arc::new(Mutex::new(SlotState::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let task_output = output.clone();
        let task_screen = screen_slot.clone();
        let task_camera = camera_slot.clone();
        let task_stop = stop.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("mezon-live-compositor".into())
            .spawn(move || {
                run_compositor(
                    task_output,
                    task_screen,
                    task_camera,
                    frame_store,
                    task_stop,
                );
            })
        {
            tracing::error!("failed to spawn live compositor thread: {err}");
        }

        (
            Self {
                screen_slot,
                camera_slot,
                stop,
            },
            output,
        )
    }

    pub fn screen_slot(&self) -> FrameSlot {
        self.screen_slot.clone()
    }

    pub fn camera_slot(&self) -> FrameSlot {
        self.camera_slot.clone()
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        clear_slot(&self.screen_slot);
        clear_slot(&self.camera_slot);
    }
}

pub(super) fn spawn_slot_reader(
    track: RtcVideoTrack,
    slot: FrameSlot,
) -> tokio::task::JoinHandle<()> {
    runtime::runtime().spawn(async move {
        let mut stream = NativeVideoStream::new(track);
        while let Some(frame) = stream.next().await {
            let i420 = frame.buffer.to_i420();
            let prev = slot.lock().store(i420);
            drop(prev);
        }
        clear_slot(&slot);
    })
}

fn scale_slot(
    slot: &FrameSlot,
    rect_fn: fn((u32, u32), (u32, u32)) -> Rect,
    canvas_size: (u32, u32),
) -> Option<(I420Buffer, Rect)> {
    let (mut buf, taken_epoch) = slot.lock().take()?;
    let rect = rect_fn((buf.width(), buf.height()), canvas_size);
    let scaled = (rect.w > 0 && rect.h > 0).then(|| buf.scale(rect.w as i32, rect.h as i32));
    let rejected = slot.lock().put_back(buf, taken_epoch);
    drop(rejected);
    scaled.map(|s| (s, rect))
}

fn run_compositor(
    output: NativeVideoSource,
    screen_slot: FrameSlot,
    camera_slot: FrameSlot,
    frame_store: Arc<VideoFrameStore>,
    stop: Arc<AtomicBool>,
) {
    let frame_interval = Duration::from_nanos(1_000_000_000 / u64::from(CANVAS_FPS));
    let started = Instant::now();
    let mut next = started;
    let mut tick_count: u64 = 0;
    let mut preview_buf: Vec<u8> = Vec::new();
    let canvas_size = (CANVAS_WIDTH, CANVAS_HEIGHT);

    while !stop.load(Ordering::Acquire) {
        let mut canvas = new_studio_black_canvas(CANVAS_WIDTH, CANVAS_HEIGHT);

        if let Some((scaled_screen, screen_rect)) = scale_slot(&screen_slot, fit_rect, canvas_size)
        {
            blit_i420(&mut canvas, &scaled_screen, screen_rect.x, screen_rect.y);
            if let Some((scaled_cam, cam_rect)) = scale_slot(&camera_slot, pip_rect, canvas_size) {
                blit_i420(&mut canvas, &scaled_cam, cam_rect.x, cam_rect.y);
            }
        } else if let Some((scaled_cam, cam_rect)) = scale_slot(&camera_slot, fit_rect, canvas_size)
        {
            blit_i420(&mut canvas, &scaled_cam, cam_rect.x, cam_rect.y);
        }

        if tick_count.is_multiple_of(PREVIEW_EVERY_N_TICKS) {
            let preview_i420 = canvas.scale(PREVIEW_WIDTH as i32, PREVIEW_HEIGHT as i32);
            crate::screen::publish_i420_preview(
                &frame_store,
                LIVE_PREVIEW_KEY,
                &preview_i420,
                PREVIEW_WIDTH,
                PREVIEW_HEIGHT,
                &mut preview_buf,
            );
        }
        tick_count = tick_count.wrapping_add(1);

        let frame = VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: started.elapsed().as_micros() as i64,
            frame_metadata: None,
            buffer: canvas,
        };
        output.capture_frame(&frame);

        next += frame_interval;
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        } else if now - next > frame_interval {
            next = now;
        }
    }
}

fn new_studio_black_canvas(width: u32, height: u32) -> I420Buffer {
    let mut canvas = I420Buffer::new(width, height);
    let (y, u, v) = canvas.data_mut();
    y.fill(STUDIO_BLACK_Y);
    u.fill(STUDIO_NEUTRAL_UV);
    v.fill(STUDIO_NEUTRAL_UV);
    canvas
}

pub(super) fn fit_rect((src_w, src_h): (u32, u32), (canvas_w, canvas_h): (u32, u32)) -> Rect {
    let cw = canvas_w & !1;
    let ch = canvas_h & !1;
    if src_w == 0 || src_h == 0 || cw < 2 || ch < 2 {
        return Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
    }

    let lhs = u64::from(src_w) * u64::from(ch);
    let rhs = u64::from(cw) * u64::from(src_h);
    let (w, h) = if lhs == rhs {
        (cw, ch)
    } else if lhs > rhs {
        let scaled_h = ((u64::from(cw) * u64::from(src_h)) / u64::from(src_w)) as u32;
        (cw, (scaled_h & !1).clamp(2, ch))
    } else {
        let scaled_w = ((u64::from(ch) * u64::from(src_w)) / u64::from(src_h)) as u32;
        ((scaled_w & !1).clamp(2, cw), ch)
    };

    let x = ((cw - w) / 2) & !1;
    let y = ((ch - h) / 2) & !1;
    Rect { x, y, w, h }
}

pub(super) fn pip_rect((cam_w, cam_h): (u32, u32), (canvas_w, canvas_h): (u32, u32)) -> Rect {
    let cw = canvas_w & !1;
    let ch = canvas_h & !1;
    if cam_w == 0 || cam_h == 0 || cw < 8 || ch < 8 {
        return Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
    }

    let mut w = ((cw / 4) & !1).max(2);
    let mut h = (((u64::from(w) * u64::from(cam_h)) / u64::from(cam_w)) as u32 & !1).max(2);

    let max_h = (ch.saturating_sub(PIP_MARGIN * 2) & !1).max(2);
    if h > max_h {
        h = max_h;
        w = (((u64::from(h) * u64::from(cam_w)) / u64::from(cam_h)) as u32 & !1).clamp(2, cw);
    }

    let x = cw.saturating_sub(w + PIP_MARGIN) & !1;
    let y = ch.saturating_sub(h + PIP_MARGIN) & !1;
    Rect { x, y, w, h }
}

pub(super) fn blit_i420(dst: &mut I420Buffer, src: &I420Buffer, x: u32, y: u32) {
    let x = (x & !1) as usize;
    let y = (y & !1) as usize;
    let dst_w = dst.width() as usize;
    let dst_h = dst.height() as usize;
    let src_w = src.width() as usize;
    let src_h = src.height() as usize;

    if x >= dst_w || y >= dst_h || src_w == 0 || src_h == 0 {
        return;
    }

    let copy_w = src_w.min(dst_w - x) & !1;
    let copy_h = src_h.min(dst_h - y) & !1;
    if copy_w == 0 || copy_h == 0 {
        return;
    }

    let (src_sy, src_su, src_sv) = src.strides();
    let (src_y, src_u, src_v) = src.data();
    let (dst_sy, dst_su, dst_sv) = dst.strides();
    let (dst_y, dst_u, dst_v) = dst.data_mut();

    let src_sy = src_sy as usize;
    let dst_sy = dst_sy as usize;
    for row in 0..copy_h {
        let s = row * src_sy;
        let d = (y + row) * dst_sy + x;
        dst_y[d..d + copy_w].copy_from_slice(&src_y[s..s + copy_w]);
    }

    let cx = x / 2;
    let cy = y / 2;
    let cw = copy_w / 2;
    let ch = copy_h / 2;
    let src_su = src_su as usize;
    let src_sv = src_sv as usize;
    let dst_su = dst_su as usize;
    let dst_sv = dst_sv as usize;

    for row in 0..ch {
        let su = row * src_su;
        let du = (cy + row) * dst_su + cx;
        dst_u[du..du + cw].copy_from_slice(&src_u[su..su + cw]);

        let sv = row * src_sv;
        let dv = (cy + row) * dst_sv + cx;
        dst_v[dv..dv + cw].copy_from_slice(&src_v[sv..sv + cw]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_rect_handles_same_wider_taller_and_odd_sizes() {
        assert_eq!(
            fit_rect((1280, 720), (1920, 1080)),
            Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080
            }
        );

        let wider = fit_rect((1920, 800), (1920, 1080));
        assert_eq!(
            wider,
            Rect {
                x: 0,
                y: 140,
                w: 1920,
                h: 800
            }
        );

        let taller = fit_rect((1440, 1080), (1920, 1080));
        assert_eq!(
            taller,
            Rect {
                x: 240,
                y: 0,
                w: 1440,
                h: 1080
            }
        );

        let odd = fit_rect((1919, 1079), (1920, 1080));
        assert_eq!(odd.x % 2, 0);
        assert_eq!(odd.y % 2, 0);
        assert_eq!(odd.w % 2, 0);
        assert_eq!(odd.h % 2, 0);
        assert!(odd.x + odd.w <= 1920);
        assert!(odd.y + odd.h <= 1080);

        let odd_canvas = fit_rect((999, 777), (1921, 1081));
        assert_eq!(odd_canvas.x % 2, 0);
        assert_eq!(odd_canvas.y % 2, 0);
        assert_eq!(odd_canvas.w % 2, 0);
        assert_eq!(odd_canvas.h % 2, 0);
    }

    #[test]
    fn pip_rect_places_quarter_width_at_bottom_right_with_even_bounds() {
        let rect = pip_rect((640, 360), (1920, 1080));
        assert_eq!(
            rect,
            Rect {
                x: 1408,
                y: 778,
                w: 480,
                h: 270
            }
        );

        let odd = pip_rect((641, 479), (1920, 1080));
        assert_eq!(odd.x % 2, 0);
        assert_eq!(odd.y % 2, 0);
        assert_eq!(odd.w % 2, 0);
        assert_eq!(odd.h % 2, 0);
        assert_eq!(odd.w, 480);
    }

    #[test]
    fn blit_i420_copies_all_three_planes_into_target_subrect() {
        let mut dst = new_studio_black_canvas(8, 8);
        let mut src = I420Buffer::new(4, 4);
        {
            let (sy, su, sv) = src.data_mut();
            sy.fill(210);
            su.fill(44);
            sv.fill(222);
        }

        blit_i420(&mut dst, &src, 2, 4);

        let (dy_stride, du_stride, dv_stride) = dst.strides();
        let (dy, du, dv) = dst.data();

        for row in 0..8usize {
            for col in 0..8usize {
                let expected = if (4..8).contains(&row) && (2..6).contains(&col) {
                    210
                } else {
                    STUDIO_BLACK_Y
                };
                assert_eq!(dy[row * dy_stride as usize + col], expected);
            }
        }

        for crow in 0..4usize {
            for ccol in 0..4usize {
                let inside = (2..4).contains(&crow) && (1..3).contains(&ccol);
                let expected_u = if inside { 44 } else { STUDIO_NEUTRAL_UV };
                let expected_v = if inside { 222 } else { STUDIO_NEUTRAL_UV };
                assert_eq!(du[crow * du_stride as usize + ccol], expected_u);
                assert_eq!(dv[crow * dv_stride as usize + ccol], expected_v);
            }
        }
    }

    #[test]
    fn slot_state_put_back_repeats_static_keeps_newer_and_drops_stopped() {
        let make_frame = |y_val: u8| {
            let mut buf = I420Buffer::new(4, 4);
            let (y, u, v) = buf.data_mut();
            y.fill(y_val);
            u.fill(STUDIO_NEUTRAL_UV);
            v.fill(STUDIO_NEUTRAL_UV);
            buf
        };

        let mut slot = SlotState::default();
        slot.store(make_frame(50));

        let (taken, epoch) = slot.take().expect("frame should be present");
        assert!(slot.frame.is_none());
        assert!(slot.put_back(taken, epoch).is_none());
        assert_eq!(slot.frame.as_ref().map(|f| f.data().0[0]), Some(50));

        let (taken_old, epoch_old) = slot.take().expect("restored frame should be present");
        slot.store(make_frame(99));
        assert!(slot.put_back(taken_old, epoch_old).is_some());
        assert_eq!(slot.frame.as_ref().map(|f| f.data().0[0]), Some(99));

        let (taken_before_stop, epoch_before_stop) =
            slot.take().expect("newer frame should be present");
        assert!(slot.clear().is_none());
        assert!(
            slot.put_back(taken_before_stop, epoch_before_stop)
                .is_some()
        );
        assert!(slot.frame.is_none());
    }
}
