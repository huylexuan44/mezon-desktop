use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, AppContext, Context, Entity, Global, RenderImage, Subscription, Task, Window};
use mezon_client::AppApi;
pub use mezon_voice::{LIVE_PREVIEW_KEY, LiveFailure, LiveSource};
use mezon_voice::{
    LiveEndpoint, LiveEndpointRefresher, LiveEvent, LiveOptions, LiveSession, PickedScreen,
    VideoFrameStore,
};
use parking_lot::Mutex;

use crate::Settings;
use crate::account::AccountStore;
use crate::ids::{ChannelId, ClanId};
use crate::stream::StreamStore;
use crate::voice::VoiceRenderFrame;

const MAX_PENDING_ERRORS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LivePhase {
    #[default]
    Idle,
    Starting {
        clan_id: ClanId,
        channel_id: ChannelId,
    },
    Live {
        clan_id: ClanId,
        channel_id: ChannelId,
    },
    Reconnecting {
        clan_id: ClanId,
        channel_id: ChannelId,
        attempt: u32,
    },
}

pub struct LiveRequest {
    pub screen: Option<PickedScreen>,
    pub system_audio: bool,
    pub mic: bool,
    pub camera: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveError {
    Failure(LiveFailure),
    SourceUnavailable(LiveSource),
}

struct CachedRenderFrame {
    seq: u64,
    frame: VoiceRenderFrame,
}

struct GlobalLiveStore(Entity<LiveStore>);
impl Global for GlobalLiveStore {}

pub struct LiveStore {
    api: Arc<AppApi>,
    phase: LivePhase,
    mic_on: bool,
    camera_on: bool,
    system_audio_on: bool,
    screen_on: bool,
    last_error: Option<LiveFailure>,
    pending_errors: VecDeque<LiveError>,
    selected_input: Option<String>,
    selected_camera: Option<String>,
    pending_screen: Option<PickedScreen>,
    session: Option<LiveSession>,
    frame_store: Option<Arc<VideoFrameStore>>,
    generation: u64,
    render_cache: Mutex<HashMap<u64, CachedRenderFrame>>,
    pending_texture_drops: Mutex<Vec<Arc<RenderImage>>>,
    pending_texture_replaces: Mutex<Vec<Arc<RenderImage>>>,
    pending_texture_work: AtomicBool,
    _events_task: Option<Task<()>>,
    _app_quit_subscription: Option<Subscription>,
}

impl LiveStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalLiveStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalLiveStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalLiveStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let quit_sub = cx.on_app_quit(|this, _cx| {
            this._events_task = None;
            let events_rx = this.session.as_ref().map(|session| {
                session.stop();
                session.events()
            });
            async move {
                if let Some(events_rx) = events_rx {
                    while let Ok(event) = events_rx.recv_async().await {
                        if matches!(event, LiveEvent::Ended) {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            api,
            phase: LivePhase::Idle,
            mic_on: false,
            camera_on: false,
            system_audio_on: false,
            screen_on: false,
            last_error: None,
            pending_errors: VecDeque::new(),
            selected_input: None,
            selected_camera: None,
            pending_screen: None,
            session: None,
            frame_store: None,
            generation: 0,
            render_cache: Mutex::new(HashMap::new()),
            pending_texture_drops: Mutex::new(Vec::new()),
            pending_texture_replaces: Mutex::new(Vec::new()),
            pending_texture_work: AtomicBool::new(false),
            _events_task: None,
            _app_quit_subscription: Some(quit_sub),
        }
    }

    pub fn phase(&self) -> LivePhase {
        self.phase
    }

    pub fn active_channel(&self) -> Option<(ClanId, ChannelId)> {
        match self.phase {
            LivePhase::Idle => None,
            LivePhase::Starting {
                clan_id,
                channel_id,
            }
            | LivePhase::Live {
                clan_id,
                channel_id,
            }
            | LivePhase::Reconnecting {
                clan_id,
                channel_id,
                ..
            } => Some((clan_id, channel_id)),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active_channel().is_some()
    }

    pub fn is_active_in(&self, channel_id: ChannelId) -> bool {
        self.active_channel()
            .is_some_and(|(_, active_ch)| active_ch == channel_id)
    }

    pub fn mic_on(&self) -> bool {
        self.mic_on
    }

    pub fn camera_on(&self) -> bool {
        self.camera_on
    }

    pub fn system_audio_on(&self) -> bool {
        self.system_audio_on
    }

    pub fn screen_on(&self) -> bool {
        self.screen_on
    }

    pub fn last_error(&self) -> Option<LiveFailure> {
        self.last_error
    }

    pub fn take_error(&mut self) -> Option<LiveError> {
        let err = self.pending_errors.pop_front()?;
        if matches!(err, LiveError::Failure(_))
            && !self
                .pending_errors
                .iter()
                .any(|queued| matches!(queued, LiveError::Failure(_)))
        {
            self.last_error = None;
        }
        Some(err)
    }

    pub fn frame_store(&self) -> Option<Arc<VideoFrameStore>> {
        self.frame_store.clone()
    }

    pub fn render_frame(&self, key: u64) -> Option<VoiceRenderFrame> {
        let store = self.frame_store.as_ref()?;
        let cached_seq = self.render_cache.lock().get(&key).map(|entry| entry.seq);
        let Some(frame) = store.take_new(key, cached_seq) else {
            return self
                .render_cache
                .lock()
                .get(&key)
                .map(|entry| entry.frame.clone());
        };
        let seq = frame.seq;
        #[cfg(target_os = "macos")]
        if let Some(surface) = frame.surface {
            let rendered = VoiceRenderFrame::Surface(surface);
            let previous = self.render_cache.lock().insert(
                key,
                CachedRenderFrame {
                    seq,
                    frame: rendered.clone(),
                },
            );
            if let Some(CachedRenderFrame {
                frame: VoiceRenderFrame::Image(image),
                ..
            }) = previous
            {
                self.pending_texture_drops.lock().push(image);
                self.pending_texture_work.store(true, Ordering::Release);
            }
            return Some(rendered);
        }
        let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)?;
        let weak_store = Arc::downgrade(store);
        let recycler = Arc::new(move |buffer| {
            if let Some(store) = weak_store.upgrade() {
                store.recycle(key, buffer);
            }
        });
        let mut render_image = RenderImage::new_recyclable(image::Frame::new(buffer), recycler);
        #[cfg_attr(not(target_os = "macos"), allow(clippy::bind_instead_of_map))]
        let previous_id = self
            .render_cache
            .lock()
            .get(&key)
            .and_then(|entry| match &entry.frame {
                VoiceRenderFrame::Image(image) => Some(image.id),
                #[cfg(target_os = "macos")]
                VoiceRenderFrame::Surface(_) => None,
            });
        if let Some(id) = previous_id {
            render_image = render_image.with_id(id);
        }
        let image = Arc::new(render_image);
        let rendered = VoiceRenderFrame::Image(image.clone());
        let previous = self.render_cache.lock().insert(
            key,
            CachedRenderFrame {
                seq,
                frame: rendered.clone(),
            },
        );
        if previous_id.is_some() {
            let mut replaces = self.pending_texture_replaces.lock();
            if let Some(existing) = replaces.iter_mut().find(|queued| queued.id == image.id) {
                *existing = image.clone();
            } else {
                replaces.push(image.clone());
            }
            self.pending_texture_work.store(true, Ordering::Release);
        } else if let Some(CachedRenderFrame {
            frame: VoiceRenderFrame::Image(previous),
            ..
        }) = previous
        {
            self.pending_texture_drops.lock().push(previous);
            self.pending_texture_work.store(true, Ordering::Release);
        }
        Some(rendered)
    }

    pub fn flush_texture_drops(&self, window: &mut Window, cx: &mut App) {
        if !self.pending_texture_work.swap(false, Ordering::AcqRel) {
            return;
        }
        let drops: Vec<Arc<RenderImage>> = std::mem::take(&mut *self.pending_texture_drops.lock());
        let dropped: HashSet<_> = drops.iter().map(|image| image.id).collect();
        if !dropped.is_empty() {
            self.pending_texture_replaces
                .lock()
                .retain(|image| !dropped.contains(&image.id));
        }
        for image in drops {
            cx.drop_image(image, Some(window));
        }
        let replaces: Vec<Arc<RenderImage>> =
            std::mem::take(&mut *self.pending_texture_replaces.lock());
        for image in replaces {
            cx.update_render_image(&image, Some(window));
        }
    }

    pub fn start(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        request: LiveRequest,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.phase, LivePhase::Idle) {
            return;
        }

        if let Some(stream) = StreamStore::try_global(cx)
            && stream.read(cx).is_session_channel(channel_id)
        {
            stream.update(cx, |s, cx| s.leave_stream(cx));
        }

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;

        self.phase = LivePhase::Starting {
            clan_id,
            channel_id,
        };
        self.screen_on = request.screen.is_some();
        self.pending_screen = request.screen;
        self.system_audio_on = request.system_audio;
        self.mic_on = request.mic;
        self.camera_on = request.camera;
        self.last_error = None;
        self.pending_errors.clear();

        let (input_device_id, output_device_id, camera_device_id) = Settings::try_global(cx)
            .map(|settings| {
                let s = settings.read(cx);
                (
                    s.input_device_id.clone(),
                    s.output_device_id.clone(),
                    s.camera_device_id.clone(),
                )
            })
            .unwrap_or_default();
        self.selected_input = input_device_id;
        self.selected_camera = camera_device_id;

        let local_user_id = AccountStore::try_global(cx)
            .and_then(|acc| acc.read(cx).account.as_ref().map(|a| a.user_id.to_string()))
            .unwrap_or_else(|| "0".to_string());

        let api = self.api.clone();
        let raw_channel_id = channel_id.get();

        self._events_task = Some(cx.spawn(async move |this, cx| {
            let playback = api.get_stream_playback(raw_channel_id, true).await;
            let endpoint = match playback {
                Ok(resp) if !resp.token.is_empty() && !resp.whip_url.is_empty() => LiveEndpoint {
                    whip_url: resp.whip_url,
                    token: resp.token,
                },
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.generation != generation {
                            return;
                        }
                        this.apply_event(LiveEvent::Failed(LiveFailure::Forbidden));
                        this.apply_event(LiveEvent::Ended);
                        cx.notify();
                    });
                    return;
                }
                Err(err) => {
                    let forbidden = err
                        .downcast_ref::<mezon_client::transport::ApiStatusError>()
                        .is_some_and(|status| {
                            status.code
                                == mezon_client::transport::ApiStatusError::PERMISSION_DENIED
                                || status.code == 403
                        });
                    let failure = if forbidden {
                        LiveFailure::Forbidden
                    } else {
                        LiveFailure::Network
                    };
                    tracing::warn!(
                        channel_id = raw_channel_id,
                        "get_stream_playback(publish) failed"
                    );
                    let _ = this.update(cx, |this, cx| {
                        if this.generation != generation {
                            return;
                        }
                        this.apply_event(LiveEvent::Failed(failure));
                        this.apply_event(LiveEvent::Ended);
                        cx.notify();
                    });
                    return;
                }
            };

            let refresh_api = api.clone();
            let refresh = LiveEndpointRefresher::new(move || {
                let refresh_api = refresh_api.clone();
                async move {
                    let resp = refresh_api
                        .get_stream_playback(raw_channel_id, true)
                        .await
                        .ok()?;
                    if resp.token.is_empty() || resp.whip_url.is_empty() {
                        return None;
                    }
                    Some(LiveEndpoint {
                        whip_url: resp.whip_url,
                        token: resp.token,
                    })
                }
            });

            let events_rx = this
                .update(cx, |this, cx| {
                    if this.generation != generation
                        || !matches!(this.phase, LivePhase::Starting { .. })
                    {
                        return None;
                    }
                    let screen = if this.screen_on {
                        this.pending_screen.take()
                    } else {
                        this.pending_screen = None;
                        None
                    };
                    let session = LiveSession::start(LiveOptions {
                        endpoint,
                        refresh,
                        local_user_id,
                        screen,
                        system_audio: this.system_audio_on,
                        mic_enabled: this.mic_on,
                        input_device_id: this.selected_input.clone(),
                        output_device_id,
                        camera_enabled: this.camera_on,
                        camera_device_id: this.selected_camera.clone(),
                    });
                    let events = session.events();
                    this.frame_store = Some(session.frame_store());
                    this.session = Some(session);
                    cx.notify();
                    Some(events)
                })
                .ok()
                .flatten();

            let Some(events_rx) = events_rx else {
                return;
            };

            while let Ok(event) = events_rx.recv_async().await {
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        let ended = matches!(event, LiveEvent::Ended);
                        if this.apply_event(event) {
                            cx.notify();
                        }
                        !ended
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));

        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, LivePhase::Idle) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self._events_task = None;
        self.pending_screen = None;
        if let Some(session) = self.session.take() {
            session.stop();
        }
        self.frame_store = None;
        self.clear_render_cache();
        self.mic_on = false;
        self.camera_on = false;
        self.system_audio_on = false;
        self.screen_on = false;
        self.phase = LivePhase::Idle;
        cx.notify();
    }

    pub fn set_mic_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if !self.is_active() || self.mic_on == on {
            return;
        }
        self.mic_on = on;
        if let Some(session) = &self.session {
            session.set_mic_enabled(on);
        }
        cx.notify();
    }

    pub fn set_camera_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if !self.is_active() || self.camera_on == on {
            return;
        }
        self.camera_on = on;
        if let Some(session) = &self.session {
            session.set_camera_enabled(on);
        }
        cx.notify();
    }

    pub fn set_system_audio(&mut self, on: bool, cx: &mut Context<Self>) {
        if !self.is_active() || self.system_audio_on == on {
            return;
        }
        self.system_audio_on = on;
        if let Some(session) = &self.session {
            session.set_system_audio(on);
        }
        cx.notify();
    }

    pub fn change_screen(
        &mut self,
        pick: PickedScreen,
        system_audio: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.is_active() {
            return;
        }
        self.screen_on = true;
        let sys_changed = self.system_audio_on != system_audio;
        self.system_audio_on = system_audio;
        if let Some(session) = &self.session {
            session.set_screen(Some(pick));
            if sys_changed {
                session.set_system_audio(system_audio);
            }
        } else {
            self.pending_screen = Some(pick);
        }
        cx.notify();
    }

    pub fn stop_screen(&mut self, cx: &mut Context<Self>) {
        if !self.is_active() || !self.screen_on {
            return;
        }
        self.screen_on = false;
        self.pending_screen = None;
        if let Some(session) = &self.session {
            session.set_screen(None);
        }
        cx.notify();
    }

    pub fn set_input_device(&mut self, device_id: Option<String>, _cx: &mut Context<Self>) {
        if self.selected_input == device_id {
            return;
        }
        self.selected_input = device_id.clone();
        if let Some(session) = &self.session {
            session.set_input_device(device_id);
        }
    }

    pub fn set_camera_device(&mut self, device_id: Option<String>, _cx: &mut Context<Self>) {
        if self.selected_camera == device_id {
            return;
        }
        self.selected_camera = device_id.clone();
        if let Some(session) = &self.session {
            session.set_camera_device(device_id);
        }
    }

    pub fn on_logout(&mut self, cx: &mut Context<Self>) {
        self.last_error = None;
        self.pending_errors.clear();
        self.selected_input = None;
        self.selected_camera = None;
        self.stop(cx);
    }

    fn apply_event(&mut self, event: LiveEvent) -> bool {
        match event {
            LiveEvent::Connecting => false,
            LiveEvent::Live => match self.active_channel() {
                Some((clan_id, channel_id)) => {
                    let next = LivePhase::Live {
                        clan_id,
                        channel_id,
                    };
                    if self.phase != next {
                        self.phase = next;
                        true
                    } else {
                        false
                    }
                }
                None => false,
            },
            LiveEvent::Reconnecting { attempt } => match self.active_channel() {
                Some((clan_id, channel_id)) => {
                    let next = LivePhase::Reconnecting {
                        clan_id,
                        channel_id,
                        attempt,
                    };
                    if self.phase != next {
                        self.phase = next;
                        true
                    } else {
                        false
                    }
                }
                None => false,
            },
            LiveEvent::SourceUnavailable { source, .. } => {
                match source {
                    LiveSource::Screen => self.screen_on = false,
                    LiveSource::SystemAudio => self.system_audio_on = false,
                    LiveSource::Microphone => self.mic_on = false,
                    LiveSource::Camera => self.camera_on = false,
                }
                self.push_error(LiveError::SourceUnavailable(source));
                true
            }
            LiveEvent::Failed(failure) => {
                self.last_error = Some(failure);
                self.push_error(LiveError::Failure(failure));
                true
            }
            LiveEvent::Ended => {
                let was_active = !matches!(self.phase, LivePhase::Idle)
                    || self.session.is_some()
                    || self.mic_on
                    || self.camera_on
                    || self.system_audio_on
                    || self.screen_on;
                self.session = None;
                self.frame_store = None;
                self.pending_screen = None;
                self.clear_render_cache();
                self.mic_on = false;
                self.camera_on = false;
                self.system_audio_on = false;
                self.screen_on = false;
                self.phase = LivePhase::Idle;
                was_active
            }
        }
    }

    fn push_error(&mut self, error: LiveError) {
        if self.pending_errors.len() >= MAX_PENDING_ERRORS {
            self.pending_errors.pop_front();
        }
        self.pending_errors.push_back(error);
    }

    fn clear_render_cache(&self) {
        let mut cache = self.render_cache.lock();
        if !cache.is_empty() {
            let mut drops = self.pending_texture_drops.lock();
            for (_, entry) in cache.drain() {
                match entry.frame {
                    VoiceRenderFrame::Image(image) => drops.push(image),
                    #[cfg(target_os = "macos")]
                    VoiceRenderFrame::Surface(_) => {}
                }
            }
            self.pending_texture_work.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> LiveStore {
        let api = Arc::new(AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        LiveStore {
            api,
            phase: LivePhase::Starting {
                clan_id: ClanId(10),
                channel_id: ChannelId(20),
            },
            mic_on: true,
            camera_on: true,
            system_audio_on: true,
            screen_on: true,
            last_error: None,
            pending_errors: VecDeque::new(),
            selected_input: None,
            selected_camera: None,
            pending_screen: None,
            session: None,
            frame_store: None,
            generation: 1,
            render_cache: Mutex::new(HashMap::new()),
            pending_texture_drops: Mutex::new(Vec::new()),
            pending_texture_replaces: Mutex::new(Vec::new()),
            pending_texture_work: AtomicBool::new(false),
            _events_task: None,
            _app_quit_subscription: None,
        }
    }

    #[test]
    fn apply_event_transitions_through_starting_live_reconnecting_live_and_ended() {
        let mut store = test_store();
        assert!(store.is_active_in(ChannelId(20)));
        assert!(!store.is_active_in(ChannelId(99)));

        assert!(!store.apply_event(LiveEvent::Connecting));
        assert_eq!(
            store.phase(),
            LivePhase::Starting {
                clan_id: ClanId(10),
                channel_id: ChannelId(20),
            }
        );

        assert!(store.apply_event(LiveEvent::Live));
        assert_eq!(
            store.phase(),
            LivePhase::Live {
                clan_id: ClanId(10),
                channel_id: ChannelId(20),
            }
        );

        assert!(store.apply_event(LiveEvent::Reconnecting { attempt: 2 }));
        assert_eq!(
            store.phase(),
            LivePhase::Reconnecting {
                clan_id: ClanId(10),
                channel_id: ChannelId(20),
                attempt: 2,
            }
        );
        assert!(store.is_active_in(ChannelId(20)));

        assert!(store.apply_event(LiveEvent::Live));
        assert_eq!(
            store.phase(),
            LivePhase::Live {
                clan_id: ClanId(10),
                channel_id: ChannelId(20),
            }
        );

        assert!(store.apply_event(LiveEvent::Ended));
        assert_eq!(store.phase(), LivePhase::Idle);
        assert!(!store.is_active());
        assert!(!store.mic_on());
        assert!(!store.camera_on());
        assert!(!store.system_audio_on());
        assert!(!store.screen_on());
    }

    #[test]
    fn apply_event_failed_records_error_that_survives_ended_until_taken() {
        let mut store = test_store();

        assert!(store.apply_event(LiveEvent::Failed(LiveFailure::ChannelBusy)));
        assert_eq!(store.last_error(), Some(LiveFailure::ChannelBusy));

        assert!(store.apply_event(LiveEvent::Ended));
        assert_eq!(store.phase(), LivePhase::Idle);
        assert_eq!(store.last_error(), Some(LiveFailure::ChannelBusy));

        assert_eq!(
            store.take_error(),
            Some(LiveError::Failure(LiveFailure::ChannelBusy))
        );
        assert_eq!(store.last_error(), None);
        assert_eq!(store.take_error(), None);
    }

    #[test]
    fn apply_event_source_unavailable_clears_matching_flag_and_queues_toast() {
        let mut store = test_store();

        assert!(store.apply_event(LiveEvent::SourceUnavailable {
            source: LiveSource::Screen,
            reason: "denied".into(),
        }));
        assert!(!store.screen_on());
        assert!(store.system_audio_on());
        assert!(store.mic_on());
        assert!(store.camera_on());

        assert!(store.apply_event(LiveEvent::SourceUnavailable {
            source: LiveSource::SystemAudio,
            reason: "unsupported".into(),
        }));
        assert!(!store.system_audio_on());

        assert!(store.apply_event(LiveEvent::SourceUnavailable {
            source: LiveSource::Microphone,
            reason: "no input".into(),
        }));
        assert!(!store.mic_on());

        assert!(store.apply_event(LiveEvent::SourceUnavailable {
            source: LiveSource::Camera,
            reason: "busy".into(),
        }));
        assert!(!store.camera_on());

        assert_eq!(
            store.take_error(),
            Some(LiveError::SourceUnavailable(LiveSource::Screen))
        );
        assert_eq!(
            store.take_error(),
            Some(LiveError::SourceUnavailable(LiveSource::SystemAudio))
        );
        assert_eq!(
            store.take_error(),
            Some(LiveError::SourceUnavailable(LiveSource::Microphone))
        );
        assert_eq!(
            store.take_error(),
            Some(LiveError::SourceUnavailable(LiveSource::Camera))
        );
        assert_eq!(store.take_error(), None);
    }
}
