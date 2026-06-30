use gpui::{AnyElement, FontWeight, SharedString, div, prelude::*, px};
use mezon_store::{Message, MessageCode};

use super::content::render_message_content;
use super::context::{CONTENT_INSET, CONTENT_RIGHT_PAD, RowCtx};
use super::time::format_message_time;
use crate::components::primitives::{Icon, IconName};

/// Render a system message row (React `MessageWithSystem`): an icon plus the
/// server-provided text. Covers Welcome/join-leave, CreatePin, CreateThread,
/// AuditLog and UpcomingEvent.
pub fn render_system_message(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let icon = match msg.code {
        MessageCode::CreatePin => IconName::PinRight,
        MessageCode::CreateThread => IconName::ThreadIcon,
        MessageCode::AuditLog => IconName::AuditLogIcon,
        MessageCode::UpcomingEvent => IconName::UpcomingEventIcon,
        _ => IconName::WelcomeIcon,
    };

    div()
        .id(SharedString::from(format!("msg-sys-{}", msg.id.0)))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .pl(px(CONTENT_INSET - 40.))
        .pr(px(CONTENT_RIGHT_PAD))
        .py_1()
        .when(!ctx.suppress_hover, |d| {
            let bg = theme.bg_hover;
            d.hover(move |s| s.bg(bg))
        })
        .child(
            div()
                .w(px(24.))
                .flex_none()
                .flex()
                .justify_center()
                .child(Icon::new(icon).size_4().text_color(theme.text_muted)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme.text_muted)
                .child(render_message_content(msg, ctx)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(format_message_time(msg.create_time, ctx.locale)),
        )
        .into_any_element()
}

/// Channel-start welcome header (React `ChatWelcome`, shown for the `Indicator`
/// sentinel message). Minimal P0 version.
pub fn render_welcome(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    div()
        .id(SharedString::from(format!("msg-welcome-{}", msg.id.0)))
        .flex()
        .flex_col()
        .gap_1()
        .px_4()
        .pt_4()
        .pb_2()
        .w_full()
        .child(
            div()
                .text_size(px(28.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(mezon_i18n::t(ctx.locale, "chat.welcomeTitle")),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_muted)
                .child(mezon_i18n::t(ctx.locale, "chat.welcomeSubtitle")),
        )
        .into_any_element()
}

/// "New messages" unread boundary (React `UnreadMessageBreak`).
pub fn render_unread_break(theme: &crate::theme::Theme, locale: &str) -> AnyElement {
    div()
        .id("unread-break")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .py_0p5()
        .w_full()
        .child(div().flex_1().h(px(1.)).bg(theme.mention_badge))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.mention_badge)
                .child(mezon_i18n::t(locale, "chat.newMessages")),
        )
        .into_any_element()
}
