use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::Freshness;
use crate::badge::BadgeService;
use crate::clan::{ClanEvent, ClanList};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const RECENT_EMOJI_CAP: usize = 20;

const EMOJI_ACTION_CREATED: i32 = 1;
const EMOJI_ACTION_UPDATE: i32 = 2;
const EMOJI_ACTION_DELETE: i32 = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Emoji {
    pub id: String,
    pub shortname: String,
    pub src: String,
    pub category: String,
    pub clan_id: String,
    pub clan_logo: String,
}

#[derive(Debug, Clone)]
pub enum EmojiEvent {
    Changed,
}

pub struct EmojiStore {
    by_id: HashMap<String, Emoji>,
    order: Vec<String>,
    recent_ids: Vec<String>,
    freshness: Freshness,
    loading: bool,
    api: Arc<AppApi>,
    _clan_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalEmojiStore(Entity<EmojiStore>);
impl Global for GlobalEmojiStore {}

impl EventEmitter<EmojiEvent> for EmojiStore {}

impl EmojiStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalEmojiStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalEmojiStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalEmojiStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.by_id.clear();
        self.order.clear();
        self.recent_ids.clear();
        self.freshness.mark_stale();
        self.loading = false;
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let clan_sub = cx.subscribe(&ClanList::global(cx), |this, _clan, event, cx| {
            if let ClanEvent::ActiveClanChanged(Some(_)) = event {
                this.ensure_loaded(cx);
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        let mut store = Self {
            by_id: HashMap::new(),
            order: Vec::new(),
            recent_ids: Vec::new(),
            freshness: Freshness::new(),
            loading: false,
            api,
            _clan_sub: clan_sub,
            _conn_watch: conn_watch,
        };
        store.fetch_recent(cx);
        store
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::ClanEmoji, &entity, |this, event, cx| {
                this.handle_event(event, cx)
            });
            dispatch.on(RealtimeKind::MessageReaction, &entity, |this, event, cx| {
                this.handle_reaction_echo(event, cx)
            });
            dispatch.on_lagged(&entity, |this, cx| this.refresh(cx));
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
                    if this.update(cx, |this, cx| this.ensure_loaded(cx)).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.freshness.is_fresh(crate::CACHE_TTL) {
            self.fetch(cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx);
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_emojis_by_user_id().await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result.map(|emojis| {
                    emojis
                        .into_iter()
                        .filter_map(emoji_from_proto)
                        .collect::<Vec<_>>()
                }) {
                    Ok(emojis) => {
                        this.by_id.clear();
                        this.order.clear();
                        for emoji in emojis {
                            this.insert(emoji);
                        }
                        this.freshness.mark_fetched();
                        tracing::info!("EmojiStore: fetched {} emojis", this.order.len());
                        cx.emit(EmojiEvent::Changed);
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_emojis_by_user_id failed: {e}"),
                }
            });
        })
        .detach();
    }

    fn fetch_recent(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.emoji_recent_list().await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(recents) => {
                    let mut seen = HashSet::new();
                    this.recent_ids = recents
                        .into_iter()
                        .map(|r| r.emoji_id.to_string())
                        .filter(|id| seen.insert(id.clone()))
                        .take(RECENT_EMOJI_CAP)
                        .collect();
                    cx.emit(EmojiEvent::Changed);
                    cx.notify();
                }
                Err(e) => tracing::error!("emoji_recent_list failed: {e}"),
            });
        })
        .detach();
    }

    fn handle_reaction_echo(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::MessageReaction(r) = event else {
            return;
        };
        if r.action {
            return;
        }
        let Some(current_uid) = BadgeService::global(cx).read(cx).current_user_id(cx) else {
            return;
        };
        if current_uid.get().to_string() != r.sender_id.to_string() {
            return;
        }
        push_recent(&mut self.recent_ids, r.emoji_id.to_string());
        cx.emit(EmojiEvent::Changed);
        cx.notify();
    }

    pub fn recent(&self, limit: usize) -> Vec<&Emoji> {
        self.recent_ids
            .iter()
            .filter_map(|id| self.by_id.get(id))
            .take(limit)
            .collect()
    }

    fn insert(&mut self, emoji: Emoji) {
        let id = emoji.id.clone();
        if self.by_id.insert(id.clone(), emoji).is_none() {
            self.order.push(id);
        }
    }

    fn remove(&mut self, id: &str) -> bool {
        if self.by_id.remove(id).is_some() {
            self.order.retain(|e| e != id);
            true
        } else {
            false
        }
    }

    pub fn all(&self) -> Vec<&Emoji> {
        ordered_emojis(&self.by_id, &self.order).collect()
    }

    pub fn for_clan(&self, clan_id: &str) -> Vec<&Emoji> {
        ordered_emojis(&self.by_id, &self.order)
            .filter(|emoji| emoji.clan_id == clan_id)
            .collect()
    }

    pub fn suggest(&self, query: &str, clan_id: &str, limit: usize) -> Vec<&Emoji> {
        suggest_in(&self.by_id, &self.order, query, clan_id, limit)
    }

    pub fn by_category(&self, active_clan_id: Option<&str>) -> Vec<(String, Vec<&Emoji>)> {
        let mut groups = by_category_in(&self.by_id, &self.order, active_clan_id);
        let recent = self.recent(RECENT_EMOJI_CAP);
        if !recent.is_empty() {
            groups.insert(0, ("Recent".to_string(), recent));
        }
        groups
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::ClanEmoji(e) = event else {
            return;
        };
        let changed = match e.action {
            EMOJI_ACTION_CREATED => {
                if let Some(emoji) = emoji_from_event(e) {
                    self.insert(emoji);
                    true
                } else {
                    false
                }
            }
            EMOJI_ACTION_UPDATE => {
                let id = e.id.to_string();
                match self.by_id.get_mut(&id) {
                    Some(emoji) => {
                        emoji.shortname = e.short_name.clone();
                        if !e.source.is_empty() {
                            emoji.src = e.source.clone();
                        }
                        let category = if !e.category.is_empty() {
                            e.category.clone()
                        } else {
                            e.clan_name.clone()
                        };
                        if !category.is_empty() {
                            emoji.category = category;
                        }
                        true
                    }
                    None => false,
                }
            }
            EMOJI_ACTION_DELETE => self.remove(&e.id.to_string()),
            _ => false,
        };
        if changed {
            cx.emit(EmojiEvent::Changed);
            cx.notify();
        }
    }
}

fn push_recent(recent_ids: &mut Vec<String>, id: String) {
    recent_ids.retain(|existing| existing != &id);
    recent_ids.insert(0, id);
    recent_ids.truncate(RECENT_EMOJI_CAP);
}

fn ordered_emojis<'a>(
    by_id: &'a HashMap<String, Emoji>,
    order: &'a [String],
) -> impl Iterator<Item = &'a Emoji> {
    order.iter().filter_map(move |id| by_id.get(id))
}

fn suggest_in<'a>(
    by_id: &'a HashMap<String, Emoji>,
    order: &'a [String],
    query: &str,
    clan_id: &str,
    limit: usize,
) -> Vec<&'a Emoji> {
    let needle = query.to_lowercase();
    ordered_emojis(by_id, order)
        .filter(|emoji| {
            emoji.clan_id == clan_id && emoji.shortname.to_lowercase().starts_with(&needle)
        })
        .take(limit)
        .collect()
}

fn by_category_in<'a>(
    by_id: &'a HashMap<String, Emoji>,
    order: &'a [String],
    active_clan_id: Option<&str>,
) -> Vec<(String, Vec<&'a Emoji>)> {
    let mut ordered: Vec<&Emoji> = ordered_emojis(by_id, order).collect();
    if let Some(active) = active_clan_id {
        ordered.sort_by_key(|emoji| u8::from(emoji.clan_id != active));
    }
    let mut groups: Vec<(String, Vec<&Emoji>)> = Vec::new();
    for emoji in ordered {
        match groups.iter_mut().find(|(name, _)| name == &emoji.category) {
            Some((_, list)) => list.push(emoji),
            None => groups.push((emoji.category.clone(), vec![emoji])),
        }
    }
    groups
}

fn emoji_from_proto(e: api::ClanEmoji) -> Option<Emoji> {
    if e.id == 0 || e.shortname.is_empty() {
        return None;
    }
    let category = if !e.category.is_empty() {
        e.category
    } else {
        e.clan_name
    };
    Some(Emoji {
        id: e.id.to_string(),
        shortname: e.shortname,
        src: e.src,
        category,
        clan_id: e.clan_id.to_string(),
        clan_logo: e.logo,
    })
}

fn emoji_from_event(e: &realtime::EventEmoji) -> Option<Emoji> {
    if e.id == 0 || e.short_name.is_empty() {
        return None;
    }
    let category = if !e.category.is_empty() {
        e.category.clone()
    } else {
        e.clan_name.clone()
    };
    Some(Emoji {
        id: e.id.to_string(),
        shortname: e.short_name.clone(),
        src: e.source.clone(),
        category,
        clan_id: e.clan_id.to_string(),
        clan_logo: e.logo.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto(
        id: i64,
        shortname: &str,
        category: &str,
        clan_name: &str,
        clan_id: i64,
    ) -> api::ClanEmoji {
        api::ClanEmoji {
            id,
            src: format!("https://cdn/{id}.png"),
            shortname: shortname.into(),
            category: category.into(),
            clan_name: clan_name.into(),
            clan_id,
            ..Default::default()
        }
    }

    fn store_with(emojis: Vec<Emoji>) -> (HashMap<String, Emoji>, Vec<String>) {
        let mut by_id = HashMap::new();
        let mut order = Vec::new();
        for e in emojis {
            let id = e.id.clone();
            if by_id.insert(id.clone(), e).is_none() {
                order.push(id);
            }
        }
        (by_id, order)
    }

    fn emoji(id: &str, shortname: &str, category: &str, clan_id: &str) -> Emoji {
        Emoji {
            id: id.into(),
            shortname: shortname.into(),
            src: String::new(),
            category: category.into(),
            clan_id: clan_id.into(),
            clan_logo: String::new(),
        }
    }

    fn ids(emojis: Vec<&Emoji>) -> Vec<String> {
        emojis.into_iter().map(|e| e.id.clone()).collect()
    }

    #[test]
    fn maps_proto_and_falls_back_to_clan_name_for_category() {
        let with_category = emoji_from_proto(proto(1, "smile", "Faces", "MyClan", 7)).unwrap();
        assert_eq!(with_category.id, "1");
        assert_eq!(with_category.category, "Faces");
        assert_eq!(with_category.clan_id, "7");
        let no_category = emoji_from_proto(proto(2, "wave", "", "MyClan", 7)).unwrap();
        assert_eq!(no_category.category, "MyClan");
    }

    #[test]
    fn skips_invalid_proto() {
        assert!(emoji_from_proto(proto(0, "smile", "", "", 1)).is_none());
        assert!(emoji_from_proto(proto(3, "", "", "", 1)).is_none());
    }

    #[test]
    fn event_create_maps_clan_and_category() {
        let e = realtime::EventEmoji {
            id: 9,
            short_name: "tada".into(),
            source: "https://cdn/9.png".into(),
            category: String::new(),
            clan_name: "Party".into(),
            clan_id: 42,
            action: EMOJI_ACTION_CREATED,
            ..Default::default()
        };
        let mapped = emoji_from_event(&e).unwrap();
        assert_eq!(mapped.id, "9");
        assert_eq!(mapped.shortname, "tada");
        assert_eq!(mapped.category, "Party");
        assert_eq!(mapped.clan_id, "42");
    }

    #[test]
    fn suggest_is_clan_scoped_and_prefix_filtered() {
        let (by_id, order) = store_with(vec![
            emoji("1", "smile", "Faces", "A"),
            emoji("2", "smirk", "Faces", "A"),
            emoji("3", "smile", "Faces", "B"),
            emoji("4", "wave", "Hands", "A"),
        ]);
        assert_eq!(
            ids(suggest_in(&by_id, &order, "sm", "A", 50)),
            vec!["1", "2"]
        );
        assert_eq!(ids(suggest_in(&by_id, &order, "smile", "B", 50)), vec!["3"]);
        assert!(suggest_in(&by_id, &order, "smile", "C", 50).is_empty());
        assert_eq!(ids(suggest_in(&by_id, &order, "wave", "A", 50)), vec!["4"]);
    }

    #[test]
    fn suggest_respects_limit() {
        let (by_id, order) = store_with(vec![
            emoji("1", "smile", "Faces", "A"),
            emoji("2", "smirk", "Faces", "A"),
            emoji("3", "smug", "Faces", "A"),
        ]);
        assert_eq!(suggest_in(&by_id, &order, "sm", "A", 2).len(), 2);
    }

    #[test]
    fn push_recent_moves_existing_to_front_and_dedupes() {
        let mut recent = vec!["1".to_string(), "2".to_string(), "3".to_string()];
        push_recent(&mut recent, "2".to_string());
        assert_eq!(recent, vec!["2", "1", "3"]);
    }

    #[test]
    fn push_recent_caps_at_limit() {
        let mut recent: Vec<String> = (0..RECENT_EMOJI_CAP).map(|i| i.to_string()).collect();
        push_recent(&mut recent, "new".to_string());
        assert_eq!(recent.len(), RECENT_EMOJI_CAP);
        assert_eq!(recent[0], "new");
        assert!(!recent.contains(&(RECENT_EMOJI_CAP - 1).to_string()));
    }

    #[test]
    fn by_category_orders_active_clan_first() {
        let (by_id, order) = store_with(vec![
            emoji("1", "b_smile", "ClanB", "B"),
            emoji("2", "a_smile", "ClanA", "A"),
            emoji("3", "b_wave", "ClanB", "B"),
        ]);
        let groups = by_category_in(&by_id, &order, Some("A"));
        assert_eq!(groups[0].0, "ClanA");
        assert_eq!(ids(groups[0].1.clone()), vec!["2"]);
        assert_eq!(groups[1].0, "ClanB");
        assert_eq!(ids(groups[1].1.clone()), vec!["1", "3"]);
    }
}
