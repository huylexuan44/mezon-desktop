use gpui::{
    Anchor, AnyElement, App, Context, Entity, FontWeight, MouseButton, MouseDownEvent, Pixels,
    Point, SharedString, UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px, rgb,
    uniform_list,
};
use mezon_store::{
    ChannelEvent, ChannelId, ChannelList, ChannelMembersEvent, ChannelMembersStore, ClanId,
    ClanList, ClanMember, ClanMembersEvent, ClanMembersStore, DirectKind, DirectMessageStore,
    GroupMember, GroupMembersEvent, GroupMembersStore, PresenceEvent, PresenceStore,
    ProfileContext, Settings, UserId, split_members_by_status,
};

use crate::app::shell::Shell;
use crate::chat::user_profile_popover::{ClickableContainer, profile_popover_menu};
use crate::components::primitives::{Avatar, ContextMenu, Icon, IconName, context_menu_at};
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
    user_id: UserId,
    name: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    online: bool,
    is_owner: bool,
    rcm_id: SharedString,
    popover_id: SharedString,
    trigger_id: SharedString,
}

struct RawMember {
    user_id: UserId,
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
    active_context: Option<ProfileContext>,
    open_menu: Option<(UserId, SharedString, Point<Pixels>)>,
}

impl MemberListPanel {
    pub fn new(source: MemberSource, settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&Router::global(cx), |this, _, cx| this.rebuild(cx))
            .detach();
        cx.subscribe(
            &PresenceStore::global(cx),
            |this, _, event, cx| match event {
                PresenceEvent::TypingChanged { .. } => {}
                PresenceEvent::ChannelPresenceChanged { channel_id } => {
                    let relevant = match this.source {
                        MemberSource::Channel => shows_channel(*channel_id, cx),
                        MemberSource::Group => shows_group(*channel_id, cx),
                    };
                    if relevant {
                        this.rebuild(cx);
                    }
                }
                PresenceEvent::StatusChanged => this.rebuild(cx),
            },
        )
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
                cx.subscribe(&ChannelList::global(cx), |this, _, event, cx| {
                    if let ChannelEvent::ActiveChannelChanged(_) = event {
                        this.rebuild(cx);
                    }
                })
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
            active_context: None,
            open_menu: None,
        };
        this.rebuild(cx);
        this
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.active_context = match self.source {
            MemberSource::Channel => {
                active_channel_context(cx).map(|ctx| ProfileContext::Clan(ctx.clan_id))
            }
            MemberSource::Group => active_group_dm(cx).map(ProfileContext::Direct),
        };
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
            let owner_id = DirectMessageStore::global(cx)
                .read(cx)
                .find(direct_id)
                .and_then(|dm| dm.creator_id);
            let mut rows = Vec::with_capacity(members.len() + 1);
            rows.push(Row::Header {
                kind: HeaderKind::Members,
                count: members.len(),
            });
            rows.extend(
                members
                    .into_iter()
                    .map(|raw| make_member_row(cx, raw, owner_id)),
            );
            rows
        }
        MemberSource::Channel => {
            let Some(ctx) = active_channel_context(cx) else {
                return Vec::new();
            };
            let owner_id = ClanList::global(cx)
                .read(cx)
                .clan(ctx.clan_id)
                .map(|clan| clan.creator_id);
            let (online_raw, offline_raw) = channel_raw_members(cx, ctx);
            let mut rows = Vec::with_capacity(online_raw.len() + offline_raw.len() + 2);
            rows.push(Row::Header {
                kind: HeaderKind::Online,
                count: online_raw.len(),
            });
            rows.extend(
                online_raw
                    .into_iter()
                    .map(|raw| make_member_row(cx, raw, owner_id)),
            );
            rows.push(Row::Header {
                kind: HeaderKind::Offline,
                count: offline_raw.len(),
            });
            rows.extend(
                offline_raw
                    .into_iter()
                    .map(|raw| make_member_row(cx, raw, owner_id)),
            );
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
    let presence = PresenceStore::global(cx);
    let presence = presence.read(cx);
    let online = &presence.user_online;
    let store = ClanMembersStore::global(cx);
    let store = store.read(cx);
    let pool: Vec<&ClanMember> = match &ctx.filter_ids {
        Some(ids) => ids
            .iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .collect(),
        None => store.members(ctx.clan_id),
    };
    let (online_ids, offline_ids) = split_members_by_status(&pool, online);
    let to_raw = |ids: &[UserId], is_online: bool| -> Vec<RawMember> {
        ids.iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .map(|member| RawMember {
                user_id: member.id(),
                name: member.name().to_string(),
                avatar_raw: member.avatar().to_string(),
                online: is_online,
            })
            .collect()
    };
    (to_raw(&online_ids, true), to_raw(&offline_ids, false))
}

fn group_raw_members(cx: &App, direct_id: ChannelId) -> Vec<RawMember> {
    let presence = PresenceStore::global(cx);
    let presence = presence.read(cx);
    let presence_online = &presence.user_online;
    let store = GroupMembersStore::global(cx);
    let store = store.read(cx);
    let mut members: Vec<&GroupMember> = store.members(direct_id).iter().collect();
    members.sort_by_cached_key(|m| m.name().to_lowercase());
    members
        .into_iter()
        .map(|member| RawMember {
            user_id: member.id(),
            name: member.name().to_string(),
            avatar_raw: member.avatar().to_string(),
            online: member.online || presence_online.contains(&member.id()),
        })
        .collect()
}

#[derive(Clone)]
pub(crate) struct MentionMemberRaw {
    pub user_id: String,
    pub display: String,
    pub username: String,
    pub avatar_raw: String,
    pub display_lc: String,
    pub username_lc: String,
    pub avatar_src: SharedString,
}

fn mention_avatar_src(cx: &App, avatar: &str) -> SharedString {
    if avatar.is_empty() {
        SharedString::default()
    } else {
        SharedString::from(crate::util::imgproxy::avatar_url(cx, avatar))
    }
}

pub(crate) fn mention_member_pool(cx: &App) -> Vec<MentionMemberRaw> {
    if let Some(direct_id) = active_group_dm(cx) {
        let store = GroupMembersStore::global(cx);
        let store = store.read(cx);
        return store
            .members(direct_id)
            .iter()
            .map(|m| MentionMemberRaw {
                user_id: m.id().to_string(),
                display: m.name().to_string(),
                username: m.user.username.clone(),
                avatar_raw: m.avatar().to_string(),
                display_lc: m.name().to_lowercase(),
                username_lc: m.user.username.to_lowercase(),
                avatar_src: mention_avatar_src(cx, m.avatar()),
            })
            .collect();
    }
    let Some(ctx) = active_channel_context(cx) else {
        return Vec::new();
    };
    let store = ClanMembersStore::global(cx);
    let store = store.read(cx);
    let pool: Vec<&ClanMember> = match &ctx.filter_ids {
        Some(ids) => ids
            .iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .collect(),
        None => store.members(ctx.clan_id),
    };
    pool.iter()
        .map(|m| MentionMemberRaw {
            user_id: m.user.id.to_string(),
            display: m.name().to_string(),
            username: m.user.username.clone(),
            avatar_raw: m.avatar().to_string(),
            display_lc: m.name().to_lowercase(),
            username_lc: m.user.username.to_lowercase(),
            avatar_src: mention_avatar_src(cx, m.avatar()),
        })
        .collect()
}

fn make_member_row(cx: &App, raw: RawMember, owner_id: Option<UserId>) -> Row {
    let avatar_src = if raw.avatar_raw.is_empty() {
        SharedString::default()
    } else {
        SharedString::from(crate::util::imgproxy::avatar_url(cx, &raw.avatar_raw))
    };
    let id = raw.user_id.0;
    Row::Member(MemberRow {
        rcm_id: SharedString::from(format!("member-rcm-{id}")),
        popover_id: SharedString::from(format!("member-popover-{id}")),
        trigger_id: SharedString::from(format!("member-trigger-{id}")),
        user_id: raw.user_id,
        name: raw.name.into(),
        avatar_src,
        avatar_raw: raw.avatar_raw.into(),
        online: raw.online,
        is_owner: owner_id == Some(raw.user_id),
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
    context: Option<ProfileContext>,
    settings: &Entity<Settings>,
    panel: WeakEntity<MemberListPanel>,
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

    let row_content = div()
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
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(member.name.clone()),
                )
                .when(member.is_owner, |this| {
                    this.child(
                        Icon::new(IconName::OwnerIcon)
                            .size(px(14.))
                            .text_color(rgb(0xF0B132)),
                    )
                }),
        );

    let user_id = member.user_id;
    let rcm_id = member.rcm_id.clone();

    let inner = match context {
        Some(ctx) => {
            profile_popover_menu(member.popover_id.clone(), user_id, ctx, settings.clone())
                .anchor(Anchor::TopRight)
                .attach(Anchor::TopLeft)
                .trigger(
                    ClickableContainer::new(member.trigger_id.clone())
                        .flex()
                        .flex_1()
                        .cursor_pointer()
                        .child(row_content),
                )
                .into_any_element()
        }
        None => row_content.into_any_element(),
    };

    let display_name = member.name.clone();
    div()
        .id(rcm_id)
        .flex()
        .flex_1()
        .on_mouse_down(MouseButton::Right, {
            let panel = panel.clone();
            let display_name = display_name.clone();
            move |event: &MouseDownEvent, _window, cx| {
                let position = event.position;
                if let Some(p) = panel.upgrade() {
                    p.update(cx, |this, cx| {
                        this.open_menu = Some((user_id, display_name.clone(), position));
                        cx.notify();
                    });
                }
            }
        })
        .child(inner)
        .into_any_element()
}

impl Render for MemberListPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("MemberListPanel");
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let count = self.rows.get().len();
        let entity = cx.entity();
        let avatar_image_cache = self.avatar_image_cache.clone();
        let context = self.active_context;
        let settings = self.settings.clone();
        let panel_weak = cx.entity().downgrade();
        let menu_overlay = self.open_menu.as_ref().map(|(user_id, display_name, pos)| {
            (
                *user_id,
                display_name.clone(),
                *pos,
                context,
                settings.clone(),
                locale.clone(),
                panel_weak.clone(),
            )
        });

        let list = uniform_list("member-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let locale = locale.clone();
            let rows = entity.read(cx).rows.get();
            range
                .map(|ix| match rows.get(ix) {
                    Some(Row::Header { kind, count }) => {
                        render_header(&theme, &locale, kind, *count)
                    }
                    Some(Row::Member(member)) => render_member(
                        &theme,
                        member,
                        &avatar_image_cache,
                        context,
                        &settings,
                        panel_weak.clone(),
                    ),
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
            .when_some(
                menu_overlay,
                |el, (_user_id, display_name, pos, ctx, settings, locale, panel)| {
                    el.child(context_menu_at(
                        pos,
                        build_member_menu(display_name, ctx, settings, panel, &locale),
                    ))
                },
            )
    }
}

fn toast_coming_soon(settings: Entity<Settings>) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let locale = settings.read(cx).language.clone();
        let msg = mezon_i18n::t(&locale, "common.comingSoon").to_string();
        Shell::global(cx).update(cx, |shell, cx| shell.info(msg, cx));
    }
}

fn build_member_menu(
    display_name: SharedString,
    context: Option<ProfileContext>,
    settings: Entity<Settings>,
    panel: WeakEntity<MemberListPanel>,
    locale: &str,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let is_clan = matches!(context, Some(ProfileContext::Clan(_)));

    let dismiss = {
        let panel = panel.clone();
        move |_window: &mut Window, cx: &mut App| {
            if let Some(p) = panel.upgrade() {
                p.update(cx, |this, cx| {
                    this.open_menu = None;
                    cx.notify();
                });
            }
        }
    };

    let remove_from_thread_label = mezon_i18n::t(locale, "contextMenu.member.removeFromThread")
        .replace("{{username}}", display_name.as_ref());

    let mut menu = ContextMenu::new()
        .on_dismiss(dismiss)
        .item(
            t("contextMenu.member.profile"),
            toast_coming_soon(settings.clone()),
        )
        .item(
            t("contextMenu.member.message"),
            toast_coming_soon(settings.clone()),
        )
        .item(
            t("contextMenu.member.shareContact"),
            toast_coming_soon(settings.clone()),
        )
        .item(
            t("contextMenu.member.addFriend"),
            toast_coming_soon(settings.clone()),
        )
        .separator()
        .danger_item(
            t("contextMenu.member.removeFriend"),
            toast_coming_soon(settings.clone()),
        );

    if is_clan {
        menu = menu
            .separator()
            .danger_item(
                t("contextMenu.member.banChat"),
                toast_coming_soon(settings.clone()),
            )
            .danger_item(
                t("contextMenu.member.kick"),
                toast_coming_soon(settings.clone()),
            )
            .danger_item(remove_from_thread_label, toast_coming_soon(settings));
    }

    menu
}
