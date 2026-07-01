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
