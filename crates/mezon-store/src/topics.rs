use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};
use mezon_client::{AppApi, TopicDiscussion};

use crate::CACHE_TTL;

const TOPICS_LIMIT: i32 = 50;

#[derive(Debug, Clone)]
pub enum TopicsEvent {
    Updated,
}

pub struct TopicsStore {
    topics: Vec<TopicDiscussion>,
    clan_id: Option<String>,
    loading: bool,
    fetch_generation: u64,
    fetched_at: Option<Instant>,
    api: Arc<AppApi>,
}

struct GlobalTopicsStore(Entity<TopicsStore>);
impl Global for GlobalTopicsStore {}

impl EventEmitter<TopicsEvent> for TopicsStore {}

impl TopicsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            topics: Vec::new(),
            clan_id: None,
            loading: false,
            fetch_generation: 0,
            fetched_at: None,
            api,
        });
        cx.set_global(GlobalTopicsStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalTopicsStore>().0.clone()
    }

    pub fn topics(&self) -> &[TopicDiscussion] {
        &self.topics
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn badge_label(&self) -> Option<String> {
        if self.topics.is_empty() {
            None
        } else if self.topics.len() > 9 {
            Some("9+".into())
        } else {
            Some(self.topics.len().to_string())
        }
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
        if self.loading {
            return;
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
                self.clan_id = Some(clan_id.to_string());
                self.fetched_at = Some(Instant::now());
                cx.emit(TopicsEvent::Updated);
                cx.notify();
            }
            Err(e) => {
                tracing::error!("list_sd_topics failed: {e}");
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_count_caps_at_nine_plus() {
        let store = TopicsStore {
            topics: (0..12)
                .map(|i| TopicDiscussion {
                    id: i.to_string(),
                    message_id: String::new(),
                    clan_id: String::new(),
                    channel_id: String::new(),
                    content: String::new(),
                    last_message_preview: String::new(),
                    last_message_timestamp: 0,
                })
                .collect(),
            clan_id: None,
            loading: false,
            fetch_generation: 0,
            fetched_at: None,
            api: Arc::new(AppApi::new(Arc::new(mezon_client::TransportClient::new(
                String::new(),
            )))),
        };
        assert_eq!(store.badge_label(), Some("9+".into()));
    }
}
