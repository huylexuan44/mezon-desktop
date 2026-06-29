use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, Context, Entity, ListAlignment, ListState, SharedString,
    Subscription, Task, Window, div, ease_in_out, list, prelude::*, px,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use mezon_store::{
    ChannelId, ChannelList, ClanId, ClanList, MessageId, MessagesEvent, MessagesStore,
    ProfileContext, Settings,
};

use super::context::RowCtx;
use super::dispatch::render_message_item;
use super::skeleton::message_skeleton;
use crate::image_cache::{
    AVATAR_ENTRY_MAX_BYTES, AVATAR_IMAGE_CACHE_BYTES, AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache,
    MESSAGE_ENTRY_MAX_BYTES, MESSAGE_IMAGE_CACHE_BYTES, MESSAGE_IMAGE_CACHE_CAPACITY,
};
use crate::theme::{ActiveTheme, Theme};

/// How close (in rows) to an edge of the rendered window before we reveal/fetch
/// more. Approximates React's pixel sensitive area (`MESSAGE_LIST_SENSITIVE_AREA
/// = 1500px`) so loading starts well before the user reaches the very edge,
/// rather than only at the top/bottom row.
const LOAD_MORE_ITEM_THRESHOLD: usize = 12;
/// Pixels of off-screen content rendered/measured above and below the viewport.
/// Chat rows are short and numerous, so a screenful of overdraw keeps fast
/// scrolling smooth without laying out a huge number of rows every frame (Zed's
/// 2048 suits its tall, sparse agent messages, not a dense chat list).
const LIST_OVERDRAW: f32 = 1024.;
const SCROLL_HOVER_RELEASE_MS: u64 = 150;
/// Minimum gap between scroll-triggered pagination attempts. The list emits
/// scroll events at ~120fps while the user holds the scrollbar at an edge;
/// without this, every event would re-trigger reveal/load (flooding the guard
/// and racing through cached history). Leading-edge throttle, like React's
/// debounced `loadMore`.
const PAGINATE_THROTTLE: Duration = Duration::from_millis(250);
const SKELETON_THRESHOLD_MS: u64 = 500;
const SKELETON_FADE_IN_MS: u64 = 150;
const SKELETON_SETTLE_MS: u64 = 250;
const SKELETON_FADE_OUT_MS: u64 = 180;

#[derive(Clone, Copy, PartialEq, Debug)]
enum SkeletonPhase {
    Hidden,
    Pending,
    Showing,
    Settling,
    FadingOut,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum SkeletonTimer {
    Keep,
    Drop,
    Threshold,
    Settle,
    Unmount,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SkeletonKey {
    #[default]
    None,
    Clan(ClanId),
    Conversation(ChannelId),
}

#[derive(Clone, Copy)]
enum SkeletonInput {
    Sync { loading: bool, same_key: bool },
    ThresholdElapsed,
    SettleElapsed,
    UnmountElapsed,
}

enum TimerAction {
    Keep,
    Clear,
    Arm(SkeletonInput, u64),
}

fn skeleton_timer_action(timer: SkeletonTimer) -> TimerAction {
    match timer {
        SkeletonTimer::Keep => TimerAction::Keep,
        SkeletonTimer::Drop => TimerAction::Clear,
        SkeletonTimer::Threshold => {
            TimerAction::Arm(SkeletonInput::ThresholdElapsed, SKELETON_THRESHOLD_MS)
        }
        SkeletonTimer::Settle => TimerAction::Arm(SkeletonInput::SettleElapsed, SKELETON_SETTLE_MS),
        SkeletonTimer::Unmount => {
            TimerAction::Arm(SkeletonInput::UnmountElapsed, SKELETON_FADE_OUT_MS)
        }
    }
}

fn skeleton_transition(
    phase: SkeletonPhase,
    input: SkeletonInput,
) -> (SkeletonPhase, SkeletonTimer) {
    match input {
        SkeletonInput::Sync { loading, same_key } => {
            let phase = if same_key {
                phase
            } else {
                SkeletonPhase::Hidden
            };
            match (phase, loading) {
                (SkeletonPhase::Hidden, true) => (SkeletonPhase::Pending, SkeletonTimer::Threshold),
                (SkeletonPhase::Hidden, false) => (
                    SkeletonPhase::Hidden,
                    if same_key {
                        SkeletonTimer::Keep
                    } else {
                        SkeletonTimer::Drop
                    },
                ),
                (SkeletonPhase::Pending, true) => (SkeletonPhase::Pending, SkeletonTimer::Keep),
                (SkeletonPhase::Pending, false) => (SkeletonPhase::Hidden, SkeletonTimer::Drop),
                (SkeletonPhase::Showing, true) => (SkeletonPhase::Showing, SkeletonTimer::Keep),
                (SkeletonPhase::Showing, false) => (SkeletonPhase::Settling, SkeletonTimer::Settle),
                (SkeletonPhase::Settling, true) => (SkeletonPhase::Showing, SkeletonTimer::Drop),
                (SkeletonPhase::Settling, false) => (SkeletonPhase::Settling, SkeletonTimer::Keep),
                (SkeletonPhase::FadingOut, true) => (SkeletonPhase::Showing, SkeletonTimer::Drop),
                (SkeletonPhase::FadingOut, false) => {
                    (SkeletonPhase::FadingOut, SkeletonTimer::Keep)
                }
            }
        }
        SkeletonInput::ThresholdElapsed => match phase {
            SkeletonPhase::Pending => (SkeletonPhase::Showing, SkeletonTimer::Keep),
            other => (other, SkeletonTimer::Keep),
        },
        SkeletonInput::SettleElapsed => match phase {
            SkeletonPhase::Settling => (SkeletonPhase::FadingOut, SkeletonTimer::Unmount),
            other => (other, SkeletonTimer::Keep),
        },
        SkeletonInput::UnmountElapsed => match phase {
            SkeletonPhase::FadingOut => (SkeletonPhase::Hidden, SkeletonTimer::Keep),
            other => (other, SkeletonTimer::Keep),
        },
    }
}

/// The chat message timeline. It renders the store's bounded viewport window
/// (≤ `VIEWPORT_LIMIT` rows — the React `ChannelMessages` viewport slice) via a
/// bottom-aligned `gpui::list`, which provides correct scroll anchoring and
/// tail-follow. The `ListState` is only mutated from `MessagesEvent`s so renders
/// stay flicker-free.
pub struct ChannelMessages {
    pub(crate) list_state: ListState,
    settings: Entity<Settings>,
    image_cache: Entity<LruImageCache>,
    avatar_image_cache: Entity<LruImageCache>,
    cached_for_channel: Option<ChannelId>,
    skeleton_phase: SkeletonPhase,
    skeleton_key: SkeletonKey,
    last_cold_inputs: (Option<ClanId>, bool, bool),
    _channel_list_observe: Subscription,
    _clan_list_observe: Subscription,
    _skeleton_timer: Option<Task<()>>,
    suppress_hover: bool,
    _hover_release_task: Option<Task<()>>,
    /// Last time a scroll gesture triggered a reveal/load (throttle).
    last_paginate: Option<Instant>,
    /// Last scroll event time, used to release hover suppression after idle.
    last_scroll_at: Option<Instant>,
    /// Whether the list is auto-following the tail. Only enabled when the window
    /// reaches the true newest message; disabled while `has_more_bottom` so
    /// scrolling down loads newer pages instead of snapping to the bottom.
    /// Whether the newest message is currently in view, so appended messages
    /// should keep the bottom pinned (React: scroll to bottom when not viewing
    /// older). Maintained from the scroll handler.
    at_bottom: bool,
    /// Absolute list index of the first visible row, captured from the latest
    /// scroll event. Used to re-anchor the viewport after a forward-page append
    /// (the `ListAlignment::Bottom` anchor is `None` at the edge, so `splice`
    /// can't keep the position and the list would re-pin to the new bottom).
    last_visible_start: usize,
    /// Whether the persistent top loading skeleton occupies list index 0
    /// (React `LoadingSkeletonMessages`, shown while there is more history).
    header_shown: bool,
    /// A message to scroll to on the next render (set from `MessagesEvent::
    /// JumpTo`). Deferred to render so the header reconcile has settled and the
    /// row index is correct.
    pending_jump: Option<MessageId>,
    highlight_id: Option<MessageId>,
    _highlight_timer: Option<Task<()>>,
}

impl ChannelMessages {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();

        let channel_list = ChannelList::global(cx);
        let channel_list_observe = cx.observe(&channel_list, |this, _, cx| this.reconcile_cold(cx));

        let clan_list = ClanList::global(cx);
        let clan_list_observe = cx.observe(&clan_list, |this, _, cx| this.reconcile_cold(cx));

        let store = MessagesStore::global(cx);
        cx.subscribe(&store, |this, _store, event, cx| {
            match event {
                MessagesEvent::Reset { count } => {
                    this.list_state.reset(*count);
                    // Open pinned to the newest message (React opens a channel at
                    // the bottom). We never use gpui's auto-tail; instead we scroll
                    // explicitly so loading older/newer pages never yanks the view
                    // to an edge.
                    this.list_state.scroll_to_end();
                    this.at_bottom = true;
                    // `reset` drops every row including the header; render re-adds
                    // it if more history exists.
                    this.header_shown = false;
                }
                MessagesEvent::Shifted {
                    added_top,
                    removed_top,
                    added_bottom,
                    removed_bottom,
                } => {
                    // The header (if any) sits at index 0; messages follow it.
                    let h = usize::from(this.header_shown);
                    if *removed_top > 0 {
                        this.list_state.splice(h..h + *removed_top, 0);
                    }
                    if *added_top > 0 {
                        this.list_state.splice(h..h, *added_top);
                    }
                    if *removed_bottom > 0 {
                        let n = this.list_state.item_count();
                        this.list_state
                            .splice(n.saturating_sub(*removed_bottom)..n, 0);
                    }
                    if *added_bottom > 0 {
                        let n = this.list_state.item_count();
                        this.list_state.splice(n..n, *added_bottom);
                    }
                    // Re-anchor the viewport after the splice. With
                    // `ListAlignment::Bottom`, the scroll anchor is `None` while
                    // the user is parked at an edge, so `splice` cannot shift it —
                    // the list would re-pin to the new bottom (forward paging) or
                    // stick to the skeleton header (back paging), both of which
                    // re-trigger load-more in a cascade. Anchor explicitly.
                    let following_new =
                        *added_bottom > 0 && this.at_bottom && !_store.read(cx).has_more_bottom();
                    if following_new {
                        // Parked at the true newest: follow the appended message
                        // (React `scrollToBottom` when not viewing older).
                        this.list_state.scroll_to_end();
                    } else if *added_top > 0 {
                        // Prepend (older page): anchor to the first real message so
                        // the skeleton header at index 0 doesn't keep the view at
                        // the top and re-trigger load-more.
                        let first_real = h + *added_top;
                        if this.list_state.logical_scroll_top().item_ix < first_real {
                            this.list_state.scroll_to(gpui::ListOffset {
                                item_ix: first_real,
                                offset_in_item: px(0.),
                            });
                        }
                    } else if *added_bottom > 0 || *removed_top > 0 {
                        // Forward pagination (newer page) or a non-followed append:
                        // keep the row that was at the top of the viewport in place
                        // (it shifts up by `removed_top` after the front trim), so
                        // the user stays on the same messages and the new rows sit
                        // below them.
                        let new_top = this.last_visible_start.saturating_sub(*removed_top);
                        this.list_state.scroll_to(gpui::ListOffset {
                            item_ix: new_top,
                            offset_in_item: px(0.),
                        });
                    }
                }
                MessagesEvent::Updated => {}
                MessagesEvent::JumpTo { message_id } => {
                    this.pending_jump = Some(*message_id);
                    this.highlight_id = Some(*message_id);
                    this._highlight_timer = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(1500))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            this.highlight_id = None;
                            cx.notify();
                        });
                    }));
                }
            }
            {
                let (is_empty, has_more_top, is_dm, dm_channel, conversation_loading) = {
                    let s = _store.read(cx);
                    Self::read_store_inputs(s)
                };
                this.sync_header(is_empty, has_more_top);
                this.sync_skeleton(is_dm, dm_channel, conversation_loading, is_empty, cx);
            }
            cx.notify();
        })
        .detach();

        // Generous overdraw (matching Zed's agent chat) so rows just outside the
        // viewport are measured ahead of time — keeps variable-height rows (with
        // images/attachments) from popping in or jumping the scrollbar.
        let list_state = ListState::new(0, ListAlignment::Bottom, px(LIST_OVERDRAW));
        let timeline = cx.weak_entity();
        list_state.set_scroll_handler(move |event, _window, cx| {
            // NB: do not call `list_state.item_count()` here — the list holds its
            // state borrowed while invoking this handler. Use `event.count`.
            // Load older only when the user has actually scrolled up — the top
            // is near AND there are rows below the viewport (`end < count`). In
            // normal flow the newest rows are always in the window, so the
            // bottom edge does nothing; after a jump-to-message (where the newest
            // is not loaded) reaching the bottom fetches the next newer page.
            let near_top = event.visible_range.start < LOAD_MORE_ITEM_THRESHOLD
                && event.visible_range.end < event.count;
            let near_bottom = event.visible_range.end + LOAD_MORE_ITEM_THRESHOLD >= event.count
                && event.visible_range.start > 0;
            // Pinned to the newest row when the bottom edge is in view. Drives the
            // explicit follow-on-append (React `scrollToBottom` when not viewing
            // older). `start == 0` means the whole window fits, which still counts
            // as being at the bottom.
            let at_bottom = event.visible_range.end + LOAD_MORE_ITEM_THRESHOLD >= event.count;
            let visible_start = event.visible_range.start;
            let _ = timeline.update(cx, |this, cx| {
                this.at_bottom = at_bottom;
                this.last_visible_start = visible_start;
                // Suppress hover affordances while scrolling. Record the time on
                // every event (cheap) but spawn the release watcher only once per
                // scroll session — the list fires ~120 events/sec, so spawning a
                // task per event would churn the executor.
                this.last_scroll_at = Some(Instant::now());
                if !this.suppress_hover {
                    this.suppress_hover = true;
                    cx.notify();
                    this._hover_release_task = Some(cx.spawn(async move |this, cx| {
                        let idle = Duration::from_millis(SCROLL_HOVER_RELEASE_MS);
                        loop {
                            cx.background_executor().timer(idle).await;
                            let still_scrolling = this
                                .update(cx, |this, _| {
                                    this.last_scroll_at.is_some_and(|t| t.elapsed() < idle)
                                })
                                .unwrap_or(false);
                            if still_scrolling {
                                continue;
                            }
                            let _ = this.update(cx, |this, cx| {
                                this.suppress_hover = false;
                                cx.notify();
                            });
                            break;
                        }
                    }));
                }

                // Throttle pagination: the list fires scroll events ~120fps
                // while the user holds the scrollbar at an edge. Only act once
                // per `PAGINATE_THROTTLE`.
                if !(near_top || near_bottom) {
                    return;
                }
                let now = Instant::now();
                if this
                    .last_paginate
                    .is_some_and(|t| now.duration_since(t) < PAGINATE_THROTTLE)
                {
                    return;
                }
                let store = MessagesStore::global(cx);
                tracing::debug!(
                    near_top,
                    near_bottom,
                    start = event.visible_range.start,
                    end = event.visible_range.end,
                    count = event.count,
                    has_more_bottom = store.read(cx).has_more_bottom(),
                    "timeline pagination trigger"
                );
                if near_top {
                    this.last_paginate = Some(now);
                    store.update(cx, |store, cx| store.scroll_reached_top(cx));
                } else if store.read(cx).has_more_bottom() {
                    this.last_paginate = Some(now);
                    store.update(cx, |store, cx| store.scroll_reached_bottom(cx));
                }
            });
        });

        let image_cache = cx.new(|cx| {
            LruImageCache::labeled(
                "msg-image",
                MESSAGE_IMAGE_CACHE_CAPACITY,
                MESSAGE_IMAGE_CACHE_BYTES,
                MESSAGE_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let avatar_image_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "msg-avatar",
                AVATAR_IMAGE_CACHE_CAPACITY,
                AVATAR_IMAGE_CACHE_BYTES,
                AVATAR_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let last_cold_inputs = Self::cold_inputs(cx);
        Self {
            list_state,
            settings,
            image_cache,
            avatar_image_cache,
            cached_for_channel: None,
            skeleton_phase: SkeletonPhase::Hidden,
            skeleton_key: SkeletonKey::None,
            last_cold_inputs,
            _channel_list_observe: channel_list_observe,
            _clan_list_observe: clan_list_observe,
            _skeleton_timer: None,
            suppress_hover: false,
            _hover_release_task: None,
            last_paginate: None,
            last_scroll_at: None,
            at_bottom: true,
            last_visible_start: 0,
            header_shown: false,
            pending_jump: None,
            highlight_id: None,
            _highlight_timer: None,
        }
    }

    fn reconcile_cold(&mut self, cx: &mut Context<Self>) {
        let inputs = Self::cold_inputs(cx);
        if inputs != self.last_cold_inputs {
            self.last_cold_inputs = inputs;
            let store = MessagesStore::global(cx);
            let (is_empty, has_more_top, is_dm, dm_channel, conversation_loading) = {
                let s = store.read(cx);
                Self::read_store_inputs(s)
            };
            self.sync_header(is_empty, has_more_top);
            self.sync_skeleton(is_dm, dm_channel, conversation_loading, is_empty, cx);
            cx.notify();
        }
    }

    fn sync_header(&mut self, is_empty: bool, has_more_top: bool) {
        let want_header = !is_empty && has_more_top;
        if want_header && !self.header_shown {
            self.list_state.splice(0..0, 1);
            self.header_shown = true;
        } else if !want_header && self.header_shown {
            self.list_state.splice(0..1, 0);
            self.header_shown = false;
        }
    }

    fn sync_skeleton(
        &mut self,
        is_dm: bool,
        dm_channel: Option<ChannelId>,
        conversation_loading: bool,
        is_empty: bool,
        cx: &mut Context<Self>,
    ) {
        let (active_clan, has_clan_channel, clan_loading) = Self::cold_inputs(cx);
        let has_conversation = has_clan_channel || is_dm;
        let cold_clan = !has_conversation && clan_loading;
        let loading_conversation = has_conversation && conversation_loading && is_empty;
        let loading = cold_clan || loading_conversation;
        let skeleton_key = if is_dm {
            dm_channel.map_or(SkeletonKey::None, SkeletonKey::Conversation)
        } else {
            active_clan.map_or(SkeletonKey::None, SkeletonKey::Clan)
        };
        self.advance_skeleton(loading, skeleton_key, cx);
    }

    fn read_store_inputs(store: &MessagesStore) -> (bool, bool, bool, Option<ChannelId>, bool) {
        let is_empty = store.viewport_messages().is_empty();
        let has_more_top = store.has_more_top();
        let is_dm = store.is_dm();
        let dm_channel = store.active_channel_id();
        let conversation_loading = store.is_loading();
        (
            is_empty,
            has_more_top,
            is_dm,
            dm_channel,
            conversation_loading,
        )
    }

    fn cold_inputs(cx: &mut Context<Self>) -> (Option<ClanId>, bool, bool) {
        let active_clan = ClanList::global(cx).read(cx).active_clan_id;
        let channel_list = ChannelList::global(cx);
        let channel_list = channel_list.read(cx);
        (
            active_clan,
            channel_list.active_channel().is_some(),
            active_clan.is_some_and(|clan| channel_list.is_loading_clan(clan)),
        )
    }

    fn clear_image_cache_if_channel_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel_id = ChannelList::global(cx).read(cx).active_channel_id;
        if self.cached_for_channel == channel_id {
            return;
        }
        self.cached_for_channel = channel_id;
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        self.avatar_image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
    }

    fn advance_skeleton(&mut self, loading: bool, key: SkeletonKey, cx: &mut Context<Self>) {
        let same_key = self.skeleton_key == key;
        let (next, timer) = skeleton_transition(
            self.skeleton_phase,
            SkeletonInput::Sync { loading, same_key },
        );
        self.skeleton_key = key;
        self.skeleton_phase = next;
        self.arm_skeleton_timer(timer, key, cx);
    }

    fn arm_skeleton_timer(
        &mut self,
        timer: SkeletonTimer,
        key: SkeletonKey,
        cx: &mut Context<Self>,
    ) {
        match skeleton_timer_action(timer) {
            TimerAction::Keep => {}
            TimerAction::Clear => self._skeleton_timer = None,
            TimerAction::Arm(input, delay_ms) => {
                self._skeleton_timer = Some(Self::spawn_skeleton_timer(input, delay_ms, key, cx));
            }
        }
    }

    fn spawn_skeleton_timer(
        input: SkeletonInput,
        delay_ms: u64,
        key: SkeletonKey,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay_ms))
                .await;
            this.update(cx, |this, cx| this.on_skeleton_timer(input, key, cx))
                .ok();
        })
    }

    fn on_skeleton_timer(
        &mut self,
        input: SkeletonInput,
        key: SkeletonKey,
        cx: &mut Context<Self>,
    ) {
        if self.skeleton_key != key {
            return;
        }
        let (next, timer) = skeleton_transition(self.skeleton_phase, input);
        let changed = next != self.skeleton_phase;
        self.skeleton_phase = next;
        self.arm_skeleton_timer(timer, key, cx);
        if changed {
            cx.notify();
        }
    }

    fn skeleton_overlay(&self, theme: &Theme) -> Option<gpui::AnyElement> {
        let base = || {
            div()
                .absolute()
                .inset_0()
                .bg(theme.bg_primary)
                .flex()
                .flex_col()
                .justify_end()
                .child(message_skeleton(theme, 5))
        };
        match self.skeleton_phase {
            SkeletonPhase::Showing => Some(
                base()
                    .with_animation(
                        self.skeleton_anim_id("skeleton-in"),
                        Animation::new(Duration::from_millis(SKELETON_FADE_IN_MS))
                            .with_easing(ease_in_out),
                        |el, delta| el.opacity(delta),
                    )
                    .into_any_element(),
            ),
            SkeletonPhase::Settling => Some(base().into_any_element()),
            SkeletonPhase::FadingOut => Some(
                base()
                    .with_animation(
                        self.skeleton_anim_id("skeleton-out"),
                        Animation::new(Duration::from_millis(SKELETON_FADE_OUT_MS))
                            .with_easing(ease_in_out),
                        |el, delta| el.opacity(1.0 - delta),
                    )
                    .into_any_element(),
            ),
            SkeletonPhase::Hidden | SkeletonPhase::Pending => None,
        }
    }

    fn skeleton_anim_id(&self, prefix: &'static str) -> SharedString {
        match self.skeleton_key {
            SkeletonKey::Clan(id) => SharedString::from(format!("{prefix}-c{id}")),
            SkeletonKey::Conversation(id) => SharedString::from(format!("{prefix}-d{id}")),
            SkeletonKey::None => SharedString::from(prefix),
        }
    }
}

impl Render for ChannelMessages {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("ChannelMessages");
        self.clear_image_cache_if_channel_changed(window, cx);
        self.image_cache
            .update(cx, |cache, cx| cache.sweep(window, cx));

        let store = MessagesStore::global(cx);
        let active_clan = ClanList::global(cx).read(cx).active_clan_id;
        let (is_dm, dm_channel) = {
            let s = store.read(cx);
            (s.is_dm(), s.active_channel_id())
        };
        let skeleton_overlay = self.skeleton_overlay(cx.theme());
        let header_shown = self.header_shown;

        if let Some(target) = self.pending_jump.take()
            && let Some(pos) = store
                .read(cx)
                .viewport_messages()
                .iter()
                .position(|m| m.id == target)
        {
            self.list_state
                .scroll_to_reveal_item(usize::from(header_shown) + pos);
        }

        let locale = self.settings.read(cx).language.clone();
        let list_state = self.list_state.clone();
        let suppress_hover = self.suppress_hover;
        let avatar_image_cache = self.avatar_image_cache.clone();
        let unread_boundary_id = unread_boundary(&store, cx);
        let highlight_id = self.highlight_id;
        let profile_context = channel_profile_context(is_dm, dm_channel, active_clan, cx);
        let settings = self.settings.clone();

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .image_cache(self.image_cache.clone())
            .child(
                list(list_state, move |ix, _window, cx| {
                    if header_shown && ix == 0 {
                        return div()
                            .id("msg-loading-top")
                            .py_2()
                            .child(message_skeleton(cx.theme(), 5))
                            .into_any_element();
                    }
                    let msg_ix = ix - usize::from(header_shown);
                    let store = MessagesStore::global(cx);
                    let ctx = RowCtx {
                        theme: cx.theme(),
                        locale: &locale,
                        current_user_id: "",
                        suppress_hover,
                        avatar_cache: avatar_image_cache.clone(),
                        unread_boundary_id,
                        highlight_id,
                        profile_context,
                        settings: settings.clone(),
                    };
                    render_message_item(store.read(cx).viewport_messages(), msg_ix, &ctx, cx)
                })
                .flex_1()
                .size_full(),
            )
            .children(skeleton_overlay)
            .custom_scrollbars(
                Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&self.list_state),
                window,
                cx,
            )
            .into_any_element()
    }
}

fn channel_profile_context(
    is_dm: bool,
    dm_channel: Option<ChannelId>,
    active_clan: Option<ClanId>,
    _cx: &Context<ChannelMessages>,
) -> Option<ProfileContext> {
    if is_dm {
        dm_channel.map(ProfileContext::Direct)
    } else {
        active_clan.map(ProfileContext::Clan)
    }
}

/// Find the id of the first message newer than the channel's last-seen
/// timestamp — the row above which the "New messages" break is shown. Returns
/// `None` when the channel has never been seen or is fully caught up.
fn unread_boundary(
    store: &Entity<MessagesStore>,
    cx: &Context<ChannelMessages>,
) -> Option<MessageId> {
    let channel_list = ChannelList::global(cx);
    let cl = channel_list.read(cx);
    let last_seen = cl
        .active_channel_id
        .and_then(|id| cl.find_channel(id))
        .map(|c| c.last_seen_timestamp)
        .unwrap_or(0);
    if last_seen <= 0 {
        return None;
    }
    store
        .read(cx)
        .viewport_messages()
        .iter()
        .find(|m| m.create_time > last_seen)
        .map(|m| m.id)
}

#[cfg(test)]
mod skeleton_tests {
    use super::{
        SkeletonInput, SkeletonPhase, SkeletonTimer, TimerAction, skeleton_timer_action,
        skeleton_transition,
    };

    fn sync(loading: bool, same_key: bool) -> SkeletonInput {
        SkeletonInput::Sync { loading, same_key }
    }

    const PHASES: [SkeletonPhase; 5] = [
        SkeletonPhase::Hidden,
        SkeletonPhase::Pending,
        SkeletonPhase::Showing,
        SkeletonPhase::Settling,
        SkeletonPhase::FadingOut,
    ];

    #[test]
    fn fast_load_arms_then_cancels_without_showing() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Hidden, sync(true, true)),
            (SkeletonPhase::Pending, SkeletonTimer::Threshold)
        );
        assert_eq!(
            skeleton_transition(SkeletonPhase::Pending, sync(false, true)),
            (SkeletonPhase::Hidden, SkeletonTimer::Drop)
        );
    }

    #[test]
    fn slow_load_stays_pending_then_threshold_shows() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Pending, sync(true, true)),
            (SkeletonPhase::Pending, SkeletonTimer::Keep)
        );
        assert_eq!(
            skeleton_transition(SkeletonPhase::Pending, SkeletonInput::ThresholdElapsed),
            (SkeletonPhase::Showing, SkeletonTimer::Keep)
        );
    }

    #[test]
    fn empty_but_loaded_leaves_showing_for_settle() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Showing, sync(false, true)),
            (SkeletonPhase::Settling, SkeletonTimer::Settle)
        );
    }

    #[test]
    fn settle_elapsed_starts_fade_out() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Settling, SkeletonInput::SettleElapsed),
            (SkeletonPhase::FadingOut, SkeletonTimer::Unmount)
        );
    }

    #[test]
    fn settle_tick_arms_unmount_timer() {
        let (next, timer) =
            skeleton_transition(SkeletonPhase::Settling, SkeletonInput::SettleElapsed);
        assert_eq!(next, SkeletonPhase::FadingOut);
        assert!(matches!(
            skeleton_timer_action(timer),
            TimerAction::Arm(SkeletonInput::UnmountElapsed, _)
        ));
    }

    #[test]
    fn timer_action_maps_each_token() {
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Keep),
            TimerAction::Keep
        ));
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Drop),
            TimerAction::Clear
        ));
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Threshold),
            TimerAction::Arm(SkeletonInput::ThresholdElapsed, _)
        ));
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Settle),
            TimerAction::Arm(SkeletonInput::SettleElapsed, _)
        ));
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Unmount),
            TimerAction::Arm(SkeletonInput::UnmountElapsed, _)
        ));
    }

    #[test]
    fn data_arrives_settles_then_fades_then_hides() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Showing, sync(false, true)),
            (SkeletonPhase::Settling, SkeletonTimer::Settle)
        );
        assert_eq!(
            skeleton_transition(SkeletonPhase::Settling, SkeletonInput::SettleElapsed),
            (SkeletonPhase::FadingOut, SkeletonTimer::Unmount)
        );
        assert_eq!(
            skeleton_transition(SkeletonPhase::FadingOut, SkeletonInput::UnmountElapsed),
            (SkeletonPhase::Hidden, SkeletonTimer::Keep)
        );
    }

    #[test]
    fn resync_mid_settle_returns_to_showing_without_double_timer() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Settling, sync(true, true)),
            (SkeletonPhase::Showing, SkeletonTimer::Drop)
        );
    }

    #[test]
    fn resync_mid_fade_returns_to_showing_without_double_timer() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::FadingOut, sync(true, true)),
            (SkeletonPhase::Showing, SkeletonTimer::Drop)
        );
    }

    #[test]
    fn channel_switch_resets_every_phase_to_hidden() {
        for phase in PHASES {
            assert_eq!(
                skeleton_transition(phase, sync(false, false)),
                (SkeletonPhase::Hidden, SkeletonTimer::Drop)
            );
        }
    }

    #[test]
    fn channel_switch_into_cold_arms_threshold_from_every_phase() {
        for phase in PHASES {
            assert_eq!(
                skeleton_transition(phase, sync(true, false)),
                (SkeletonPhase::Pending, SkeletonTimer::Threshold)
            );
        }
    }

    #[test]
    fn cached_channel_stays_hidden_without_touching_timer() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Hidden, sync(false, true)),
            (SkeletonPhase::Hidden, SkeletonTimer::Keep)
        );
    }

    #[test]
    fn elapsed_inputs_are_noops_off_their_phase() {
        for phase in PHASES {
            if phase != SkeletonPhase::Pending {
                assert_eq!(
                    skeleton_transition(phase, SkeletonInput::ThresholdElapsed),
                    (phase, SkeletonTimer::Keep)
                );
            }
            if phase != SkeletonPhase::Settling {
                assert_eq!(
                    skeleton_transition(phase, SkeletonInput::SettleElapsed),
                    (phase, SkeletonTimer::Keep)
                );
            }
            if phase != SkeletonPhase::FadingOut {
                assert_eq!(
                    skeleton_transition(phase, SkeletonInput::UnmountElapsed),
                    (phase, SkeletonTimer::Keep)
                );
            }
        }
    }
}
