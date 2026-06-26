use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Entity, Rgba, SharedString, Window,
    div, img, prelude::*, px, rgb,
};
use mezon_store::ClanList;

use crate::router::{Route, Router};
use crate::theme::ActiveTheme;

#[derive(Clone)]
pub(super) struct ClanRow {
    pub(super) id: SharedString,
    pub(super) row_id: SharedString,
    pub(super) group_name: SharedString,
    pub(super) name: SharedString,
    pub(super) avatar_url: Option<SharedString>,
    pub(super) badge_count: u32,
    pub(super) has_unread: bool,
    pub(super) muted: bool,
}

fn on_clan_click(
    clan_list: Entity<ClanList>,
    clan_id: SharedString,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        let id = clan_id.to_string();
        clan_list.update(cx, |m, cx| {
            m.select_clan(id.parse().unwrap_or_default(), cx);
        });
        if !matches!(
            Router::global(cx).read(cx).route(),
            Route::Chat | Route::Channel { .. }
        ) {
            crate::router::navigate(cx, Route::Chat);
        }
    }
}

pub(super) fn render_pill(
    is_active: bool,
    group_name: SharedString,
    pill_color: Rgba,
) -> AnyElement {
    if !is_active {
        return div().into_any_element();
    }

    let anim_id = SharedString::from(format!("pill-{group_name}"));
    div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .flex()
        .items_center()
        .child(
            div()
                .w(px(4.))
                .rounded_r(px(4.))
                .bg(pill_color)
                .with_animation(
                    anim_id,
                    Animation::new(Duration::from_millis(200))
                        .with_easing(|t| 1.0 - (1.0 - t).powi(3)),
                    |el, delta| el.h(px(32. * delta)),
                ),
        )
        .into_any_element()
}

fn badge_text(count: u32) -> SharedString {
    if count >= 100 {
        SharedString::from("99+")
    } else {
        SharedString::from(count.to_string())
    }
}

pub(super) fn render_clan_row(
    rows: &[ClanRow],
    ix: usize,
    cx: &App,
    clan_list_handle: Entity<ClanList>,
) -> AnyElement {
    let theme = cx.theme();
    let dm_active = matches!(
        Router::global(cx).read(cx).route(),
        Route::Direct | Route::DirectMessage { .. }
    );
    let Some(clan) = rows.get(ix) else {
        return div().into_any_element();
    };

    let clan_id = clan.id.clone();
    let is_active = clan_list_handle
        .read(cx)
        .is_active_clan(clan.id.parse().unwrap_or_default())
        && !dm_active;
    let show_badge = crate::SHOW_UNREAD_BADGE_COUNT && clan.badge_count > 0 && !clan.muted;
    let show_nub = clan.has_unread && clan.badge_count == 0 && !clan.muted && !is_active;
    let badge_count = clan.badge_count;
    let muted = clan.muted;
    let pill_color = theme.tokens.text_theme_primary;

    let avatar: AnyElement = if let Some(ref url) = clan.avatar_url {
        let proxied = crate::util::imgproxy::proxied(cx, url, 100, 100, "fill");
        let mut el = img(SharedString::from(proxied))
            .size(px(40.))
            .rounded(px(8.))
            .overflow_hidden()
            .object_fit(gpui::ObjectFit::Cover);
        if muted {
            el = el.grayscale(true);
        }
        el.into_any_element()
    } else if !clan.name.is_empty() {
        let first = clan
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        div()
            .size(px(40.))
            .rounded(px(12.))
            .bg(theme.tokens.theme_base_color)
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.tokens.text_theme_primary)
            .text_size(px(20.))
            .hover(|s| {
                s.bg(theme.tokens.bg_button_add_friend)
                    .text_color(gpui::white())
            })
            .child(SharedString::from(first))
            .into_any_element()
    } else {
        div().size(px(40.)).into_any_element()
    };

    let avatar_with_badge = div().relative().child(avatar).when(show_badge, |el| {
        let text = badge_text(badge_count);
        let wide = badge_count >= 10;
        el.child(
            div()
                .absolute()
                .bottom(px(-1.))
                .right(px(-2.))
                .flex()
                .items_center()
                .justify_center()
                .h(px(16.))
                .when(wide, |s| s.w(px(22.)))
                .when(!wide, |s| s.w(px(16.)))
                .rounded_full()
                .bg(rgb(0xDA373C))
                .border_1()
                .border_color(gpui::white())
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(gpui::white())
                .child(text),
        )
    });

    div()
        .id(clan.row_id.clone())
        .group(clan.group_name.clone())
        .relative()
        .w_full()
        .h(px(48.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(render_pill(is_active, clan.group_name.clone(), pill_color))
        .when(show_nub, |el| {
            el.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .w(px(4.))
                            .h(px(8.))
                            .rounded_r(px(4.))
                            .bg(theme.tokens.bg_unread_message),
                    ),
            )
        })
        .on_click(on_clan_click(clan_list_handle, clan_id))
        .child(avatar_with_badge)
        .into_any_element()
}
