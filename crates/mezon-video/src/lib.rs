mod frame_util;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod render_frame;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod unsupported;
#[cfg(windows)]
#[path = "windows.rs"]
mod windows_impl;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
use unsupported as platform;
#[cfg(windows)]
use windows_impl as platform;

#[cfg(target_os = "macos")]
pub use macos::VideoFrame;
#[cfg(not(target_os = "macos"))]
pub use render_frame::VideoFrame;

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("inline video playback is not supported on this platform")]
    Unsupported,
    #[error("media url is not valid")]
    InvalidUrl,
    #[error("could not open media for playback")]
    Open,
}

/// Natural pixel dimensions of a local video plus an optional downscaled JPEG poster
/// frame, extracted from the file without full playback. The analog of the web app's
/// `captureVideoPosterFromUrl` (hidden `<video>` + canvas): both feed a message
/// attachment's `width`/`height`/`thumbnail` so a sent video renders at the right size
/// with a preview frame instead of a tiny default box.
#[derive(Debug, Clone)]
pub struct VideoProbe {
    pub width: u32,
    pub height: u32,
    pub poster_jpeg: Option<Vec<u8>>,
}

/// Read a local video's natural dimensions and a poster frame (bounded to
/// `max_poster_edge` on its longest side). Returns `None` when the path is not a
/// decodable video (or on platforms without a native probe); poster generation
/// degrades to `None` independently so dimensions still come through.
#[cfg(target_os = "macos")]
pub fn probe_video(path: &str, max_poster_edge: u32) -> Option<VideoProbe> {
    macos::probe_video(path, max_poster_edge)
}

#[cfg(not(target_os = "macos"))]
pub fn probe_video(_path: &str, _max_poster_edge: u32) -> Option<VideoProbe> {
    None
}

pub struct VideoPlayer {
    inner: platform::PlayerImpl,
}

impl VideoPlayer {
    pub fn open(url: &str, max_size: Option<(u32, u32)>) -> Result<Self, PlayerError> {
        let inner = platform::PlayerImpl::open(url, max_size)?;
        Ok(Self { inner })
    }

    pub fn copy_frame(&self) -> Option<VideoFrame> {
        self.inner.copy_frame()
    }

    pub fn play(&self) {
        self.inner.play();
    }

    pub fn pause(&self) {
        self.inner.pause();
    }

    pub fn is_playing(&self) -> bool {
        self.inner.is_playing()
    }

    pub fn current_time(&self) -> f64 {
        self.inner.current_time()
    }

    pub fn duration(&self) -> f64 {
        self.inner.duration()
    }

    pub fn seek(&self, to_seconds: f64) {
        self.inner.seek(to_seconds);
    }

    pub fn set_volume(&self, volume: f32) {
        self.inner.set_volume(volume);
    }

    pub fn volume(&self) -> f32 {
        self.inner.volume()
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.set_muted(muted);
    }

    pub fn is_muted(&self) -> bool {
        self.inner.is_muted()
    }

    pub fn failed(&self) -> bool {
        self.inner.failed()
    }
}
