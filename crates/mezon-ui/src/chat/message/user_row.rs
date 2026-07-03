use gpui::{
    Anchor, AnyElement, App, KeyDownEvent, MouseButton, MouseDownEvent, div, prelude::*, px,
};
use mezon_store::{Message, MessageCode};

use super::content::render_message_content;
use super::context::{
    AVATAR_LEFT, AVATAR_SIZE, CONTENT_INSET, CONTENT_RIGHT_PAD, DEFAULT_DISPLAY_NAME_COLOR, RowCtx,
};
use super::ogp_embed::render_ogp_embed;
use super::parts::{
    avatar_element, render_attachments, render_head, render_hover_actions, render_reactions,
    render_reply,
};
use super::poll_card::render_poll_card;
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

    if show_head {
        body_column = body_column.child(render_head(msg, ctx, DEFAULT_DISPLAY_NAME_COLOR));
    }

    if msg.show_forwarded_label {
        body_column = body_column.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .w_full()
                .italic()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.tokens.text_theme_primary)
                .opacity(0.6)
                .child(
                    Icon::new(IconName::ForwardRightClick)
                        .size_4()
                        .text_color(theme.tokens.text_theme_primary),
                )
                .child(mezon_i18n::t(ctx.locale, "chat.forwarded")),
        );
    }

    let editing_input = (ctx.editing_id == Some(msg.id))
        .then(|| ctx.edit_input.clone())
        .flatten();
    body_column = body_column.child(match editing_input {
        Some(input) => render_edit_box(msg.id, input, ctx),
        None if msg.poll.is_some() => render_poll_card(msg, ctx),
        None => render_message_content(msg, ctx),
    });

    if let Some(ogp) = render_ogp_embed(msg, ctx) {
        body_column = body_column.child(ogp);
    }

    if let Some(attachments) = render_attachments(msg, ctx) {
        body_column = body_column.child(attachments);
    }
    if let Some(reactions) = render_reactions(msg, ctx) {
        body_column = body_column.child(reactions);
    }

    let body = div()
        .relative()
        .w_full()
        .when(msg.send_failed, |d| d.opacity(0.5))
        .when(msg.is_forwarded, |d| {
            d.child(
                div()
                    .absolute()
                    .left(px(58.))
                    .bottom_0()
                    .when(show_head, |b| b.top(px(50.)))
                    .when(!show_head, |b| b.top_0())
                    .w(px(4.))
                    .rounded(px(4.))
                    .bg(theme.tokens.text_theme_primary),
            )
        })
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
    let hover_host = ctx.video_host.clone();
    let hover_id = msg.id;
    div()
        .id(("msg-row", row_key as usize))
        .on_mouse_down(
            MouseButton::Right,
            move |event: &MouseDownEvent, _window, cx| {
                let position = event.position;
                let _ = context_menu_host.update(cx, |this, cx| {
                    this.open_context_menu(context_menu_id, position, cx);
                });
            },
        )
        .when(!ctx.suppress_hover, |d| {
            d.on_hover(move |hovered: &bool, _window, cx| {
                let entered = *hovered;
                let _ = hover_host.update(cx, |this, cx| this.set_row_hover(hover_id, entered, cx));
            })
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
        .child(render_hover_actions(
            msg,
            combined,
            has_reply,
            is_different_day,
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
        .child(Input::new(&input).text_size(px(16.)))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .text_xs()
                .text_color(theme.tokens.text_theme_primary)
                .child(div().pr(px(3.)).child("escape to"))
                .child(
                    div()
                        .id(("edit-cancel", message_id.0 as usize))
                        .pr(px(3.))
                        .cursor_pointer()
                        .text_color(gpui::rgb(0x3297ff))
                        .child("cancel")
                        .on_click(move |_, _, cx| {
                            let _ = cancel_host.update(cx, |this, cx| this.cancel_edit(cx));
                        }),
                )
                .child(div().pr(px(3.)).child("• enter to"))
                .child(
                    div()
                        .id(("edit-save", message_id.0 as usize))
                        .cursor_pointer()
                        .text_color(gpui::rgb(0x3297ff))
                        .child("save")
                        .on_click(move |_, window, cx| {
                            let _ = save_host.update(cx, |this, cx| this.save_edit(window, cx));
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
        ctx.avatar_cache.clone(),
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
