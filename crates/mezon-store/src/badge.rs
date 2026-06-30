use gpui::{App, AppContext, Context, Entity, Global};
use mezon_client::RealtimeEvent;
use mezon_proto::api::ChannelMessage;

use crate::AuthState;
use crate::channel::ChannelList;
use crate::clan::ClanList;
use crate::clan_members::ClanMembersStore;
use crate::direct::DirectMessageStore;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use crate::message::MessageCode;
use crate::messages::MessagesStore;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const STREAM_MODE_GROUP: i32 = 3;
const STREAM_MODE_DM: i32 = 4;

fn is_dm_message(m: &ChannelMessage) -> bool {
    m.mode == STREAM_MODE_GROUP || m.mode == STREAM_MODE_DM || m.clan_id == 0
}

fn is_content_mutation(m: &ChannelMessage) -> bool {
    matches!(
        MessageCode::from_raw(m.code),
        MessageCode::ChatUpdate
            | MessageCode::ChatRemove
            | MessageCode::UpdateEphemeralMsg
            | MessageCode::DeleteEphemeralMsg
    )
}

/// Matches React `ChatContext` `isNotCurrentDirect` before `badgeService.incrementDm`.
fn should_increment_dm_unread(cx: &App, channel_id: ChannelId, from_me: bool) -> bool {
    if from_me {
        return false;
    }
    let messages = MessagesStore::global(cx).read(cx);
    let app_focused = cx.active_window().is_some();
    let viewing_this_dm =
        messages.is_dm() && messages.active_channel_id() == Some(channel_id) && app_focused;
    !viewing_this_dm
}

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

    pub fn current_user_id(&self, cx: &App) -> Option<UserId> {
        self.current_user_id_raw(cx).map(UserId)
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

    fn current_user_id_raw(&self, cx: &App) -> Option<i64> {
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
                let user_id = self.current_user_id_raw(cx);
                let from_me = matches!(user_id, Some(uid) if uid == m.sender_id);
                let message_id = MessageId(m.message_id);

                if is_dm_message(m) {
                    let increment_unread =
                        should_increment_dm_unread(cx, channel_id, from_me) && !is_content_mutation(m);
                    DirectMessageStore::global(cx).update(cx, |dm, cx| {
                        dm.note_message(channel_id, ts, from_me, increment_unread, cx);
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
                        cl.note_channel_message(
                            clan_id,
                            channel_id,
                            is_mention,
                            seen,
                            ts,
                            message_id,
                            cx,
                        )
                    });
                    if seen {
                        MessagesStore::global(cx).update(cx, |store, _| {
                            store.set_last_read_message(channel_id, message_id);
                        });
                    }
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
                let seen_message_id = MessageId(e.message_id);
                if clan_id.is_zero() {
                    DirectMessageStore::global(cx).update(cx, |dm, cx| {
                        dm.note_read(channel_id, cx);
                    });
                } else {
                    ChannelList::global(cx).update(cx, |cl, cx| {
                        cl.apply_last_seen(
                            clan_id,
                            channel_id,
                            new_badge,
                            seen_ts,
                            seen_message_id,
                            cx,
                        )
                    });
                }
                if !seen_message_id.is_zero() {
                    MessagesStore::global(cx).update(cx, |store, _| {
                        store.set_last_read_message(channel_id, seen_message_id);
                    });
                }
            }
            _ => {}
        }
    }
}
