use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::transport::{
    ApiMessage, ApiMessageContent, OutgoingEmoji as TransportEmoji,
    OutgoingHashtag as TransportHashtag, OutgoingMention as TransportMention, OutgoingReply,
    detect_markdown, emoji_content_tokens, hashtag_content_tokens, markdown_content_tokens,
    mention_content_tokens, prioritize_avatar,
};
use mezon_client::{
    AppApi, ConnectionStatus, MezonTransport, RealtimeEvent, UploadFile, UrlAttachment,
};

use crate::AppConfig;
use crate::KeyedCache;
use crate::account::AccountStore;
use crate::channel::{ChannelEvent, ChannelList};
use crate::message::{
    Message, MessageAttachment, MessageCode, MessageReference, aggregate_reactions,
    apply_reaction_event, message_combined_with_prev, message_sort_key, parse_spans,
    recompute_message_grouping, sort_messages,
};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MESSAGE_PAGE_LIMIT: u32 = 50;
const DIRECTION_BEFORE: i32 = 3;
const DIRECTION_AFTER: i32 = 1;
/// `Direction_Mode.AROUND_TIMESTAMP` — fetch a window centered on a message
/// (used by jump-to-message when the target is not loaded).
const DIRECTION_AROUND: i32 = 2;
const CHANNEL_TYPE_CHANNEL: i32 = 1;
const STICKER_FILETYPE: &str = "sticker";
const MAX_MESSAGES_PER_CHANNEL: usize = 100;
const MAX_CACHED_CHANNELS: usize = 30;

#[derive(Debug, Clone)]
pub enum MessagesEvent {
    /// The whole viewport was replaced (channel switch / fetch). `count` is the
    /// new row count.
    Reset { count: usize },
    /// The viewport window slid: rows were added/removed at either edge. The UI
    /// applies the matching splices so the visible scroll position is preserved.
    Shifted {
        added_top: usize,
        removed_top: usize,
        added_bottom: usize,
        removed_bottom: usize,
    },
    /// An in-place change to an existing row (e.g. a reaction add/remove) that
    /// does not alter the row count — the UI just needs to re-render.
    Updated,
    /// Scroll to and briefly highlight a message that is now in the buffer
    /// (cf. React `idMessageToJump`). Emitted by [`MessagesStore::jump_to_message`]
    /// once the target is loaded — either it was already present, or an
    /// AROUND fetch (which emits `Reset` first) just brought it in.
    JumpTo { message_id: MessageId },
}

/// The message currently being replied to (composer state), mirroring React's
/// reply reference draft in `references.slice`.
#[derive(Debug, Clone)]
pub struct ReplyDraft {
    pub message_ref_id: MessageId,
    pub sender_id: UserId,
    pub sender_name: String,
    pub sender_avatar: String,
    pub content_preview: String,
    pub has_attachment: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingMention {
    pub user_id: String,
    pub role_id: String,
    pub display: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingMention {
    fn into_transport(self) -> TransportMention {
        TransportMention {
            user_id: self.user_id,
            role_id: self.role_id,
            username: self.display,
            s: self.s,
            e: self.e,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingHashtag {
    pub channel_id: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingHashtag {
    fn into_transport(self) -> TransportHashtag {
        TransportHashtag {
            channel_id: self.channel_id,
            s: self.s,
            e: self.e,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingEmoji {
    pub emoji_id: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingEmoji {
    fn into_transport(self) -> TransportEmoji {
        TransportEmoji {
            emoji_id: self.emoji_id,
            s: self.s,
            e: self.e,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OutgoingContent {
    pub mentions: Vec<OutgoingMention>,
    pub hashtags: Vec<OutgoingHashtag>,
    pub emojis: Vec<OutgoingEmoji>,
}

impl OutgoingContent {
    pub fn is_empty(&self) -> bool {
        self.mentions.is_empty() && self.hashtags.is_empty() && self.emojis.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct OutgoingAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub filetype: String,
}

#[derive(Default)]
struct MessageList {
    items: Vec<Message>,
    index: HashMap<MessageId, usize>,
    temp_ids: Vec<MessageId>,
}

impl MessageList {
    fn from_messages(items: Vec<Message>) -> Self {
        let mut list = Self {
            items,
            index: HashMap::new(),
            temp_ids: Vec::new(),
        };
        list.reindex();
        list
    }

    fn reindex(&mut self) {
        self.index.clear();
        self.index.reserve(self.items.len());
        self.temp_ids.clear();
        for (i, m) in self.items.iter().enumerate() {
            self.index.insert(m.id, i);
            if m.id.is_optimistic() {
                self.temp_ids.push(m.id);
            }
        }
    }

    fn as_slice(&self) -> &[Message] {
        &self.items
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn first(&self) -> Option<&Message> {
        self.items.first()
    }

    fn last(&self) -> Option<&Message> {
        self.items.last()
    }

    fn contains_id(&self, id: MessageId) -> bool {
        self.index.contains_key(&id)
    }

    fn position(&self, id: MessageId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    fn get_mut_by_id(&mut self, id: MessageId) -> Option<&mut Message> {
        let idx = *self.index.get(&id)?;
        self.items.get_mut(idx)
    }

    fn temp_match_position(&self, sender_id: &str, content: &str) -> Option<usize> {
        self.temp_ids.iter().find_map(|temp_id| {
            let idx = *self.index.get(temp_id)?;
            let candidate = &self.items[idx];
            (candidate.sender_id == sender_id && candidate.content == content).then_some(idx)
        })
    }

    fn replace(&mut self, items: Vec<Message>) {
        self.items = items;
        self.reindex();
    }

    fn push_trim_regroup(&mut self, msg: Message) {
        self.items.push(msg);
        trim_messages(&mut self.items);
        recompute_message_grouping(&mut self.items);
        self.reindex();
    }

    fn push_sorted(&mut self, msg: Message) {
        self.items.push(msg);
        sort_messages(&mut self.items);
        trim_messages(&mut self.items);
        recompute_message_grouping(&mut self.items);
        self.reindex();
    }

    fn push_grouped(&mut self, msg: Message) {
        let in_order = self
            .items
            .last()
            .map(|last| message_sort_key(last) <= message_sort_key(&msg))
            .unwrap_or(true);
        if !in_order {
            self.items.push(msg);
            sort_messages(&mut self.items);
            trim_messages(&mut self.items);
            recompute_message_grouping(&mut self.items);
            self.reindex();
            return;
        }
        self.items.push(msg);
        let dropped = self.items.len().saturating_sub(MAX_MESSAGES_PER_CHANNEL);
        if dropped > 0 {
            let evicted_temp_ids: Vec<MessageId> = self.items[..dropped]
                .iter()
                .filter(|m| m.id.is_optimistic())
                .map(|m| m.id)
                .collect();
            for evicted in self.items[..dropped].iter() {
                self.index.remove(&evicted.id);
            }
            self.items.drain(0..dropped);
            for position in self.index.values_mut() {
                *position -= dropped;
            }
            self.temp_ids.retain(|t| !evicted_temp_ids.contains(t));
        }
        let last = self.items.len() - 1;
        let combined = {
            let prev = last.checked_sub(1).map(|i| &self.items[i]);
            message_combined_with_prev(prev, &self.items[last])
        };
        self.items[last].combined_with_prev = combined;
        let id = self.items[last].id;
        if id.is_optimistic() {
            self.temp_ids.push(id);
        }
        self.index.insert(id, last);
    }

    fn replace_at(&mut self, idx: usize, msg: Message) {
        let old_id = self.items[idx].id;
        let new_id = msg.id;
        self.items[idx] = msg;
        if old_id == new_id {
            return;
        }
        self.index.remove(&old_id);
        self.index.insert(new_id, idx);
        if old_id.is_optimistic() {
            self.temp_ids.retain(|t| *t != old_id);
        }
        if new_id.is_optimistic() {
            self.temp_ids.push(new_id);
        }
    }

    fn replace_resort(&mut self, idx: usize, msg: Message) {
        self.items[idx] = msg;
        sort_messages(&mut self.items);
        trim_messages(&mut self.items);
        recompute_message_grouping(&mut self.items);
        self.reindex();
    }

    fn prepend_older(&mut self, mut older: Vec<Message>) -> usize {
        older.append(&mut self.items);
        sort_messages(&mut older);
        let dropped_bottom = trim_messages_back(&mut older);
        self.items = older;
        recompute_message_grouping(&mut self.items);
        self.reindex();
        dropped_bottom
    }

    fn append_newer(&mut self, mut newer: Vec<Message>) -> usize {
        self.items.append(&mut newer);
        sort_messages(&mut self.items);
        let dropped = trim_messages(&mut self.items);
        recompute_message_grouping(&mut self.items);
        self.reindex();
        dropped
    }

    fn remove_id(&mut self, id: MessageId) -> bool {
        let Some(idx) = self.index.get(&id).copied() else {
            return false;
        };
        self.items.remove(idx);
        recompute_message_grouping(&mut self.items);
        self.reindex();
        true
    }
}

struct ChannelMessages {
    messages: MessageList,
    /// More history exists above (older). Mirrors React `hasMoreTop`.
    has_more: bool,
    /// More messages exist below (newer) that are not loaded — only true after
    /// a jump-to-message loads a window that does not reach the newest message.
    /// Mirrors React `selectHasMoreBottomByChannelId`; `false` in normal flow
    /// (the newest message is always loaded), so the bottom network-load path
    /// stays inert until jump-to-message is wired.
    has_more_bottom: bool,
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
    /// Throttle state for older-history paging: when the backend answers very
    /// fast (<100ms) and the user flings the scrollbar, back off progressively
    /// so we don't blast through the whole history (cf. React `handleOnChange`).
    last_load_more: Option<Instant>,
    consecutive_loads: u32,
    fetch_generation: u64,
    /// Active reply target for the composer, if any.
    reply_target: Option<ReplyDraft>,
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
            last_load_more: None,
            consecutive_loads: 0,
            fetch_generation: 0,
            reply_target: None,
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
            dispatch.on(RealtimeKind::MessageReaction, &entity, |this, event, cx| {
                this.handle_reaction(event, cx)
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

    /// Full message buffer for the active channel (internal cache; may be large).
    pub fn messages(&self) -> &[Message] {
        self.active_channel_id
            .as_ref()
            .and_then(|id| self.cache.get(id))
            .map(|c| c.messages.as_slice())
            .unwrap_or(&[])
    }

    pub fn last_cached_message(&self, channel_id: &str) -> Option<&Message> {
        let channel_id = channel_id.parse::<ChannelId>().ok()?;
        self.cache
            .get(&channel_id)
            .and_then(|channel| channel.messages.last())
    }

    /// The messages exposed to the UI. The buffer is already bounded to
    /// `MAX_MESSAGES_PER_CHANNEL` — it *is* the sliding window — and `gpui::list`
    /// virtualizes painting, so the UI mirrors the whole buffer 1:1. Older/newer
    /// rows enter and leave the buffer as the user pages (cf. React's bounded
    /// `selectMessageViewportIdsByChannelId`).
    pub fn viewport_messages(&self) -> &[Message] {
        self.messages()
    }

    /// Emit the splice for a single row appended at the bottom, accounting for
    /// any front-trim that dropped the oldest rows to keep the buffer within the
    /// cap. `old_len` is the buffer length before the push.
    fn emit_appended(&mut self, old_len: usize, cx: &mut Context<Self>) {
        let new_len = self.messages().len();
        if new_len <= old_len {
            cx.emit(MessagesEvent::Updated);
            cx.notify();
            return;
        }
        let removed_top = (old_len + 1).saturating_sub(new_len);
        cx.emit(MessagesEvent::Shifted {
            added_top: 0,
            removed_top,
            added_bottom: 1,
            removed_bottom: 0,
        });
        cx.notify();
    }

    /// Called by the timeline when the user scrolls to the top: fetch the next
    /// older page from the server. The buffer is the whole window, so there is
    /// no local "reveal" step — reaching the top always pages over the network.
    pub fn scroll_reached_top(&mut self, cx: &mut Context<Self>) {
        if self.active_channel_id.is_none() {
            return;
        }
        self.load_more(cx);
    }

    /// True when newer messages exist on the server that are not yet loaded
    /// (only after a jump-to-message lands on an older window). Mirrors React
    /// `selectHasMoreBottomByChannelId`. `false` in normal flow.
    pub fn has_more_bottom(&self) -> bool {
        self.active_channel_id
            .as_ref()
            .and_then(|id| self.cache.get(id))
            .map(|c| c.has_more_bottom)
            .unwrap_or(false)
    }

    /// Called by the timeline when the user scrolls to the bottom: fetch the
    /// next newer page from the server (only relevant after a jump-to-message,
    /// when the newest message is not loaded). This is a network load — there is
    /// no local "reveal newer", since in normal flow the newest is always shown.
    pub fn scroll_reached_bottom(&mut self, cx: &mut Context<Self>) {
        tracing::debug!(
            has_more_bottom = self.has_more_bottom(),
            "scroll_reached_bottom"
        );
        self.load_more_bottom(cx);
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn active_channel_id(&self) -> Option<ChannelId> {
        self.active_channel_id
    }

    pub fn is_dm(&self) -> bool {
        self.is_dm
    }

    /// True while an older-history (load-more) fetch is in flight.
    pub fn is_loading_more(&self) -> bool {
        self.loading_more
    }

    fn channel_has_more(&self) -> bool {
        self.active_channel_id
            .as_ref()
            .and_then(|id| self.cache.get(id))
            .map(|c| c.has_more)
            .unwrap_or(false)
    }

    /// True while there is more history to show above the current viewport —
    /// either cached rows not yet revealed, or older pages still on the server.
    /// Mirrors React `selectHasMoreMessageByChannelId` (drives the persistent
    /// top loading skeleton).
    pub fn has_more_top(&self) -> bool {
        self.channel_has_more()
    }

    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading_more || self.loading {
            // Guard against duplicate fetches while one is already in flight
            // (cf. React `debounce`/loadingStatus). Logged to verify no dup call.
            tracing::debug!(
                loading_more = self.loading_more,
                loading = self.loading,
                "load_more skipped: fetch already in flight"
            );
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
            .map(|m| m.id)
            .filter(|id| !id.is_optimistic())
        else {
            return;
        };

        // Progressive backoff (cf. React `handleOnChange`): if loads keep firing
        // in quick succession (the user is flinging the scrollbar and the
        // backend answers in <100ms), delay each successive fetch a bit more so
        // we don't auto-page through the whole channel. Resets once the user
        // pauses for >300ms.
        let now = Instant::now();
        let rapid = self
            .last_load_more
            .map(|t| now.duration_since(t) < Duration::from_millis(300))
            .unwrap_or(false);
        self.consecutive_loads = if rapid {
            (self.consecutive_loads + 1).min(3)
        } else {
            0
        };
        self.last_load_more = Some(now);
        let backoff = Duration::from_millis(u64::from(self.consecutive_loads) * 333);

        self.loading_more = true;
        cx.notify();
        tracing::debug!(
            channel_id = channel_id.get(),
            before_message_id = oldest_id.get(),
            backoff_ms = backoff.as_millis() as u64,
            "load_more: fetching older page"
        );

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if !backoff.is_zero() {
                cx.background_executor().timer(backoff).await;
            }
            let result = api
                .list_channel_messages(
                    clan_id.get(),
                    channel_id.get(),
                    oldest_id.get(),
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
                tracing::debug!(
                    channel_id = channel_id.get(),
                    fetched = msgs.len(),
                    "load_more: page received"
                );
                let cfg = AppConfig::try_global(cx);
                let (prepended, dropped_bottom) = {
                    let Some(channel) = this.cache.get_mut(&channel_id) else {
                        return;
                    };
                    let older: Vec<Message> = msgs
                        .into_iter()
                        .filter(|m| !channel.messages.contains_id(MessageId(m.message_id)))
                        .map(|m| message_from_api(m, cfg))
                        .collect();
                    if older.is_empty() {
                        channel.has_more = false;
                        // No more history above: tell the UI so it can drop the
                        // persistent top loading skeleton.
                        cx.emit(MessagesEvent::Updated);
                        cx.notify();
                        return;
                    }
                    let prepended = older.len();
                    let dropped_bottom = channel.messages.prepend_older(older);
                    if dropped_bottom > 0 {
                        channel.has_more_bottom = true;
                    }
                    // Reached the channel start once the oldest row is the
                    // FIRST_MESSAGE sentinel (cf. React `hasMore` check).
                    channel.has_more = has_more_from_oldest(channel.messages.as_slice());
                    (prepended, dropped_bottom)
                };
                if this.active_channel_id == Some(channel_id) {
                    // Older rows were prepended; the cap may have dropped the same
                    // many newest rows off the back. Emit the exact splice so the
                    // UI window matches the buffer 1:1 — the prepend re-anchors to
                    // the prior first row, and the back-trim removes off-screen
                    // rows below.
                    cx.emit(MessagesEvent::Shifted {
                        added_top: prepended,
                        removed_top: 0,
                        added_bottom: 0,
                        removed_bottom: dropped_bottom,
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetch the next newer page from the server and append it (the bottom
    /// counterpart of [`Self::load_more`]). Only active after a jump-to-message,
    /// where `has_more_bottom` is set because the newest message is not loaded.
    pub fn load_more_bottom(&mut self, cx: &mut Context<Self>) {
        if self.loading_more || self.loading {
            tracing::debug!(
                loading_more = self.loading_more,
                loading = self.loading,
                "load_more_bottom skipped: fetch already in flight"
            );
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
        if !channel.has_more_bottom {
            tracing::debug!("load_more_bottom skipped: has_more_bottom=false");
            return;
        }
        let Some(newest_id) = channel
            .messages
            .last()
            .map(|m| m.id)
            .filter(|id| !id.is_optimistic())
        else {
            tracing::debug!("load_more_bottom skipped: no non-optimistic newest id");
            return;
        };

        self.loading_more = true;
        cx.notify();
        tracing::debug!(
            channel_id = channel_id.get(),
            after_message_id = newest_id.get(),
            buffer_len = channel.messages.len(),
            "load_more_bottom: fetching newer page"
        );

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(
                    clan_id.get(),
                    channel_id.get(),
                    newest_id.get(),
                    DIRECTION_AFTER,
                    MESSAGE_PAGE_LIMIT,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let msgs = match result {
                    Ok(msgs) => msgs,
                    Err(e) => {
                        tracing::error!("Failed to load newer messages for {channel_id}: {e}");
                        cx.notify();
                        return;
                    }
                };
                tracing::debug!(
                    channel_id = channel_id.get(),
                    anchor_after = newest_id.get(),
                    fetched = msgs.len(),
                    raw_first = msgs.first().map(|m| m.message_id).unwrap_or(0),
                    raw_last = msgs.last().map(|m| m.message_id).unwrap_or(0),
                    raw_min = msgs.iter().map(|m| m.message_id).min().unwrap_or(0),
                    raw_max = msgs.iter().map(|m| m.message_id).max().unwrap_or(0),
                    "load_more_bottom: page received (raw server ids)"
                );
                let cfg = AppConfig::try_global(cx);
                let (added, dropped) = {
                    let Some(channel) = this.cache.get_mut(&channel_id) else {
                        return;
                    };
                    let fetched = msgs.len();
                    let newer: Vec<Message> = msgs
                        .into_iter()
                        .filter(|m| !channel.messages.contains_id(MessageId(m.message_id)))
                        .map(|m| message_from_api(m, cfg))
                        .collect();
                    // A short page means we've reached the newest message.
                    channel.has_more_bottom = fetched >= MESSAGE_PAGE_LIMIT as usize;
                    if newer.is_empty() {
                        cx.emit(MessagesEvent::Updated);
                        cx.notify();
                        return;
                    }
                    let added = newer.len();
                    // Appending newer drops the oldest (front) at the cap; those
                    // older rows then become re-fetchable from the top again.
                    let dropped = channel.messages.append_newer(newer);
                    if dropped > 0 {
                        channel.has_more = true;
                    }
                    (added, dropped)
                };
                if this.active_channel_id == Some(channel_id) {
                    if let Some(ch) = this.cache.get(&channel_id) {
                        tracing::debug!(
                            anchor_after = newest_id.get(),
                            added,
                            dropped,
                            buffer_oldest = ch.messages.first().map(|m| m.id.get()).unwrap_or(0),
                            buffer_newest = ch.messages.last().map(|m| m.id.get()).unwrap_or(0),
                            "load_more_bottom: appended newer page"
                        );
                    }
                    // Newer rows were appended; the cap may have dropped the same
                    // many oldest rows off the front. Emit the exact splice so the
                    // UI window matches the buffer 1:1 and the scroll stays
                    // anchored to the prior content.
                    cx.emit(MessagesEvent::Shifted {
                        added_top: 0,
                        removed_top: dropped,
                        added_bottom: added,
                        removed_bottom: 0,
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Jump to a message (cf. React `jumpToMessage`, used by reply previews).
    /// If the target is already in the buffer, emit [`MessagesEvent::JumpTo`] so
    /// the UI scrolls to it. Otherwise fetch a window centered on it
    /// (`AROUND_TIMESTAMP`), replace the buffer, and emit `Reset` then `JumpTo`.
    pub fn jump_to_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        if self
            .cache
            .get(&channel_id)
            .is_some_and(|c| c.messages.contains_id(message_id))
        {
            cx.emit(MessagesEvent::JumpTo { message_id });
            return;
        }
        if self.loading_more || self.loading {
            return;
        }
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let anchor = message_id.get();

        self.loading_more = true;
        cx.notify();
        tracing::debug!(
            channel_id = channel_id.get(),
            message_id = anchor,
            "jump_to_message: fetching AROUND window"
        );

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(
                    clan_id.get(),
                    channel_id.get(),
                    anchor,
                    DIRECTION_AROUND,
                    MESSAGE_PAGE_LIMIT,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let msgs = match result {
                    Ok(msgs) => msgs,
                    Err(e) => {
                        tracing::error!(
                            "jump_to_message AROUND fetch failed for {channel_id}: {e}"
                        );
                        cx.notify();
                        return;
                    }
                };
                let cfg = AppConfig::try_global(cx);
                let mut window: Vec<Message> =
                    msgs.into_iter().map(|m| message_from_api(m, cfg)).collect();
                sort_messages(&mut window);
                // Centered trim if the window somehow exceeds the cap, keeping the
                // target near the middle so both directions stay scrollable.
                if window.len() > MAX_MESSAGES_PER_CHANNEL {
                    let target = window.iter().position(|m| m.id == message_id).unwrap_or(0);
                    let half = MAX_MESSAGES_PER_CHANNEL / 2;
                    let start = target
                        .saturating_sub(half)
                        .min(window.len() - MAX_MESSAGES_PER_CHANNEL);
                    window = window[start..start + MAX_MESSAGES_PER_CHANNEL].to_vec();
                }
                let found = window.iter().any(|m| m.id == message_id);
                if !found {
                    tracing::warn!(
                        message_id = anchor,
                        "jump_to_message: target not in AROUND window"
                    );
                    cx.notify();
                    return;
                }
                recompute_message_grouping(&mut window);
                let has_more = has_more_from_oldest(&window);
                if let Some(channel) = this.cache.get_mut(&channel_id) {
                    channel.messages.replace(window);
                    channel.has_more = has_more;
                    // We landed on an older window, so newer messages exist that
                    // are not loaded yet (scroll down pages them in). This
                    // self-corrects to false once the newest page is reached.
                    channel.has_more_bottom = true;
                }
                if this.active_channel_id == Some(channel_id) {
                    let count = this.messages().len();
                    cx.emit(MessagesEvent::Reset { count });
                    cx.emit(MessagesEvent::JumpTo { message_id });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Current composer reply target (React reply reference draft).
    pub fn reply_target(&self) -> Option<&ReplyDraft> {
        self.reply_target.as_ref()
    }

    /// Set the composer reply target (from a "Reply" action on a message).
    pub fn set_reply(&mut self, draft: ReplyDraft, cx: &mut Context<Self>) {
        self.reply_target = Some(draft);
        cx.notify();
    }

    /// Clear the composer reply target.
    pub fn clear_reply(&mut self, cx: &mut Context<Self>) {
        if self.reply_target.take().is_some() {
            cx.notify();
        }
    }

    pub fn send_message(
        &mut self,
        content: String,
        sender_id: String,
        sender_name: String,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
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
        let has_attachments = !attachments.is_empty();
        let reply = self.reply_target.take();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        let temp_id = MessageId::next_optimistic();

        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let OutgoingContent {
            mentions,
            hashtags,
            emojis,
        } = content_tokens;
        let transport_mentions: Vec<TransportMention> = mentions
            .into_iter()
            .map(OutgoingMention::into_transport)
            .collect();
        let transport_hashtags: Vec<TransportHashtag> = hashtags
            .into_iter()
            .map(OutgoingHashtag::into_transport)
            .collect();
        let transport_emojis: Vec<TransportEmoji> = emojis
            .into_iter()
            .map(OutgoingEmoji::into_transport)
            .collect();
        let markdowns = detect_markdown(&content);
        let mut optimistic =
            enrich_outgoing_sender(
                Message::new(temp_id, content.clone(), sender_id, sender_name, now),
                cx,
                self.active_clan_id,
            );
        if !transport_mentions.is_empty()
            || !transport_hashtags.is_empty()
            || !transport_emojis.is_empty()
            || !markdowns.is_empty()
        {
            let tokens = ApiMessageContent {
                t: content.clone(),
                mentions: mention_content_tokens(&transport_mentions),
                hg: hashtag_content_tokens(&transport_hashtags),
                ej: emoji_content_tokens(&transport_emojis),
                mk: markdown_content_tokens(&markdowns),
                ..Default::default()
            };
            optimistic = optimistic.with_spans(parse_spans(&tokens));
        }
        if let Some(draft) = &reply {
            optimistic = optimistic.with_references(vec![MessageReference {
                message_ref_id: draft.message_ref_id,
                sender_id: draft.sender_id,
                sender_name: draft.sender_name.clone(),
                sender_avatar: draft.sender_avatar.clone(),
                content: draft.content_preview.clone(),
                has_attachment: draft.has_attachment,
            }]);
        }
        let old_len = channel.messages.len();
        channel.messages.push_trim_regroup(optimistic);
        self.emit_appended(old_len, cx);

        let api = self.api.clone();
        let reply_ref = reply.map(|draft| OutgoingReply {
            message_ref_id: draft.message_ref_id.get(),
            content: draft.content_preview,
            has_attachment: draft.has_attachment,
            message_sender_id: draft.sender_id.get(),
            message_sender_username: draft.sender_name.clone(),
            message_sender_avatar: draft.sender_avatar,
            message_sender_clan_nick: String::new(),
            message_sender_display_name: draft.sender_name,
        });
        cx.spawn(async move |this, cx| {
            let result = if has_attachments {
                let files = cx
                    .background_spawn(async move {
                        attachments
                            .into_iter()
                            .filter_map(|att| {
                                std::fs::read(&att.path)
                                    .inspect_err(|e| {
                                        tracing::error!(
                                            "attachment read failed for {:?}: {e}",
                                            att.path
                                        )
                                    })
                                    .ok()
                                    .map(|data| UploadFile {
                                        filename: att.filename,
                                        filetype: att.filetype,
                                        data,
                                    })
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                api.send_message_with_attachments(
                    clan_id.get(),
                    channel_id.get(),
                    &content,
                    is_public,
                    mode,
                    files,
                )
                .await
            } else if let Some(reply_ref) = reply_ref {
                api.send_channel_message_reply(
                    clan_id.get(),
                    channel_id.get(),
                    &content,
                    is_public,
                    mode,
                    reply_ref,
                    transport_mentions,
                    transport_hashtags,
                    transport_emojis,
                )
                .await
            } else {
                api.send_channel_message(
                    clan_id.get(),
                    channel_id.get(),
                    &content,
                    is_public,
                    mode,
                    transport_mentions,
                    transport_hashtags,
                    transport_emojis,
                )
                .await
            };
            match result {
                Ok(sent) => {
                    let _ = this.update(cx, |this, cx| {
                        let confirmed = message_from_api(sent, AppConfig::try_global(cx));
                        this.reconcile_temp(channel_id, temp_id, confirmed, cx);
                    });
                }
                Err(e) => {
                    tracing::error!("send_channel_message failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.remove_temp(channel_id, temp_id, cx);
                    });
                }
            }
        })
        .detach();
    }

    pub fn send_sticker(
        &mut self,
        url: String,
        filename: String,
        sender_id: String,
        sender_name: String,
        cx: &mut Context<Self>,
    ) {
        if url.is_empty() {
            return;
        }
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
        let temp_id = MessageId::next_optimistic();

        let optimistic_attachment = MessageAttachment::from_api(
            mezon_client::transport::ApiAttachment {
                url: url.clone(),
                filename: filename.clone(),
                filetype: STICKER_FILETYPE.to_string(),
                width: 0,
                height: 0,
            },
            AppConfig::try_global(cx),
        );

        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let optimistic = enrich_outgoing_sender(
            Message::new(temp_id, String::new(), sender_id, sender_name, now)
                .with_attachments(vec![optimistic_attachment]),
            cx,
            self.active_clan_id,
        );
        let old_len = channel.messages.len();
        channel.messages.push_trim_regroup(optimistic);
        self.emit_appended(old_len, cx);

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .send_message_with_attachment_urls(
                    clan_id.get(),
                    channel_id.get(),
                    is_public,
                    mode,
                    vec![UrlAttachment {
                        url,
                        filename,
                        filetype: STICKER_FILETYPE.to_string(),
                    }],
                )
                .await;
            match result {
                Ok(sent) => {
                    let _ = this.update(cx, |this, cx| {
                        let confirmed = message_from_api(sent, AppConfig::try_global(cx));
                        this.reconcile_temp(channel_id, temp_id, confirmed, cx);
                    });
                }
                Err(e) => {
                    tracing::error!("send sticker failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.remove_temp(channel_id, temp_id, cx);
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
            self.reply_target = None;
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
        self.reply_target = None;
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
                self.set_channel(channel_id, messages);
                if is_current {
                    self.loading = false;
                    let count = self.messages().len();
                    cx.emit(MessagesEvent::Reset { count });
                    cx.notify();
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch messages for {channel_id}: {e}");
                if is_current {
                    self.loading = false;
                    let count = self.messages().len();
                    cx.emit(MessagesEvent::Reset { count });
                    cx.notify();
                }
            }
        }
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
        if channel.messages.contains_id(msg.id) {
            if let Some(idx) = channel.messages.position(msg.id) {
                let existing = channel.messages.as_slice()[idx].clone();
                if message_needs_enrichment(&existing) {
                    channel.messages.replace_at(idx, msg);
                    if is_active {
                        cx.emit(MessagesEvent::Updated);
                        cx.notify();
                    }
                }
            }
            return;
        }
        let old_len = channel.messages.len();
        let appended = match channel
            .messages
            .temp_match_position(&msg.sender_id, &msg.content)
        {
            Some(idx) => {
                channel.messages.replace_resort(idx, msg);
                false
            }
            None => {
                channel.messages.push_grouped(msg);
                true
            }
        };
        if is_active {
            if appended {
                self.emit_appended(old_len, cx);
            } else {
                cx.emit(MessagesEvent::Updated);
                cx.notify();
            }
        }
    }

    fn handle_reaction(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::MessageReaction(r) = event else {
            return;
        };
        let channel_id = ChannelId(r.channel_id);
        let is_active = self.active_channel_id == Some(channel_id);
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_mut_by_id(MessageId(r.message_id)) else {
            return;
        };
        apply_reaction_event(
            &mut msg.reactions,
            &r.emoji_id.to_string(),
            &r.emoji,
            &r.sender_id.to_string(),
            r.action,
        );
        if is_active {
            cx.emit(MessagesEvent::Updated);
            cx.notify();
        }
    }

    fn reconcile_temp(
        &mut self,
        channel_id: ChannelId,
        temp_id: MessageId,
        confirmed: Message,
        cx: &mut Context<Self>,
    ) {
        let (pushed, old_len) = {
            let Some(channel) = self.cache.get_mut(&channel_id) else {
                return;
            };
            let old_len = channel.messages.len();
            if let Some(idx) = channel.messages.position(temp_id) {
                let temp = channel.messages.as_slice()[idx].clone();
                let merged = merge_confirmed_with_temp(&temp, confirmed);
                channel.messages.replace_at(idx, merged);
                (false, old_len)
            } else if !channel.messages.contains_id(confirmed.id) {
                channel.messages.push_sorted(confirmed);
                (true, old_len)
            } else {
                (false, old_len)
            }
        };
        if self.active_channel_id != Some(channel_id) {
            return;
        }
        if pushed {
            self.emit_appended(old_len, cx);
        } else {
            cx.emit(MessagesEvent::Updated);
            cx.notify();
        }
    }

    fn remove_temp(&mut self, channel_id: ChannelId, temp_id: MessageId, cx: &mut Context<Self>) {
        let removed = {
            let Some(channel) = self.cache.get_mut(&channel_id) else {
                return;
            };
            channel.messages.remove_id(temp_id)
        };
        if removed && self.active_channel_id == Some(channel_id) {
            cx.emit(MessagesEvent::Shifted {
                added_top: 0,
                removed_top: 0,
                added_bottom: 0,
                removed_bottom: 1,
            });
            cx.notify();
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
        let has_more = has_more_from_oldest(&messages);
        self.cache.insert(
            channel_id,
            ChannelMessages {
                messages: MessageList::from_messages(messages),
                has_more,
                // Normal open loads the newest page, so nothing newer exists yet.
                // Jump-to-message will set this when it loads an older window.
                has_more_bottom: false,
            },
            active.as_ref(),
        );
    }
}

/// Whether there is more history above the loaded buffer, mirroring React
/// `hasMore = lastLoadMessage?.code !== EMessageCode.FIRST_MESSAGE`
/// (`messages.slice.ts`). The very first message of a channel carries code 4
/// (`FIRST_MESSAGE`, which we map to `MessageCode::Indicator`); once it is the
/// oldest loaded row there is nothing older to fetch. An empty buffer has
/// nothing more to load.
fn has_more_from_oldest(messages: &[Message]) -> bool {
    messages
        .first()
        .is_some_and(|m| m.code != MessageCode::Indicator)
}

fn outgoing_sender_profile(cx: &App, clan_id: Option<ClanId>, fallback_name: &str) -> (String, String) {
    let store = AccountStore::global(cx).read(cx);
    let account = store.account.as_ref();
    let clan = store
        .clan_profile
        .as_ref()
        .filter(|profile| clan_id.is_none_or(|id| profile.clan_id == id));

    let name = clan
        .and_then(|profile| {
            (!profile.nick_name.is_empty()).then(|| profile.nick_name.clone())
        })
        .or_else(|| {
            account.map(|acct| {
                if !acct.display_name.is_empty() {
                    acct.display_name.clone()
                } else if !acct.username.is_empty() {
                    acct.username.clone()
                } else {
                    fallback_name.to_string()
                }
            })
        })
        .unwrap_or_else(|| fallback_name.to_string());

    let avatar = prioritize_avatar(
        clan
            .and_then(|profile| profile.avatar_url.as_deref())
            .unwrap_or(""),
        account
            .and_then(|acct| acct.avatar_url.as_deref())
            .unwrap_or(""),
    );

    (name, avatar)
}

fn enrich_outgoing_sender(
    mut msg: Message,
    cx: &App,
    clan_id: Option<ClanId>,
) -> Message {
    let (name, avatar) = outgoing_sender_profile(cx, clan_id, &msg.sender_name);
    if !name.is_empty() {
        msg.sender_name = name;
    }
    if !avatar.is_empty() {
        let proxied = AppConfig::try_global(cx)
            .map(|cfg| cfg.avatar_proxy(&avatar))
            .unwrap_or_else(|| avatar.clone());
        msg = msg.with_avatar(avatar).with_avatar_proxied(proxied);
    }
    msg
}

fn message_needs_enrichment(msg: &Message) -> bool {
    msg.sender_name.is_empty()
        || (msg.avatar_url.is_empty() && msg.avatar_proxied.is_empty())
        || msg.sender_id.is_empty()
        || msg.sender_id == "0"
        || msg.create_time == 0
}

fn merge_confirmed_with_temp(temp: &Message, mut confirmed: Message) -> Message {
    if confirmed.sender_name.is_empty() {
        confirmed.sender_name = temp.sender_name.clone();
    }
    if confirmed.sender_id.is_empty() || confirmed.sender_id == "0" {
        confirmed.sender_id = temp.sender_id.clone();
        confirmed.sender_user_id = temp.sender_user_id;
    }
    if confirmed.avatar_url.is_empty() && !temp.avatar_url.is_empty() {
        confirmed.avatar_url = temp.avatar_url.clone();
        confirmed.avatar_proxied = temp.avatar_proxied.clone();
    }
    if confirmed.create_time == 0 && temp.create_time != 0 {
        confirmed.create_time = temp.create_time;
        confirmed.timestamp_label = temp.timestamp_label.clone();
        confirmed.day_label = temp.day_label.clone();
    }
    if confirmed.spans.is_empty() && !temp.spans.is_empty() {
        confirmed.spans = temp.spans.clone();
    }
    confirmed
}

fn prepare_messages(msgs: Vec<ApiMessage>, cfg: Option<&AppConfig>) -> Vec<Message> {
    let mut messages: Vec<Message> = msgs.into_iter().map(|m| message_from_api(m, cfg)).collect();
    sort_messages(&mut messages);
    trim_messages(&mut messages);
    recompute_message_grouping(&mut messages);
    messages
}

/// Cap the buffer to `MAX_MESSAGES_PER_CHANNEL`, dropping the oldest rows.
/// Returns how many rows were dropped from the front. Used when newer rows are
/// appended (the window slides toward the present).
fn trim_messages(messages: &mut Vec<Message>) -> usize {
    if messages.len() <= MAX_MESSAGES_PER_CHANNEL {
        return 0;
    }
    let drop = messages.len() - MAX_MESSAGES_PER_CHANNEL;
    messages.drain(0..drop);
    drop
}

/// Cap the buffer to `MAX_MESSAGES_PER_CHANNEL`, dropping the newest rows.
/// Returns how many rows were dropped from the back. Used when older rows are
/// prepended (the window slides toward history) so the just-loaded older rows
/// are kept; the dropped newest rows can be re-fetched via `load_more_bottom`.
fn trim_messages_back(messages: &mut Vec<Message>) -> usize {
    if messages.len() <= MAX_MESSAGES_PER_CHANNEL {
        return 0;
    }
    let drop = messages.len() - MAX_MESSAGES_PER_CHANNEL;
    messages.truncate(MAX_MESSAGES_PER_CHANNEL);
    drop
}

fn message_from_api(m: ApiMessage, cfg: Option<&AppConfig>) -> Message {
    let avatar_proxied = cfg
        .map(|c| c.avatar_proxy(&m.avatar))
        .unwrap_or_else(|| m.avatar.clone());
    let spans = parse_spans(&m.content_tokens);
    let references = m
        .references
        .iter()
        .map(|r| message_reference_from_api(r, cfg))
        .collect();
    let reactions = aggregate_reactions(&m.reactions);
    Message::new(
        MessageId(m.message_id),
        m.content,
        m.sender_id.to_string(),
        m.sender_name,
        m.create_time,
    )
    .with_code(MessageCode::from_raw(m.code))
    .with_spans(spans)
    .with_references(references)
    .with_reactions(reactions)
    .with_edited(m.update_time, m.hide_editted)
    .with_avatar(m.avatar)
    .with_avatar_proxied(avatar_proxied)
    .with_attachments(
        m.attachments
            .into_iter()
            .map(|a| MessageAttachment::from_api(a, cfg))
            .collect(),
    )
}

fn message_reference_from_api(
    r: &mezon_client::transport::ApiMessageRef,
    cfg: Option<&AppConfig>,
) -> MessageReference {
    let sender_name = if !r.message_sender_clan_nick.is_empty() {
        r.message_sender_clan_nick.clone()
    } else if !r.message_sender_display_name.is_empty() {
        r.message_sender_display_name.clone()
    } else {
        r.message_sender_username.clone()
    };
    // The reference content is itself a JSON `IExtendedMessage`; extract its text.
    let content = serde_json::from_str::<mezon_client::transport::ApiMessageContent>(&r.content)
        .map(|c| c.t)
        .unwrap_or_else(|_| r.content.clone());
    let sender_avatar = cfg
        .map(|c| c.avatar_proxy(&r.message_sender_avatar))
        .unwrap_or_else(|| r.message_sender_avatar.clone());
    MessageReference {
        message_ref_id: MessageId(r.message_ref_id),
        sender_id: UserId(r.message_sender_id),
        sender_name,
        sender_avatar,
        content,
        has_attachment: r.has_attachment,
    }
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
    use crate::ids::UserId;
    use crate::message::MessageSpan;

    #[test]
    fn outgoing_mention_maps_to_transport_with_utf16_offsets() {
        let mention = OutgoingMention {
            user_id: "42".into(),
            role_id: String::new(),
            display: "@bob".into(),
            s: 2,
            e: 6,
        };
        let transport = mention.into_transport();
        assert_eq!(transport.user_id, "42");
        assert_eq!(transport.username, "@bob");
        assert_eq!(transport.s, 2);
        assert_eq!(transport.e, 6);
    }

    #[test]
    fn sticker_attachment_is_recognized_as_image() {
        let attachment = MessageAttachment::from_api(
            mezon_client::transport::ApiAttachment {
                url: "https://cdn/1.webp".into(),
                filename: "1".into(),
                filetype: STICKER_FILETYPE.into(),
                width: 0,
                height: 0,
            },
            None,
        );
        assert_eq!(attachment.filetype, "sticker");
        assert_eq!(attachment.url, "https://cdn/1.webp");
        assert!(attachment.is_image());
        assert_eq!(
            (attachment.display_width, attachment.display_height),
            (280.0, 150.0)
        );
    }

    #[test]
    fn optimistic_mention_tokens_round_trip_to_a_coloured_span() {
        let mentions = vec![OutgoingMention {
            user_id: "42".into(),
            role_id: String::new(),
            display: "@bob".into(),
            s: 0,
            e: 4,
        }];
        let transport: Vec<TransportMention> = mentions
            .into_iter()
            .map(OutgoingMention::into_transport)
            .collect();
        let tokens = ApiMessageContent {
            t: "@bob hi".into(),
            mentions: mention_content_tokens(&transport),
            ..Default::default()
        };
        let spans = parse_spans(&tokens);
        assert_eq!(
            spans,
            vec![
                MessageSpan::Mention {
                    display: "@bob".into(),
                    user_id: Some("42".into()),
                    role_id: None,
                },
                MessageSpan::Text(" hi".into()),
            ]
        );
    }

    #[test]
    fn message_from_api_maps_fields() {
        let m = message_from_api(
            ApiMessage {
                message_id: 1,
                content: "hi".into(),
                content_tokens: mezon_client::transport::ApiMessageContent {
                    t: "hi".into(),
                    ..Default::default()
                },
                code: 0,
                sender_id: 1,
                sender_name: "Alice".into(),
                avatar: "av.png".into(),
                create_time: 100,
                update_time: 0,
                hide_editted: false,
                attachments: vec![],
                references: vec![],
                reactions: vec![],
            },
            None,
        );
        assert_eq!(m.id, MessageId(1));
        assert_eq!(m.content, "hi");
        assert_eq!(m.sender_id, "1");
        assert_eq!(m.sender_user_id, Some(UserId(1)));
        assert_eq!(m.sender_name, "Alice");
        assert_eq!(m.avatar_url, "av.png");
        assert_eq!(m.avatar_proxied, "av.png");
    }

    fn assert_list_consistent(list: &MessageList) {
        assert_eq!(list.index.len(), list.items.len());
        for (i, m) in list.items.iter().enumerate() {
            assert_eq!(list.index.get(&m.id), Some(&i));
        }
        let mut expected_temps: Vec<MessageId> = list
            .items
            .iter()
            .filter(|m| m.id.is_optimistic())
            .map(|m| m.id)
            .collect();
        let mut actual_temps: Vec<MessageId> = list.temp_ids.clone();
        actual_temps.sort();
        expected_temps.sort();
        assert_eq!(actual_temps, expected_temps);
    }

    #[test]
    fn push_message_grouped_appends_in_order() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(1), "a", "u1", "U1", 100),
            Message::new(MessageId(2), "b", "u1", "U1", 110),
        ]);
        list.push_grouped(Message::new(MessageId(3), "c", "u1", "U1", 120));
        assert_eq!(list.len(), 3);
        assert_eq!(list.as_slice()[2].id, MessageId(3));
        assert!(list.as_slice()[2].combined_with_prev);
        assert_list_consistent(&list);
    }

    #[test]
    fn push_message_grouped_resorts_when_out_of_order() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(1), "a", "u1", "U1", 100),
            Message::new(MessageId(3), "c", "u1", "U1", 120),
        ]);
        list.push_grouped(Message::new(MessageId(2), "b", "u1", "U1", 110));
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(ids, [MessageId(1), MessageId(2), MessageId(3)]);
        assert_list_consistent(&list);
    }

    #[test]
    fn push_message_grouped_breaks_group_for_different_sender() {
        let mut list =
            MessageList::from_messages(vec![Message::new(MessageId(1), "a", "u1", "U1", 100)]);
        list.push_grouped(Message::new(MessageId(2), "b", "u2", "U2", 105));
        assert!(!list.as_slice()[1].combined_with_prev);
        assert_list_consistent(&list);
    }

    #[test]
    fn trim_messages_drops_oldest() {
        let mut msgs: Vec<Message> = (0..MAX_MESSAGES_PER_CHANNEL + 5)
            .map(|i| Message::new(MessageId(i as i64), format!("m{i}"), "u", "User", i as i64))
            .collect();
        trim_messages(&mut msgs);
        assert_eq!(msgs.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(msgs.first().unwrap().id, MessageId(5));
        assert_eq!(
            msgs.last().unwrap().id,
            MessageId((MAX_MESSAGES_PER_CHANNEL + 4) as i64)
        );
    }

    fn channel_msgs(msgs: Vec<Message>) -> ChannelMessages {
        ChannelMessages {
            messages: MessageList::from_messages(msgs),
            has_more: false,
            has_more_bottom: false,
        }
    }

    fn remove_temp_in(ch: &mut ChannelMessages, temp_id: MessageId) {
        ch.messages.remove_id(temp_id);
    }

    fn reconcile_temp_in(ch: &mut ChannelMessages, temp_id: MessageId, confirmed: Message) {
        if let Some(idx) = ch.messages.position(temp_id) {
            let temp = ch.messages.as_slice()[idx].clone();
            let merged = merge_confirmed_with_temp(&temp, confirmed);
            ch.messages.replace_at(idx, merged);
        } else if !ch.messages.contains_id(confirmed.id) {
            ch.messages.push_sorted(confirmed);
        }
    }

    #[test]
    fn remove_temp_drops_message_by_id() {
        let temp1 = MessageId::next_optimistic();
        let mut ch = channel_msgs(vec![
            Message::new(temp1, "hello", "u1", "U", 100),
            Message::new(MessageId(2), "world", "u1", "U", 200),
        ]);
        remove_temp_in(&mut ch, temp1);
        assert_eq!(ch.messages.len(), 1);
        assert_eq!(ch.messages.as_slice()[0].id, MessageId(2));
        assert_list_consistent(&ch.messages);
    }

    #[test]
    fn remove_temp_noop_when_id_not_found() {
        let non_existent = MessageId::next_optimistic();
        let mut ch = channel_msgs(vec![Message::new(MessageId(1), "hello", "u1", "U", 100)]);
        remove_temp_in(&mut ch, non_existent);
        assert_eq!(ch.messages.len(), 1);
    }

    #[test]
    fn reconcile_temp_preserves_sender_from_optimistic_when_ack_sparse() {
        let temp_id = MessageId::next_optimistic();
        let temp = Message::new(temp_id, "hello", "42", "gia.chuvan", 1_700_000_000)
            .with_avatar("avatar.png")
            .with_avatar_proxied(gpui::SharedString::from("proxy.png"));
        let mut ch = channel_msgs(vec![temp]);
        let sparse_ack = Message::new(MessageId(99), "hello", "0", String::new(), 0);
        reconcile_temp_in(&mut ch, temp_id, sparse_ack);
        let row = &ch.messages.as_slice()[0];
        assert_eq!(row.id, MessageId(99));
        assert_eq!(row.sender_id, "42");
        assert_eq!(row.sender_name, "gia.chuvan");
        assert_eq!(row.avatar_url, "avatar.png");
        assert_eq!(row.create_time, 1_700_000_000);
    }

    #[test]
    fn reconcile_temp_matches_only_by_temp_id_not_content() {
        let temp1 = MessageId::next_optimistic();
        let temp2 = MessageId::next_optimistic();
        let mut ch = channel_msgs(vec![
            Message::new(temp1, "same text", "u1", "U", 100),
            Message::new(temp2, "same text", "u1", "U", 110),
        ]);
        let confirmed = Message::new(MessageId(42), "same text", "u1", "U", 120);
        reconcile_temp_in(&mut ch, temp1, confirmed);
        assert_eq!(ch.messages.len(), 2);
        assert_eq!(ch.messages.as_slice()[0].id, MessageId(42));
        assert_eq!(ch.messages.as_slice()[1].id, temp2);
        assert_list_consistent(&ch.messages);
    }

    #[test]
    fn temp_match_reconciles_optimistic_row_in_place() {
        let temp1 = MessageId::next_optimistic();
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(100), "earlier", "u1", "U", 100),
            Message::new(temp1, "hello world", "u9", "Me", 200),
        ]);
        assert_eq!(list.temp_match_position("u9", "hello world"), Some(1));
        assert_eq!(list.temp_match_position("u9", "other"), None);
        let idx = list.temp_match_position("u9", "hello world").unwrap();
        list.replace_resort(
            idx,
            Message::new(MessageId(250), "hello world", "u9", "Me", 200),
        );
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(ids, [MessageId(100), MessageId(250)]);
        assert!(list.temp_ids.is_empty());
        assert_list_consistent(&list);
    }

    #[test]
    fn append_update_remove_keep_index_and_order() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(10), "a", "u1", "U", 100),
            Message::new(MessageId(20), "b", "u1", "U", 110),
        ]);
        list.push_grouped(Message::new(MessageId(30), "c", "u1", "U", 120));
        assert_eq!(list.position(MessageId(30)), Some(2));
        list.get_mut_by_id(MessageId(20)).unwrap().content = "edited".into();
        assert_eq!(list.as_slice()[1].content, "edited");
        assert!(list.remove_id(MessageId(10)));
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(ids, [MessageId(20), MessageId(30)]);
        assert_eq!(list.position(MessageId(10)), None);
        assert_list_consistent(&list);
    }

    #[test]
    fn prepend_older_and_append_newer_preserve_order_and_index() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(50), "e", "u1", "U", 150),
            Message::new(MessageId(60), "f", "u1", "U", 160),
        ]);
        let dropped = list.prepend_older(vec![
            Message::new(MessageId(30), "c", "u1", "U", 130),
            Message::new(MessageId(40), "d", "u1", "U", 140),
        ]);
        assert_eq!(dropped, 0);
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            [MessageId(30), MessageId(40), MessageId(50), MessageId(60)]
        );
        assert_list_consistent(&list);
        list.append_newer(vec![Message::new(MessageId(70), "g", "u1", "U", 170)]);
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            [
                MessageId(30),
                MessageId(40),
                MessageId(50),
                MessageId(60),
                MessageId(70)
            ]
        );
        assert_eq!(list.position(MessageId(70)), Some(4));
        assert_list_consistent(&list);
    }

    #[test]
    fn window_replace_rebuilds_index() {
        let mut list =
            MessageList::from_messages(vec![Message::new(MessageId(1), "a", "u1", "U", 100)]);
        list.replace(vec![
            Message::new(MessageId(8), "h", "u1", "U", 180),
            Message::new(MessageId(9), "i", "u1", "U", 190),
        ]);
        assert_eq!(list.position(MessageId(1)), None);
        assert_eq!(list.position(MessageId(8)), Some(0));
        assert_eq!(list.position(MessageId(9)), Some(1));
        assert_list_consistent(&list);
    }

    #[test]
    fn append_at_cap_evicts_front_and_reindexes() {
        let mut list = MessageList::from_messages(
            (0..MAX_MESSAGES_PER_CHANNEL)
                .map(|i| Message::new(MessageId(i as i64), "m", "u", "U", i as i64))
                .collect(),
        );
        list.push_grouped(Message::new(
            MessageId(MAX_MESSAGES_PER_CHANNEL as i64),
            "newest",
            "u",
            "U",
            MAX_MESSAGES_PER_CHANNEL as i64,
        ));
        assert_eq!(list.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(list.position(MessageId(0)), None);
        assert_eq!(list.as_slice()[0].id, MessageId(1));
        assert_eq!(
            list.as_slice().last().unwrap().id,
            MessageId(MAX_MESSAGES_PER_CHANNEL as i64)
        );
        assert_list_consistent(&list);
    }

    #[test]
    fn incremental_in_order_append_at_cap_matches_full_reindex() {
        let temp_old = MessageId::next_optimistic();
        let mut items = vec![Message::new(temp_old, "x", "u", "U", 0)];
        items.extend(
            (1..MAX_MESSAGES_PER_CHANNEL)
                .map(|i| Message::new(MessageId(i as i64), "m", "u", "U", i as i64)),
        );
        let mut list = MessageList::from_messages(items);
        assert_eq!(list.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(list.temp_ids, vec![temp_old]);

        let temp_new = MessageId::next_optimistic();
        list.push_grouped(Message::new(temp_new, "y", "u", "U", 999));

        let incremental_index = list.index.clone();
        let incremental_temp_ids = list.temp_ids.clone();
        list.reindex();
        assert_eq!(list.index, incremental_index);
        assert_eq!(list.temp_ids, incremental_temp_ids);

        assert_eq!(list.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(list.position(temp_old), None);
        assert_eq!(list.temp_ids, vec![temp_new]);
        assert_eq!(list.as_slice()[0].id, MessageId(1));
        assert_eq!(list.as_slice().last().unwrap().id, temp_new);
        assert_list_consistent(&list);
    }
}
