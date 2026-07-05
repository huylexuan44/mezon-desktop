use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, TopicDiscussion};

use crate::CACHE_TTL;

const TOPICS_LIMIT: i32 = 50;

#[derive(Debug, Clone)]
pub enum TopicsEvent {
    Updated,
}

pub struct TopicsStore {
    topics: Vec<TopicDiscussion>,
    topic_index: std::collections::HashMap<String, usize>,
    clan_id: Option<String>,
    loading: bool,
    fetch_generation: u64,
    fetched_at: Option<Instant>,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalTopicsStore(Entity<TopicsStore>);
impl Global for GlobalTopicsStore {}

impl EventEmitter<TopicsEvent> for TopicsStore {}

impl TopicsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalTopicsStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            topics: Vec::new(),
            topic_index: std::collections::HashMap::new(),
            clan_id: None,
            loading: false,
            fetch_generation: 0,
            fetched_at: None,
            api,
            _conn_watch: conn_watch,
        }
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
                    if this
                        .update(cx, |this, cx| this.refetch_active_clan(cx))
                        .is_err()
                    {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    fn refetch_active_clan(&mut self, cx: &mut Context<Self>) {
        self.fetched_at = None;
        if let Some(clan_id) = self.clan_id.clone() {
            self.fetch(&clan_id, cx);
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalTopicsStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalTopicsStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.topics.clear();
        self.topic_index.clear();
        self.clan_id = None;
        self.loading = false;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        self.fetched_at = None;
        cx.emit(TopicsEvent::Updated);
        cx.notify();
    }

    pub fn topics(&self) -> &[TopicDiscussion] {
        &self.topics
    }

    pub fn topic_by_id(&self, id: &str) -> Option<&TopicDiscussion> {
        self.topic_index
            .get(id)
            .and_then(|&index| self.topics.get(index))
    }

    pub fn topics_for(&self, clan_id: &str) -> &[TopicDiscussion] {
        if self.clan_id.as_deref() == Some(clan_id) {
            &self.topics
        } else {
            &[]
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    fn is_fresh(&self, clan_id: &str) -> bool {
        self.clan_id.as_deref() == Some(clan_id)
            && self.fetched_at.is_some_and(|t| t.elapsed() < CACHE_TTL)
    }

    pub fn fetch_if_needed(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.is_fresh(clan_id) {
            return;
        }
        self.fetch(clan_id, cx);
    }

    pub fn fetch(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.loading && self.clan_id.as_deref() == Some(clan_id) {
            return;
        }
        if self.clan_id.as_deref() != Some(clan_id) {
            self.topics.clear();
            self.topic_index.clear();
            self.clan_id = Some(clan_id.to_string());
            self.fetched_at = None;
        }
        self.loading = true;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;
        cx.notify();

        let api = self.api.clone();
        let clan_id = clan_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = api.list_sd_topics(&clan_id, TOPICS_LIMIT).await;
            let _ = this.update(cx, |this, cx| {
                this.apply_fetch_result(&clan_id, generation, result, cx);
            });
        })
        .detach();
    }

    fn apply_fetch_result(
        &mut self,
        clan_id: &str,
        generation: u64,
        result: Result<Vec<TopicDiscussion>, anyhow::Error>,
        cx: &mut Context<Self>,
    ) {
        if self.fetch_generation != generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(mut topics) => {
                topics.sort_by_key(|t| std::cmp::Reverse(t.last_message_timestamp));
                self.topics = topics;
                self.topic_index = self
                    .topics
                    .iter()
                    .enumerate()
                    .map(|(index, topic)| (topic.id.clone(), index))
                    .collect();
                self.clan_id = Some(clan_id.to_string());
                self.fetched_at = Some(Instant::now());
                cx.emit(TopicsEvent::Updated);
                cx.notify();
            }
            Err(e) => {
                tracing::error!("list_sd_topics failed: {e}");
                cx.emit(TopicsEvent::Updated);
                cx.notify();
            }
        }
    }
}
