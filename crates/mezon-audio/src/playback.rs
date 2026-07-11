use std::cell::{Cell, RefCell};
use std::num::{NonZeroU16, NonZeroU32};
use std::rc::{Rc, Weak};

use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::sync::Arc;
use std::time::Duration;

use crate::AudioError;
use crate::decode::DecodedPcm;

thread_local! {
    static SHARED_SINK: RefCell<Weak<MixerDeviceSink>> = const { RefCell::new(Weak::new()) };
}

fn shared_sink() -> Result<Rc<MixerDeviceSink>, AudioError> {
    SHARED_SINK.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(existing) = slot.upgrade() {
            return Ok(existing);
        }
        let sink = DeviceSinkBuilder::open_default_sink()
            .map_err(|e| AudioError::Output(e.to_string()))?;
        let sink = Rc::new(sink);
        *slot = Rc::downgrade(&sink);
        Ok(sink)
    })
}

struct PcmData {
    samples: Arc<[f32]>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    duration: f64,
}

fn downmix_to_playable(samples: Arc<[f32]>, channels: usize) -> (Arc<[f32]>, u16) {
    let ch = channels.max(1);
    if ch <= 2 {
        return (samples, ch as u16);
    }
    let frames = samples.len() / ch;
    let mut mono = Vec::with_capacity(frames);
    for frame in samples.chunks_exact(ch) {
        mono.push(frame.iter().sum::<f32>() / ch as f32);
    }
    (mono.into(), 1)
}

struct SharedSamplesSource {
    samples: Arc<[f32]>,
    position: usize,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    duration: f64,
}

impl Iterator for SharedSamplesSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.samples.get(self.position).copied();
        if sample.is_some() {
            self.position += 1;
        }
        sample
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.samples.len().saturating_sub(self.position);
        (remaining, Some(remaining))
    }
}

impl Source for SharedSamplesSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.position >= self.samples.len() {
            Some(0)
        } else {
            Some(self.samples.len())
        }
    }

    fn channels(&self) -> NonZeroU16 {
        self.channels
    }

    fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(self.duration))
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let channels = self.channels.get() as usize;
        let frame = (pos.as_secs_f64() * self.sample_rate.get() as f64) as usize;
        let sample = (frame * channels).min(self.samples.len());
        self.position = sample - sample % channels;
        Ok(())
    }
}

pub struct AudioPlayer {
    _sink: Rc<MixerDeviceSink>,
    player: Player,
    data: RefCell<Option<PcmData>>,
    started: Cell<bool>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self, AudioError> {
        let sink = shared_sink()?;
        let player = Player::connect_new(sink.mixer());
        Ok(Self {
            _sink: sink,
            player,
            data: RefCell::new(None),
            started: Cell::new(false),
        })
    }

    pub fn set_data(&self, pcm: DecodedPcm) {
        let sample_rate =
            NonZeroU32::new(pcm.sample_rate).unwrap_or(NonZeroU32::new(48_000).unwrap());
        let duration = pcm.duration_secs();
        let (samples, channel_count) = downmix_to_playable(pcm.samples, pcm.channels);
        let channels = NonZeroU16::new(channel_count).unwrap_or(NonZeroU16::MIN);
        *self.data.borrow_mut() = Some(PcmData {
            samples,
            channels,
            sample_rate,
            duration,
        });
    }

    pub fn is_ready(&self) -> bool {
        self.data.borrow().is_some()
    }

    pub fn set_volume(&self, volume: f32) {
        self.player.set_volume(volume);
    }

    pub fn play(&self) {
        if let Some(data) = self.data.borrow().as_ref() {
            if self.player.empty() {
                self.player.append(SharedSamplesSource {
                    samples: Arc::clone(&data.samples),
                    position: 0,
                    channels: data.channels,
                    sample_rate: data.sample_rate,
                    duration: data.duration,
                });
            }
            self.started.set(true);
            self.player.play();
        }
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn is_playing(&self) -> bool {
        !self.player.is_paused() && !self.player.empty()
    }

    pub fn finished(&self) -> bool {
        self.started.get() && self.player.empty()
    }

    pub fn position_secs(&self) -> f64 {
        self.player.get_pos().as_secs_f64()
    }

    pub fn duration_secs(&self) -> f64 {
        self.data
            .borrow()
            .as_ref()
            .map(|d| d.duration)
            .unwrap_or(0.0)
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.player.clear();
    }
}
