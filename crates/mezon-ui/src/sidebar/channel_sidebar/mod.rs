use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, ListState, MouseButton, MouseDownEvent, SharedString,
    Subscription, Task, WeakEntity, Window, div, list, prelude::*, px,
};
use mezon_store::{ChannelList, ClanId, ClanList, FAVOR_CATE_ID, Settings};

use crate::components::compositions::channel_row::ChannelRow;
use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size, context_menu_at};
use crate::theme::ActiveTheme;

mod items;
mod menu;
use items::{AppChannelSlot, SidebarItem, VoiceMemberSlot};
use menu::{OpenMenu, build_channel_menu, on_category_click, on_channel_click};

pub struct ChannelSidebar {
    clan_list: Entity<ClanList>,
    channel_list: Entity<ChannelList>,
    settings: Entity<Settings>,
    items: Rc<Vec<SidebarItem>>,
    list_state: ListState,
    active_clan_name: String,
    active_clan_id: Option<ClanId>,
    loaded_clans: HashSet<ClanId>,
    skeleton_armed: bool,
    _skeleton_timer: Option<Task<()>>,
    channel_list_handle: Entity<ChannelList>,
    open_menu: Option<OpenMenu>,
    _clan_observe: Subscription,
    _channel_observe: Subscription,
    _settings_observe: Subscription,
    _router_observe: Subscription,
}

impl ChannelSidebar {
    pub fn new(
        clan_list: Entity<ClanList>,
        channel_list: Entity<ChannelList>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let channel_list_handle = channel_list.clone();

        let clan_observe = cx.observe(&clan_list, |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });
        let channel_observe = cx.observe(&channel_list, |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });
        let settings_observe = cx.observe(&settings, |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });
        let router_observe = cx.observe(&crate::router::Router::global(cx), |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });

        let mut this = Self {
            clan_list,
            channel_list,
            settings,
            items: Rc::new(Vec::new()),
            list_state: ListState::new(0, gpui::ListAlignment::Top, px(32.)),
            active_clan_name: String::new(),
            active_clan_id: None,
            loaded_clans: HashSet::new(),
            skeleton_armed: false,
            _skeleton_timer: None,
            channel_list_handle,
            open_menu: None,
            _clan_observe: clan_observe,
            _channel_observe: channel_observe,
            _settings_observe: settings_observe,
            _router_observe: router_observe,
        };
        this.rebuild_items(cx);
        this
    }

    fn rebuild_items(&mut self, cx: &mut Context<Self>) -> bool {
        let locale = self.settings.read(cx).language.clone();
        let clans = self.clan_list.read(cx);
        let channels = self.channel_list.read(cx);

        let new_clan_id = clans.active_clan_id;
        let clan_changed = self.active_clan_id != new_clan_id;

        let new_clan_name = clans
            .active_clan()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| mezon_i18n::t(&locale, "sidebar.selectClan").to_string());
        let name_changed = new_clan_name != self.active_clan_name;
        self.active_clan_name = new_clan_name;
        self.active_clan_id = new_clan_id;

        let active_channel_id = match crate::router::Router::global(cx).read(cx).route() {
            crate::router::Route::Channel { channel_id, .. }
            | crate::router::Route::Thread { channel_id, .. }
            | crate::router::Route::Canvas { channel_id, .. } => Some(channel_id),
            _ => None,
        };
        let mut items = Vec::new();

        let app_channels: Vec<AppChannelSlot> = clans
            .active_clan_id
            .as_ref()
            .map(|cid| {
                channels
                    .app_channels_for_clan(*cid)
                    .iter()
                    .map(AppChannelSlot::from)
                    .collect()
            })
            .unwrap_or_default();

        items.push(SidebarItem::BannerAndEvents {
            banner_url: clans.active_clan_banner().map(|s| s.to_string()),
            app_channels,
        });

        let mut arm_skeleton = false;
        if let Some(clan_id) = clans.active_clan_id.as_ref() {
            let categories = channels.categories_for_clan(*clan_id);
            if categories.is_empty() {
                if self.loaded_clans.contains(clan_id) {
                    self.skeleton_armed = false;
                } else {
                    if clan_changed {
                        self.skeleton_armed = false;
                        arm_skeleton = true;
                    }
                    if self.skeleton_armed {
                        items.push(SidebarItem::Skeleton);
                    }
                }
            } else {
                self.loaded_clans.insert(*clan_id);
                self.skeleton_armed = false;
                self._skeleton_timer = None;
                for category in categories {
                    let is_favorites = category.id == FAVOR_CATE_ID;
                    let collapsed = channels.is_category_collapsed(*clan_id, &category.id);
                    let name = if is_favorites {
                        mezon_i18n::t(&locale, "channelList.favoriteChannel").to_string()
                    } else {
                        category.name.clone()
                    };
                    items.push(SidebarItem::Category {
                        elem_id: SharedString::from(format!("cat-{}", &category.id)),
                        id: category.id.clone(),
                        name_upper: name.to_uppercase(),
                        collapsed,
                    });
                    if !collapsed {
                        let ch_slice = &category.channels;
                        for (idx, ch) in ch_slice.iter().enumerate() {
                            let is_thread = !is_favorites && ch.parent_id.is_some();
                            let (line_above, line_below) = if is_thread {
                                let pid = ch.parent_id;
                                let has_prev = ch_slice[..idx].iter().any(|c| c.parent_id == pid);
                                let has_next =
                                    ch_slice[idx + 1..].iter().any(|c| c.parent_id == pid);
                                (has_prev, has_next)
                            } else {
                                (false, false)
                            };
                            let badge_count = ch.badge_count;
                            let badge_label = if badge_count > 99 {
                                SharedString::from("99+")
                            } else if badge_count > 0 {
                                SharedString::from(badge_count.to_string())
                            } else {
                                SharedString::from("")
                            };
                            items.push(SidebarItem::Channel {
                                elem_id: SharedString::from(format!(
                                    "ch-{}-{}",
                                    &category.id, &ch.id
                                )),
                                id: ch.id.to_string(),
                                name: truncate_channel_label(&ch.name),
                                channel_type: ch.channel_type,
                                unread: ch.is_unread(),
                                private: ch.private,
                                selected: active_channel_id == Some(ch.id),
                                badge_count,
                                badge_label,
                                muted: ch.muted,
                                is_thread,
                                line_above,
                                line_below,
                                voice_members: ch
                                    .voice_members
                                    .iter()
                                    .map(VoiceMemberSlot::from)
                                    .collect(),
                            });
                        }
                    }
                }
            }
        } else {
            items.push(SidebarItem::Skeleton);
        }

        if arm_skeleton {
            self._skeleton_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                this.update(cx, |this, cx| {
                    this.skeleton_armed = true;
                    cx.notify();
                })
                .ok();
            }));
        }

        let new_count = items.len();
        let old_count = self.items.len();
        let items_changed = *self.items != items;
        self.items = Rc::new(items);

        if clan_changed || old_count == 0 {
            self.list_state.reset(new_count);
        } else if new_count > old_count {
            self.list_state
                .splice(old_count..old_count, new_count - old_count);
        } else if new_count < old_count {
            self.list_state.splice(new_count..old_count, 0);
        }

        items_changed || name_changed || clan_changed
    }
}

impl Render for ChannelSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("ChannelSidebar");
        let theme = cx.theme();
        let items = self.items.clone();
        let channel_list_handle = self.channel_list_handle.clone();
        let active_clan_id_for_nav = self.active_clan_id;
        let list_state = self.list_state.clone();
        let sidebar = cx.entity().downgrade();
        let menu_overlay = self.open_menu.as_ref().map(|menu| {
            (
                menu.position,
                menu.channel_type,
                menu.is_thread,
                self.settings.read(cx).language.clone(),
            )
        });

        let list_element = list(list_state, {
            let sidebar = sidebar.clone();
            move |ix, _window, cx| {
                render_sidebar_item(
                    &items,
                    ix,
                    cx,
                    channel_list_handle.clone(),
                    active_clan_id_for_nav,
                    sidebar.clone(),
                )
            }
        })
        .size_full();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .pb(px(68.))
            .bg(theme.bg_secondary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .h(px(50.))
                    .px_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(self.active_clan_name.clone()),
                    ),
            )
            .child(div().flex_1().min_h_0().child(list_element))
            .when_some(
                menu_overlay,
                move |el, (position, channel_type, is_thread, locale)| {
                    el.child(context_menu_at(
                        position,
                        build_channel_menu(sidebar, &locale, channel_type, is_thread),
                    ))
                },
            )
    }
}

fn sk_bar(sk: gpui::Rgba, w: gpui::Pixels, h: gpui::Pixels) -> gpui::Div {
    div().w(w).h(h).rounded(px(4.)).bg(sk)
}

fn sk_channel_row(sk: gpui::Rgba, text_w: f32) -> gpui::Div {
    div()
        .h(px(34.))
        .w_full()
        .px_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(sk_bar(sk, px(18.), px(18.)))
        .child(sk_bar(sk, px(text_w), px(14.)))
}

fn sk_category(sk: gpui::Rgba, name_w: f32, widths: [f32; 4]) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .pt(px(10.))
                .pb(px(6.))
                .px_2()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(sk_bar(sk, px(18.), px(18.)))
                .child(sk_bar(sk, px(name_w), px(12.))),
        )
        .child(sk_channel_row(sk, widths[0]))
        .child(sk_channel_row(sk, widths[1]))
        .child(sk_channel_row(sk, widths[2]))
        .child(sk_channel_row(sk, widths[3]))
}

fn render_skeleton(cx: &App) -> AnyElement {
    let sk = cx.theme().bg_hover;
    div()
        .flex()
        .flex_col()
        .child(div().w_full().h(px(136.)).mb_2().rounded(px(4.)).bg(sk))
        .child(sk_category(sk, 70., [88., 64., 100., 76.]))
        .child(sk_category(sk, 92., [72., 96., 60., 84.]))
        .child(sk_category(sk, 60., [92., 68., 80., 64.]))
        .into_any_element()
}

fn nav_row(icon: IconName, label: &'static str, theme: &crate::theme::Theme) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px_2()
        .h(px(34.))
        .gap_2()
        .rounded(px(4.))
        .cursor_pointer()
        .text_color(theme.text_secondary)
        .child(
            Icon::new(icon)
                .size(px(20.))
                .text_color(theme.text_secondary),
        )
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label),
        )
}

const NUMBER_APPS_SHOW_OFF: usize = 4;
const MAX_CHANNEL_LABEL_CHARS: usize = 20;

fn truncate_channel_label(name: &str) -> String {
    if name.chars().count() > MAX_CHANNEL_LABEL_CHARS {
        let head: String = name.chars().take(MAX_CHANNEL_LABEL_CHARS).collect();
        format!("{head}...")
    } else {
        name.to_string()
    }
}

fn render_banner_and_events(
    banner_url: Option<&str>,
    app_channels: &[AppChannelSlot],
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let divider_color = theme.border;
    let hover_bg = theme.bg_hover;

    let mut col = div().flex().flex_col().w_full();

    if let Some(url) = banner_url {
        col = col.child(
            div().w_full().h(px(136.)).mb_2().overflow_hidden().child(
                gpui::img(crate::util::imgproxy::proxied(cx, url, 300, 300, "fit"))
                    .w_full()
                    .h_full()
                    .object_fit(gpui::ObjectFit::Cover),
            ),
        );
    }

    let nav_col = div()
        .flex()
        .flex_col()
        .w_full()
        .p_2()
        .gap_1()
        .child(nav_row(IconName::IconEvents, "Events", theme))
        .child(nav_row(IconName::MemberList, "Members", theme));

    col = col.child(nav_col);

    let hr = || div().w_full().h(px(1.)).ml(px(3.)).bg(divider_color);

    if !app_channels.is_empty() {
        let show_list: Vec<&AppChannelSlot> =
            app_channels.iter().take(NUMBER_APPS_SHOW_OFF).collect();
        let has_more = app_channels.len() > NUMBER_APPS_SHOW_OFF + 1;

        let app_row = if show_list.len() < NUMBER_APPS_SHOW_OFF {
            div().flex().flex_row().items_center().gap_2().py_1().px_2()
        } else {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_1()
                .px_2()
                .justify_center()
        };

        let mut app_row = app_row;
        for slot in &show_list {
            let icon_el: AnyElement = if let Some(logo) = &slot.app_logo {
                gpui::img(logo.clone())
                    .w(px(24.))
                    .h(px(24.))
                    .into_any_element()
            } else {
                gpui::svg()
                    .path("icons/channel-app-fallback.svg")
                    .w(px(24.))
                    .h(px(24.))
                    .text_color(theme.text_primary)
                    .into_any_element()
            };
            app_row = app_row.child(
                div()
                    .w(px(40.))
                    .h(px(40.))
                    .p_2()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(hover_bg))
                    .child(icon_el),
            );
        }
        if has_more {
            app_row = app_row.child(
                div()
                    .w(px(40.))
                    .h(px(40.))
                    .p_2()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(hover_bg))
                    .child(
                        Icon::new(IconName::RightIcon)
                            .size(px(24.))
                            .text_color(theme.text_primary),
                    ),
            );
        }

        col = col.child(hr()).child(app_row).child(hr());
    } else {
        col = col.child(hr());
    }

    col.into_any_element()
}

fn render_sidebar_item(
    items: &[SidebarItem],
    ix: usize,
    cx: &App,
    channel_list_handle: Entity<ChannelList>,
    active_clan_id_for_nav: Option<ClanId>,
    sidebar: WeakEntity<ChannelSidebar>,
) -> AnyElement {
    let theme = cx.theme();
    let Some(item) = items.get(ix) else {
        return div().into_any_element();
    };

    match item {
        SidebarItem::Skeleton => render_skeleton(cx),

        SidebarItem::BannerAndEvents {
            banner_url,
            app_channels,
        } => render_banner_and_events(banner_url.as_deref(), app_channels, cx),

        SidebarItem::Category {
            elem_id,
            id,
            name_upper,
            collapsed,
        } => {
            let category_id = id.clone();
            let category_name = name_upper.clone();
            let clan_id_for_toggle = active_clan_id_for_nav.unwrap_or_default();

            let mut header = div()
                .id(elem_id.clone())
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .px_2()
                .py_1()
                .cursor_pointer()
                .text_color(theme.text_muted)
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM);

            let icon = if *collapsed {
                IconName::CaretRight
            } else {
                IconName::CaretDown
            };
            header = header
                .child(Icon::new(icon).size(px(18.0)).text_color(theme.text_muted))
                .child(div().ml_1().child(category_name));

            header.interactivity().on_click(on_category_click(
                channel_list_handle.clone(),
                clan_id_for_toggle,
                category_id,
            ));

            div()
                .pt(px(10.))
                .pb(px(6.))
                .flex()
                .flex_col()
                .child(header)
                .into_any_element()
        }

        SidebarItem::Channel {
            elem_id,
            id,
            name,
            channel_type,
            unread,
            private,
            selected,
            badge_count,
            badge_label,
            muted,
            is_thread,
            line_above,
            line_below,
            voice_members,
        } => {
            let ch_id = id.clone();
            let row_handle = channel_list_handle.clone();
            let clan_id_inner = active_clan_id_for_nav;
            let selected_bg = theme.bg_primary;
            let brand = theme.brand;
            let text_primary = theme.text_primary;
            let text_color = if *muted {
                theme.text_muted
            } else if *selected {
                theme.text_primary
            } else {
                theme.text_secondary
            };

            let row_content = if *is_thread {
                let line_color = theme.text_muted;
                let line_above_val = *line_above;
                let line_below_val = *line_below;
                const ELBOW_TOP: gpui::Pixels = px(12.);
                const ELBOW_SIZE: gpui::Pixels = px(10.);

                let mut connector = div().relative().flex_none().w(px(20.)).h_full();
                if line_above_val || line_below_val {
                    connector = connector.child(
                        div()
                            .absolute()
                            .left(px(8.))
                            .top(px(0.))
                            .w(px(1.))
                            .when(line_below_val, |el| el.bottom(px(0.)))
                            .when(!line_below_val, |el| el.h(ELBOW_TOP))
                            .bg(line_color),
                    );
                }
                let connector = connector.child(
                    div()
                        .absolute()
                        .left(px(8.))
                        .top(ELBOW_TOP)
                        .w(ELBOW_SIZE)
                        .h(ELBOW_SIZE)
                        .border_l_1()
                        .border_b_1()
                        .border_color(line_color)
                        .rounded_bl_sm(),
                );

                div()
                    .h(px(34.))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .when(*selected, move |el| el.bg(selected_bg))
                    .child(div().flex_none().w(px(16.)))
                    .child(connector)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .pr_2()
                            .text_sm()
                            .text_color(text_color)
                            .when(*unread && !*muted, |el| {
                                el.font_weight(gpui::FontWeight::BOLD)
                            })
                            .child(div().flex_1().child(name.clone()))
                            .when(
                                crate::SHOW_UNREAD_BADGE_COUNT && *badge_count > 0 && !*muted,
                                move |el| {
                                    el.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .min_w(px(16.))
                                            .h(px(16.))
                                            .px_1()
                                            .rounded_full()
                                            .bg(brand)
                                            .text_color(text_primary)
                                            .text_xs()
                                            .child(badge_label.clone()),
                                    )
                                },
                            ),
                    )
                    .into_any_element()
            } else {
                ChannelRow::new(name.clone(), *channel_type)
                    .selected(*selected)
                    .unread(*unread)
                    .private(*private)
                    .badge_count(*badge_count)
                    .muted(*muted)
                    .render(theme)
                    .into_any_element()
            };

            let mut channel_col = div()
                .id(elem_id.clone())
                .w_full()
                .flex()
                .flex_col()
                .child(row_content);

            if !voice_members.is_empty() {
                let voice_pl = if *is_thread { px(40.) } else { px(32.) };
                let members_el =
                    div()
                        .flex()
                        .flex_col()
                        .pl(voice_pl)
                        .children(voice_members.iter().map(|m| {
                            let name_text = if m.display_name.is_empty() {
                                m.user_id.clone()
                            } else {
                                m.display_name.clone()
                            };
                            let avatar = if m.avatar_url.is_empty() {
                                Avatar::new().name(name_text.clone())
                            } else {
                                let proxied = crate::util::imgproxy::avatar_url(cx, &m.avatar_url);
                                Avatar::new()
                                    .src(SharedString::from(proxied))
                                    .name(name_text.clone())
                            };
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .px_2()
                                .py(px(2.))
                                .gap_1()
                                .child(avatar.with_size(Size::XSmall))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(name_text),
                                )
                        }));
                channel_col = channel_col.child(members_el);
            }

            let mut channel_col = channel_col.on_mouse_down(MouseButton::Right, {
                let channel_type = *channel_type;
                let is_thread = *is_thread;
                move |event: &MouseDownEvent, _window, cx| {
                    let position = event.position;
                    if let Some(view) = sidebar.upgrade() {
                        view.update(cx, |this, cx| {
                            this.open_menu = Some(OpenMenu {
                                channel_type,
                                is_thread,
                                position,
                            });
                            cx.notify();
                        });
                    }
                }
            });

            channel_col.interactivity().on_click(on_channel_click(
                row_handle,
                ch_id,
                clan_id_inner,
            ));

            channel_col.into_any_element()
        }
    }
}
