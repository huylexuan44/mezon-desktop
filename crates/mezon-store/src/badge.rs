use gpui::{App, AppContext, Context, Entity, Global};
use mezon_client::RealtimeEvent;

use crate::AuthState;
use crate::channel::ChannelList;
use crate::clan::ClanList;
use crate::clan_members::ClanMembersStore;
use crate::direct::DirectMessageStore;
use crate::ids::{ChannelId, ClanId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const STREAM_MODE_GROUP: i32 = 3;
const STREAM_MODE_DM: i32 = 4;

pub struct BadgeService {
    auth_state: Entity<AuthState>,
}

struct GlobalBadgeService(Entity<BadgeService>);
impl Global for GlobalBadgeService {}

impl BadgeService {
    pub fn init(auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(auth_state, cx));
        cx.set_global(GlobalBadgeService(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalBadgeService>().0.clone()
    }

    fn new(auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ChannelMessage,
                RealtimeKind::MarkAsRead,
                RealtimeKind::LastSeenUpdated,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
        });
        Self { auth_state }
    }

    fn current_user_id(&self, cx: &App) -> Option<i64> {
        match self.auth_state.read(cx) {
            AuthState::Authenticated(session) | AuthState::Connecting(session) => {
                session.user_id.parse::<i64>().ok()
            }
            _ => None,
        }
    }

    fn is_mention(
        &self,
        content: &str,
        references: &[u8],
        user_id: i64,
        clan_id: ClanId,
        cx: &App,
    ) -> bool {
        let role_ids: Vec<i64> = ClanMembersStore::global(cx)
            .read(cx)
            .member(clan_id, UserId(user_id))
            .map(|member| member.role_ids.iter().map(|role| role.get()).collect())
            .unwrap_or_default();
        mezon_client::transport::is_mention_or_reply(content, references, user_id, &role_ids)
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::ChannelMessage(m) => {
                let channel_id = ChannelId(m.channel_id);
                let ts = if m.create_time_seconds > 0 {
                    i64::from(m.create_time_seconds)
                } else {
                    0
                };
                let user_id = self.current_user_id(cx);
                let from_me = matches!(user_id, Some(uid) if uid == m.sender_id);

                if m.mode == STREAM_MODE_GROUP || m.mode == STREAM_MODE_DM {
                    DirectMessageStore::global(cx).update(cx, |dm, cx| {
                        dm.note_message(channel_id, ts, from_me, cx);
                    });
                } else {
                    let clan_id = ClanId(m.clan_id);
                    let is_active =
                        ChannelList::global(cx).read(cx).active_channel_id == Some(channel_id);
                    let app_focused = cx.active_window().is_some();
                    let seen = from_me || (is_active && app_focused);
                    let is_mention = if seen {
                        false
                    } else {
                        user_id
                            .map(|uid| self.is_mention(&m.content, &m.references, uid, clan_id, cx))
                            .unwrap_or(false)
                    };
                    ChannelList::global(cx).update(cx, |cl, cx| {
                        cl.note_channel_message(clan_id, channel_id, is_mention, seen, ts, cx)
                    });
                    if !seen {
                        ClanList::global(cx).update(cx, |cls, cx| {
                            cls.note_channel_message(clan_id, is_mention, cx)
                        });
                    }
                }
            }
            RealtimeEvent::MarkAsRead(e) => {
                let channel_id = ChannelId(e.channel_id);
                let clan_id = ClanId(e.clan_id);
                DirectMessageStore::global(cx).update(cx, |dm, cx| {
                    dm.note_read(channel_id, cx);
                });
                ChannelList::global(cx).update(cx, |cl, cx| cl.apply_read(clan_id, channel_id, cx));
                ClanList::global(cx).update(cx, |cls, cx| cls.apply_badge_read(clan_id, cx));
            }
            RealtimeEvent::LastSeenUpdated(e) => {
                let channel_id = ChannelId(e.channel_id);
                let clan_id = ClanId(e.clan_id);
                let new_badge = e.badge_count.max(0) as u32;
                let seen_ts = i64::from(e.timestamp_seconds);
                ChannelList::global(cx).update(cx, |cl, cx| {
                    cl.apply_last_seen(clan_id, channel_id, new_badge, seen_ts, cx)
                });
            }
            _ => {}
        }
    }
}
