use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription};
use mezon_client::AppApi;
use mezon_client::MezonTransport;
use mezon_client::transport::{ApiThreadDesc, THREAD_LIST_LIMIT};
use mezon_client::RealtimeEvent;
use mezon_proto::{api, realtime};

use crate::channel::{Channel, ChannelEvent, ChannelList, ChannelType};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

pub const THREAD_STATUS_ARCHIVED: i32 = 0;
pub const THREAD_STATUS_JOINED: i32 = 1;
pub const THREAD_STATUS_ACTIVE_PUBLIC: i32 = 2;
pub const THREAD_STATUS_ACTIVE_PRIVATE: i32 = 3;

pub const CHANNEL_TYPE_THREAD: u32 = 7;

pub const MIN_THREAD_NAME_LEN: usize = 3;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(500);
const LOAD_MORE_THRESHOLD: usize = 6;
const MESSAGE_CODE_CHAT: i32 = 0;
const MESSAGE_CODE_CHAT_UPDATE: i32 = 1;

#[derive(Debug, Clone)]
pub struct ThreadSummary {
    pub channel_id: String,
    pub channel_label: String,
    pub clan_id: String,
    pub parent_id: String,
    pub channel_private: i32,
    pub active: i32,
    pub creator_id: String,
    pub last_message_content: String,
    pub last_message_sender_id: String,
    pub last_message_sender_name: String,
    pub last_message_sender_avatar: String,
    pub last_sent_timestamp: i64,
    pub member_count: i32,
}

#[derive(Debug, Clone)]
pub enum ThreadsEvent {
    ThreadCreated {
        channel_id: String,
        clan_id: String,
    },
    CreateFailed {
        message: String,
    },
}

pub struct ThreadsStore {
    list_channel_id: Option<String>,
    clan_id: Option<String>,
    category_id: Option<String>,
    threads: Vec<ThreadSummary>,
    search_query: String,
    search_results: Option<Vec<ThreadSummary>>,
    search_generation: u64,
    loaded_channel: Option<String>,
    current_page: i32,
    has_more: bool,
    loading: bool,
    loading_more: bool,
    searching: bool,
    creating: bool,
    submitting: bool,
    create_private: i32,
    name_error: Option<String>,
    api: Arc<AppApi>,
    _channel_sub: Subscription,
}

struct GlobalThreadsStore(Entity<ThreadsStore>);
impl Global for GlobalThreadsStore {}

impl EventEmitter<ThreadsEvent> for ThreadsStore {}

impl ThreadsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalThreadsStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalThreadsStore>().0.clone()
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _list, event, cx| {
            if let ChannelEvent::ActiveChannelChanged(channel_id) = event {
                this.on_active_channel_changed(channel_id.clone(), cx);
            }
        });
        Self::register_realtime(cx);
        let mut store = Self {
            list_channel_id: None,
            clan_id: None,
            category_id: None,
            threads: Vec::new(),
            search_query: String::new(),
            search_results: None,
            search_generation: 0,
            loaded_channel: None,
            current_page: 0,
            has_more: false,
            loading: false,
            loading_more: false,
            searching: false,
            creating: false,
            submitting: false,
            create_private: 0,
            name_error: None,
            api,
            _channel_sub: channel_sub,
        };
        if let Some(channel) = ChannelList::global(cx).read(cx).active_channel() {
            store.apply_channel(channel);
        }
        store
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ChannelCreated,
                RealtimeKind::ChannelUpdated,
                RealtimeKind::ChannelMessage,
                RealtimeKind::ChannelDeleted,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.on_realtime_event(event, cx);
                });
            }
        });
    }

    fn on_realtime_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let Some(list_id) = self.list_channel_id.clone() else {
            return;
        };
        match event {
            RealtimeEvent::ChannelCreated(ev)
                if ev.parent_id != 0 && ev.parent_id.to_string() == list_id =>
            {
                self.apply_thread_created(ev, cx);
            }
            RealtimeEvent::ChannelUpdated(ev)
                if ev.channel_type == CHANNEL_TYPE_THREAD as i32 =>
            {
                self.apply_thread_updated(ev, cx);
            }
            RealtimeEvent::ChannelMessage(msg) => self.apply_thread_message(msg, cx),
            RealtimeEvent::ChannelDeleted(ev) => {
                self.apply_thread_deleted(&ev.channel_id.to_string(), cx);
            }
            _ => {}
        }
    }

    fn apply_thread_message(&mut self, msg: &api::ChannelMessage, cx: &mut Context<Self>) {
        let channel_id = msg.channel_id.to_string();
        if !self.threads.iter().any(|t| t.channel_id == channel_id)
            && !self
                .search_results
                .as_ref()
                .is_some_and(|results| results.iter().any(|t| t.channel_id == channel_id))
        {
            return;
        }
        let mut changed = false;
        if patch_thread_in_list(&mut self.threads, &channel_id, msg) {
            changed = true;
        }
        if let Some(results) = self.search_results.as_mut()
            && patch_thread_in_list(results, &channel_id, msg)
        {
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_thread_updated(&mut self, ev: &realtime::ChannelUpdatedEvent, cx: &mut Context<Self>) {
        let channel_id = ev.channel_id.to_string();
        let mut changed = false;
        for thread in self
            .threads
            .iter_mut()
            .filter(|t| t.channel_id == channel_id)
        {
            patch_thread_from_updated(thread, ev);
            changed = true;
        }
        if let Some(results) = self.search_results.as_mut() {
            for thread in results.iter_mut().filter(|t| t.channel_id == channel_id) {
                patch_thread_from_updated(thread, ev);
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_thread_deleted(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        let before = self.threads.len();
        self.threads.retain(|t| t.channel_id != channel_id);
        if let Some(results) = self.search_results.as_mut() {
            results.retain(|t| t.channel_id != channel_id);
        }
        if self.threads.len() != before {
            cx.notify();
        }
    }

    fn apply_thread_created(&mut self, ev: &realtime::ChannelCreatedEvent, cx: &mut Context<Self>) {
        if ev.channel_type != CHANNEL_TYPE_THREAD as i32 {
            return;
        }
        let Some(summary) = filter_threads(vec![thread_from_created_event(ev)]).into_iter().next()
        else {
            return;
        };
        let before = self.threads.len();
        merge_threads(&mut self.threads, vec![summary.clone()]);
        if let Some(results) = self.search_results.as_mut() {
            merge_threads(results, vec![summary]);
        }
        if self.threads.len() != before {
            cx.notify();
        }
    }

    pub fn threads(&self) -> &[ThreadSummary] {
        &self.threads
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn search_results(&self) -> Option<&[ThreadSummary]> {
        self.search_results.as_deref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_loading_more(&self) -> bool {
        self.loading_more
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    pub fn is_creating(&self) -> bool {
        self.creating
    }

    pub fn is_submitting(&self) -> bool {
        self.submitting
    }

    pub fn create_private(&self) -> bool {
        self.create_private != 0
    }

    pub fn set_create_private(&mut self, private: bool, cx: &mut Context<Self>) {
        self.create_private = i32::from(private);
        cx.notify();
    }

    pub fn name_error(&self) -> Option<&str> {
        self.name_error.as_deref()
    }

    pub fn show_threads_popover(&self, cx: &App) -> bool {
        self.list_channel_id
            .as_ref()
            .and_then(|id| ChannelList::global(cx).read(cx).find_channel(id))
            .is_some_and(|ch| {
                !matches!(ch.channel_type, ChannelType::Thread)
                    && matches!(
                        ch.channel_type,
                        ChannelType::Text | ChannelType::Forum | ChannelType::Announcement
                    )
            })
    }

    fn apply_channel(&mut self, channel: &Channel) {
        self.list_channel_id = Some(list_channel_id_for(channel));
        self.clan_id = Some(channel.clan_id.clone());
        self.category_id = channel.category_id.clone();
    }

    fn on_active_channel_changed(&mut self, channel_id: Option<String>, cx: &mut Context<Self>) {
        match channel_id {
            None => {
                self.list_channel_id = None;
                self.clan_id = None;
                self.category_id = None;
            }
            Some(id) => {
                if let Some(channel) = ChannelList::global(cx).read(cx).find_channel(&id) {
                    self.apply_channel(channel);
                } else {
                    self.list_channel_id = Some(id);
                    self.clan_id = Some("0".to_string());
                    self.category_id = None;
                }
            }
        }
        self.threads.clear();
        self.search_query.clear();
        self.search_results = None;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.searching = false;
        self.loaded_channel = None;
        self.current_page = 0;
        self.has_more = false;
        self.loading_more = false;
        self.submitting = false;
        self.name_error = None;
        cx.notify();
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.list_channel_id.clone() else {
            return;
        };
        if self.loading || self.loaded_channel.as_deref() == Some(channel_id.as_str()) {
            return;
        }
        self.fetch(cx);
    }

    pub fn open_popover(&mut self, cx: &mut Context<Self>) {
        self.loaded_channel = None;
        self.current_page = 0;
        self.has_more = false;
        self.threads.clear();
        self.loading = false;
        self.loading_more = false;
        self.ensure_loaded(cx);
    }

    pub fn list_clan_id(&self) -> Option<&str> {
        self.clan_id.as_deref()
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loaded_channel = None;
        self.current_page = 0;
        self.has_more = false;
        self.fetch(cx);
    }

    pub fn maybe_load_more(&mut self, visible_end: usize, cx: &mut Context<Self>) {
        if !self.search_query.trim().is_empty() {
            return;
        }
        let count = self.threads.len();
        if count == 0 || count.saturating_sub(visible_end) > LOAD_MORE_THRESHOLD {
            return;
        }
        self.fetch_more(cx);
    }

    pub fn fetch_more(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.loading_more || !self.has_more {
            return;
        }
        self.fetch_page(self.current_page + 1, true, cx);
    }

    pub fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query.clone();
        if query.trim().is_empty() {
            self.search_results = None;
            self.searching = false;
            self.search_generation = self.search_generation.wrapping_add(1);
            cx.notify();
            return;
        }
        self.searching = true;
        cx.notify();
        self.schedule_search(cx, query);
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>, query: String) {
        let Some(channel_id) = self.list_channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        let trimmed = query.trim().to_string();
        let api = self.api.clone();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            if this
                .update(cx, |this, _| {
                    this.search_generation != generation
                        || this.search_query.trim() != trimmed
                        || this.list_channel_id.as_deref() != Some(channel_id.as_str())
                })
                .unwrap_or(true)
            {
                return;
            }

            let result = api.search_thread(&clan_id, &channel_id, &trimmed).await;

            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                if this.search_query.trim() != trimmed {
                    return;
                }
                if this.list_channel_id.as_deref() != Some(channel_id.as_str()) {
                    this.searching = false;
                    cx.notify();
                    return;
                }
                this.searching = false;
                match result {
                    Ok(list) => {
                        this.search_results = Some(filter_threads(
                            list.into_iter().map(thread_from_api).collect(),
                        ));
                    }
                    Err(e) => tracing::error!("search_thread failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.fetch_page(1, false, cx);
    }

    fn fetch_page(&mut self, page: i32, append: bool, cx: &mut Context<Self>) {
        let Some(channel_id) = self.list_channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        if append {
            if self.loading_more || !self.has_more {
                return;
            }
            self.loading_more = true;
        } else if self.loading {
            return;
        } else {
            self.loading = true;
        }
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_thread_descs(&channel_id, &clan_id, page).await;
            let _ = this.update(cx, |this, cx| {
                if append {
                    this.loading_more = false;
                } else {
                    this.loading = false;
                }
                if this.list_channel_id.as_deref() != Some(channel_id.as_str()) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(list) => {
                        let page_full = page_has_more(list.len());
                        let batch =
                            filter_threads(list.into_iter().map(thread_from_api).collect());
                        if append {
                            merge_threads(&mut this.threads, batch);
                        } else {
                            this.threads = batch;
                            this.loaded_channel = Some(channel_id);
                        }
                        this.current_page = page;
                        this.has_more = page_full;
                    }
                    Err(e) => tracing::error!("list_thread_descs page {page} failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn start_create(&mut self, cx: &mut Context<Self>) {
        self.creating = true;
        self.submitting = false;
        self.create_private = 0;
        self.name_error = None;
        cx.notify();
    }

    pub fn cancel_create(&mut self, cx: &mut Context<Self>) {
        self.creating = false;
        self.submitting = false;
        self.create_private = 0;
        self.name_error = None;
        cx.notify();
    }

    pub fn submit_create(&mut self, name: String, message: String, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        let name = name.trim().to_string();
        let message = message.trim().to_string();

        if name.len() <= MIN_THREAD_NAME_LEN {
            self.name_error = Some("thread_name_too_short".into());
            cx.notify();
            return;
        }
        if message.is_empty() {
            self.name_error = Some("initial_message_required".into());
            cx.notify();
            return;
        }

        let Some(parent_id) = self.list_channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        let category_id = self.category_id.clone();
        let channel_private = self.create_private;

        self.name_error = None;
        self.creating = true;
        self.submitting = true;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            match api.check_duplicate_thread_name(&name, &parent_id).await {
                Ok(true) => {
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        this.name_error = Some("thread_name_exists".into());
                        cx.notify();
                    });
                    return;
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::error!("check_duplicate_thread_name failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.emit(ThreadsEvent::CreateFailed {
                            message: e.to_string(),
                        });
                        cx.notify();
                    });
                    return;
                }
            }

            let create_result = api
                .create_channel(
                    &clan_id,
                    &name,
                    CHANNEL_TYPE_THREAD,
                    category_id.as_deref(),
                    Some(&parent_id),
                    channel_private,
                )
                .await;

            let thread = match create_result {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("create_channel (thread) failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.emit(ThreadsEvent::CreateFailed {
                            message: e.to_string(),
                        });
                        cx.notify();
                    });
                    return;
                }
            };

            let thread_id = thread.channel_id.clone();
            let content = serde_json::json!({ "t": message }).to_string();
            if let Err(e) = api
                .send_channel_message(&clan_id, &thread_id, &content, false, 2)
                .await
            {
                tracing::error!("send starter message to thread failed: {e}");
            }

            if let Err(e) = api
                .join_chat(&clan_id, &thread_id, CHANNEL_TYPE_THREAD as i32, false)
                .await
            {
                tracing::warn!("join_chat after thread create failed: {e}");
            }

            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                this.submitting = false;
                this.loaded_channel = None;
                ChannelList::global(cx).update(cx, |list, cx| {
                    list.refresh_clan(clan_id.clone(), cx);
                });
                cx.emit(ThreadsEvent::ThreadCreated {
                    channel_id: thread_id,
                    clan_id: clan_id.clone(),
                });
                cx.notify();
            });
        })
        .detach();
    }
}

fn list_channel_id_for(channel: &Channel) -> String {
    if channel.channel_type == ChannelType::Thread {
        channel
            .parent_id
            .clone()
            .unwrap_or_else(|| channel.id.clone())
    } else {
        channel.id.clone()
    }
}

fn page_has_more(batch_len: usize) -> bool {
    batch_len >= THREAD_LIST_LIMIT as usize
}

fn merge_threads(into: &mut Vec<ThreadSummary>, batch: Vec<ThreadSummary>) {
    for thread in batch {
        if !into
            .iter()
            .any(|existing| existing.channel_id == thread.channel_id)
        {
            into.push(thread);
        }
    }
}

fn filter_threads(threads: Vec<ThreadSummary>) -> Vec<ThreadSummary> {
    threads
        .into_iter()
        .filter(|t| {
            if t.channel_private != 0 {
                t.active == THREAD_STATUS_JOINED || t.active == THREAD_STATUS_ACTIVE_PRIVATE
            } else {
                true
            }
        })
        .collect()
}

fn thread_from_api(t: ApiThreadDesc) -> ThreadSummary {
    ThreadSummary {
        channel_id: t.channel_id,
        channel_label: t.channel_label,
        clan_id: t.clan_id,
        parent_id: t.parent_id,
        channel_private: t.channel_private,
        active: t.active,
        creator_id: t.creator_id,
        last_message_content: t.last_message_content,
        last_message_sender_id: t.last_message_sender_id,
        last_message_sender_name: t.last_message_sender_name,
        last_message_sender_avatar: t.last_message_sender_avatar,
        last_sent_timestamp: t.last_sent_timestamp,
        member_count: t.member_count,
    }
}

fn thread_from_created_event(ev: &realtime::ChannelCreatedEvent) -> ThreadSummary {
    ThreadSummary {
        channel_id: ev.channel_id.to_string(),
        channel_label: ev.channel_label.clone(),
        clan_id: ev.clan_id.to_string(),
        parent_id: ev.parent_id.to_string(),
        channel_private: ev.channel_private,
        active: ev.status,
        creator_id: ev.creator_id.to_string(),
        last_message_content: String::new(),
        last_message_sender_id: ev.creator_id.to_string(),
        last_message_sender_name: String::new(),
        last_message_sender_avatar: String::new(),
        last_sent_timestamp: 0,
        member_count: 0,
    }
}

fn patch_thread_from_updated(thread: &mut ThreadSummary, ev: &realtime::ChannelUpdatedEvent) {
    if !ev.channel_label.is_empty() {
        thread.channel_label = ev.channel_label.clone();
    }
    thread.active = ev.status;
    thread.channel_private = i32::from(ev.channel_private);
}

fn patch_thread_from_message(thread: &mut ThreadSummary, msg: &api::ChannelMessage) {
    thread.last_sent_timestamp = i64::from(msg.create_time_seconds);
    if msg.code == MESSAGE_CODE_CHAT || msg.code == MESSAGE_CODE_CHAT_UPDATE {
        let api_msg = MezonTransport::message_from_proto(msg.clone());
        thread.last_message_content = api_msg.content;
        thread.last_message_sender_id = api_msg.sender_id;
        thread.last_message_sender_name = api_msg.sender_name;
        thread.last_message_sender_avatar = api_msg.avatar;
    }
}

fn patch_thread_in_list(
    threads: &mut [ThreadSummary],
    channel_id: &str,
    msg: &api::ChannelMessage,
) -> bool {
    let Some(thread) = threads.iter_mut().find(|t| t.channel_id == channel_id) else {
        return false;
    };
    patch_thread_from_message(thread, msg);
    true
}

pub fn group_threads(
    threads: &[ThreadSummary],
) -> (
    Vec<&ThreadSummary>,
    Vec<&ThreadSummary>,
    Vec<&ThreadSummary>,
) {
    let mut joined = Vec::new();
    let mut active = Vec::new();
    let mut older = Vec::new();

    for t in threads {
        match t.active {
            THREAD_STATUS_JOINED => joined.push(t),
            THREAD_STATUS_ARCHIVED => older.push(t),
            _ => active.push(t),
        }
    }

    let sort = |v: &mut Vec<&ThreadSummary>| {
        v.sort_by_key(|t| std::cmp::Reverse(t.last_sent_timestamp));
    };
    sort(&mut joined);
    sort(&mut active);
    sort(&mut older);

    (joined, active, older)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_has_more_when_batch_full() {
        assert!(page_has_more(THREAD_LIST_LIMIT as usize));
    }

    #[test]
    fn page_has_more_false_when_batch_partial() {
        assert!(!page_has_more(10));
    }

    #[test]
    fn merge_threads_dedupes_by_channel_id() {
        let mut threads = vec![ThreadSummary {
            channel_id: "1".into(),
            channel_label: "a".into(),
            clan_id: "c".into(),
            parent_id: "p".into(),
            channel_private: 0,
            active: THREAD_STATUS_JOINED,
            creator_id: String::new(),
            last_message_content: String::new(),
            last_message_sender_id: String::new(),
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp: 0,
            member_count: 0,
        }];
        merge_threads(
            &mut threads,
            vec![
                ThreadSummary {
                    channel_id: "1".into(),
                    channel_label: "dup".into(),
                    clan_id: "c".into(),
                    parent_id: "p".into(),
                    channel_private: 0,
                    active: THREAD_STATUS_JOINED,
                    creator_id: String::new(),
                    last_message_content: String::new(),
                    last_message_sender_id: String::new(),
                    last_message_sender_name: String::new(),
                    last_message_sender_avatar: String::new(),
                    last_sent_timestamp: 0,
                    member_count: 0,
                },
                ThreadSummary {
                    channel_id: "2".into(),
                    channel_label: "b".into(),
                    clan_id: "c".into(),
                    parent_id: "p".into(),
                    channel_private: 0,
                    active: THREAD_STATUS_JOINED,
                    creator_id: String::new(),
                    last_message_content: String::new(),
                    last_message_sender_id: String::new(),
                    last_message_sender_name: String::new(),
                    last_message_sender_avatar: String::new(),
                    last_sent_timestamp: 0,
                    member_count: 0,
                },
            ],
        );
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].channel_label, "a");
    }

    #[test]
    fn patch_thread_from_message_updates_preview_on_chat() {
        let mut thread = ThreadSummary {
            channel_id: "1".into(),
            channel_label: "t".into(),
            clan_id: "c".into(),
            parent_id: "p".into(),
            channel_private: 0,
            active: THREAD_STATUS_JOINED,
            creator_id: String::new(),
            last_message_content: "old".into(),
            last_message_sender_id: "9".into(),
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp: 1,
            member_count: 0,
        };
        let msg = api::ChannelMessage {
            channel_id: 1,
            sender_id: 42,
            content: r#"{"t":"hello"}"#.into(),
            create_time_seconds: 99,
            code: MESSAGE_CODE_CHAT,
            ..Default::default()
        };
        patch_thread_from_message(&mut thread, &msg);
        assert_eq!(thread.last_sent_timestamp, 99);
        assert_eq!(thread.last_message_content, "hello");
        assert_eq!(thread.last_message_sender_id, "42");
    }

    #[test]
    fn patch_thread_from_message_keeps_preview_on_remove() {
        let mut thread = ThreadSummary {
            channel_id: "1".into(),
            channel_label: "t".into(),
            clan_id: "c".into(),
            parent_id: "p".into(),
            channel_private: 0,
            active: THREAD_STATUS_JOINED,
            creator_id: String::new(),
            last_message_content: "keep".into(),
            last_message_sender_id: "9".into(),
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp: 1,
            member_count: 0,
        };
        let msg = api::ChannelMessage {
            channel_id: 1,
            create_time_seconds: 50,
            code: 2,
            ..Default::default()
        };
        patch_thread_from_message(&mut thread, &msg);
        assert_eq!(thread.last_sent_timestamp, 50);
        assert_eq!(thread.last_message_content, "keep");
        assert_eq!(thread.last_message_sender_id, "9");
    }
}
