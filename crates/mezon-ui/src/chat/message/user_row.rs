use gpui::{Anchor, AnyElement, SharedString, div, prelude::*, px};
use mezon_store::{Message, MessageCode};

use super::content::render_message_content;
use super::context::{
    AVATAR_LEFT, AVATAR_SIZE, CONTENT_INSET, CONTENT_RIGHT_PAD, DEFAULT_DISPLAY_NAME_COLOR, RowCtx,
};
use super::parts::{
    avatar_element, render_attachments, render_head, render_hover_actions, render_reactions,
    render_reply,
};
use crate::chat::user_profile_popover::{ClickableContainer, profile_popover_menu};
use crate::components::primitives::{Icon, IconName};

const GROUP_MARGIN_TOP: f32 = 10.;
const MESSAGE_ROW_MIN_HEIGHT: f32 = 30.;

fn message_pad_top(combined: bool, has_reply: bool, code: MessageCode) -> bool {
    !combined || (has_reply && !matches!(code, MessageCode::CreatePin))
}

pub fn render_user_message(msg: &Message, combined: bool, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let has_reply = !msg.references.is_empty();
    let show_head = mezon_store::should_show_message_head(msg, combined);
    let row_key = msg.row_anchor_id.0;

    let mut body_column = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .mb_1()
        .pl(px(CONTENT_INSET))
        .pr(px(CONTENT_RIGHT_PAD));

    if msg.is_forwarded {
        body_column = body_column.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .mb_0p5()
                .text_xs()
                .text_color(theme.text_muted)
                .child(
                    Icon::new(IconName::ReplyCorner)
                        .size_4()
                        .text_color(theme.text_muted),
                )
                .child(mezon_i18n::t(ctx.locale, "chat.forwarded")),
        );
    }

    if show_head {
        body_column = body_column.child(render_head(msg, ctx, DEFAULT_DISPLAY_NAME_COLOR));
    }

    body_column = body_column.child(render_message_content(msg, ctx));

    if let Some(attachments) = render_attachments(msg, ctx) {
        body_column = body_column.child(attachments);
    }
    if let Some(reactions) = render_reactions(msg, ctx) {
        body_column = body_column.child(reactions);
    }

    let body = div()
        .relative()
        .w_full()
        .when_some(
            show_head.then(|| build_avatar_element(msg, ctx)),
            |d, avatar_element| {
                d.child(
                    div()
                        .absolute()
                        .left(px(AVATAR_LEFT))
                        .top(px(2.))
                        .w(px(AVATAR_SIZE))
                        .h(px(AVATAR_SIZE))
                        .child(avatar_element),
                )
            },
        )
        .child(body_column);

    let hover_bg = theme.bg_hover;
    let highlighted = ctx.highlight_id.is_some_and(|id| id == msg.id);
    let mentioned = msg.highlights_viewer_direct
        || mezon_store::message_row_highlight_roles(msg, ctx.current_role_ids);
    let mention_bg = theme.tokens.bg_highlight;
    let mention_border = theme.tokens.border_left_highlight;
    div()
        .id(("msg-row", row_key as usize))
        .when(!ctx.suppress_hover, |d| {
            d.group(SharedString::from(format!("msg-{row_key}")))
        })
        .relative()
        .w_full()
        .min_h(px(MESSAGE_ROW_MIN_HEIGHT))
        .when(!combined, |d| d.mt(px(GROUP_MARGIN_TOP)))
        .when(message_pad_top(combined, has_reply, msg.code), |d| d.pt_3())
        .when(highlighted, |d| {
            d.bg(gpui::Rgba {
                a: 0.16,
                ..theme.brand
            })
        })
        .when(mentioned && !highlighted, |d| {
            d.bg(mention_bg).border_l_3().border_color(mention_border)
        })
        .when(!ctx.suppress_hover, |d| d.hover(move |s| s.bg(hover_bg)))
        .when(has_reply, |d| {
            d.child(render_reply(&msg.references[0], ctx))
        })
        .child(body)
        .child(render_hover_actions(msg, theme, ctx.suppress_hover))
        .into_any_element()
}

fn build_avatar_element(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let plain = avatar_element(msg, ctx);
    let Some(profile_ctx) = ctx.profile_context else {
        return plain;
    };
    let Some(user_id) = msg.sender_user_id else {
        return plain;
    };
    let settings = ctx.settings.clone();
    let avatar_key = user_id.get() as usize;
    profile_popover_menu(
        ("msg-avatar-popover", avatar_key),
        user_id,
        profile_ctx,
        settings,
    )
    .anchor(Anchor::TopLeft)
    .attach(Anchor::TopRight)
    .trigger(
        ClickableContainer::new(("msg-avatar-trigger", avatar_key))
            .size_full()
            .cursor_pointer()
            .child(plain),
    )
    .into_any_element()
}
