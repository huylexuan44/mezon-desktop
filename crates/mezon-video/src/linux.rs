use std::cell::Cell;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

use crate::{PlayerError, VideoFrame};

fn ensure_gstreamer() -> Result<(), PlayerError> {
    static INIT: Once = Once::new();
    static READY: AtomicBool = AtomicBool::new(false);
    INIT.call_once(|| {
        if gst::init().is_ok() {
            READY.store(true, Ordering::SeqCst);
        }
    });
    if READY.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(PlayerError::Open)
    }
}

pub struct PlayerImpl {
    playbin: gst::Element,
    appsink: gst_app::AppSink,
    failed: Cell<bool>,
}

impl PlayerImpl {
    pub fn open(url: &str) -> Result<Self, PlayerError> {
        if url.is_empty() {
            return Err(PlayerError::InvalidUrl);
        }
        ensure_gstreamer()?;
        Self::build(url).map_err(|error| {
            tracing::warn!(target: "mezon_video", %error, "failed to build gstreamer pipeline");
            PlayerError::Open
        })
    }

    fn build(url: &str) -> Result<Self, gst::glib::BoolError> {
        let playbin = gst::ElementFactory::make("playbin").build()?;
        let caps = gst_video::VideoCapsBuilder::new()
            .format(gst_video::VideoFormat::Bgra)
            .build();
        let appsink = gst_app::AppSink::builder()
            .caps(&caps)
            .max_buffers(2)
            .drop(true)
            .build();
        playbin.set_property("uri", url);
        playbin.set_property("video-sink", &appsink);
        Ok(Self {
            playbin,
            appsink,
            failed: Cell::new(false),
        })
    }

    fn drain_bus(&self) {
        if let Some(bus) = self.playbin.bus() {
            while let Some(message) = bus.pop() {
                if let gst::message::MessageView::Error(_) = message.view() {
                    self.failed.set(true);
                }
            }
        }
    }

    pub fn copy_frame(&self) -> Option<VideoFrame> {
        self.drain_bus();
        if self.failed.get() {
            return None;
        }
        let sample = self.appsink.try_pull_sample(gst::ClockTime::ZERO)?;
        let buffer = sample.buffer()?;
        let caps = sample.caps()?;
        let info = gst_video::VideoInfo::from_caps(caps).ok()?;
        let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;

        let width = frame.width();
        let height = frame.height();
        let stride = usize::try_from(*frame.plane_stride().first()?).ok()?;
        let data = frame.plane_data(0).ok()?;
        let out = crate::frame_util::pack_bgra_rows(data, stride, width, height)?;
        crate::render_frame::bgra_to_frame(width, height, out)
    }

    pub fn play(&self) {
        if let Err(error) = self.playbin.set_state(gst::State::Playing) {
            tracing::warn!(target: "mezon_video", %error, "gstreamer set_state(Playing) failed");
        }
    }

    pub fn pause(&self) {
        if let Err(error) = self.playbin.set_state(gst::State::Paused) {
            tracing::warn!(target: "mezon_video", %error, "gstreamer set_state(Paused) failed");
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playbin.current_state() == gst::State::Playing
    }

    pub fn current_time(&self) -> f64 {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(clock_time_to_seconds)
            .unwrap_or(0.0)
    }

    pub fn duration(&self) -> f64 {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(clock_time_to_seconds)
            .unwrap_or(0.0)
    }

    pub fn seek(&self, to_seconds: f64) {
        let target = if to_seconds.is_finite() && to_seconds >= 0.0 {
            to_seconds
        } else {
            0.0
        };
        let position = gst::ClockTime::from_nseconds((target * 1_000_000_000.0) as u64);
        if let Err(error) = self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, position)
        {
            tracing::warn!(target: "mezon_video", %error, "gstreamer seek failed");
        }
    }

    pub fn set_volume(&self, volume: f32) {
        let target = (volume as f64).clamp(0.0, 1.0);
        self.playbin.set_property("volume", target);
    }

    pub fn volume(&self) -> f32 {
        self.playbin.property::<f64>("volume") as f32
    }

    pub fn set_muted(&self, muted: bool) {
        self.playbin.set_property("mute", muted);
    }

    pub fn is_muted(&self) -> bool {
        self.playbin.property::<bool>("mute")
    }

    pub fn failed(&self) -> bool {
        self.drain_bus();
        self.failed.get()
    }
}

impl Drop for PlayerImpl {
    fn drop(&mut self) {
        if let Err(error) = self.playbin.set_state(gst::State::Null) {
            tracing::warn!(target: "mezon_video", %error, "gstreamer teardown failed");
        }
    }
}

fn clock_time_to_seconds(time: gst::ClockTime) -> f64 {
    time.nseconds() as f64 / 1_000_000_000.0
}
