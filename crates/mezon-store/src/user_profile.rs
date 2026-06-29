use gpui::App;

use crate::clan_members::{ClanMember, ClanMembersStore};
use crate::ids::{ClanId, UserId};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserProfileView {
    pub user_id: UserId,
    pub display_name: String,
    pub username: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileContext {
    Clan(ClanId),
}

impl UserProfileView {
    pub fn from_clan_member(member: &ClanMember) -> Self {
        Self {
            user_id: member.id(),
            display_name: member.name().to_string(),
            username: member.user.username.clone(),
            avatar_url: member.avatar().to_string(),
        }
    }
}

pub fn resolve_user_profile(
    user_id: UserId,
    context: ProfileContext,
    cx: &App,
) -> Option<UserProfileView> {
    match context {
        ProfileContext::Clan(clan_id) => ClanMembersStore::global(cx)
            .read(cx)
            .member(clan_id, user_id)
            .map(UserProfileView::from_clan_member),
    }
}
