use crate::ids::{ChannelId, ClanId};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::transport::ApiMessage;
use mezon_client::{
    AppApi, ConnectionStatus, DIRECTION_AROUND_TIMESTAMP, MezonTransport, RealtimeEvent,
};

use crate::AppConfig;
use crate::KeyedCache;
use crate::channel::{
    ChannelEvent, ChannelList, Message, MessageAttachment, message_combined_with_prev,
    recompute_message_grouping,
};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MESSAGE_PAGE_LIMIT: u32 = 50;
const DIRECTION_BEFORE: i32 = 3;
const CHANNEL_TYPE_CHANNEL: i32 = 1;
const MAX_MESSAGES_PER_CHANNEL: usize = 2_000;
const MAX_CACHED_CHANNELS: usize = 30;

#[derive(Debug, Clone)]
pub enum MessagesEvent {
    Reset { count: usize },
    Appended,
    OlderPrepended { count: usize },
    JumpTo { index: usize },
}

struct ChannelMessages {
    messages: Vec<Message>,
    has_more: bool,
}

const STREAM_MODE_CHANNEL: i32 = 2;

pub struct MessagesStore {
    cache: KeyedCache<ChannelId, ChannelMessages>,
    active_channel_id: Option<ChannelId>,
    active_clan_id: Option<ClanId>,
    is_public: bool,
    is_dm: bool,
    mode: i32,
    loading: bool,
    loading_more: bool,
    fetch_generation: u64,
    pending_jump_message_id: Option<String>,
    joined_channels: HashSet<ChannelId>,
    api: Arc<AppApi>,
    _channel_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalMessagesStore(Entity<MessagesStore>);
impl Global for GlobalMessagesStore {}

impl EventEmitter<MessagesEvent> for MessagesStore {}

impl MessagesStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalMessagesStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalMessagesStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalMessagesStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _channel, event, cx| {
            if let ChannelEvent::ActiveChannelChanged(channel_id) = event {
                this.on_active_channel_changed(*channel_id, cx);
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNELS)),
            active_channel_id: None,
            active_clan_id: None,
            is_public: true,
            is_dm: false,
            mode: STREAM_MODE_CHANNEL,
            loading: false,
            loading_more: false,
            fetch_generation: 0,
            pending_jump_message_id: None,
            joined_channels: HashSet::new(),
            api,
            _channel_sub: channel_sub,
            _conn_watch: conn_watch,
        }
    }

    /// Register realtime handlers with the central dispatcher (cf. `add_message_handler`).
    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::ChannelMessage, &entity, |this, event, cx| {
                this.handle_event(event, cx)
            });
            dispatch.on_lagged(&entity, |this, cx| this.resync(cx));
        });
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = false;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected && !was_connected {
                    was_connected = true;
                    if this.update(cx, |this, cx| this.resync(cx)).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn messages(&self) -> &[Message] {
        self.active_channel_id
            .as_ref()
            .and_then(|id| self.cache.get(id))
            .map(|c| c.messages.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading_more || self.loading {
            return;
        }
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let Some(channel) = self.cache.get(&channel_id) else {
            return;
        };
        if !channel.has_more {
            return;
        }
        let Some(oldest_id) = channel
            .messages
            .first()
            .map(|m| m.id.clone())
            .filter(|id| !id.starts_with("temp-"))
        else {
            return;
        };

        self.loading_more = true;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(
                    clan_id.get(),
                    channel_id.get(),
                    oldest_id.parse::<i64>().unwrap_or(0),
                    DIRECTION_BEFORE,
                    MESSAGE_PAGE_LIMIT,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let msgs = match result {
                    Ok(msgs) => msgs,
                    Err(e) => {
                        tracing::error!("Failed to load more messages for {channel_id}: {e}");
                        cx.notify();
                        return;
                    }
                };
                let Some(channel) = this.cache.get_mut(&channel_id) else {
                    return;
                };
                let existing: std::collections::HashSet<&str> =
                    channel.messages.iter().map(|m| m.id.as_str()).collect();
                let cfg = AppConfig::try_global(cx);
                let mut older: Vec<Message> = msgs
                    .into_iter()
                    .filter(|m| !existing.contains(m.message_id.to_string().as_str()))
                    .map(|m| message_from_api(m, cfg))
                    .collect();
                if older.is_empty() {
                    channel.has_more = false;
                    cx.notify();
                    return;
                }
                let prepended = older.len();
                older.append(&mut channel.messages);
                older.sort_by_key(|m| m.create_time);
                trim_messages(&mut older);
                channel.messages = older;
                recompute_message_grouping(&mut channel.messages);
                if this.active_channel_id == Some(channel_id) {
                    cx.emit(MessagesEvent::OlderPrepended { count: prepended });
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn send_message(
        &mut self,
        content: String,
        sender_id: String,
        sender_name: String,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let is_public = self.is_public;
        let mode = self.mode;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        let temp_id = format!("temp-{now}");

        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        channel.messages.push(Message::new(
            temp_id.clone(),
            content.clone(),
            sender_id,
            sender_name,
            now,
        ));
        trim_messages(&mut channel.messages);
        recompute_message_grouping(&mut channel.messages);
        cx.emit(MessagesEvent::Appended);
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            match api
                .send_channel_message(clan_id.get(), channel_id.get(), &content, is_public, mode)
                .await
            {
                Ok(sent) => {
                    let _ = this.update(cx, |this, cx| {
                        let confirmed = message_from_api(sent, AppConfig::try_global(cx));
                        this.reconcile_temp(channel_id, &temp_id, confirmed);
                        if this.active_channel_id == Some(channel_id) {
                            cx.emit(MessagesEvent::Appended);
                            cx.notify();
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("send_channel_message failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.remove_temp(channel_id, &temp_id);
                        if this.active_channel_id == Some(channel_id) {
                            cx.emit(MessagesEvent::Appended);
                            cx.notify();
                        }
                    });
                }
            }
        })
        .detach();
    }

    fn on_active_channel_changed(&mut self, channel_id: Option<ChannelId>, cx: &mut Context<Self>) {
        let Some(channel_id) = channel_id else {
            self.active_channel_id = None;
            self.active_clan_id = None;
            self.is_dm = false;
            self.loading = false;
            self.loading_more = false;
            cx.emit(MessagesEvent::Reset { count: 0 });
            cx.notify();
            return;
        };
        self.open_channel(channel_id, cx);
    }

    /// Open a clan channel as the active conversation (looks up clan/privacy from `ChannelList`).
    pub fn open_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.active_channel_id == Some(channel_id) && !self.is_dm {
            if self.loading {
                return;
            }
            let empty = self
                .cache
                .get(&channel_id)
                .map(|c| c.messages.is_empty())
                .unwrap_or(true);
            if !empty || self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
                return;
            }
            self.refetch_current_messages(cx);
            return;
        }
        let Some(channel) = ChannelList::global(cx)
            .read(cx)
            .find_channel(channel_id)
            .cloned()
        else {
            return;
        };
        self.activate(
            channel.clan_id,
            channel_id,
            !channel.private,
            false,
            CHANNEL_TYPE_CHANNEL,
            STREAM_MODE_CHANNEL,
            cx,
        );
    }

    /// Open a direct message / group conversation (clan_id = 0) as the active conversation.
    /// `channel_type` is the raw DM type (3 = DM, 2 = group).
    pub fn open_direct(
        &mut self,
        channel_id: ChannelId,
        channel_type: i32,
        cx: &mut Context<Self>,
    ) {
        if self.active_channel_id == Some(channel_id) && self.is_dm {
            if self.loading {
                return;
            }
            let empty = self
                .cache
                .get(&channel_id)
                .map(|c| c.messages.is_empty())
                .unwrap_or(true);
            if !empty || self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
                return;
            }
            self.refetch_current_messages(cx);
            return;
        }
        let mode = if channel_type == 2 { 3 } else { 4 };
        self.activate(ClanId(0), channel_id, false, true, channel_type, mode, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn activate(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        is_public: bool,
        is_dm: bool,
        join_type: i32,
        mode: i32,
        cx: &mut Context<Self>,
    ) {
        self.active_channel_id = Some(channel_id);
        self.active_clan_id = Some(clan_id);
        self.is_public = is_public;
        self.is_dm = is_dm;
        self.mode = mode;
        self.loading_more = false;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;

        if !self.joined_channels.contains(&channel_id) {
            self.joined_channels.insert(channel_id);
            self.spawn_join(clan_id, channel_id, join_type, is_public, cx);
        }

        if self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
            self.cache.touch(&channel_id);
            self.loading = false;
            let count = self.messages().len();
            cx.emit(MessagesEvent::Reset { count });
            cx.notify();
            return;
        }

        self.loading = true;
        if self.cache.contains(&channel_id) {
            self.cache.touch(&channel_id);
            let count = self.messages().len();
            cx.emit(MessagesEvent::Reset { count });
        } else {
            cx.emit(MessagesEvent::Reset { count: 0 });
        }
        cx.notify();
        self.spawn_initial_fetch(clan_id, channel_id, generation, cx);
    }

    fn spawn_join(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
        join_type: i32,
        is_public: bool,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .join_chat(clan_id.get(), channel_id.get(), join_type, is_public)
                .await
            {
                tracing::warn!("join_chat failed: {e}");
            }
        })
        .detach();
    }

    fn spawn_initial_fetch(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(clan_id.get(), channel_id.get(), 0, 0, MESSAGE_PAGE_LIMIT)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_initial_fetch_result(channel_id, generation, result, cx);
            });
        })
        .detach();
    }

    fn apply_initial_fetch_result(
        &mut self,
        channel_id: ChannelId,
        generation: u64,
        result: Result<Vec<ApiMessage>, anyhow::Error>,
        cx: &mut Context<Self>,
    ) {
        let is_active = self.active_channel_id == Some(channel_id);
        let is_current = is_active && self.fetch_generation == generation;

        match result {
            Ok(msgs) => {
                let messages = prepare_messages(msgs, AppConfig::try_global(cx));
                let count = messages.len();
                self.set_channel(channel_id, messages);
                if is_current {
                    self.loading = false;
                    cx.emit(MessagesEvent::Reset { count });
                    self.try_emit_jump(cx);
                    cx.notify();
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch messages for {channel_id}: {e}");
                if is_current {
                    self.loading = false;
                    let count = self.messages().len();
                    cx.emit(MessagesEvent::Reset { count });
                    self.try_emit_jump(cx);
                    cx.notify();
                }
            }
        }
    }

    pub fn jump_to_message(
        &mut self,
        clan_id: &str,
        channel_id: &str,
        message_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Ok(clan_id) = ClanId::from_str(clan_id) else {
            tracing::warn!("jump_to_message: invalid clan_id");
            return;
        };
        let Ok(channel_id) = ChannelId::from_str(channel_id) else {
            tracing::warn!("jump_to_message: invalid channel_id");
            return;
        };
        self.pending_jump_message_id = Some(message_id.to_string());
        if self.active_channel_id == Some(channel_id) && self.active_clan_id == Some(clan_id) {
            if self.try_emit_jump(cx) {
                return;
            }
            self.fetch_around_message(clan_id, channel_id, message_id, cx);
            return;
        }
        let Some(channel) = ChannelList::global(cx)
            .read(cx)
            .find_channel(channel_id)
            .cloned()
        else {
            return;
        };
        self.active_channel_id = Some(channel_id);
        self.active_clan_id = Some(channel.clan_id);
        self.is_public = !channel.private;
        self.is_dm = false;
        self.mode = STREAM_MODE_CHANNEL;
        self.loading_more = false;
        if !self.joined_channels.contains(&channel_id) {
            self.joined_channels.insert(channel_id);
            self.spawn_join(
                channel.clan_id,
                channel_id,
                CHANNEL_TYPE_CHANNEL,
                !channel.private,
                cx,
            );
        }
        cx.emit(MessagesEvent::Reset { count: 0 });
        self.fetch_around_message(channel.clan_id, channel_id, message_id, cx);
    }

    fn try_emit_jump(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(message_id) = self.pending_jump_message_id.clone() else {
            return false;
        };
        if let Some(index) = self.messages().iter().position(|m| m.id == message_id) {
            self.pending_jump_message_id = None;
            cx.emit(MessagesEvent::JumpTo { index });
            cx.notify();
            true
        } else {
            false
        }
    }

    fn fetch_around_message(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        message_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Ok(anchor_id) = message_id.parse::<i64>() else {
            tracing::warn!("jump_to_message: invalid message_id");
            return;
        };
        self.loading = true;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(
                    clan_id.get(),
                    channel_id.get(),
                    anchor_id,
                    DIRECTION_AROUND_TIMESTAMP,
                    MESSAGE_PAGE_LIMIT,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_initial_fetch_result(channel_id, generation, result, cx);
            });
        })
        .detach();
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::ChannelMessage(m) = event else {
            return;
        };
        let channel_id = ChannelId(m.channel_id);
        let is_active = self.active_channel_id == Some(channel_id);
        let cfg = AppConfig::try_global(cx);
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let msg = message_from_api(MezonTransport::message_from_proto(m.clone()), cfg);
        if channel.messages.iter().any(|x| x.id == msg.id) {
            return;
        }
        if let Some(slot) = channel.messages.iter_mut().find(|x| {
            x.id.starts_with("temp-") && x.sender_id == msg.sender_id && x.content == msg.content
        }) {
            *slot = msg;
            channel.messages.sort_by_key(|m| m.create_time);
            trim_messages(&mut channel.messages);
            recompute_message_grouping(&mut channel.messages);
        } else {
            push_message_grouped(&mut channel.messages, msg);
        }
        if is_active {
            cx.emit(MessagesEvent::Appended);
            cx.notify();
        }
    }

    fn reconcile_temp(&mut self, channel_id: ChannelId, temp_id: &str, confirmed: Message) {
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        if let Some(slot) = channel.messages.iter_mut().find(|m| m.id == temp_id) {
            *slot = confirmed;
        } else if !channel.messages.iter().any(|m| m.id == confirmed.id) {
            channel.messages.push(confirmed);
            channel.messages.sort_by_key(|m| m.create_time);
            trim_messages(&mut channel.messages);
            recompute_message_grouping(&mut channel.messages);
        }
    }

    fn remove_temp(&mut self, channel_id: ChannelId, temp_id: &str) {
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let before = channel.messages.len();
        channel.messages.retain(|m| m.id != temp_id);
        if channel.messages.len() != before {
            recompute_message_grouping(&mut channel.messages);
        }
    }

    fn resync(&mut self, cx: &mut Context<Self>) {
        tracing::info!("MessagesStore resync — marking message cache stale");
        self.cache.mark_all_stale();
        self.joined_channels.clear();
        self.refetch_current_messages(cx);
    }

    /// Force a refetch of the open channel ignoring the cache (cf. React `noCache: true`).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refetch_current_messages(cx);
    }

    fn refetch_current_messages(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };

        self.loading = true;
        self.loading_more = false;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(clan_id.get(), channel_id.get(), 0, 0, MESSAGE_PAGE_LIMIT)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_initial_fetch_result(channel_id, generation, result, cx);
            });
        })
        .detach();
    }

    fn set_channel(&mut self, channel_id: ChannelId, messages: Vec<Message>) {
        let active = self.active_channel_id;
        self.cache.insert(
            channel_id,
            ChannelMessages {
                messages,
                has_more: true,
            },
            active.as_ref(),
        );
    }
}

fn prepare_messages(msgs: Vec<ApiMessage>, cfg: Option<&AppConfig>) -> Vec<Message> {
    let mut messages: Vec<Message> = msgs.into_iter().map(|m| message_from_api(m, cfg)).collect();
    messages.sort_by_key(|m| m.create_time);
    trim_messages(&mut messages);
    recompute_message_grouping(&mut messages);
    messages
}

fn push_message_grouped(messages: &mut Vec<Message>, msg: Message) {
    let in_order = messages
        .last()
        .map(|last| last.create_time <= msg.create_time)
        .unwrap_or(true);
    messages.push(msg);
    if in_order {
        trim_messages(messages);
        let last = messages.len() - 1;
        let combined = {
            let prev = last.checked_sub(1).map(|i| &messages[i]);
            message_combined_with_prev(prev, &messages[last])
        };
        messages[last].combined_with_prev = combined;
    } else {
        messages.sort_by_key(|m| m.create_time);
        trim_messages(messages);
        recompute_message_grouping(messages);
    }
}

fn trim_messages(messages: &mut Vec<Message>) {
    if messages.len() <= MAX_MESSAGES_PER_CHANNEL {
        return;
    }
    let drop = messages.len() - MAX_MESSAGES_PER_CHANNEL;
    messages.drain(0..drop);
}

fn message_from_api(m: ApiMessage, cfg: Option<&AppConfig>) -> Message {
    let avatar_proxied = cfg
        .map(|c| c.avatar_proxy(&m.avatar))
        .unwrap_or_else(|| m.avatar.clone());
    Message::new(
        m.message_id.to_string(),
        m.content,
        m.sender_id.to_string(),
        m.sender_name,
        m.create_time,
    )
    .with_avatar(m.avatar)
    .with_avatar_proxied(avatar_proxied)
    .with_attachments(
        m.attachments
            .into_iter()
            .map(|a| MessageAttachment::from_api(a, cfg))
            .collect(),
    )
}

impl MessageAttachment {
    pub(crate) fn from_api(
        a: mezon_client::transport::ApiAttachment,
        cfg: Option<&AppConfig>,
    ) -> Self {
        let width = a.width.max(0) as u32;
        let height = a.height.max(0) as u32;
        let (proxied_src, display_width, display_height) = cfg
            .map(|c| c.attachment_proxy(&a.url, width, height))
            .unwrap_or_else(|| {
                let (w, h) = crate::config::attachment_display_dimensions(width, height);
                (a.url.clone(), w, h)
            });
        Self {
            url: a.url,
            filename: a.filename,
            filetype: a.filetype,
            width,
            height,
            proxied_src: proxied_src.into(),
            display_width,
            display_height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_from_api_maps_fields() {
        let m = message_from_api(
            ApiMessage {
                message_id: 1,
                content: "hi".into(),
                sender_id: 1,
                sender_name: "Alice".into(),
                avatar: "av.png".into(),
                create_time: 100,
                attachments: vec![],
            },
            None,
        );
        assert_eq!(m.id, "1");
        assert_eq!(m.content, "hi");
        assert_eq!(m.sender_name, "Alice");
        assert_eq!(m.avatar_url, "av.png");
        assert_eq!(m.avatar_proxied, "av.png");
    }

    #[test]
    fn push_message_grouped_appends_in_order() {
        let mut msgs = vec![
            Message::new("1", "a", "u1", "U1", 100),
            Message::new("2", "b", "u1", "U1", 110),
        ];
        recompute_message_grouping(&mut msgs);
        push_message_grouped(&mut msgs, Message::new("3", "c", "u1", "U1", 120));
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].id, "3");
        assert!(msgs[2].combined_with_prev);
    }

    #[test]
    fn push_message_grouped_resorts_when_out_of_order() {
        let mut msgs = vec![
            Message::new("1", "a", "u1", "U1", 100),
            Message::new("3", "c", "u1", "U1", 120),
        ];
        recompute_message_grouping(&mut msgs);
        push_message_grouped(&mut msgs, Message::new("2", "b", "u1", "U1", 110));
        let ids: Vec<&str> = msgs.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3"]);
    }

    #[test]
    fn push_message_grouped_breaks_group_for_different_sender() {
        let mut msgs = vec![Message::new("1", "a", "u1", "U1", 100)];
        recompute_message_grouping(&mut msgs);
        push_message_grouped(&mut msgs, Message::new("2", "b", "u2", "U2", 105));
        assert!(!msgs[1].combined_with_prev);
    }

    #[test]
    fn trim_messages_drops_oldest() {
        let mut msgs: Vec<Message> = (0..MAX_MESSAGES_PER_CHANNEL + 5)
            .map(|i| Message::new(i.to_string(), format!("m{i}"), "u", "User", i as i64))
            .collect();
        trim_messages(&mut msgs);
        assert_eq!(msgs.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(msgs.first().unwrap().id, "5");
        assert_eq!(msgs.last().unwrap().id, "2004");
    }

    fn channel_msgs(msgs: Vec<Message>) -> ChannelMessages {
        ChannelMessages {
            messages: msgs,
            has_more: false,
        }
    }

    fn remove_temp_in(ch: &mut ChannelMessages, temp_id: &str) {
        let before = ch.messages.len();
        ch.messages.retain(|m| m.id != temp_id);
        if ch.messages.len() != before {
            recompute_message_grouping(&mut ch.messages);
        }
    }

    fn reconcile_temp_in(ch: &mut ChannelMessages, temp_id: &str, confirmed: Message) {
        if let Some(slot) = ch.messages.iter_mut().find(|m| m.id == temp_id) {
            *slot = confirmed;
        } else if !ch.messages.iter().any(|m| m.id == confirmed.id) {
            ch.messages.push(confirmed);
            ch.messages.sort_by_key(|m| m.create_time);
            trim_messages(&mut ch.messages);
            recompute_message_grouping(&mut ch.messages);
        }
    }

    #[test]
    fn remove_temp_drops_message_by_id() {
        let mut ch = channel_msgs(vec![
            Message::new("temp-1", "hello", "u1", "U", 100),
            Message::new("msg-2", "world", "u1", "U", 200),
        ]);
        remove_temp_in(&mut ch, "temp-1");
        assert_eq!(ch.messages.len(), 1);
        assert_eq!(ch.messages[0].id, "msg-2");
    }

    #[test]
    fn remove_temp_noop_when_id_not_found() {
        let mut ch = channel_msgs(vec![Message::new("msg-1", "hello", "u1", "U", 100)]);
        remove_temp_in(&mut ch, "temp-999");
        assert_eq!(ch.messages.len(), 1);
    }

    #[test]
    fn reconcile_temp_matches_only_by_temp_id_not_content() {
        let mut ch = channel_msgs(vec![
            Message::new("temp-1", "same text", "u1", "U", 100),
            Message::new("temp-2", "same text", "u1", "U", 110),
        ]);
        let confirmed = Message::new("server-42", "same text", "u1", "U", 120);
        reconcile_temp_in(&mut ch, "temp-1", confirmed);
        assert_eq!(ch.messages.len(), 2);
        assert_eq!(ch.messages[0].id, "server-42");
        assert_eq!(ch.messages[1].id, "temp-2");
    }
}
