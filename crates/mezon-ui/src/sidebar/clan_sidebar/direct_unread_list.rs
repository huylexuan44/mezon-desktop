use gpui::{
    AnyElement, App, ClickEvent, SharedString, Window, div, img, prelude::*, px, rgb,
};
use mezon_store::{ChannelId, DirectKind, DirectMessageStore};

use crate::components::primitives::Avatar;
use crate::router::{Route, navigate};
use crate::util::assets::AVATAR_GROUP;

#[derive(Clone)]
pub(super) struct DirectUnreadItem {
    pub channel_id: ChannelId,
    pub label: SharedString,
    pub kind: DirectKind,
    pub unread_count: u32,
    pub avatar_src: SharedString,
    pub avatar_raw: SharedString,
}

pub(super) fn build_direct_unread_items(store: &DirectMessageStore, cx: &App) -> Vec<DirectUnreadItem> {
    store
        .channels()
        .iter()
        .filter(|ch| ch.unread_count > 0)
        .map(|ch| DirectUnreadItem {
            channel_id: ch.id,
            label: SharedString::from(ch.label.clone()),
            kind: ch.kind,
            unread_count: ch.unread_count,
            avatar_src: SharedString::from(crate::util::imgproxy::avatar_url(cx, &ch.avatar)),
            avatar_raw: SharedString::from(ch.avatar.clone()),
        })
        .collect()
}

fn badge_text(count: u32) -> SharedString {
    if count >= 100 {
        SharedString::from("99+")
    } else {
        SharedString::from(count.to_string())
    }
}

fn on_direct_unread_click(
    channel_id: ChannelId,
    channel_type: i32,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        navigate(
            cx,
            Route::DirectMessage {
                direct_id: channel_id,
                message_type: channel_type.to_string(),
            },
        );
    }
}

pub(super) fn render_direct_unread_list(items: &[DirectUnreadItem]) -> impl IntoElement {
    div()
        .id("direct-unread-list")
        .max_h(px(256.))
        .overflow_y_scroll()
        .p(px(2.))
        .flex()
        .flex_col()
        .gap_1()
        .children(items.iter().map(render_direct_unread_item))
}

fn render_direct_unread_item(item: &DirectUnreadItem) -> AnyElement {
    let channel_id = item.channel_id;
    let channel_type = item.kind.channel_type();
    let unread_count = item.unread_count;
    let badge = badge_text(unread_count);
    let wide = unread_count >= 10;

    let avatar_size = px(40.);
    let avatar = if item.kind == DirectKind::Group && item.avatar_src.is_empty() {
        img(AVATAR_GROUP)
            .size(avatar_size)
            .rounded(px(8.))
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element()
    } else {
        let mut avatar = Avatar::new()
            .name(item.label.clone())
            .size_px(avatar_size);
        if !item.avatar_src.is_empty() {
            avatar = avatar.src(item.avatar_src.to_string());
            if !item.avatar_raw.is_empty() && item.avatar_raw != item.avatar_src {
                avatar = avatar.fallback_src(item.avatar_raw.to_string());
            }
        } else if !item.avatar_raw.is_empty() {
            avatar = avatar.src(item.avatar_raw.to_string());
        }
        div()
            .size(avatar_size)
            .rounded(px(8.))
            .overflow_hidden()
            .child(avatar)
            .into_any_element()
    };

    div()
        .id(SharedString::from(format!("direct-unread-{}", channel_id)))
        .flex()
        .items_end()
        .cursor_pointer()
        .child(
            div()
                .relative()
                .child(avatar)
                .child(
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
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(gpui::white())
                        .child(badge),
                ),
        )
        .on_click(on_direct_unread_click(channel_id, channel_type))
        .into_any_element()
}
