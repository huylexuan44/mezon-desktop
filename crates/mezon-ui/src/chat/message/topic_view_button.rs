use chrono::Timelike;
use gpui::{AnyElement, ObjectFit, SharedString, div, img, prelude::*, px};
use mezon_store::{ClanMembersStore, Message, TopicsStore, message_time::local_datetime};

use super::coming_soon_toast;
use super::context::RowCtx;
use super::time::format_message_time;
use crate::components::primitives::{Icon, IconName};

const AVATAR_SIZE: f32 = 28.0;
const AVATAR_ROUNDING: f32 = 6.0;
const MIN_WIDTH: f32 = 250.0;

pub fn render_topic_view_button(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;

    let creator = msg
        .topic_creator_id
        .zip(ctx.clan_id)
        .and_then(|(user_id, clan_id)| {
            let store = ClanMembersStore::try_global(ctx.app)?;
            let store = store.read(ctx.app);
            store
                .member(clan_id, user_id)
                .map(|member| (member.name().to_string(), member.avatar().to_string()))
        });
    let (creator_name, creator_avatar) = creator.unwrap_or_default();

    let time_label = msg
        .topic_id
        .and_then(|topic_id| {
            let store = TopicsStore::try_global(ctx.app)?;
            let store = store.read(ctx.app);
            store
                .topic_by_id(&topic_id.to_string())
                .map(|topic| topic.last_message_timestamp as i64)
        })
        .and_then(local_datetime)
        .map(|dt| {
            let hhmm: SharedString = format!("{:02}:{:02}", dt.hour(), dt.minute()).into();
            format_message_time(&hhmm, Some(dt.date_naive()), ctx.locale, ctx.now)
        });

    let meta = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x_2()
        .flex_1()
        .min_w_0()
        .when_some(time_label, |d, label| {
            d.child(
                div()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(label),
            )
        });

    let left = div()
        .flex()
        .items_center()
        .gap_2()
        .flex_1()
        .min_w_0()
        .text_size(px(14.))
        .child(creator_avatar_element(&creator_name, &creator_avatar, ctx))
        .child(meta);

    let locale = SharedString::from(ctx.locale);
    let button = div()
        .id(("topic-view-button", msg.id.0 as usize))
        .flex()
        .items_center()
        .justify_between()
        .gap_1()
        .min_w(px(MIN_WIDTH))
        .my_1()
        .p_1()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.tokens.border_theme_primary)
        .bg(theme.tokens.bg_item_theme_hover)
        .text_color(theme.tokens.text_theme_primary)
        .cursor_pointer()
        .on_click(move |_, _, cx| coming_soon_toast(&locale, cx))
        .child(left)
        .child(
            Icon::new(IconName::ArrowRight)
                .size(px(16.))
                .text_color(theme.tokens.text_theme_primary),
        );

    div().flex().min_w_0().child(button).into_any_element()
}

fn creator_avatar_element(name: &str, avatar_url: &str, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let size = px(AVATAR_SIZE);
    let base = div()
        .size(size)
        .flex_shrink_0()
        .rounded(px(AVATAR_ROUNDING))
        .overflow_hidden();

    if avatar_url.is_empty() {
        let initial = name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        base.flex()
            .items_center()
            .justify_center()
            .bg(theme.tokens.bg_tertiary)
            .text_color(theme.tokens.text_theme_primary)
            .text_size(px(12.))
            .child(initial)
            .into_any_element()
    } else {
        let proxied = crate::util::imgproxy::avatar_url(ctx.app, avatar_url);
        base.image_cache(ctx.avatar_cache.clone())
            .child(
                img(SharedString::from(proxied))
                    .size(size)
                    .rounded(px(AVATAR_ROUNDING))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element()
    }
}
