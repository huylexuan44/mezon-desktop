use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use livekit::track::LocalVideoTrack;
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use scap::capturer::{Capturer, Options, Resolution};
use scap::frame::{Frame, FrameType};

use crate::screen_picker::{PickedScreen, scap_target_for_pick};
use crate::video::{VideoFrameStore, bgra_to_i420, local_screen_key};

const CAPTURE_FPS: u32 = 15;
const PREVIEW_MAX_WIDTH: usize = 640;
const PREVIEW_MAX_HEIGHT: usize = 400;

pub struct ScreenStopper {
    stop: Arc<AtomicBool>,
}

impl ScreenStopper {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn start_screen(
    identity: String,
    frame_store: Arc<VideoFrameStore>,
    pick: PickedScreen,
) -> (
    ScreenStopper,
    flume::Receiver<Result<LocalVideoTrack, String>>,
) {
    let stop = Arc::new(AtomicBool::new(false));
    let (track_tx, track_rx) = flume::bounded(1);

    let thread_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("mezon-screen".into())
        .spawn(move || {
            let _guard = crate::runtime::handle().enter();

            if !scap::is_supported() {
                let _ = track_tx.send(Err("screen capture not supported".into()));
                return;
            }
            if !scap::has_permission() && !scap::request_permission() {
                let _ = track_tx.send(Err("screen recording permission denied".into()));
                return;
            }

            let capture_target = match scap_target_for_pick(pick) {
                Ok(target) => target,
                Err(e) => {
                    let _ = track_tx.send(Err(e));
                    return;
                }
            };

            let options = Options {
                fps: CAPTURE_FPS,
                target: capture_target,
                show_cursor: true,
                show_highlight: false,
                excluded_targets: None,
                output_type: FrameType::BGRAFrame,
                output_resolution: Resolution::_1080p,
                ..Default::default()
            };

            let mut capturer = match Capturer::build(options) {
                Ok(capturer) => capturer,
                Err(e) => {
                    let _ = track_tx.send(Err(format!("screen capture init failed: {e}")));
                    return;
                }
            };
            capturer.start_capture();

            let key = local_screen_key(&identity);
            let started = Instant::now();
            let mut source: Option<NativeVideoSource> = None;
            let mut src_w = 0u32;
            let mut src_h = 0u32;
            let mut sent_track = false;

            while !thread_stop.load(Ordering::Relaxed) {
                let frame = match capturer.get_next_frame() {
                    Ok(frame) => frame,
                    Err(_) => break,
                };
                let Frame::BGRA(bgra) = frame else {
                    continue;
                };
                let width = (bgra.width as u32 & !1).max(2);
                let height = (bgra.height as u32 & !1).max(2);
                if width == 0 || height == 0 || bgra.data.is_empty() {
                    continue;
                }

                if source.is_none() {
                    src_w = width;
                    src_h = height;
                    let new_source = NativeVideoSource::new(
                        VideoResolution {
                            width: src_w,
                            height: src_h,
                        },
                        true,
                    );
                    let track = LocalVideoTrack::create_video_track(
                        "screen",
                        RtcVideoSource::Native(new_source.clone()),
                    );
                    source = Some(new_source);
                    if track_tx.send(Ok(track)).is_err() {
                        return;
                    }
                    sent_track = true;
                    tracing::info!("screen capture started: {src_w}x{src_h}");
                }

                if width != src_w || height != src_h {
                    tracing::debug!(
                        "screen capture resolution changed: {src_w}x{src_h} -> {width}x{height}"
                    );
                    src_w = width;
                    src_h = height;
                }

                let row_stride = bgra.data.len() / bgra.height.max(1) as usize;
                let mut i420 = I420Buffer::new(src_w, src_h);
                {
                    let (sy, su, sv) = i420.strides();
                    let (dy, du, dv) = i420.data_mut();
                    bgra_to_i420(
                        &bgra.data,
                        src_w as usize,
                        src_h as usize,
                        row_stride,
                        dy,
                        du,
                        dv,
                        sy as usize,
                        su as usize,
                        sv as usize,
                    );
                }
                if let Some(source) = &source {
                    let frame = VideoFrame {
                        rotation: VideoRotation::VideoRotation0,
                        timestamp_us: started.elapsed().as_micros() as i64,
                        frame_metadata: None,
                        buffer: i420,
                    };
                    source.capture_frame(&frame);
                }

                let (pw, ph, preview) =
                    downscale_bgra(&bgra.data, src_w as usize, src_h as usize, row_stride);
                frame_store.publish(key, pw, ph, preview);
            }

            capturer.stop_capture();
            frame_store.remove(local_screen_key(&identity));
            if !sent_track {
                let _ = track_tx.send(Err("screen capture produced no frames".into()));
            }
            tracing::info!("screen capture stopped");
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn screen capture thread: {e}");
    }

    (ScreenStopper { stop }, track_rx)
}

fn downscale_bgra(src: &[u8], width: usize, height: usize, row_stride: usize) -> (u32, u32, Vec<u8>) {
    let scale = (PREVIEW_MAX_WIDTH as f32 / width.max(1) as f32)
        .min(PREVIEW_MAX_HEIGHT as f32 / height.max(1) as f32)
        .min(1.0);
    let dw = ((width as f32 * scale) as usize).max(1);
    let dh = ((height as f32 * scale) as usize).max(1);
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let sy = y * height / dh;
        let s_row = sy * row_stride;
        let d_row = y * dw * 4;
        for x in 0..dw {
            let sx = x * width / dw;
            let s = s_row + sx * 4;
            let d = d_row + x * 4;
            if s + 4 <= src.len() {
                out[d..d + 4].copy_from_slice(&src[s..s + 4]);
            }
        }
    }
    (dw as u32, dh as u32, out)
}
