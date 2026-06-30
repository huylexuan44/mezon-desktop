use std::sync::Arc;

use gpui::{AnyElement, Entity, FontWeight, ObjectFit, SharedString, div, img, prelude::*, px};
use mezon_store::{
    AlbumLayout, Message, MessageAttachment, MessageId, MessageReference, MessagesStore, Reaction,
    ReplyDraft, ViewerMedia,
};

// image-viewer: disabled, reimplement later
// use super::image_viewer::ImageViewer;

use super::context::{REPLY_USERNAME_COLOR, RowCtx};
use super::gif_video::GifVideoView;
use super::time::format_message_time;
use super::video_player::VideoActivation;
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
pub fn render_head(msg: &Message, ctx: &RowCtx, name_color: u32) -> AnyElement {
    let theme = ctx.theme;
    let time_label = format_message_time(msg.create_time, ctx.locale);
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
                .child(time_label),
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
pub fn render_attachments(msg: &Message, ctx: &RowCtx) -> Option<AnyElement> {
    if msg.attachments.is_empty() {
        return None;
    }
    let theme = ctx.theme;
    let mut videos = Vec::new();
    let mut images: Vec<(usize, &MessageAttachment)> = Vec::new();
    let mut documents = Vec::new();
    for (idx, att) in msg.attachments.iter().enumerate() {
        if att.is_unsupported_media() {
            documents.push(att);
        } else if att.is_video() {
            videos.push(att);
        } else if att.is_image() {
            images.push((idx, att));
        } else {
            documents.push(att);
        }
    }

    let uploader = Uploader {
        name: SharedString::from(msg.sender_name.clone()),
        avatar: if msg.avatar_proxied.is_empty() {
            SharedString::from(msg.avatar_url.clone())
        } else {
            msg.avatar_proxied.clone()
        },
    };

    let mut col = div().flex().flex_col().gap_2().mt_1().w_full();
    for (i, att) in videos.iter().enumerate() {
        col = col.child(render_video(msg.id, i, att, ctx));
    }
    if images.len() >= 2
        && let Some(layout) = msg.album_layout.as_ref()
    {
        col = col.child(render_album(
            msg.id,
            &images,
            layout,
            &msg.viewer_media,
            theme,
            &uploader,
        ));
    } else if let Some(&(att_index, att)) = images.first() {
        let gif_player = att
            .tenor_mp4
            .as_ref()
            .and_then(|_| ctx.gif_videos.get(&(msg.id, att_index)).cloned());
        col = col.child(render_photo(
            msg.id,
            0,
            att,
            theme,
            &msg.viewer_media,
            &uploader,
            gif_player,
        ));
    }
    for att in &documents {
        col = col.child(render_file_box(att, theme));
    }
    Some(col.into_any_element())
}

// image-viewer: fields read only by the disabled viewer; reimplement later
#[allow(dead_code)]
struct Uploader {
    name: SharedString,
    avatar: SharedString,
}

fn render_album(
    msg_id: MessageId,
    images: &[(usize, &MessageAttachment)],
    layout: &AlbumLayout,
    _gallery: &Arc<[ViewerMedia]>,
    theme: &Theme,
    _uploader: &Uploader,
) -> AnyElement {
    let mut container = div()
        .relative()
        .w(px(layout.container_width))
        .h(px(layout.container_height))
        .max_w(px(464.))
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_tertiary);
    for (index, (tile, image)) in layout.tiles.iter().zip(images.iter()).enumerate() {
        let att = image.1;
        let src = att.proxied_src.clone();
        // image-viewer: disabled, reimplement later
        // let gallery = gallery.clone();
        // let uploader_name = uploader.name.clone();
        // let uploader_avatar = uploader.avatar.clone();
        let tile_element = div()
            .id(SharedString::from(format!(
                "msg-album-{}-{}",
                msg_id.0, index
            )))
            .absolute()
            .left(px(tile.x))
            .top(px(tile.y))
            .w(px(tile.width))
            .h(px(tile.height))
            .bg(theme.bg_tertiary)
            // image-viewer: click-to-open disabled, reimplement later
            // .cursor_pointer()
            .when(!src.is_empty(), |d| {
                d.child(img(src).size_full().object_fit(ObjectFit::Cover))
            });
        // image-viewer: disabled, reimplement later
        // .on_click(move |_, window, cx| {
        //     ImageViewer::open(
        //         gallery.clone(),
        //         index,
        //         uploader_name.clone(),
        //         uploader_avatar.clone(),
        //         window,
        //         cx,
        //     );
        // });
        container = container.child(tile_element);
    }
    container.into_any_element()
}

fn render_photo(
    msg_id: MessageId,
    index: usize,
    att: &MessageAttachment,
    theme: &Theme,
    _gallery: &Arc<[ViewerMedia]>,
    _uploader: &Uploader,
    gif_player: Option<Entity<GifVideoView>>,
) -> AnyElement {
    let src = att.proxied_src.clone();
    if src.is_empty() {
        return attachment_box(att.filename.clone(), theme);
    }
    if let Some(player) = gif_player {
        return div()
            .id(SharedString::from(format!("msg-gif-{}-{}", msg_id.0, index)))
            .w(px(att.display_width))
            .h(px(att.display_height))
            .max_w_full()
            .child(player)
            .into_any_element();
    }
    let object_fit = if is_gif(&att.url) {
        ObjectFit::Contain
    } else {
        ObjectFit::Cover
    };
    let fallback_bg = theme.bg_tertiary;
    let fallback_fg = theme.text_muted;
    // image-viewer: disabled, reimplement later
    // let is_sticker = att.filetype == "sticker";
    // let gallery = gallery.clone();
    // let uploader_name = uploader.name.clone();
    // let uploader_avatar = uploader.avatar.clone();
    div()
        .id(SharedString::from(format!(
            "msg-img-{}-{}",
            msg_id.0, index
        )))
        .w(px(att.display_width))
        .h(px(att.display_height))
        .rounded_md()
        .overflow_hidden()
        .bg(theme.bg_tertiary)
        // image-viewer: click-to-open disabled, reimplement later
        // .when(!is_sticker, |d| {
        //     d.cursor_pointer().on_click(move |_, window, cx| {
        //         ImageViewer::open(
        //             gallery.clone(),
        //             index,
        //             uploader_name.clone(),
        //             uploader_avatar.clone(),
        //             window,
        //             cx,
        //         );
        //     })
        // })
        .child(
            img(src)
                .size_full()
                .object_fit(object_fit)
                .with_fallback(move || {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(fallback_bg)
                        .child(
                            Icon::new(IconName::ImageThumbnail)
                                .size(px(32.))
                                .text_color(fallback_fg),
                        )
                        .into_any_element()
                }),
        )
        .into_any_element()
}

fn render_video(
    msg_id: MessageId,
    index: usize,
    att: &MessageAttachment,
    ctx: &RowCtx,
) -> AnyElement {
    if let Some(view) = ctx.active_videos.get(&(msg_id, index)) {
        return div()
            .w(px(att.display_width))
            .h(px(att.display_height))
            .max_w_full()
            .child(view.clone())
            .into_any_element();
    }
    render_video_poster(msg_id, index, att, ctx)
}

fn render_video_poster(
    msg_id: MessageId,
    index: usize,
    att: &MessageAttachment,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let url = SharedString::from(att.url.clone());
    let thumbnail = att.thumbnail_proxied.clone();
    let width = att.display_width;
    let height = att.display_height;
    let host = ctx.video_host.clone();
    let overlay = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.3,
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(48.))
                .h(px(48.))
                .rounded_full()
                .bg(gpui::Rgba {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.5,
                })
                .child(
                    Icon::new(IconName::PlayButton)
                        .size(px(20.))
                        .text_color(gpui::white()),
                ),
        );
    div()
        .id(SharedString::from(format!(
            "msg-video-{}-{}",
            msg_id.0, index
        )))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(att.display_width))
        .h(px(att.display_height))
        .max_w_full()
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_tertiary)
        .cursor_pointer()
        .when(!thumbnail.is_empty(), |d| {
            d.child(
                img(thumbnail.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
        })
        .child(overlay)
        .on_click(move |_, window, cx| {
            let activation = VideoActivation {
                url: url.clone(),
                poster: thumbnail.clone(),
                width,
                height,
            };
            let _ = host.update(cx, |host, cx| {
                host.activate_video((msg_id, index), activation, window, cx);
            });
        })
        .into_any_element()
}

fn render_file_box(att: &MessageAttachment, theme: &Theme) -> AnyElement {
    let label = if att.filename.is_empty() {
        "Attachment".to_string()
    } else {
        att.filename.clone()
    };
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
        .child(label)
        .into_any_element()
}

fn is_gif(url: &str) -> bool {
    url.contains(".gif")
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
