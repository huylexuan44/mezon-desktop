mod decode;
mod playback;

pub use decode::{DecodedPcm, decode_audio};
pub use playback::AudioPlayer;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("failed to demux audio: {0}")]
    Demux(String),
    #[error("failed to decode audio: {0}")]
    Decode(String),
    #[error("no audio track in stream")]
    NoAudioTrack,
    #[error("no audio output device")]
    NoOutputDevice,
    #[error("audio output error: {0}")]
    Output(String),
}
