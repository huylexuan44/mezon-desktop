use std::io::Cursor;

use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::{CODEC_TYPE_NULL, CODEC_TYPE_OPUS, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::AudioError;

const OPUS_SAMPLE_RATE: u32 = 48_000;
const OPUS_MAX_FRAME: usize = 5760;
const MAX_DECODED_FRAMES: usize = OPUS_SAMPLE_RATE as usize * 60 * 15;

pub struct DecodedPcm {
    pub samples: std::sync::Arc<[f32]>,
    pub channels: usize,
    pub sample_rate: u32,
}

impl DecodedPcm {
    pub fn frames(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
    }

    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.sample_rate as f64
        }
    }
}

pub fn decode_audio(bytes: Vec<u8>) -> Result<DecodedPcm, AudioError> {
    let source = Cursor::new(bytes);
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AudioError::Demux(e.to_string()))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(AudioError::NoAudioTrack)?;

    let track_id = track.id;
    let codec = track.codec_params.codec;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(1)
        .clamp(1, 2);

    if codec == CODEC_TYPE_OPUS {
        decode_opus(format.as_mut(), track_id, channels)
    } else {
        decode_symphonia(format.as_mut(), track_id)
    }
}

fn decode_opus(
    format: &mut dyn symphonia::core::formats::FormatReader,
    track_id: u32,
    channels: usize,
) -> Result<DecodedPcm, AudioError> {
    let mut decoder = OpusDecoder::new(OPUS_SAMPLE_RATE as i32, channels as i32)?;
    let mut scratch = vec![0.0f32; OPUS_MAX_FRAME * channels];
    let mut samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(AudioError::Demux(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder.decode_float(&packet.data, &mut scratch, OPUS_MAX_FRAME as i32);
        if decoded > 0 {
            samples.extend_from_slice(&scratch[..decoded as usize * channels]);
            if samples.len() / channels >= MAX_DECODED_FRAMES {
                break;
            }
        }
    }

    if samples.is_empty() {
        return Err(AudioError::Decode("opus produced no samples".into()));
    }

    Ok(DecodedPcm {
        samples: samples.into(),
        channels,
        sample_rate: OPUS_SAMPLE_RATE,
    })
}

fn decode_symphonia(
    format: &mut dyn symphonia::core::formats::FormatReader,
    track_id: u32,
) -> Result<DecodedPcm, AudioError> {
    let params = format
        .tracks()
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.codec_params.clone())
        .ok_or(AudioError::NoAudioTrack)?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| AudioError::Decode(e.to_string()))?;

    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = params.sample_rate.unwrap_or(48_000);
    let mut channels = params.channels.map(|c| c.count()).unwrap_or(1).max(1);
    let mut buffer: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(AudioError::Demux(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        };
        append_interleaved(
            decoded,
            &mut buffer,
            &mut samples,
            &mut sample_rate,
            &mut channels,
        );
        if samples.len() / channels.max(1) >= MAX_DECODED_FRAMES {
            break;
        }
    }

    if samples.is_empty() {
        return Err(AudioError::Decode("no samples produced".into()));
    }

    Ok(DecodedPcm {
        samples: samples.into(),
        channels,
        sample_rate,
    })
}

fn append_interleaved(
    decoded: AudioBufferRef<'_>,
    buffer: &mut Option<SampleBuffer<f32>>,
    samples: &mut Vec<f32>,
    sample_rate: &mut u32,
    channels: &mut usize,
) {
    let spec = *decoded.spec();
    let capacity = decoded.capacity() as u64;
    *sample_rate = spec.rate;
    *channels = spec.channels.count().max(1);
    let buf = buffer.get_or_insert_with(|| SampleBuffer::new(capacity, spec));
    buf.copy_interleaved_ref(decoded);
    samples.extend_from_slice(buf.samples());
}

struct OpusDecoder {
    inner: *mut unsafe_libopus::OpusDecoder,
}

impl OpusDecoder {
    fn new(sample_rate: i32, channels: i32) -> Result<Self, AudioError> {
        let mut error = 0i32;
        let inner =
            unsafe { unsafe_libopus::opus_decoder_create(sample_rate, channels, &mut error) };
        if inner.is_null() || error != 0 {
            return Err(AudioError::Decode(format!(
                "opus_decoder_create failed (error {error})"
            )));
        }
        Ok(Self { inner })
    }

    fn decode_float(&mut self, packet: &[u8], pcm: &mut [f32], max_frame: i32) -> i32 {
        unsafe {
            unsafe_libopus::opus_decode_float(
                self.inner,
                packet.as_ptr(),
                packet.len() as i32,
                pcm.as_mut_ptr(),
                max_frame,
                0,
            )
        }
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_decoder_destroy(self.inner) };
    }
}
