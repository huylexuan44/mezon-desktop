use gpui::{
    AnyElement, App, Context, Entity, FontWeight, SharedString, UniformListScrollHandle, Window,
    div, prelude::*, px, uniform_list,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelMembersEvent, ChannelMembersStore, ClanId, ClanList, ClanMember,
    ClanMembersEvent, ClanMembersStore, DirectKind, DirectMessageStore, GroupMember,
    GroupMembersEvent, GroupMembersStore, PresenceEvent, PresenceStore, Settings, UserId,
    split_members_by_status,
};

use crate::components::primitives::Avatar;
use crate::image_cache::{
    AVATAR_ENTRY_MAX_BYTES, AVATAR_IMAGE_CACHE_BYTES, AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache,
};
use crate::router::{Route, Router};
use crate::theme::{ActiveTheme, Theme};
use crate::util::reactive::Derived;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MemberSource {
    Channel,
    Group,
}

#[derive(PartialEq)]
enum HeaderKind {
    Online,
    Offline,
    Members,
}

#[derive(PartialEq)]
enum Row {
    Header { kind: HeaderKind, count: usize },
    Member(MemberRow),
}

#[derive(PartialEq)]
struct MemberRow {
    name: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    online: bool,
}

struct RawMember {
    name: String,
    avatar_raw: String,
    online: bool,
}

pub struct MemberListPanel {
    source: MemberSource,
    settings: Entity<Settings>,
    rows: Derived<Vec<Row>>,
    list_scroll: UniformListScrollHandle,
    avatar_image_cache: Entity<LruImageCache>,
}

impl MemberListPanel {
    pub fn new(source: MemberSource, settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&Router::global(cx), |this, _, cx| this.rebuild(cx))
            .detach();
        cx.subscribe(&PresenceStore::global(cx), |this, _, event, cx| {
            if !matches!(event, PresenceEvent::TypingChanged { .. }) {
                this.rebuild(cx);
            }
        })
        .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();

        match source {
            MemberSource::Channel => {
                cx.subscribe(&ClanMembersStore::global(cx), |this, _, event, cx| {
                    let ClanMembersEvent::Changed { clan_id } = event;
                    if shows_clan(*clan_id, cx) {
                        this.rebuild(cx);
                    }
                })
                .detach();
                cx.subscribe(&ChannelMembersStore::global(cx), |this, _, event, cx| {
                    let ChannelMembersEvent::Changed { channel_id } = event;
                    if shows_channel(*channel_id, cx) {
                        this.rebuild(cx);
                    }
                })
                .detach();
                cx.observe(&ChannelList::global(cx), |this, _, cx| this.rebuild(cx))
                    .detach();
            }
            MemberSource::Group => {
                cx.subscribe(&GroupMembersStore::global(cx), |this, _, event, cx| {
                    let GroupMembersEvent::Changed { channel_id } = event;
                    if shows_group(*channel_id, cx) {
                        this.rebuild(cx);
                    }
                })
                .detach();
                cx.observe(&DirectMessageStore::global(cx), |this, _, cx| {
                    this.rebuild(cx)
                })
                .detach();
            }
        }

        let avatar_image_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "member-avatar",
                AVATAR_IMAGE_CACHE_CAPACITY,
                AVATAR_IMAGE_CACHE_BYTES,
                AVATAR_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let mut this = Self {
            source,
            settings,
            rows: Derived::default(),
            list_scroll: UniformListScrollHandle::new(),
            avatar_image_cache,
        };
        this.rebuild(cx);
        this
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let rows = compute_rows(self.source, cx);
        self.rows.set(rows, cx);
    }
}

fn shows_clan(clan_id: ClanId, cx: &App) -> bool {
    ClanList::global(cx).read(cx).active_clan_id == Some(clan_id)
}

fn shows_channel(channel_id: ChannelId, cx: &App) -> bool {
    ChannelList::global(cx).read(cx).active_channel_id == Some(channel_id)
}

fn shows_group(channel_id: ChannelId, cx: &App) -> bool {
    active_group_dm(cx) == Some(channel_id)
}

fn compute_rows(source: MemberSource, cx: &mut Context<MemberListPanel>) -> Vec<Row> {
    match source {
        MemberSource::Group => {
            let Some(direct_id) = active_group_dm(cx) else {
                return Vec::new();
            };
            let members = group_raw_members(cx, direct_id);
            if members.is_empty() {
                return Vec::new();
            }
            let mut rows = Vec::with_capacity(members.len() + 1);
            rows.push(Row::Header {
                kind: HeaderKind::Members,
                count: members.len(),
            });
            rows.extend(members.into_iter().map(|raw| make_member_row(cx, raw)));
            rows
        }
        MemberSource::Channel => {
            let Some(ctx) = active_channel_context(cx) else {
                return Vec::new();
            };
            let (online_raw, offline_raw) = channel_raw_members(cx, ctx);
            let mut rows = Vec::with_capacity(online_raw.len() + offline_raw.len() + 2);
            rows.push(Row::Header {
                kind: HeaderKind::Online,
                count: online_raw.len(),
            });
            rows.extend(online_raw.into_iter().map(|raw| make_member_row(cx, raw)));
            rows.push(Row::Header {
                kind: HeaderKind::Offline,
                count: offline_raw.len(),
            });
            rows.extend(offline_raw.into_iter().map(|raw| make_member_row(cx, raw)));
            rows
        }
    }
}

fn active_group_dm(cx: &App) -> Option<ChannelId> {
    let Route::DirectMessage { direct_id, .. } = Router::global(cx).read(cx).route() else {
        return None;
    };
    let is_group = DirectMessageStore::global(cx)
        .read(cx)
        .find(direct_id)
        .map(|dm| dm.kind == DirectKind::Group)
        .unwrap_or(false);
    is_group.then_some(direct_id)
}

struct ChannelContext {
    clan_id: ClanId,
    filter_ids: Option<Vec<UserId>>,
}

fn active_channel_context(cx: &App) -> Option<ChannelContext> {
    if matches!(
        Router::global(cx).read(cx).route(),
        Route::DirectMessage { .. } | Route::Direct | Route::Friends
    ) {
        return None;
    }
    let channel_list = ChannelList::global(cx);
    let channels = channel_list.read(cx);
    let (channel_id, use_filter, clan_id) = match channels.active_channel() {
        Some(channel) => {
            let is_thread = channel.parent_id.map(|p| !p.is_zero()).unwrap_or(false);
            (
                Some(channel.id),
                channel.private || is_thread,
                channel.clan_id,
            )
        }
        None => (
            None,
            false,
            ClanList::global(cx)
                .read(cx)
                .active_clan_id
                .unwrap_or_default(),
        ),
    };
    if clan_id.is_zero() {
        return None;
    }
    let filter_ids = if use_filter {
        channel_id.map(|cid| ChannelMembersStore::global(cx).read(cx).member_ids(cid))
    } else {
        None
    };
    Some(ChannelContext {
        clan_id,
        filter_ids,
    })
}

fn channel_raw_members(cx: &App, ctx: ChannelContext) -> (Vec<RawMember>, Vec<RawMember>) {
    let online = PresenceStore::global(cx).read(cx).user_online.clone();
    let store = ClanMembersStore::global(cx);
    let store = store.read(cx);
    let pool: Vec<&ClanMember> = match &ctx.filter_ids {
        Some(ids) => ids
            .iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .collect(),
        None => store.members(ctx.clan_id),
    };
    let (online_ids, offline_ids) = split_members_by_status(&pool, &online);
    let to_raw = |ids: &[UserId], is_online: bool| -> Vec<RawMember> {
        ids.iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .map(|member| RawMember {
                name: member.name().to_string(),
                avatar_raw: member.avatar().to_string(),
                online: is_online,
            })
            .collect()
    };
    (to_raw(&online_ids, true), to_raw(&offline_ids, false))
}

fn group_raw_members(cx: &App, direct_id: ChannelId) -> Vec<RawMember> {
    let presence_online = PresenceStore::global(cx).read(cx).user_online.clone();
    let store = GroupMembersStore::global(cx);
    let store = store.read(cx);
    let mut members: Vec<&GroupMember> = store.members(direct_id).iter().collect();
    members.sort_by_key(|m| m.name().to_lowercase());
    members
        .into_iter()
        .map(|member| RawMember {
            name: member.name().to_string(),
            avatar_raw: member.avatar().to_string(),
            online: member.online || presence_online.contains(&member.id()),
        })
        .collect()
}

fn make_member_row(cx: &App, raw: RawMember) -> Row {
    // The dev image proxy 404s on avatar source URLs, forcing a fallback to the
    // raw (full-resolution) file. For the member list we skip the proxy on dev
    // and use the raw URL directly: it avoids the wasted 404 round-trip, and the
    // dev server already serves avatars at avatar size.
    let skip_proxy = mezon_store::AppConfig::try_global(cx)
        .map(|cfg| cfg.is_dev_imgproxy())
        .unwrap_or(false);
    let avatar_src = if raw.avatar_raw.is_empty() {
        SharedString::default()
    } else if skip_proxy {
        SharedString::from(raw.avatar_raw.clone())
    } else {
        SharedString::from(crate::util::imgproxy::avatar_url(cx, &raw.avatar_raw))
    };
    Row::Member(MemberRow {
        name: raw.name.into(),
        avatar_src,
        avatar_raw: raw.avatar_raw.into(),
        online: raw.online,
    })
}

fn render_header(theme: &Theme, locale: &str, kind: &HeaderKind, count: usize) -> AnyElement {
    let label = match kind {
        HeaderKind::Members => {
            format!("{} - {}", mezon_i18n::t(locale, "common.members"), count).to_uppercase()
        }
        HeaderKind::Online => mezon_i18n::t(locale, "memberPage.onlineCount")
            .replace("{{count}}", &count.to_string())
            .to_uppercase(),
        HeaderKind::Offline => mezon_i18n::t(locale, "memberPage.offlineCount")
            .replace("{{count}}", &count.to_string())
            .to_uppercase(),
    };
    div()
        .flex()
        .items_center()
        .px_4()
        .h(px(48.))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(label),
        )
        .into_any_element()
}

fn render_member(
    theme: &Theme,
    member: &MemberRow,
    avatar_image_cache: &Entity<LruImageCache>,
) -> AnyElement {
    let mut avatar = Avatar::new()
        .name(member.name.clone())
        .size_px(px(32.))
        .image_cache(avatar_image_cache.clone());
    if !member.avatar_src.is_empty() {
        avatar = avatar
            .src(member.avatar_src.clone())
            .fallback_src(member.avatar_raw.clone());
    }

    let dot_color = if member.online {
        theme.status_online
    } else {
        theme.text_muted
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(9.))
        .px_4()
        .h(px(48.))
        .when(!member.online, |this| this.opacity(0.5))
        .child(
            div().relative().flex_shrink_0().child(avatar).child(
                div()
                    .absolute()
                    .bottom(px(-1.))
                    .right(px(-1.))
                    .size(px(12.))
                    .rounded_full()
                    .border_2()
                    .border_color(theme.bg_secondary)
                    .bg(dot_color),
            ),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(member.name.clone()),
        )
        .into_any_element()
}

impl Render for MemberListPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("MemberListPanel");
        // Avatars use a plain byte-budget LRU (no per-frame viewport sweep):
        // they are small, disk-cached, and reused, so sweeping them would force
        // constant re-fetches (and re-trigger failed loads) on every re-render.
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let count = self.rows.get().len();
        let entity = cx.entity();
        let avatar_image_cache = self.avatar_image_cache.clone();

        let list = uniform_list("member-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let locale = locale.clone();
            let rows = entity.read(cx).rows.get();
            range
                .map(|ix| match rows.get(ix) {
                    Some(Row::Header { kind, count }) => {
                        render_header(&theme, &locale, kind, *count)
                    }
                    Some(Row::Member(member)) => render_member(&theme, member, &avatar_image_cache),
                    None => div().into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0()
        .pr(px(2.));

        div()
            .flex()
            .flex_col()
            .w(px(245.))
            .h_full()
            .flex_shrink_0()
            .bg(theme.bg_secondary)
            .border_l_1()
            .border_color(theme.border)
            .child(list)
    }
}
