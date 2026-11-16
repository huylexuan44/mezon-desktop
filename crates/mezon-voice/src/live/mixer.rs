use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use libwebrtc::audio_source::native::NativeAudioSource;
use libwebrtc::prelude::{AudioFrame, AudioSourceOptions};

use crate::audio::{AudioFormat, MicResampler, clamp_i16};
use crate::runtime;

pub(super) const LIVE_SAMPLE_RATE: u32 = 48_000;
pub(super) const LIVE_CHANNELS: u32 = 2;
const FRAMES_PER_TICK: usize = 480;
pub(super) const SAMPLES_PER_TICK: usize = FRAMES_PER_TICK * LIVE_CHANNELS as usize;
const PRIME_FRAMES: usize = 960;
const MAX_QUEUE_FRAMES: usize = 5_760;
const SENDER_CHANNEL_CAPACITY: usize = 10;

pub(super) struct SourceQueue {
    samples: VecDeque<i16>,
    channels: usize,
    primed: bool,
}

impl SourceQueue {
    pub(super) fn new_mono() -> Self {
        Self {
            samples: VecDeque::with_capacity(PRIME_FRAMES * 2),
            channels: 1,
            primed: false,
        }
    }

    pub(super) fn new_stereo() -> Self {
        Self {
            samples: VecDeque::with_capacity(PRIME_FRAMES * LIVE_CHANNELS as usize * 2),
            channels: LIVE_CHANNELS as usize,
            primed: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.samples.clear();
        self.primed = false;
    }

    pub(super) fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    #[cfg(test)]
    pub(super) fn is_primed(&self) -> bool {
        self.primed
    }

    pub(super) fn push_samples(&mut self, chunk: &[i16]) {
        let usable = chunk.len() - (chunk.len() % self.channels);
        self.samples.extend(chunk[..usable].iter().copied());
        self.trim_and_prime();
    }

    fn trim_and_prime(&mut self) {
        let frames = self.frames();
        if frames > MAX_QUEUE_FRAMES {
            let drop_frames = frames - PRIME_FRAMES;
            self.samples.drain(..drop_frames * self.channels);
        }
        if self.frames() >= PRIME_FRAMES {
            self.primed = true;
        }
    }

    fn ready_for_tick(&mut self) -> bool {
        self.trim_and_prime();
        if self.primed && self.frames() >= FRAMES_PER_TICK {
            true
        } else {
            self.primed = false;
            false
        }
    }
}

pub(super) enum MixerCmd {
    SetMicSource {
        mic_rx: flume::Receiver<Vec<i16>>,
        input_format_rx: flume::Receiver<AudioFormat>,
    },
    SetSystemSource(Option<flume::Receiver<Vec<i16>>>),
}

pub(super) struct Mixer {
    cmd_tx: flume::Sender<MixerCmd>,
    mic_on: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Mixer {
    pub fn start(mic_enabled: bool) -> (Self, NativeAudioSource) {
        let source = NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            LIVE_SAMPLE_RATE,
            LIVE_CHANNELS,
            crate::AUDIO_SOURCE_QUEUE_SIZE_MS,
        );
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let mic_on = Arc::new(AtomicBool::new(mic_enabled));

        let task_source = source.clone();
        let task_mic_on = mic_on.clone();
        let task = runtime::runtime().spawn(async move {
            run_mixer(cmd_rx, task_mic_on, task_source).await;
        });

        (
            Self {
                cmd_tx,
                mic_on,
                task,
            },
            source,
        )
    }

    pub fn set_mic_enabled(&self, on: bool) {
        self.mic_on.store(on, Ordering::Relaxed);
    }

    pub fn set_mic_source(
        &self,
        mic_rx: flume::Receiver<Vec<i16>>,
        input_format_rx: flume::Receiver<AudioFormat>,
    ) {
        let _ = self.cmd_tx.send(MixerCmd::SetMicSource {
            mic_rx,
            input_format_rx,
        });
    }

    pub fn set_system_source(&self, rx: Option<flume::Receiver<Vec<i16>>>) {
        let _ = self.cmd_tx.send(MixerCmd::SetSystemSource(rx));
    }

    pub fn stop(self) {
        self.task.abort();
    }
}

async fn run_mixer(
    cmd_rx: flume::Receiver<MixerCmd>,
    mic_on: Arc<AtomicBool>,
    source: NativeAudioSource,
) {
    struct AbortOnDrop(tokio::task::JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    let (frame_tx, frame_rx) =
        flume::bounded::<Box<[i16; SAMPLES_PER_TICK]>>(SENDER_CHANNEL_CAPACITY);
    let _sender = AbortOnDrop(runtime::runtime().spawn(async move {
        while let Ok(samples) = frame_rx.recv_async().await {
            let frame = AudioFrame {
                data: Cow::Borrowed(samples.as_slice()),
                sample_rate: LIVE_SAMPLE_RATE,
                num_channels: LIVE_CHANNELS,
                samples_per_channel: FRAMES_PER_TICK as u32,
            };
            if let Err(err) = source.capture_frame(&frame).await {
                tracing::warn!("live audio capture_frame failed: {err}");
            }
        }
    }));

    let mut mic_rx: Option<flume::Receiver<Vec<i16>>> = None;
    let mut input_format_rx: Option<flume::Receiver<AudioFormat>> = None;
    let mut sys_rx: Option<flume::Receiver<Vec<i16>>> = None;
    let mut current_in_fmt: Option<AudioFormat> = None;
    let mut resampler = MicResampler::new(LIVE_SAMPLE_RATE);
    let mut resample_buf: Vec<i16> = Vec::new();
    let mut mic_queue = SourceQueue::new_mono();
    let mut sys_queue = SourceQueue::new_stereo();

    let mut interval = tokio::time::interval(Duration::from_millis(10));

    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv_async() => {
                match cmd {
                    Ok(MixerCmd::SetMicSource { mic_rx: new_mic, input_format_rx: new_fmt }) => {
                        mic_rx = Some(new_mic);
                        input_format_rx = Some(new_fmt);
                        current_in_fmt = None;
                        resampler = MicResampler::new(LIVE_SAMPLE_RATE);
                        mic_queue.clear();
                    }
                    Ok(MixerCmd::SetSystemSource(new_sys)) => {
                        sys_rx = new_sys;
                        sys_queue.clear();
                    }
                    Err(_) => break,
                }
            }
            fmt = recv_opt(&input_format_rx) => {
                match fmt {
                    Some(new_fmt) => current_in_fmt = Some(new_fmt),
                    None => input_format_rx = None,
                }
            }
            samples = recv_opt(&mic_rx) => {
                match samples {
                    Some(chunk) => push_mic_chunk(
                        &chunk,
                        current_in_fmt,
                        mic_on.load(Ordering::Relaxed),
                        &mut resampler,
                        &mut resample_buf,
                        &mut mic_queue,
                    ),
                    None => mic_rx = None,
                }
            }
            samples = recv_opt(&sys_rx) => {
                match samples {
                    Some(chunk) => sys_queue.push_samples(&chunk),
                    None => sys_rx = None,
                }
            }
            _ = interval.tick() => {
                if let Some(rx) = &input_format_rx {
                    while let Ok(new_fmt) = rx.try_recv() {
                        current_in_fmt = Some(new_fmt);
                    }
                }
                let live_mic = mic_on.load(Ordering::Relaxed);
                if let Some(rx) = &mic_rx {
                    while let Ok(chunk) = rx.try_recv() {
                        push_mic_chunk(
                            &chunk,
                            current_in_fmt,
                            live_mic,
                            &mut resampler,
                            &mut resample_buf,
                            &mut mic_queue,
                        );
                    }
                }
                if let Some(rx) = &sys_rx {
                    while let Ok(chunk) = rx.try_recv() {
                        sys_queue.push_samples(&chunk);
                    }
                }

                let mut out = Box::new([0i16; SAMPLES_PER_TICK]);
                mix_tick(&mut mic_queue, &mut sys_queue, live_mic, &mut out);
                let _ = frame_tx.try_send(out);
            }
        }
    }
}

async fn recv_opt<T>(rx: &Option<flume::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv_async().await.ok(),
        None => std::future::pending().await,
    }
}

fn push_mic_chunk(
    chunk: &[i16],
    in_fmt: Option<AudioFormat>,
    mic_on: bool,
    resampler: &mut MicResampler,
    resample_buf: &mut Vec<i16>,
    mic_queue: &mut SourceQueue,
) {
    if !mic_on {
        mic_queue.clear();
        return;
    }
    let Some(fmt) = in_fmt else {
        return;
    };
    resample_buf.clear();
    resampler.process(chunk, fmt.sample_rate, fmt.channels, resample_buf);
    mic_queue.push_samples(resample_buf);
}

pub(super) fn mix_tick(
    mic: &mut SourceQueue,
    system: &mut SourceQueue,
    mic_on: bool,
    out: &mut [i16; SAMPLES_PER_TICK],
) {
    if !mic_on {
        mic.clear();
    }

    let use_mic = mic_on && mic.ready_for_tick();
    let use_sys = system.ready_for_tick();

    if !use_mic && !use_sys {
        out.fill(0);
        return;
    }

    for frame_idx in 0..FRAMES_PER_TICK {
        let mic_sample = if use_mic {
            mic.samples.pop_front().unwrap_or(0) as f32
        } else {
            0.0
        };
        let (sys_l, sys_r) = if use_sys {
            (
                system.samples.pop_front().unwrap_or(0) as f32,
                system.samples.pop_front().unwrap_or(0) as f32,
            )
        } else {
            (0.0, 0.0)
        };

        out[frame_idx * 2] = clamp_i16(mic_sample + sys_l);
        out[frame_idx * 2 + 1] = clamp_i16(mic_sample + sys_r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_tick_duplicates_mono_mic_to_stereo_when_primed() {
        let mut mic = SourceQueue::new_mono();
        let mut samples = vec![0i16; PRIME_FRAMES];
        samples[0] = 1200;
        samples[1] = -800;
        samples[2] = 42;
        mic.push_samples(&samples);

        let mut system = SourceQueue::new_stereo();
        let mut out = [99i16; SAMPLES_PER_TICK];

        mix_tick(&mut mic, &mut system, true, &mut out);

        assert_eq!(&out[..6], &[1200, 1200, -800, -800, 42, 42]);
        assert!(out[6..].iter().all(|&s| s == 0));
        assert_eq!(mic.frames(), PRIME_FRAMES - FRAMES_PER_TICK);
        assert!(mic.is_primed());
    }

    #[test]
    fn mix_tick_preserves_stereo_system_audio_without_downmixing() {
        let mut mic = SourceQueue::new_mono();
        let mut system = SourceQueue::new_stereo();
        let mut sys_samples = vec![0i16; PRIME_FRAMES * 2];
        sys_samples[..4].copy_from_slice(&[100, -200, 300, -400]);
        system.push_samples(&sys_samples);

        let mut out = [99i16; SAMPLES_PER_TICK];
        mix_tick(&mut mic, &mut system, true, &mut out);

        assert_eq!(&out[..4], &[100, -200, 300, -400]);
        assert!(out[4..].iter().all(|&s| s == 0));
        assert_eq!(system.frames(), PRIME_FRAMES - FRAMES_PER_TICK);
    }

    #[test]
    fn mix_tick_sums_and_clamps_to_i16_bounds() {
        let mut mic = SourceQueue::new_mono();
        let mut mic_samples = vec![0i16; PRIME_FRAMES];
        mic_samples[0] = 30_000;
        mic_samples[1] = -30_000;
        mic.push_samples(&mic_samples);

        let mut system = SourceQueue::new_stereo();
        let mut sys_samples = vec![0i16; PRIME_FRAMES * 2];
        sys_samples[..4].copy_from_slice(&[10_000, -5_000, -10_000, 5_000]);
        system.push_samples(&sys_samples);

        let mut out = [0i16; SAMPLES_PER_TICK];
        mix_tick(&mut mic, &mut system, true, &mut out);

        assert_eq!(out[0], i16::MAX);
        assert_eq!(out[1], 25_000);
        assert_eq!(out[2], i16::MIN);
        assert_eq!(out[3], -25_000);
    }

    #[test]
    fn mix_tick_drains_muted_mic_contributes_zero_and_unprimes() {
        let mut mic = SourceQueue::new_mono();
        mic.push_samples(&vec![5000i16; PRIME_FRAMES]);
        assert!(mic.is_primed());

        let mut system = SourceQueue::new_stereo();
        let mut sys_samples = vec![0i16; PRIME_FRAMES * 2];
        sys_samples[..2].copy_from_slice(&[77, -88]);
        system.push_samples(&sys_samples);

        let mut out = [99i16; SAMPLES_PER_TICK];
        mix_tick(&mut mic, &mut system, false, &mut out);

        assert_eq!(mic.frames(), 0);
        assert!(!mic.is_primed());
        assert_eq!(&out[..2], &[77, -88]);
        assert!(out[2..].iter().all(|&s| s == 0));
    }

    #[test]
    fn mix_tick_no_partial_frame_outputs_zeros_pops_nothing_and_unprimes() {
        let mut mic = SourceQueue::new_mono();
        mic.push_samples(&vec![1234i16; 300]);
        let mut system = SourceQueue::new_stereo();
        let mut out = [99i16; SAMPLES_PER_TICK];

        mix_tick(&mut mic, &mut system, true, &mut out);
        assert!(out.iter().all(|&s| s == 0));
        assert_eq!(mic.frames(), 300);
        assert!(!mic.is_primed());

        let mut primed_mic = SourceQueue::new_mono();
        primed_mic.push_samples(&vec![4000i16; PRIME_FRAMES]);
        mix_tick(&mut primed_mic, &mut system, true, &mut out);
        mix_tick(&mut primed_mic, &mut system, true, &mut out);
        assert_eq!(primed_mic.frames(), 0);
        assert!(primed_mic.is_primed());

        primed_mic.push_samples(&vec![2222i16; 300]);
        out.fill(77);
        mix_tick(&mut primed_mic, &mut system, true, &mut out);
        assert!(out.iter().all(|&s| s == 0));
        assert_eq!(primed_mic.frames(), 300);
        assert!(!primed_mic.is_primed());
    }

    #[test]
    fn mix_tick_primes_at_960_frames() {
        let mut mic = SourceQueue::new_mono();
        let mut system = SourceQueue::new_stereo();
        let mut out = [55i16; SAMPLES_PER_TICK];

        mic.push_samples(&vec![300i16; FRAMES_PER_TICK]);
        assert!(!mic.is_primed());
        mix_tick(&mut mic, &mut system, true, &mut out);
        assert!(out.iter().all(|&s| s == 0));
        assert_eq!(mic.frames(), FRAMES_PER_TICK);

        mic.push_samples(&vec![300i16; PRIME_FRAMES - FRAMES_PER_TICK - 1]);
        assert_eq!(mic.frames(), PRIME_FRAMES - 1);
        assert!(!mic.is_primed());
        out.fill(55);
        mix_tick(&mut mic, &mut system, true, &mut out);
        assert!(out.iter().all(|&s| s == 0));
        assert_eq!(mic.frames(), PRIME_FRAMES - 1);

        mic.push_samples(&[300i16]);
        assert_eq!(mic.frames(), PRIME_FRAMES);
        assert!(mic.is_primed());
        mix_tick(&mut mic, &mut system, true, &mut out);
        assert!(out.iter().all(|&s| s == 300));
        assert_eq!(mic.frames(), FRAMES_PER_TICK);
        assert!(mic.is_primed());

        out.fill(0);
        mix_tick(&mut mic, &mut system, true, &mut out);
        assert!(out.iter().all(|&s| s == 300));
        assert_eq!(mic.frames(), 0);
    }

    #[test]
    fn source_queue_latency_cap_trims_to_960_whole_frames() {
        let mut mic = SourceQueue::new_mono();
        let mono_at_cap: Vec<i16> = (0..MAX_QUEUE_FRAMES as i16).collect();
        mic.push_samples(&mono_at_cap);
        assert_eq!(mic.frames(), MAX_QUEUE_FRAMES);

        mic.push_samples(&[9999]);
        assert_eq!(mic.frames(), PRIME_FRAMES);
        assert!(mic.is_primed());
        assert_eq!(
            mic.samples.front().copied(),
            Some((MAX_QUEUE_FRAMES + 1 - PRIME_FRAMES) as i16)
        );
        assert_eq!(mic.samples.back().copied(), Some(9999));

        let mut system = SourceQueue::new_stereo();
        let mut stereo_over_cap = Vec::with_capacity((MAX_QUEUE_FRAMES + 5) * 2);
        for idx in 0..(MAX_QUEUE_FRAMES + 5) as i16 {
            stereo_over_cap.push(idx);
            stereo_over_cap.push(-idx);
        }
        system.push_samples(&stereo_over_cap);
        assert_eq!(system.frames(), PRIME_FRAMES);
        assert_eq!(system.samples.len(), PRIME_FRAMES * 2);
        let expected_first = (MAX_QUEUE_FRAMES + 5 - PRIME_FRAMES) as i16;
        assert_eq!(system.samples[0], expected_first);
        assert_eq!(system.samples[1], -expected_first);
        let expected_last = (MAX_QUEUE_FRAMES + 4) as i16;
        assert_eq!(system.samples[PRIME_FRAMES * 2 - 2], expected_last);
        assert_eq!(system.samples[PRIME_FRAMES * 2 - 1], -expected_last);
    }
}
