use std::cell::{Cell, RefCell};
use std::num::{NonZeroU16, NonZeroU32};
use std::rc::{Rc, Weak};

use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

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
    samples: Vec<f32>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    duration: f64,
}

fn downmix_to_playable(samples: Vec<f32>, channels: usize) -> (Vec<f32>, u16) {
    let ch = channels.max(1);
    if ch <= 2 {
        return (samples, ch as u16);
    }
    let frames = samples.len() / ch;
    let mut mono = Vec::with_capacity(frames);
    for frame in samples.chunks_exact(ch) {
        mono.push(frame.iter().sum::<f32>() / ch as f32);
    }
    (mono, 1)
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
                self.player.append(SamplesBuffer::new(
                    data.channels,
                    data.sample_rate,
                    data.samples.clone(),
                ));
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
