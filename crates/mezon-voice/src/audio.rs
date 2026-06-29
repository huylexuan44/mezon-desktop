use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;

#[derive(Clone, Copy, Debug)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u32,
}

#[derive(Default)]
pub struct PlaybackMixer {
    tracks: Mutex<HashMap<u64, VecDeque<i16>>>,
}

impl PlaybackMixer {
    const MAX_BUFFERED: usize = 48_000;

    pub fn push(&self, key: u64, samples: &[i16]) {
        let mut tracks = self.tracks.lock();
        let buf = tracks.entry(key).or_default();
        buf.extend(samples.iter().copied());
        while buf.len() > Self::MAX_BUFFERED {
            buf.pop_front();
        }
    }

    pub fn remove(&self, key: u64) {
        self.tracks.lock().remove(&key);
    }

    fn mix_into(&self, out: &mut [i16]) {
        for slot in out.iter_mut() {
            *slot = 0;
        }
        let mut tracks = self.tracks.lock();
        for buf in tracks.values_mut() {
            for slot in out.iter_mut() {
                let Some(sample) = buf.pop_front() else {
                    break;
                };
                *slot =
                    (*slot as i32 + sample as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }
    }
}

enum AudioCmd {
    SetInputActive(bool),
    Shutdown,
}

pub struct AudioIo {
    ctrl_tx: flume::Sender<AudioCmd>,
    pub input_format: AudioFormat,
    pub output_format: AudioFormat,
    pub mic_rx: flume::Receiver<Vec<i16>>,
    pub mixer: Arc<PlaybackMixer>,
}

impl AudioIo {
    pub fn set_input_active(&self, active: bool) {
        let _ = self.ctrl_tx.send(AudioCmd::SetInputActive(active));
    }

    pub fn start(
        input_device_id: Option<String>,
        output_device_id: Option<String>,
    ) -> Result<Self> {
        let mixer = Arc::new(PlaybackMixer::default());
        let (mic_tx, mic_rx) = flume::bounded::<Vec<i16>>(128);
        let (ctrl_tx, ctrl_rx) = flume::unbounded::<AudioCmd>();
        let (fmt_tx, fmt_rx) = flume::bounded::<Result<(AudioFormat, AudioFormat), String>>(1);

        let mixer_for_thread = mixer.clone();
        std::thread::Builder::new()
            .name("mezon-voice-audio".into())
            .spawn(move || {
                let prepared = prepare_streams(
                    input_device_id.as_deref(),
                    output_device_id.as_deref(),
                    mixer_for_thread,
                );
                match prepared {
                    Ok((out_stream, out_fmt, input, in_supported, in_fmt)) => {
                        let _ = fmt_tx.send(Ok((in_fmt, out_fmt)));
                        // The input stream is built lazily so the macOS microphone
                        // permission prompt only appears when the user first turns
                        // their mic on, not when they join the channel.
                        let mut in_stream: Option<cpal::Stream> = None;
                        while let Ok(cmd) = ctrl_rx.recv() {
                            match cmd {
                                AudioCmd::SetInputActive(true) => {
                                    if in_stream.is_none() {
                                        request_macos_microphone_permission();
                                        match build_input(&input, &in_supported, mic_tx.clone()) {
                                            Ok(stream) => in_stream = Some(stream),
                                            Err(e) => {
                                                tracing::warn!("voice mic stream build failed: {e}")
                                            }
                                        }
                                    }
                                    if let Some(stream) = &in_stream
                                        && let Err(e) = stream.play()
                                    {
                                        tracing::warn!("voice mic stream play failed: {e}");
                                    }
                                }
                                AudioCmd::SetInputActive(false) => {
                                    if let Some(stream) = &in_stream
                                        && let Err(e) = stream.pause()
                                    {
                                        tracing::warn!("voice mic stream pause failed: {e}");
                                    }
                                }
                                AudioCmd::Shutdown => break,
                            }
                        }
                        drop(in_stream);
                        drop(out_stream);
                    }
                    Err(e) => {
                        let _ = fmt_tx.send(Err(e.to_string()));
                    }
                }
            })?;

        let formats = fmt_rx
            .recv()
            .map_err(|_| anyhow!("voice audio thread terminated before init"))?;
        let (input_format, output_format) =
            formats.map_err(|e| anyhow!("audio device init failed: {e}"))?;

        tracing::info!(
            "voice audio started: in={}Hz/{}ch out={}Hz/{}ch",
            input_format.sample_rate,
            input_format.channels,
            output_format.sample_rate,
            output_format.channels,
        );

        Ok(Self {
            ctrl_tx,
            input_format,
            output_format,
            mic_rx,
            mixer,
        })
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        let _ = self.ctrl_tx.send(AudioCmd::Shutdown);
    }
}

fn err_fn(e: cpal::StreamError) {
    tracing::warn!("voice audio stream error: {e}");
}

#[cfg(target_os = "macos")]
fn mic_authorization_status() -> i64 {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    unsafe {
        let Some(cls) = Class::get("AVCaptureDevice") else {
            return 3;
        };
        let media_type: id = NSString::alloc(nil).init_str("soun");
        msg_send![cls, authorizationStatusForMediaType: media_type]
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn microphone_denied() -> bool {
    matches!(mic_authorization_status(), 1 | 2)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn microphone_denied() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn request_macos_microphone_permission() {
    use std::time::Duration;

    use block::ConcreteBlock;
    use cocoa::base::{BOOL, NO, id, nil};
    use cocoa::foundation::NSString;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    if mic_authorization_status() != 0 {
        return;
    }
    let Some(cls) = Class::get("AVCaptureDevice") else {
        return;
    };
    let media_type: id = unsafe { NSString::alloc(nil).init_str("soun") };
    let (tx, rx) = flume::bounded::<bool>(1);
    let handler = ConcreteBlock::new(move |granted: BOOL| {
        let _ = tx.send(granted != NO);
    });
    let handler = handler.copy();
    let _: () = unsafe {
        msg_send![cls, requestAccessForMediaType: media_type completionHandler: &*handler]
    };
    let _ = rx.recv_timeout(Duration::from_secs(15));
}

#[cfg(not(target_os = "macos"))]
fn request_macos_microphone_permission() {}

fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn i16_to_f32(s: i16) -> f32 {
    s as f32 / -(i16::MIN as f32)
}

type PreparedStreams = (
    cpal::Stream,
    AudioFormat,
    cpal::Device,
    cpal::SupportedStreamConfig,
    AudioFormat,
);

fn prepare_streams(
    input_device_id: Option<&str>,
    output_device_id: Option<&str>,
    mixer: Arc<PlaybackMixer>,
) -> Result<PreparedStreams> {
    let host = cpal::default_host();

    let input = select_input(&host, input_device_id)?;
    let output = select_output(&host, output_device_id)?;

    let in_supported = input.default_input_config()?;
    let out_supported = output.default_output_config()?;

    let in_fmt = AudioFormat {
        sample_rate: in_supported.sample_rate(),
        channels: in_supported.channels() as u32,
    };
    let out_fmt = AudioFormat {
        sample_rate: out_supported.sample_rate(),
        channels: out_supported.channels() as u32,
    };

    let out_stream = build_output(&output, &out_supported, mixer)?;
    out_stream.play()?;

    Ok((out_stream, out_fmt, input, in_supported, in_fmt))
}

fn select_input(host: &cpal::Host, id: Option<&str>) -> Result<cpal::Device> {
    if let Some(id) = id
        && let Ok(mut devices) = host.input_devices()
        && let Some(device) = devices.find(|d| matches!(d.id(), Ok(did) if did.to_string() == id))
    {
        return Ok(device);
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no audio input device available"))
}

fn select_output(host: &cpal::Host, id: Option<&str>) -> Result<cpal::Device> {
    if let Some(id) = id
        && let Ok(mut devices) = host.output_devices()
        && let Some(device) = devices.find(|d| matches!(d.id(), Ok(did) if did.to_string() == id))
    {
        return Ok(device);
    }
    host.default_output_device()
        .ok_or_else(|| anyhow!("no audio output device available"))
}

fn build_input(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    mic_tx: flume::Sender<Vec<i16>>,
) -> Result<cpal::Stream> {
    let config: cpal::StreamConfig = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let tx = mic_tx;
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let pcm: Vec<i16> = data.iter().copied().map(f32_to_i16).collect();
                    let _ = tx.try_send(pcm);
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let tx = mic_tx;
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let _ = tx.try_send(data.to_vec());
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let tx = mic_tx;
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let pcm: Vec<i16> = data.iter().map(|&u| (u as i32 - 32768) as i16).collect();
                    let _ = tx.try_send(pcm);
                },
                err_fn,
                None,
            )?
        }
        other => bail!("unsupported input sample format: {other:?}"),
    };
    Ok(stream)
}

fn build_output(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    mixer: Arc<PlaybackMixer>,
) -> Result<cpal::Stream> {
    let config: cpal::StreamConfig = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let mut tmp: Vec<i16> = Vec::new();
            device.build_output_stream(
                &config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    tmp.clear();
                    tmp.resize(out.len(), 0);
                    mixer.mix_into(&mut tmp);
                    for (o, s) in out.iter_mut().zip(tmp.iter().copied()) {
                        *o = i16_to_f32(s);
                    }
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config,
            move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                mixer.mix_into(out);
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::U16 => {
            let mut tmp: Vec<i16> = Vec::new();
            device.build_output_stream(
                &config,
                move |out: &mut [u16], _: &cpal::OutputCallbackInfo| {
                    tmp.clear();
                    tmp.resize(out.len(), 0);
                    mixer.mix_into(&mut tmp);
                    for (o, s) in out.iter_mut().zip(tmp.iter().copied()) {
                        *o = (s as i32 + 32768) as u16;
                    }
                },
                err_fn,
                None,
            )?
        }
        other => bail!("unsupported output sample format: {other:?}"),
    };
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_sums_tracks_and_clamps() {
        let mixer = PlaybackMixer::default();
        mixer.push(1, &[10_000, -10_000]);
        mixer.push(2, &[30_000, -30_000]);
        let mut out = [0i16; 2];
        mixer.mix_into(&mut out);
        assert_eq!(out[0], i16::MAX);
        assert_eq!(out[1], i16::MIN);
    }

    #[test]
    fn mixer_outputs_silence_when_empty() {
        let mixer = PlaybackMixer::default();
        let mut out = [123i16; 4];
        mixer.mix_into(&mut out);
        assert_eq!(out, [0, 0, 0, 0]);
    }

    #[test]
    fn mixer_drops_oldest_when_overflowing() {
        let mixer = PlaybackMixer::default();
        let big = vec![7i16; PlaybackMixer::MAX_BUFFERED + 100];
        mixer.push(1, &big);
        let len = mixer.tracks.lock().get(&1).unwrap().len();
        assert_eq!(len, PlaybackMixer::MAX_BUFFERED);
    }
}
