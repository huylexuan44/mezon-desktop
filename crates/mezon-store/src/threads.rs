use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription};
use mezon_client::AppApi;
use mezon_client::transport::ApiThreadDesc;
use mezon_client::RealtimeEvent;

use crate::channel::{Channel, ChannelEvent, ChannelList, ChannelType};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

pub const THREAD_STATUS_ARCHIVED: i32 = 0;
pub const THREAD_STATUS_JOINED: i32 = 1;
pub const THREAD_STATUS_ACTIVE_PUBLIC: i32 = 2;
pub const THREAD_STATUS_ACTIVE_PRIVATE: i32 = 3;

pub const CHANNEL_TYPE_THREAD: u32 = 7;

pub const MIN_THREAD_NAME_LEN: usize = 3;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub struct ThreadSummary {
    pub channel_id: String,
    pub channel_label: String,
    pub clan_id: String,
    pub parent_id: String,
    pub channel_private: i32,
    pub active: i32,
    pub last_message_content: String,
    pub last_message_sender_id: String,
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
    loading: bool,
    searching: bool,
    creating: bool,
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
            loading: false,
            searching: false,
            creating: false,
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
        let relevant = match event {
            RealtimeEvent::ChannelCreated(ev) => {
                ev.parent_id != 0 && ev.parent_id.to_string() == list_id
            }
            RealtimeEvent::ChannelUpdated(ev) => self
                .threads
                .iter()
                .any(|t| t.channel_id == ev.channel_id.to_string()),
            RealtimeEvent::ChannelMessage(msg) => self
                .threads
                .iter()
                .any(|t| t.channel_id == msg.channel_id.to_string()),
            RealtimeEvent::ChannelDeleted(ev) => self
                .threads
                .iter()
                .any(|t| t.channel_id == ev.channel_id.to_string()),
            _ => false,
        };
        if relevant {
            self.invalidate_loaded(cx);
        }
    }

    fn invalidate_loaded(&mut self, cx: &mut Context<Self>) {
        self.loaded_channel = None;
        if self.list_channel_id.is_some() {
            self.ensure_loaded(cx);
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

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    pub fn is_creating(&self) -> bool {
        self.creating
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

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loaded_channel = None;
        self.fetch(cx);
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
        let Some(channel_id) = self.list_channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_thread_descs(&channel_id, &clan_id, 1).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if this.list_channel_id.as_deref() != Some(channel_id.as_str()) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(list) => {
                        this.threads =
                            filter_threads(list.into_iter().map(thread_from_api).collect());
                        this.loaded_channel = Some(channel_id);
                    }
                    Err(e) => tracing::error!("list_thread_descs failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn start_create(&mut self, cx: &mut Context<Self>) {
        self.creating = true;
        self.name_error = None;
        cx.notify();
    }

    pub fn cancel_create(&mut self, cx: &mut Context<Self>) {
        self.creating = false;
        self.name_error = None;
        cx.notify();
    }

    pub fn submit_create(&mut self, name: String, message: String, cx: &mut Context<Self>) {
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

        self.name_error = None;
        self.creating = true;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let dup = api.check_duplicate_thread_name(&name, &parent_id).await;
            if let Ok(true) = dup {
                let _ = this.update(cx, |this, cx| {
                    this.creating = false;
                    this.name_error = Some("thread_name_exists".into());
                    cx.notify();
                });
                return;
            }

            let create_result = api
                .create_channel(
                    &clan_id,
                    &name,
                    CHANNEL_TYPE_THREAD,
                    category_id.as_deref(),
                    Some(&parent_id),
                )
                .await;

            let thread = match create_result {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("create_channel (thread) failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.creating = false;
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
        last_message_content: t.last_message_content,
        last_message_sender_id: t.last_message_sender_id,
        last_sent_timestamp: t.last_sent_timestamp,
        member_count: t.member_count,
    }
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
