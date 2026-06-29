use gpui::{AnyElement, FontWeight, SharedString, div, img, prelude::*, px};
use mezon_store::{Message, MessageId, MessageReference, MessagesStore, Reaction, ReplyDraft};

use super::context::{REPLY_USERNAME_COLOR, RowCtx};
use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size};
use crate::theme::Theme;

/// Build the 40px message avatar (proxied src with raw fallback), cf. React
/// `MessageAvatar`.
pub fn avatar_element(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let mut avatar = Avatar::new()
        .name(msg.sender_name.clone())
        .with_size(Size::Small)
        .image_cache(ctx.avatar_cache.clone());
    let proxied = msg.avatar_proxied.clone();
    if !proxied.is_empty() {
        avatar = avatar.src(proxied.clone());
        if !msg.avatar_url.is_empty() && msg.avatar_url != proxied.as_ref() {
            avatar = avatar.fallback_src(msg.avatar_url.clone());
        }
    } else if !msg.avatar_url.is_empty() {
        avatar = avatar.src(msg.avatar_url.clone());
    }
    avatar.into_any_element()
}

/// Username + timestamp header (React `MessageHead`).
pub fn render_head(msg: &Message, theme: &Theme, name_color: u32) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_baseline()
        .gap_2()
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::rgb(name_color))
                .child(msg.sender_name.clone()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child(msg.timestamp_label.clone()),
        )
        .into_any_element()
}

/// Reply quote block shown above a message (React `MessageReply`).
pub fn render_reply(reference: &MessageReference, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let preview = if reference.content.is_empty() {
        if reference.has_attachment {
            mezon_i18n::t(ctx.locale, "chat.clickToSeeAttachment").to_string()
        } else {
            String::new()
        }
    } else {
        reference.content.clone()
    };

    let avatar = if reference.sender_avatar.is_empty() {
        Avatar::new()
            .name(reference.sender_name.clone())
            .size_px(px(20.))
            .image_cache(ctx.avatar_cache.clone())
    } else {
        Avatar::new()
            .name(reference.sender_name.clone())
            .src(reference.sender_avatar.clone())
            .size_px(px(20.))
            .image_cache(ctx.avatar_cache.clone())
    };

    let jump_target = reference.message_ref_id;
    div()
        .id(SharedString::from(format!(
            "reply-{}",
            reference.message_ref_id
        )))
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(24.))
        .pl(px(super::context::REPLY_INSET))
        .pr(px(super::context::CONTENT_RIGHT_PAD))
        .text_size(px(14.))
        .cursor_pointer()
        .when(!jump_target.is_zero(), |d| {
            d.on_click(move |_, _, cx| {
                MessagesStore::global(cx)
                    .update(cx, |store, cx| store.jump_to_message(jump_target, cx));
            })
        })
        .child(
            Icon::new(IconName::ReplyCorner)
                .size_4()
                .text_color(theme.text_muted),
        )
        .child(avatar)
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::rgb(REPLY_USERNAME_COLOR))
                .child(reference.sender_name.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_color(theme.text_muted)
                .child(preview),
        )
        .into_any_element()
}

/// Render image/file attachments below the message body (React
/// `MessageAttachment`, simplified to image + generic file for P0).
pub fn render_attachments(msg: &Message, theme: &Theme) -> Option<AnyElement> {
    if msg.attachments.is_empty() {
        return None;
    }
    let mut col = div().flex().flex_col().gap_2().mt_1().w_full();
    for (i, att) in msg.attachments.iter().enumerate() {
        if att.is_image() {
            let src = att.proxied_src.clone();
            if src.is_empty() {
                col = col.child(attachment_box(att.filename.clone(), theme));
            } else {
                col = col.child(
                    img(src)
                        .id(SharedString::from(format!("msg-img-{}-{}", msg.id.0, i)))
                        .w(px(att.display_width))
                        .h(px(att.display_height))
                        .max_w(px(400.))
                        .rounded_md(),
                );
            }
        } else {
            let label = if att.filename.is_empty() {
                "Attachment".to_string()
            } else {
                att.filename.clone()
            };
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.bg_tertiary)
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child(
                        Icon::new(IconName::FileIcon)
                            .size_4()
                            .text_color(theme.text_secondary),
                    )
                    .child(label),
            );
        }
    }
    Some(col.into_any_element())
}

fn attachment_box(label: String, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(240.))
        .h(px(120.))
        .rounded_md()
        .bg(theme.bg_tertiary)
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.text_muted)
        .child(if label.is_empty() {
            "image".to_string()
        } else {
            label
        })
        .into_any_element()
}

/// Reaction pills row (React `MessageReaction`), display-only for now.
pub fn render_reactions(msg: &Message, ctx: &RowCtx) -> Option<AnyElement> {
    if msg.reactions.is_empty() {
        return None;
    }
    let theme = ctx.theme;
    let mut row = div().flex().flex_row().flex_wrap().gap_2().mt_1().w_full();
    for (i, reaction) in msg.reactions.iter().enumerate() {
        row = row.child(reaction_pill(
            msg.id,
            i,
            reaction,
            ctx.current_user_id,
            theme,
        ));
    }
    Some(row.into_any_element())
}

fn reaction_pill(
    msg_id: MessageId,
    index: usize,
    reaction: &Reaction,
    current_user_id: &str,
    theme: &Theme,
) -> AnyElement {
    let reacted =
        !current_user_id.is_empty() && reaction.sender_ids.iter().any(|id| id == current_user_id);
    let label = if reaction.emoji.is_empty() {
        format!("{}", reaction.count)
    } else {
        format!("{} {}", reaction.emoji, reaction.count)
    };
    let mut pill = div()
        .id(SharedString::from(format!(
            "reaction-{}-{}",
            msg_id.0, index
        )))
        .flex()
        .flex_row()
        .items_center()
        .h(px(24.))
        .px_2()
        .rounded_md()
        .text_sm()
        .text_color(theme.text_secondary);
    if reacted {
        pill = pill
            .bg(gpui::Rgba {
                a: 0.18,
                ..theme.brand
            })
            .border_1()
            .border_color(theme.brand);
    } else {
        pill = pill.bg(theme.bg_tertiary);
    }
    pill.child(label).into_any_element()
}

/// Floating hover action bar (React `ChannelMessageOpt`), revealed on row hover.
/// The Reply action sets the composer reply target; the rest are visual for now.
/// While the list is scrolling (`suppress_hover`) the bar is not rendered, so it
/// cannot flash in under the cursor (cf. React `toggleDisableHover`).
pub fn render_hover_actions(msg: &Message, theme: &Theme, suppress_hover: bool) -> AnyElement {
    if suppress_hover {
        return div().into_any_element();
    }
    let group_name = SharedString::from(format!("msg-{}", msg.id.0));
    let bg_hover = theme.bg_hover;
    let action = move |id: &str, icon: IconName| {
        div()
            .id(SharedString::from(id.to_string()))
            .p_1()
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .child(Icon::new(icon).size_4().text_color(theme.text_secondary))
    };

    let reply_draft = ReplyDraft {
        message_ref_id: msg.id,
        sender_id: msg.sender_user_id.unwrap_or_default(),
        sender_name: msg.sender_name.clone(),
        sender_avatar: msg.avatar_url.clone(),
        content_preview: msg.content.clone(),
        has_attachment: !msg.attachments.is_empty(),
    };

    div()
        .absolute()
        .right(px(24.))
        .top(px(-16.))
        .flex()
        .flex_row()
        .items_center()
        .gap_0p5()
        .p_0p5()
        .rounded_lg()
        .bg(theme.bg_floating)
        .border_1()
        .border_color(theme.border)
        .opacity(0.)
        .group_hover(group_name, |s| s.opacity(1.))
        .child(action("react", IconName::Smile))
        .child(
            action("reply", IconName::ReplyCorner).on_click(move |_, _, cx| {
                let draft = reply_draft.clone();
                MessagesStore::global(cx).update(cx, |store, cx| store.set_reply(draft, cx));
            }),
        )
        .child(action("edit", IconName::PenEdit))
        .child(action("delete", IconName::TrashIcon))
        .into_any_element()
}

/// Date separator row (React `MessageDateDivider`).
pub fn render_date_divider(theme: &Theme, label: &str) -> AnyElement {
    div()
        .id(SharedString::from(format!("date-sep-{}", label)))
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .w_full()
        .child(div().flex_1().h(px(1.)).bg(theme.border))
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(label.to_string()),
        )
        .child(div().flex_1().h(px(1.)).bg(theme.border))
        .into_any_element()
}
