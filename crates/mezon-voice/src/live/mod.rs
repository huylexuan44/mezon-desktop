mod compositor;
mod mixer;
mod sdp;
mod whip;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use futures::future::BoxFuture;
use libwebrtc::MediaType;
use libwebrtc::audio_track::RtcAudioTrack;
use libwebrtc::media_stream_track::MediaStreamTrack;
use libwebrtc::peer_connection::{OfferOptions, PeerConnection, PeerConnectionState};
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt as _;
use libwebrtc::peer_connection_factory::{
    ContinualGatheringPolicy, IceTransportsType, PeerConnectionFactory, RtcConfiguration,
};
use libwebrtc::rtp_parameters::{DegradationPreference, RtpCodecCapability, RtpEncodingParameters};
use libwebrtc::rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit};
use libwebrtc::session_description::{SdpType, SessionDescription};
use libwebrtc::video_source::native::NativeVideoSource;
use libwebrtc::video_track::{ContentHint, RtcVideoTrack};
use reqwest::Url;

use self::compositor::{Compositor, FrameSlot, clear_slot, spawn_slot_reader};
use self::mixer::Mixer;
use self::whip::WhipError;
use crate::audio::AudioIo;
use crate::camera::{self, CameraController};
use crate::record::RecordTaps;
use crate::screen::{self, ScreenStopper};
use crate::screen_audio::{self, ScreenAudioCapture};
use crate::video::VideoFrameStore;
use crate::{PickedScreen, ScreenShareMode, runtime};

pub const LIVE_PREVIEW_KEY: u64 = 0x4C49_5645_5F50_5256;

const AUDIO_MAX_BITRATE_BPS: u64 = 128_000;
const VIDEO_MAX_BITRATE_BPS: u64 = 4_000_000;
const VIDEO_MAX_FRAMERATE: f64 = 30.0;
const LIVE_STREAM_ID: &str = "mezon-live";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DISCONNECTED_GRACE: Duration = Duration::from_secs(5);
const SUSPEND_GAP: Duration = Duration::from_secs(5);
const MAX_RECONNECT_ATTEMPTS: u32 = 6;
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(16);

struct WallClockWatch {
    last_tick: SystemTime,
    suspended: bool,
}

impl WallClockWatch {
    fn new(now: SystemTime) -> Self {
        Self {
            last_tick: now,
            suspended: false,
        }
    }

    fn observe(&mut self, now: SystemTime) -> bool {
        if now
            .duration_since(self.last_tick)
            .is_ok_and(|elapsed| elapsed >= SUSPEND_GAP)
        {
            self.suspended = true;
        }
        self.last_tick = now;
        self.suspended
    }

    fn on_connected(&mut self) {
        self.suspended = false;
    }
}

struct StaleResource {
    url: Url,
    token: String,
    trust_takeover: bool,
}

#[derive(Clone)]
pub struct LiveEndpoint {
    pub whip_url: String,
    pub token: String,
}

#[derive(Clone)]
pub struct LiveEndpointRefresher(
    Arc<dyn Fn() -> BoxFuture<'static, Option<LiveEndpoint>> + Send + Sync>,
);

impl LiveEndpointRefresher {
    pub fn new<F, Fut>(mint: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<LiveEndpoint>> + Send + 'static,
    {
        Self(Arc::new(move || Box::pin(mint())))
    }

    pub(crate) async fn refresh(&self) -> Option<LiveEndpoint> {
        (self.0)().await
    }
}

impl std::fmt::Debug for LiveEndpointRefresher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveEndpointRefresher(..)")
    }
}

pub struct LiveOptions {
    pub endpoint: LiveEndpoint,
    pub refresh: LiveEndpointRefresher,
    pub local_user_id: String,
    pub screen: Option<PickedScreen>,
    pub system_audio: bool,
    pub mic_enabled: bool,
    pub input_device_id: Option<String>,
    pub output_device_id: Option<String>,
    pub camera_enabled: bool,
    pub camera_device_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveSource {
    Screen,
    SystemAudio,
    Microphone,
    Camera,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveFailure {
    ChannelBusy,
    Forbidden,
    ServerBusy,
    Unsupported,
    Network,
    Replaced,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveEvent {
    Connecting,
    Live,
    Reconnecting { attempt: u32 },
    SourceUnavailable { source: LiveSource, reason: String },
    SourceEnded { source: LiveSource },
    Failed(LiveFailure),
    Ended,
}

enum LiveCommand {
    SetMicEnabled(bool),
    SetSystemAudio(bool),
    SetCameraEnabled(bool),
    SetScreen(Option<PickedScreen>),
    SetInputDevice(Option<String>),
    SetCameraDevice(Option<String>),
    Stop,
}

pub struct LiveSession {
    cmd_tx: flume::Sender<LiveCommand>,
    events: flume::Receiver<LiveEvent>,
    frame_store: Arc<VideoFrameStore>,
}

impl LiveSession {
    pub fn start(options: LiveOptions) -> Self {
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let (evt_tx, evt_rx) = flume::unbounded();
        let frame_store = Arc::new(VideoFrameStore::default());

        let task_store = frame_store.clone();
        runtime::runtime().spawn(async move {
            session_main(options, cmd_rx, evt_tx, task_store).await;
        });

        Self {
            cmd_tx,
            events: evt_rx,
            frame_store,
        }
    }

    pub fn events(&self) -> flume::Receiver<LiveEvent> {
        self.events.clone()
    }

    pub fn frame_store(&self) -> Arc<VideoFrameStore> {
        self.frame_store.clone()
    }

    pub fn set_mic_enabled(&self, on: bool) {
        let _ = self.cmd_tx.send(LiveCommand::SetMicEnabled(on));
    }

    pub fn set_system_audio(&self, on: bool) {
        let _ = self.cmd_tx.send(LiveCommand::SetSystemAudio(on));
    }

    pub fn set_camera_enabled(&self, on: bool) {
        let _ = self.cmd_tx.send(LiveCommand::SetCameraEnabled(on));
    }

    pub fn set_screen(&self, pick: Option<PickedScreen>) {
        let _ = self.cmd_tx.send(LiveCommand::SetScreen(pick));
    }

    pub fn set_input_device(&self, id: Option<String>) {
        let _ = self.cmd_tx.send(LiveCommand::SetInputDevice(id));
    }

    pub fn set_camera_device(&self, id: Option<String>) {
        let _ = self.cmd_tx.send(LiveCommand::SetCameraDevice(id));
    }

    pub fn stop(&self) {
        let _ = self.cmd_tx.send(LiveCommand::Stop);
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ActiveScreen {
    generation: u64,
    stopper: ScreenStopper,
    reader: tokio::task::JoinHandle<()>,
    _track: RtcVideoTrack,
    slot: FrameSlot,
}

impl ActiveScreen {
    fn stop(self) {
        self.stopper.stop();
        self.reader.abort();
        clear_slot(&self.slot);
    }
}

struct ActiveCamera {
    controller: CameraController,
    reader: tokio::task::JoinHandle<()>,
    _track: RtcVideoTrack,
    slot: FrameSlot,
}

impl ActiveCamera {
    fn stop(self) {
        self.controller.stop();
        self.reader.abort();
        clear_slot(&self.slot);
    }
}

enum ConnectOutcome {
    Published {
        pc: PeerConnection,
        pc_gen: u64,
        endpoint: LiveEndpoint,
        resource_url: Url,
    },
    Failed {
        endpoint: LiveEndpoint,
        error: WhipError,
    },
}

struct ReconnectState {
    attempt: u32,
    last_server_busy: bool,
    retry_deadline: Option<tokio::time::Instant>,
    stale_resource: Option<(Url, String)>,
}

fn backoff_for_attempt(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(4);
    Duration::from_secs(1u64 << shift)
}

async fn session_main(
    options: LiveOptions,
    cmd_rx: flume::Receiver<LiveCommand>,
    evt_tx: flume::Sender<LiveEvent>,
    preview_store: Arc<VideoFrameStore>,
) {
    let LiveOptions {
        mut endpoint,
        refresh,
        local_user_id,
        screen: initial_screen,
        system_audio: initial_system_audio,
        mut mic_enabled,
        mut input_device_id,
        output_device_id,
        camera_enabled: initial_camera_enabled,
        mut camera_device_id,
    } = options;

    let client = match whip::build_client() {
        Ok(client) => client,
        Err(err) => {
            tracing::error!("failed to build live http client: {}", err.without_url());
            let _ = evt_tx.send(LiveEvent::Failed(LiveFailure::Network));
            let _ = evt_tx.send(LiveEvent::Ended);
            return;
        }
    };

    let factory = PeerConnectionFactory::default();
    let source_store = Arc::new(VideoFrameStore::default());

    let (compositor, compositor_source) = Compositor::start(preview_store.clone());
    let video_track = factory.create_video_track("mezon-live-video", compositor_source);
    video_track.set_content_hint(ContentHint::Detailed);

    let (mixer, audio_source) = Mixer::start(mic_enabled);
    let audio_track = factory.create_audio_track("mezon-live-audio", audio_source);

    let video_caps = factory.get_rtp_sender_capabilities(MediaType::Video);
    let h264_prefs = sdp::h264_preferences(&video_caps.codecs);
    if h264_prefs.is_empty() {
        tracing::error!("no usable H264 packetization-mode=1 encoder available for live stream");
        compositor.stop();
        mixer.stop();
        preview_store.remove(LIVE_PREVIEW_KEY);
        let _ = evt_tx.send(LiveEvent::Failed(LiveFailure::Unsupported));
        let _ = evt_tx.send(LiveEvent::Ended);
        return;
    }

    let (screen_tx, screen_rx) =
        flume::unbounded::<(u64, Result<(ScreenStopper, NativeVideoSource), String>)>();
    let (screen_end_tx, screen_end_rx) = flume::unbounded::<u64>();
    let (cam_tx, cam_rx) =
        flume::unbounded::<(u64, Result<(CameraController, NativeVideoSource), String>)>();
    let (sys_audio_tx, sys_audio_rx) =
        flume::unbounded::<(u64, Result<ScreenAudioCapture, String>)>();
    let (audio_io_tx, audio_io_rx) =
        flume::unbounded::<(Option<String>, Result<AudioIo, String>)>();
    let (pc_state_tx, pc_state_rx) = flume::unbounded::<(u64, PeerConnectionState)>();
    let (connect_tx, connect_rx) = flume::unbounded::<(u64, ConnectOutcome)>();

    let mut screen_gen: u64 = 0;
    let mut camera_gen: u64 = 0;
    let mut camera_opening = false;
    let mut camera_switch_pending = false;
    let mut sys_audio_gen: u64 = 0;
    let mut sys_audio_opening = false;
    let mut audio_io_opening = false;

    let mut active_screen: Option<ActiveScreen> = None;
    let mut active_camera: Option<ActiveCamera> = None;
    let mut active_sys_audio: Option<ScreenAudioCapture> = None;
    let mut audio_io: Option<AudioIo> = None;

    if let Some(pick) = initial_screen {
        screen_gen = screen_gen.wrapping_add(1);
        spawn_screen_open(
            screen_gen,
            local_user_id.clone(),
            source_store.clone(),
            pick,
            screen_tx.clone(),
            screen_end_tx.clone(),
        );
    }

    if initial_camera_enabled {
        camera_gen = camera_gen.wrapping_add(1);
        camera_opening = true;
        spawn_camera_open(
            camera_gen,
            local_user_id.clone(),
            source_store.clone(),
            camera_device_id.clone(),
            cam_tx.clone(),
        );
    }

    if initial_system_audio {
        sys_audio_gen = sys_audio_gen.wrapping_add(1);
        sys_audio_opening = true;
        spawn_sys_audio_open(sys_audio_gen, sys_audio_tx.clone());
    }

    if mic_enabled {
        audio_io_opening = true;
        spawn_audio_io_open(
            input_device_id.clone(),
            output_device_id.clone(),
            audio_io_tx.clone(),
        );
    }

    let _ = evt_tx.send(LiveEvent::Connecting);

    let mut pc: Option<PeerConnection> = None;
    let mut resource_url: Option<Url> = None;
    let mut current_pc_gen: u64 = 1;
    let mut connect_gen: u64 = 1;
    let mut connect_in_flight = true;
    let mut currently_live = false;
    let mut connect_deadline: Option<tokio::time::Instant> = None;
    let mut disconnected_deadline: Option<tokio::time::Instant> = None;
    let mut reconnect = ReconnectState {
        attempt: 0,
        last_server_busy: false,
        retry_deadline: None,
        stale_resource: None,
    };
    let mut watch = WallClockWatch::new(SystemTime::now());
    let mut watch_interval = tokio::time::interval(Duration::from_secs(1));
    watch_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    spawn_connect_attempt(
        connect_gen,
        current_pc_gen,
        false,
        None,
        factory.clone(),
        audio_track.clone(),
        video_track.clone(),
        h264_prefs.clone(),
        client.clone(),
        endpoint.clone(),
        refresh.clone(),
        pc_state_tx.clone(),
        connect_tx.clone(),
    );

    loop {
        tokio::select! {
            cmd = cmd_rx.recv_async() => {
                match cmd {
                    Ok(LiveCommand::SetMicEnabled(on)) => {
                        mic_enabled = on;
                        mixer.set_mic_enabled(on);
                        if let Some(io) = &audio_io {
                            io.set_input_active(on);
                        } else if on && !audio_io_opening {
                            audio_io_opening = true;
                            spawn_audio_io_open(
                                input_device_id.clone(),
                                output_device_id.clone(),
                                audio_io_tx.clone(),
                            );
                        }
                    }
                    Ok(LiveCommand::SetSystemAudio(true)) => {
                        if active_sys_audio.is_none() && !sys_audio_opening {
                            sys_audio_gen = sys_audio_gen.wrapping_add(1);
                            sys_audio_opening = true;
                            spawn_sys_audio_open(sys_audio_gen, sys_audio_tx.clone());
                        }
                    }
                    Ok(LiveCommand::SetSystemAudio(false)) => {
                        sys_audio_gen = sys_audio_gen.wrapping_add(1);
                        sys_audio_opening = false;
                        mixer.set_system_source(None);
                        if let Some(cap) = active_sys_audio.take() {
                            drop_sys_audio(cap);
                        }
                    }
                    Ok(LiveCommand::SetCameraEnabled(true)) => {
                        if active_camera.is_none() && !camera_opening {
                            camera_gen = camera_gen.wrapping_add(1);
                            camera_opening = true;
                            camera_switch_pending = false;
                            spawn_camera_open(
                                camera_gen,
                                local_user_id.clone(),
                                source_store.clone(),
                                camera_device_id.clone(),
                                cam_tx.clone(),
                            );
                        }
                    }
                    Ok(LiveCommand::SetCameraEnabled(false)) => {
                        camera_gen = camera_gen.wrapping_add(1);
                        camera_opening = false;
                        camera_switch_pending = false;
                        if let Some(cam) = active_camera.take() {
                            cam.stop();
                        }
                    }
                    Ok(LiveCommand::SetScreen(Some(pick))) => {
                        screen_gen = screen_gen.wrapping_add(1);
                        if let Some(scr) = active_screen.take() {
                            scr.stop();
                        }
                        spawn_screen_open(
                            screen_gen,
                            local_user_id.clone(),
                            source_store.clone(),
                            pick,
                            screen_tx.clone(),
                            screen_end_tx.clone(),
                        );
                    }
                    Ok(LiveCommand::SetScreen(None)) => {
                        screen_gen = screen_gen.wrapping_add(1);
                        if let Some(scr) = active_screen.take() {
                            scr.stop();
                        }
                    }
                    Ok(LiveCommand::SetInputDevice(id)) => {
                        input_device_id = id.clone();
                        if let Some(io) = &audio_io {
                            io.set_input_device(id);
                        }
                    }
                    Ok(LiveCommand::SetCameraDevice(id)) => {
                        if camera_device_id != id {
                            camera_device_id = id.clone();
                            if let Some(cam) = &active_camera {
                                cam.controller.switch(id);
                            } else if camera_opening {
                                camera_switch_pending = true;
                            }
                        }
                    }
                    Ok(LiveCommand::Stop) | Err(_) => {
                        break;
                    }
                }
            }
            res = screen_rx.recv_async() => {
                if let Ok((generation, outcome)) = res {
                    match outcome {
                        Ok((stopper, src)) if generation == screen_gen => {
                            if let Some(old) = active_screen.take() {
                                old.stop();
                            }
                            let track = factory.create_video_track("live-screen", src);
                            let slot = compositor.screen_slot();
                            let reader = spawn_slot_reader(track.clone(), slot.clone());
                            active_screen = Some(ActiveScreen {
                                generation,
                                stopper,
                                reader,
                                _track: track,
                                slot,
                            });
                        }
                        Ok((stopper, _)) => {
                            stopper.stop();
                        }
                        Err(reason) if generation == screen_gen => {
                            tracing::warn!("live screen source unavailable: {reason}");
                            let _ = evt_tx.send(LiveEvent::SourceUnavailable {
                                source: LiveSource::Screen,
                                reason,
                            });
                        }
                        Err(_) => {}
                    }
                }
            }
            ended_gen = screen_end_rx.recv_async() => {
                if let Ok(generation) = ended_gen
                    && active_screen
                        .as_ref()
                        .is_some_and(|a| a.generation == generation)
                    && let Some(scr) = active_screen.take()
                {
                    scr.stop();
                    tracing::info!("live screen source ended");
                    let _ = evt_tx.send(LiveEvent::SourceEnded {
                        source: LiveSource::Screen,
                    });
                }
            }
            res = cam_rx.recv_async() => {
                if let Ok((generation, outcome)) = res {
                    match outcome {
                        Ok((controller, src)) if generation == camera_gen => {
                            camera_opening = false;
                            if camera_switch_pending {
                                camera_switch_pending = false;
                                controller.switch(camera_device_id.clone());
                            }
                            if let Some(old) = active_camera.take() {
                                old.stop();
                            }
                            let track = factory.create_video_track("live-camera", src);
                            let slot = compositor.camera_slot();
                            let reader = spawn_slot_reader(track.clone(), slot.clone());
                            active_camera = Some(ActiveCamera {
                                controller,
                                reader,
                                _track: track,
                                slot,
                            });
                        }
                        Ok((controller, _)) => {
                            controller.stop();
                        }
                        Err(reason) if generation == camera_gen => {
                            camera_opening = false;
                            camera_switch_pending = false;
                            tracing::warn!("live camera source unavailable: {reason}");
                            let _ = evt_tx.send(LiveEvent::SourceUnavailable {
                                source: LiveSource::Camera,
                                reason,
                            });
                        }
                        Err(_) => {}
                    }
                }
            }
            res = sys_audio_rx.recv_async() => {
                if let Ok((generation, outcome)) = res {
                    match outcome {
                        Ok(capture) if generation == sys_audio_gen => {
                            sys_audio_opening = false;
                            mixer.set_system_source(Some(capture.rx.clone()));
                            if let Some(old) = active_sys_audio.replace(capture) {
                                drop_sys_audio(old);
                            }
                        }
                        Ok(capture) => {
                            drop_sys_audio(capture);
                        }
                        Err(reason) if generation == sys_audio_gen => {
                            sys_audio_opening = false;
                            tracing::warn!("live system audio unavailable: {reason}");
                            let _ = evt_tx.send(LiveEvent::SourceUnavailable {
                                source: LiveSource::SystemAudio,
                                reason,
                            });
                        }
                        Err(_) => {}
                    }
                }
            }
            res = audio_io_rx.recv_async() => {
                if let Ok((opened_input, outcome)) = res {
                    audio_io_opening = false;
                    match outcome {
                        Ok(io) => {
                            if opened_input != input_device_id {
                                io.set_input_device(input_device_id.clone());
                            }
                            io.set_input_active(mic_enabled);
                            mixer.set_mic_source(io.mic_rx.clone(), io.input_format_rx.clone());
                            audio_io = Some(io);
                        }
                        Err(reason) => {
                            mic_enabled = false;
                            mixer.set_mic_enabled(false);
                            tracing::warn!("live microphone unavailable: {reason}");
                            let _ = evt_tx.send(LiveEvent::SourceUnavailable {
                                source: LiveSource::Microphone,
                                reason,
                            });
                        }
                    }
                }
            }
            res = connect_rx.recv_async() => {
                if let Ok((generation, outcome)) = res {
                    if generation != connect_gen {
                        if let ConnectOutcome::Published { pc: stale_pc, endpoint: stale_ep, resource_url: stale_url, .. } = outcome {
                            stale_pc.on_connection_state_change(None);
                            stale_pc.close();
                            let client_clone = client.clone();
                            runtime::runtime().spawn(async move {
                                whip::delete(&client_clone, &stale_url, &stale_ep.token).await;
                            });
                        }
                        continue;
                    }
                    connect_in_flight = false;
                    match outcome {
                        ConnectOutcome::Published {
                            pc: new_pc,
                            pc_gen,
                            endpoint: new_endpoint,
                            resource_url: new_url,
                        } => {
                            endpoint = new_endpoint;
                            resource_url = Some(new_url);
                            current_pc_gen = pc_gen;
                            let now_state = new_pc.connection_state();
                            pc = Some(new_pc);
                            connect_deadline = Some(tokio::time::Instant::now() + CONNECT_TIMEOUT);
                            disconnected_deadline = None;
                            if now_state == PeerConnectionState::Connected {
                                connect_deadline = None;
                                reconnect.attempt = 0;
                                reconnect.last_server_busy = false;
                                watch.on_connected();
                                if !currently_live {
                                    currently_live = true;
                                    tracing::info!("live session connected");
                                    let _ = evt_tx.send(LiveEvent::Live);
                                }
                            }
                        }
                        ConnectOutcome::Failed {
                            endpoint: new_endpoint,
                            error,
                        } => {
                            endpoint = new_endpoint;
                            match error {
                                WhipError::Terminal { failure, .. } => {
                                    let _ = evt_tx.send(LiveEvent::Failed(failure));
                                    break;
                                }
                                WhipError::Retry { after, status } => {
                                    reconnect.last_server_busy = status == Some(503);
                                    if !begin_reconnect(
                                        &mut reconnect,
                                        after,
                                        &mut pc,
                                        &mut current_pc_gen,
                                        &mut resource_url,
                                        &mut currently_live,
                                        &mut connect_deadline,
                                        &mut disconnected_deadline,
                                        &endpoint.token,
                                        &evt_tx,
                                    ) {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            state_msg = pc_state_rx.recv_async() => {
                if let Ok((pc_gen, state)) = state_msg {
                    if pc_gen != current_pc_gen || pc.is_none() {
                        continue;
                    }
                    tracing::info!(?state, "live peer connection state changed");
                    match state {
                        PeerConnectionState::Connected => {
                            connect_deadline = None;
                            disconnected_deadline = None;
                            reconnect.attempt = 0;
                            reconnect.last_server_busy = false;
                            watch.on_connected();
                            if !currently_live {
                                currently_live = true;
                                let _ = evt_tx.send(LiveEvent::Live);
                            }
                        }
                        PeerConnectionState::Disconnected => {
                            if disconnected_deadline.is_none() {
                                disconnected_deadline =
                                    Some(tokio::time::Instant::now() + DISCONNECTED_GRACE);
                            }
                        }
                        PeerConnectionState::Failed => {
                            if !begin_reconnect(
                                &mut reconnect,
                                None,
                                &mut pc,
                                &mut current_pc_gen,
                                &mut resource_url,
                                &mut currently_live,
                                &mut connect_deadline,
                                &mut disconnected_deadline,
                                &endpoint.token,
                                &evt_tx,
                            ) {
                                break;
                            }
                        }
                        PeerConnectionState::New
                        | PeerConnectionState::Connecting
                        | PeerConnectionState::Closed => {}
                    }
                }
            }
            _ = sleep_until_opt(connect_deadline) => {
                connect_deadline = None;
                tracing::warn!("live peer connection did not reach Connected within 15s");
                if !begin_reconnect(
                    &mut reconnect,
                    None,
                    &mut pc,
                    &mut current_pc_gen,
                    &mut resource_url,
                    &mut currently_live,
                    &mut connect_deadline,
                    &mut disconnected_deadline,
                    &endpoint.token,
                    &evt_tx,
                ) {
                    break;
                }
            }
            _ = sleep_until_opt(disconnected_deadline) => {
                disconnected_deadline = None;
                tracing::warn!("live peer connection remained Disconnected for 5s");
                if !begin_reconnect(
                    &mut reconnect,
                    None,
                    &mut pc,
                    &mut current_pc_gen,
                    &mut resource_url,
                    &mut currently_live,
                    &mut connect_deadline,
                    &mut disconnected_deadline,
                    &endpoint.token,
                    &evt_tx,
                ) {
                    break;
                }
            }
            _ = watch_interval.tick() => {
                watch.observe(SystemTime::now());
            }
            _ = sleep_until_opt(reconnect.retry_deadline) => {
                reconnect.retry_deadline = None;
                current_pc_gen = current_pc_gen.wrapping_add(1);
                connect_gen = connect_gen.wrapping_add(1);
                connect_in_flight = true;
                let trust_takeover = !watch.observe(SystemTime::now());
                let stale_resource =
                    reconnect
                        .stale_resource
                        .take()
                        .map(|(url, token)| StaleResource {
                            url,
                            token,
                            trust_takeover,
                        });
                spawn_connect_attempt(
                    connect_gen,
                    current_pc_gen,
                    true,
                    stale_resource,
                    factory.clone(),
                    audio_track.clone(),
                    video_track.clone(),
                    h264_prefs.clone(),
                    client.clone(),
                    endpoint.clone(),
                    refresh.clone(),
                    pc_state_tx.clone(),
                    connect_tx.clone(),
                );
            }
        }
    }

    if connect_in_flight {
        let cleanup_client = client.clone();
        let expected_gen = connect_gen;
        runtime::runtime().spawn(async move {
            if let Ok((
                g,
                ConnectOutcome::Published {
                    pc: late_pc,
                    endpoint: late_ep,
                    resource_url: late_url,
                    ..
                },
            )) = connect_rx.recv_async().await
                && g == expected_gen
            {
                late_pc.on_connection_state_change(None);
                late_pc.close();
                whip::delete(&cleanup_client, &late_url, &late_ep.token).await;
            }
        });
    }

    if let Some(res_url) = resource_url.take() {
        whip::delete(&client, &res_url, &endpoint.token).await;
    }

    if let Some(active_pc) = pc.take() {
        active_pc.on_connection_state_change(None);
        active_pc.close();
    }

    if let Some(scr) = active_screen.take() {
        scr.stop();
    }
    if let Some(cam) = active_camera.take() {
        cam.stop();
    }
    if let Some(cap) = active_sys_audio.take() {
        let _ = tokio::task::spawn_blocking(move || drop(cap)).await;
    }
    crate::shutdown_audio_io(&mut audio_io).await;

    compositor.stop();
    mixer.stop();
    preview_store.remove(LIVE_PREVIEW_KEY);

    let _ = evt_tx.send(LiveEvent::Ended);
}

#[allow(clippy::too_many_arguments)]
fn begin_reconnect(
    reconnect: &mut ReconnectState,
    retry_after: Option<Duration>,
    pc: &mut Option<PeerConnection>,
    current_pc_gen: &mut u64,
    resource_url: &mut Option<Url>,
    currently_live: &mut bool,
    connect_deadline: &mut Option<tokio::time::Instant>,
    disconnected_deadline: &mut Option<tokio::time::Instant>,
    token: &str,
    evt_tx: &flume::Sender<LiveEvent>,
) -> bool {
    *currently_live = false;
    *connect_deadline = None;
    *disconnected_deadline = None;
    *current_pc_gen = current_pc_gen.wrapping_add(1);

    if let Some(old_pc) = pc.take() {
        old_pc.on_connection_state_change(None);
        old_pc.close();
    }

    if let Some(old_url) = resource_url.take() {
        reconnect.stale_resource = Some((old_url, token.to_owned()));
    }

    reconnect.attempt = reconnect.attempt.saturating_add(1);
    if reconnect.attempt > MAX_RECONNECT_ATTEMPTS {
        let failure = if reconnect.last_server_busy {
            LiveFailure::ServerBusy
        } else {
            LiveFailure::Network
        };
        let _ = evt_tx.send(LiveEvent::Failed(failure));
        return false;
    }

    let delay = retry_after
        .map(|d| d.min(MAX_RETRY_BACKOFF))
        .unwrap_or_else(|| backoff_for_attempt(reconnect.attempt));
    reconnect.retry_deadline = Some(tokio::time::Instant::now() + delay);

    tracing::info!(
        attempt = reconnect.attempt,
        delay_ms = delay.as_millis() as u64,
        "scheduling live session reconnect"
    );
    let _ = evt_tx.send(LiveEvent::Reconnecting {
        attempt: reconnect.attempt,
    });
    true
}

#[allow(clippy::too_many_arguments)]
fn spawn_connect_attempt(
    connect_gen: u64,
    pc_gen: u64,
    is_reconnect: bool,
    stale_resource: Option<StaleResource>,
    factory: PeerConnectionFactory,
    audio_track: RtcAudioTrack,
    video_track: RtcVideoTrack,
    h264_prefs: Vec<RtpCodecCapability>,
    client: reqwest::Client,
    mut endpoint: LiveEndpoint,
    refresh: LiveEndpointRefresher,
    pc_state_tx: flume::Sender<(u64, PeerConnectionState)>,
    connect_tx: flume::Sender<(u64, ConnectOutcome)>,
) {
    runtime::runtime().spawn(async move {
        if let Some(stale) = stale_resource {
            let delete_status = whip::delete(&client, &stale.url, &stale.token).await;
            if whip::was_replaced(delete_status, stale.trust_takeover) {
                tracing::warn!(
                    "live session was replaced by another publisher on this account; not reconnecting"
                );
                let _ = connect_tx
                    .send_async((
                        connect_gen,
                        ConnectOutcome::Failed {
                            endpoint,
                            error: WhipError::Terminal {
                                failure: LiveFailure::Replaced,
                                status: 404,
                            },
                        },
                    ))
                    .await;
                return;
            }
            if delete_status == Some(404) {
                tracing::info!(
                    "previous live session already expired after a suspension; reconnecting"
                );
            }
        }

        if is_reconnect
            && let Some(fresh) = refresh.refresh().await
            && !fresh.whip_url.is_empty()
            && !fresh.token.is_empty()
        {
            endpoint = fresh;
        }

        let built = build_pc_and_offer(
            &factory,
            &audio_track,
            &video_track,
            h264_prefs,
            pc_gen,
            pc_state_tx,
        )
        .await;

        let (pc, offer_sdp) = match built {
            Ok(pair) => pair,
            Err(error) => {
                let _ = connect_tx
                    .send_async((connect_gen, ConnectOutcome::Failed { endpoint, error }))
                    .await;
                return;
            }
        };

        let mut published = whip::publish(&client, &endpoint, &offer_sdp).await;
        if is_reconnect
            && matches!(
                published,
                Err(WhipError::Terminal {
                    failure: LiveFailure::Forbidden,
                    status: 401
                })
            )
            && let Some(fresh) = refresh.refresh().await
            && !fresh.whip_url.is_empty()
            && !fresh.token.is_empty()
        {
            endpoint = fresh;
            published = whip::publish(&client, &endpoint, &offer_sdp).await;
        }

        let whip_session = match published {
            Ok(session) => session,
            Err(error) => {
                pc.on_connection_state_change(None);
                pc.close();
                let _ = connect_tx
                    .send_async((connect_gen, ConnectOutcome::Failed { endpoint, error }))
                    .await;
                return;
            }
        };

        let answer_desc = match SessionDescription::parse(&whip_session.answer, SdpType::Answer) {
            Ok(desc) => desc,
            Err(err) => {
                tracing::warn!("whip answer SDP parse failed: {}", err.description);
                pc.on_connection_state_change(None);
                pc.close();
                whip::delete(&client, &whip_session.resource_url, &endpoint.token).await;
                let _ = connect_tx
                    .send_async((
                        connect_gen,
                        ConnectOutcome::Failed {
                            endpoint,
                            error: WhipError::Retry {
                                after: None,
                                status: None,
                            },
                        },
                    ))
                    .await;
                return;
            }
        };

        if let Err(err) = pc.set_remote_description(answer_desc).await {
            tracing::warn!("live set_remote_description failed: {err}");
            pc.on_connection_state_change(None);
            pc.close();
            whip::delete(&client, &whip_session.resource_url, &endpoint.token).await;
            let _ = connect_tx
                .send_async((
                    connect_gen,
                    ConnectOutcome::Failed {
                        endpoint,
                        error: WhipError::Retry {
                            after: None,
                            status: None,
                        },
                    },
                ))
                .await;
            return;
        }

        let _ = connect_tx
            .send_async((
                connect_gen,
                ConnectOutcome::Published {
                    pc,
                    pc_gen,
                    endpoint,
                    resource_url: whip_session.resource_url,
                },
            ))
            .await;
    });
}

async fn build_pc_and_offer(
    factory: &PeerConnectionFactory,
    audio_track: &RtcAudioTrack,
    video_track: &RtcVideoTrack,
    h264_prefs: Vec<RtpCodecCapability>,
    pc_gen: u64,
    pc_state_tx: flume::Sender<(u64, PeerConnectionState)>,
) -> Result<(PeerConnection, String), WhipError> {
    let config = RtcConfiguration {
        ice_servers: Vec::new(),
        continual_gathering_policy: ContinualGatheringPolicy::GatherOnce,
        ice_transport_type: IceTransportsType::All,
    };

    let pc = factory.create_peer_connection(config).map_err(|err| {
        tracing::error!("live create_peer_connection failed: {err}");
        WhipError::Retry {
            after: None,
            status: None,
        }
    })?;

    pc.on_connection_state_change(Some(Box::new(move |state| {
        let _ = pc_state_tx.send((pc_gen, state));
    })));

    let audio_transceiver = pc
        .add_transceiver(
            MediaStreamTrack::Audio(audio_track.clone()),
            RtpTransceiverInit {
                direction: RtpTransceiverDirection::SendOnly,
                stream_ids: vec![LIVE_STREAM_ID.into()],
                send_encodings: vec![RtpEncodingParameters {
                    max_bitrate: Some(AUDIO_MAX_BITRATE_BPS),
                    ..Default::default()
                }],
            },
        )
        .map_err(|err| {
            tracing::error!("live audio add_transceiver failed: {err}");
            pc.on_connection_state_change(None);
            pc.close();
            WhipError::Retry {
                after: None,
                status: None,
            }
        })?;

    let audio_caps = factory.get_rtp_sender_capabilities(MediaType::Audio);
    let opus_prefs: Vec<RtpCodecCapability> = audio_caps
        .codecs
        .into_iter()
        .filter(|cap| cap.mime_type.eq_ignore_ascii_case("audio/opus"))
        .collect();
    if !opus_prefs.is_empty()
        && let Err(err) = audio_transceiver.set_codec_preferences(opus_prefs)
    {
        tracing::warn!("live opus set_codec_preferences failed: {err}");
    }

    let video_transceiver = pc
        .add_transceiver(
            MediaStreamTrack::Video(video_track.clone()),
            RtpTransceiverInit {
                direction: RtpTransceiverDirection::SendOnly,
                stream_ids: vec![LIVE_STREAM_ID.into()],
                send_encodings: vec![RtpEncodingParameters {
                    max_bitrate: Some(VIDEO_MAX_BITRATE_BPS),
                    max_framerate: Some(VIDEO_MAX_FRAMERATE),
                    ..Default::default()
                }],
            },
        )
        .map_err(|err| {
            tracing::error!("live video add_transceiver failed: {err}");
            pc.on_connection_state_change(None);
            pc.close();
            WhipError::Retry {
                after: None,
                status: None,
            }
        })?;

    let video_sender = video_transceiver.sender();
    let mut params = video_sender.parameters();
    params.set_degradation_preference(DegradationPreference::MaintainResolution);
    if let Err(err) = video_sender.set_parameters(params) {
        tracing::warn!("live MaintainResolution set_parameters failed: {err}");
    }

    video_transceiver
        .set_codec_preferences(h264_prefs)
        .map_err(|err| {
            tracing::error!("live H264 set_codec_preferences failed: {err}");
            pc.on_connection_state_change(None);
            pc.close();
            WhipError::Terminal {
                failure: LiveFailure::Unsupported,
                status: 406,
            }
        })?;

    let raw_offer = pc
        .create_offer(OfferOptions::default())
        .await
        .map_err(|err| {
            tracing::error!("live create_offer failed: {err}");
            pc.on_connection_state_change(None);
            pc.close();
            WhipError::Retry {
                after: None,
                status: None,
            }
        })?;

    let munged_sdp = sdp::munge_offer(&raw_offer.to_string());
    let local_desc = SessionDescription::parse(&munged_sdp, SdpType::Offer).map_err(|err| {
        tracing::error!("live munged offer parse failed: {}", err.description);
        pc.on_connection_state_change(None);
        pc.close();
        WhipError::Terminal {
            failure: LiveFailure::Unsupported,
            status: 400,
        }
    })?;

    pc.set_local_description(local_desc).await.map_err(|err| {
        tracing::error!("live set_local_description failed: {err}");
        pc.on_connection_state_change(None);
        pc.close();
        WhipError::Retry {
            after: None,
            status: None,
        }
    })?;

    Ok((pc, munged_sdp))
}

fn spawn_screen_open(
    generation: u64,
    identity: String,
    frame_store: Arc<VideoFrameStore>,
    pick: PickedScreen,
    tx: flume::Sender<(u64, Result<(ScreenStopper, NativeVideoSource), String>)>,
    screen_end_tx: flume::Sender<u64>,
) {
    runtime::runtime().spawn(async move {
        let (stopper, rx) = screen::start_screen(
            identity,
            frame_store,
            Arc::new(AtomicBool::new(false)),
            pick,
            ScreenShareMode::Video.capture_fps(),
            0,
        );
        let outcome = match rx.recv_async().await {
            Ok(Ok((source, _w, _h))) => Ok((stopper, source)),
            Ok(Err(err)) => Err(err),
            Err(_) => Err("screen capture thread exited".into()),
        };
        let started = outcome.is_ok();
        if tx.send_async((generation, outcome)).await.is_err() || !started {
            return;
        }
        while rx.recv_async().await.is_ok() {}
        let _ = screen_end_tx.send_async(generation).await;
    });
}

fn spawn_camera_open(
    generation: u64,
    identity: String,
    frame_store: Arc<VideoFrameStore>,
    device_id: Option<String>,
    tx: flume::Sender<(u64, Result<(CameraController, NativeVideoSource), String>)>,
) {
    runtime::runtime().spawn(async move {
        let (controller, rx) = camera::start_camera(identity, frame_store, device_id);
        let outcome = match rx.recv_async().await {
            Ok(Ok(source)) => Ok((controller, source)),
            Ok(Err(err)) => Err(err),
            Err(_) => Err("camera thread exited".into()),
        };
        let _ = tx.send_async((generation, outcome)).await;
    });
}

fn spawn_sys_audio_open(
    generation: u64,
    tx: flume::Sender<(u64, Result<ScreenAudioCapture, String>)>,
) {
    runtime::runtime().spawn(async move {
        let outcome = match tokio::task::spawn_blocking(screen_audio::start_screen_audio).await {
            Ok(res) => res,
            Err(err) => Err(format!("system audio init task failed: {err}")),
        };
        let _ = tx.send_async((generation, outcome)).await;
    });
}

fn spawn_audio_io_open(
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    tx: flume::Sender<(Option<String>, Result<AudioIo, String>)>,
) {
    runtime::runtime().spawn(async move {
        let opened_input = input_device_id.clone();
        let outcome = match tokio::task::spawn_blocking(move || {
            AudioIo::start(input_device_id, output_device_id, RecordTaps::default())
                .map_err(|e| e.to_string())
        })
        .await
        {
            Ok(res) => res,
            Err(err) => Err(format!("audio device init task failed: {err}")),
        };
        let _ = tx.send_async((opened_input, outcome)).await;
    });
}

fn drop_sys_audio(capture: ScreenAudioCapture) {
    drop(tokio::task::spawn_blocking(move || drop(capture)));
}

async fn sleep_until_opt(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(instant) => tokio::time::sleep_until(instant).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_clock_watch_tracks_suspensions_backward_jumps_and_reconnect_reset() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut watch = WallClockWatch::new(base);

        for sec in 1..=10 {
            assert!(!watch.observe(base + Duration::from_secs(sec)));
        }

        let rewound = base + Duration::from_secs(5);
        assert!(!watch.observe(rewound));
        assert!(!watch.observe(rewound + Duration::from_secs(1)));

        let woke = rewound + Duration::from_secs(61);
        assert!(watch.observe(woke));
        assert!(watch.observe(woke + Duration::from_secs(1)));
        assert!(watch.observe(woke + Duration::from_secs(2)));

        watch.on_connected();
        assert!(!watch.observe(woke + Duration::from_secs(3)));
        assert!(watch.observe(woke + Duration::from_secs(3) + SUSPEND_GAP));
    }
}
