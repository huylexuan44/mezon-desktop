use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::ids::{ChannelId, UserId};
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::RealtimeEvent;

use crate::channel::ChannelList;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const STATUS_NOTIFY_DEBOUNCE_MS: u64 = 5000;

#[derive(Debug, Clone)]
pub enum PresenceEvent {
    TypingChanged { channel_id: ChannelId },
    ChannelPresenceChanged { channel_id: ChannelId },
    StatusChanged,
}

#[derive(Debug)]
pub struct PresenceStore {
    pub typing_by_channel: HashMap<ChannelId, HashSet<String>>,
    pub channel_online: HashMap<ChannelId, HashSet<UserId>>,
    pub user_online: HashSet<UserId>,
    status_notify_task: Option<Task<()>>,
    _channel_sub: Subscription,
}

struct GlobalPresenceStore(Entity<PresenceStore>);
impl Global for GlobalPresenceStore {}

impl EventEmitter<PresenceEvent> for PresenceStore {}

impl PresenceStore {
    pub fn init(api: Arc<mezon_client::AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalPresenceStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPresenceStore>().0.clone()
    }

    pub fn typing_users(&self, channel_id: ChannelId) -> &HashSet<String> {
        static EMPTY: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);
        self.typing_by_channel.get(&channel_id).unwrap_or(&EMPTY)
    }

    pub fn is_online(&self, user_id: UserId) -> bool {
        self.user_online.contains(&user_id)
    }

    fn new(_api: Arc<mezon_client::AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _channel, event, cx| {
            if let crate::channel::ChannelEvent::ActiveChannelChanged(Some(_)) = event {
                this.typing_by_channel.clear();
                cx.emit(PresenceEvent::StatusChanged);
                cx.notify();
            }
        });

        Self {
            typing_by_channel: HashMap::new(),
            channel_online: HashMap::new(),
            user_online: HashSet::new(),
            status_notify_task: None,
            _channel_sub: channel_sub,
        }
    }

    /// Register realtime handlers with the central dispatcher (cf. `add_message_handler`).
    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::MessageTyping,
                RealtimeKind::ChannelPresence,
                RealtimeKind::StatusPresence,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, cx| {
                tracing::warn!("PresenceStore realtime lagged — clearing state");
                this.typing_by_channel.clear();
                this.channel_online.clear();
                this.user_online.clear();
                cx.emit(PresenceEvent::StatusChanged);
                cx.notify();
            });
        });
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::MessageTyping(e) => {
                let cid = ChannelId(e.channel_id);
                let channel_id = self.apply_typing(
                    cid,
                    &e.sender_display_name,
                    &e.sender_username,
                    &e.sender_id.to_string(),
                    e.mode,
                );
                cx.emit(PresenceEvent::TypingChanged { channel_id });
            }
            RealtimeEvent::ChannelPresence(e) => {
                let cid = ChannelId(e.channel_id);
                let joins: Vec<UserId> = e.joins.iter().map(|u| UserId(u.user_id)).collect();
                let leaves: Vec<UserId> = e.leaves.iter().map(|u| UserId(u.user_id)).collect();
                self.apply_channel_presence(cid, &joins, &leaves);
                cx.emit(PresenceEvent::ChannelPresenceChanged { channel_id: cid });
                cx.notify();
            }
            RealtimeEvent::StatusPresence(e) => {
                let joins: Vec<UserId> = e.joins.iter().map(|u| UserId(u.user_id)).collect();
                let leaves: Vec<UserId> = e.leaves.iter().map(|u| UserId(u.user_id)).collect();
                self.apply_status_presence(&joins, &leaves);
                self.schedule_status_notify(cx);
            }
            _ => {}
        }
    }

    fn schedule_status_notify(&mut self, cx: &mut Context<Self>) {
        if self.status_notify_task.is_some() {
            return;
        }
        let delay = Duration::from_millis(STATUS_NOTIFY_DEBOUNCE_MS);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |store, cx| {
                store.status_notify_task = None;
                cx.emit(PresenceEvent::StatusChanged);
                cx.notify();
            });
        });
        self.status_notify_task = Some(task);
    }

    pub(crate) fn apply_typing(
        &mut self,
        channel_id: ChannelId,
        display_name: &str,
        username: &str,
        sender_id: &str,
        mode: i32,
    ) -> ChannelId {
        let name = if !display_name.is_empty() {
            display_name.to_owned()
        } else if !username.is_empty() {
            username.to_owned()
        } else {
            sender_id.to_owned()
        };
        let entry = self.typing_by_channel.entry(channel_id).or_default();
        if mode == 0 {
            entry.insert(name);
        } else {
            entry.remove(&name);
            if entry.is_empty() {
                self.typing_by_channel.remove(&channel_id);
            }
        }
        channel_id
    }

    pub(crate) fn apply_channel_presence(
        &mut self,
        channel_id: ChannelId,
        joins: &[UserId],
        leaves: &[UserId],
    ) {
        let entry = self.channel_online.entry(channel_id).or_default();
        for uid in joins {
            entry.insert(*uid);
            self.user_online.insert(*uid);
        }
        for uid in leaves {
            entry.remove(uid);
            self.user_online.remove(uid);
        }
        if entry.is_empty() {
            self.channel_online.remove(&channel_id);
        }
    }

    pub(crate) fn apply_status_presence(&mut self, joins: &[UserId], leaves: &[UserId]) {
        for uid in joins {
            self.user_online.insert(*uid);
        }
        for uid in leaves {
            self.user_online.remove(uid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_store() -> PresenceStore {
        PresenceStore {
            typing_by_channel: HashMap::new(),
            channel_online: HashMap::new(),
            user_online: HashSet::new(),
            status_notify_task: None,
            _channel_sub: gpui::Subscription::new(|| {}),
        }
    }

    #[test]
    fn typing_start_adds_user_by_display_name() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "Alice", "alice_user", "uid1", 0);
        assert!(store.typing_by_channel[&ChannelId(1)].contains("Alice"));
    }

    #[test]
    fn typing_start_falls_back_to_username_when_no_display_name() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "", "alice_user", "uid1", 0);
        assert!(store.typing_by_channel[&ChannelId(1)].contains("alice_user"));
    }

    #[test]
    fn typing_start_falls_back_to_sender_id_when_no_name() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "", "", "uid1", 0);
        assert!(store.typing_by_channel[&ChannelId(1)].contains("uid1"));
    }

    #[test]
    fn typing_stop_removes_user_and_cleans_empty_channel() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "Alice", "", "", 0);
        store.apply_typing(ChannelId(1), "Alice", "", "", 1);
        assert!(!store.typing_by_channel.contains_key(&ChannelId(1)));
    }

    #[test]
    fn typing_stop_leaves_other_users_in_channel() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "Alice", "", "", 0);
        store.apply_typing(ChannelId(1), "Bob", "", "", 0);
        store.apply_typing(ChannelId(1), "Alice", "", "", 1);
        assert!(!store.typing_by_channel[&ChannelId(1)].contains("Alice"));
        assert!(store.typing_by_channel[&ChannelId(1)].contains("Bob"));
    }

    #[test]
    fn channel_presence_join_adds_to_channel_and_global() {
        let mut store = empty_store();
        store.apply_channel_presence(ChannelId(1), &[UserId(1), UserId(2)], &[]);
        assert!(store.channel_online[&ChannelId(1)].contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(2)));
    }

    #[test]
    fn channel_presence_leave_removes_from_channel_and_global() {
        let mut store = empty_store();
        store.apply_channel_presence(ChannelId(1), &[UserId(1)], &[]);
        store.apply_channel_presence(ChannelId(1), &[], &[UserId(1)]);
        assert!(!store.channel_online.contains_key(&ChannelId(1)));
        assert!(!store.user_online.contains(&UserId(1)));
    }

    #[test]
    fn channel_presence_empty_channel_cleaned_up() {
        let mut store = empty_store();
        store.apply_channel_presence(ChannelId(1), &[UserId(1)], &[]);
        store.apply_channel_presence(ChannelId(1), &[], &[UserId(1)]);
        assert!(!store.channel_online.contains_key(&ChannelId(1)));
    }

    #[test]
    fn status_presence_join_adds_to_user_online() {
        let mut store = empty_store();
        store.apply_status_presence(&[UserId(1), UserId(2)], &[]);
        assert!(store.user_online.contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(2)));
    }

    #[test]
    fn status_presence_leave_removes_from_user_online() {
        let mut store = empty_store();
        store.apply_status_presence(&[UserId(1)], &[]);
        store.apply_status_presence(&[], &[UserId(1)]);
        assert!(!store.user_online.contains(&UserId(1)));
    }

    #[test]
    fn typing_users_returns_set_for_channel() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "Alice", "", "", 0);
        let users = store.typing_users(ChannelId(1));
        assert!(users.contains("Alice"));
        assert_eq!(users.len(), 1);
    }

    #[test]
    fn typing_users_returns_empty_for_unknown_channel() {
        let store = empty_store();
        assert!(store.typing_users(ChannelId(999)).is_empty());
    }
}
