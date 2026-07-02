use gpui::{
    Anchor, AnyElement, App, KeyDownEvent, MouseButton, MouseDownEvent, SharedString, div,
    prelude::*, px,
};
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
use crate::components::primitives::{Icon, IconName, Input};

const GROUP_MARGIN_TOP: f32 = 10.;
const MESSAGE_ROW_MIN_HEIGHT: f32 = 30.;

fn message_pad_top(combined: bool, has_reply: bool, code: MessageCode) -> bool {
    !combined || (has_reply && !matches!(code, MessageCode::CreatePin))
}

pub fn render_user_message(
    msg: &Message,
    combined: bool,
    is_different_day: bool,
    ctx: &RowCtx,
    cx: &App,
) -> AnyElement {
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

    let editing_input = (ctx.editing_id == Some(msg.id))
        .then(|| ctx.edit_input.clone())
        .flatten();
    body_column = body_column.child(match editing_input {
        Some(input) => render_edit_box(msg.id, input, ctx),
        None => render_message_content(msg, ctx),
    });

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
            show_head.then(|| build_avatar_element(msg, ctx, cx)),
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
    let context_menu_id = msg.id;
    let context_menu_host = ctx.video_host.clone();
    let group_name = SharedString::from(format!("msg-{row_key}"));
    div()
        .id(("msg-row", row_key as usize))
        .when(!ctx.suppress_hover, |d| d.group(group_name.clone()))
        .on_mouse_down(
            MouseButton::Right,
            move |event: &MouseDownEvent, _window, cx| {
                let position = event.position;
                let _ = context_menu_host.update(cx, |this, cx| {
                    this.open_context_menu(context_menu_id, position, cx);
                });
            },
        )
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
        .child(render_hover_actions(
            msg,
            combined,
            has_reply,
            is_different_day,
            group_name,
            ctx,
        ))
        .into_any_element()
}

fn render_edit_box(
    message_id: mezon_store::MessageId,
    input: gpui::Entity<crate::components::primitives::InputState>,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let save_host = ctx.video_host.clone();
    let cancel_host = ctx.video_host.clone();
    let escape_host = ctx.video_host.clone();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .on_key_down(move |event: &KeyDownEvent, _window, cx| {
            if event.keystroke.key == "escape" {
                let _ = escape_host.update(cx, |this, cx| this.cancel_edit(cx));
            }
        })
        .child(Input::new(&input))
        .child(
            div()
                .flex()
                .flex_row()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .id(("edit-save", message_id.0 as usize))
                        .p_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover))
                        .child(
                            Icon::new(IconName::CheckIcon)
                                .size_4()
                                .text_color(theme.brand),
                        )
                        .on_click(move |_, window, cx| {
                            let _ = save_host.update(cx, |this, cx| this.save_edit(window, cx));
                        }),
                )
                .child(
                    div()
                        .id(("edit-cancel", message_id.0 as usize))
                        .p_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover))
                        .child(
                            Icon::new(IconName::CloseIcon)
                                .size_4()
                                .text_color(theme.text_muted),
                        )
                        .on_click(move |_, _, cx| {
                            let _ = cancel_host.update(cx, |this, cx| this.cancel_edit(cx));
                        }),
                ),
        )
        .into_any_element()
}

fn build_avatar_element(msg: &Message, ctx: &RowCtx, cx: &App) -> AnyElement {
    let plain = avatar_element(msg, ctx, cx);
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
