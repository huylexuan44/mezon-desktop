use std::sync::Arc;

use gpui::{
    AnyElement, App, Entity, FontWeight, ObjectFit, SharedString, Transformation, div, img,
    prelude::*, px, radians, rems,
};
use mezon_store::{
    AlbumLayout, AppConfig, ChannelType, Message, MessageAttachment, MessageCode, MessageId,
    MessageReference, MessagesStore, Reaction, ViewerMedia, resolve_avatar_url,
};

use super::context::{REPLY_USERNAME_COLOR, RowCtx};
use super::gif_video::GifVideoView;
use super::reaction_detail::{UserReactionPanel, emoji_error_fallback};
use super::time::format_message_time;
use super::video_player::VideoActivation;
use crate::app::shell::Shell;
use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size};
use crate::theme::Theme;

pub fn avatar_element(msg: &Message, ctx: &RowCtx, cx: &App) -> AnyElement {
    let is_anonymous = AppConfig::try_global(cx)
        .map(|config| {
            !config.anonymous_user_id.is_empty() && msg.sender_id == config.anonymous_user_id
        })
        .unwrap_or(false);
    let (raw_url, proxied) = resolve_message_avatar_urls(msg, ctx, cx);
    let mut avatar = Avatar::new()
        .name(msg.sender_name.clone())
        .with_size(Size::Small)
        .anonymous(is_anonymous)
        .image_cache(ctx.avatar_cache.clone());
    if is_anonymous {
        return avatar.into_any_element();
    }
    if let Some(proxied) = proxied {
        avatar = avatar.src(proxied);
        if !raw_url.is_empty() {
            avatar = avatar.fallback_src(raw_url);
        }
    } else if !raw_url.is_empty() {
        avatar = avatar.src(raw_url);
    }
    avatar.into_any_element()
}

fn resolve_message_avatar_urls(
    msg: &Message,
    ctx: &RowCtx,
    cx: &App,
) -> (String, Option<SharedString>) {
    if let Some(context) = ctx.profile_context
        && let Some(user_id) = msg.sender_user_id
        && let Some(avatar_url) = resolve_avatar_url(user_id, context, cx)
        && !avatar_url.is_empty()
    {
        let proxied = crate::util::imgproxy::avatar_url(cx, &avatar_url);
        return (avatar_url, Some(SharedString::from(proxied)));
    }

    let proxied = msg.avatar_proxied.clone();
    if !proxied.is_empty() {
        return (msg.avatar_url.to_string(), Some(proxied));
    }
    (msg.avatar_url.to_string(), None)
}

pub fn render_head(msg: &Message, ctx: &RowCtx, name_color: u32) -> AnyElement {
    let theme = ctx.theme;
    let time_label = format_message_time(&msg.time_hhmm, msg.local_date, ctx.locale, ctx.now);
    div()
        .flex()
        .flex_row()
        .items_baseline()
        .gap_2()
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
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

fn reply_preview_line(content: &str) -> String {
    const MAX_CHARS: usize = 120;
    let mut out = String::new();
    let mut chars = 0usize;
    let mut first = true;
    for word in content.split_whitespace() {
        if !first {
            out.push(' ');
            chars += 1;
        }
        first = false;
        for ch in word.chars() {
            if chars >= MAX_CHARS {
                return out;
            }
            out.push(ch);
            chars += 1;
        }
    }
    out
}

pub fn render_reply(reference: &MessageReference, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    if reference.message_ref_id.is_zero() {
        return div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(24.))
            .pl(px(super::context::REPLY_INSET))
            .pr(px(super::context::CONTENT_RIGHT_PAD))
            .text_size(px(14.))
            .child(
                Icon::new(IconName::ReplyCorner)
                    .size_4()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .size_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme.tokens.bg_icon_theme)
                    .child(
                        Icon::new(IconName::IconReplyMessDeletedWeb)
                            .size_4()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(
                div()
                    .italic()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(ctx.locale, "message.messageDeleteReply").to_string()),
            )
            .into_any_element();
    }

    let preview = if reference.content.is_empty() {
        if reference.has_attachment {
            mezon_i18n::t(ctx.locale, "chat.clickToSeeAttachment").to_string()
        } else {
            String::new()
        }
    } else {
        reply_preview_line(&reference.content)
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
        .id(("reply", reference.message_ref_id.0 as usize))
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
                .truncate()
                .text_color(theme.tokens.text_theme_message)
                .child(preview),
        )
        .into_any_element()
}

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
        name: msg.sender_name.clone(),
        avatar: if msg.avatar_proxied.is_empty() {
            msg.avatar_url.clone()
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
            .id(("msg-album", index))
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
            .id(("msg-gif", index))
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
        .id(("msg-img", index))
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
        .id(("msg-video", index))
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

pub fn render_reactions(msg: &Message, ctx: &RowCtx) -> Option<AnyElement> {
    if msg.reactions.is_empty() {
        return None;
    }
    let mut row = div().flex().flex_row().flex_wrap().gap_2().mt_1().w_full();
    for (i, reaction) in msg.reactions.iter().enumerate() {
        row = row.child(reaction_pill(i, reaction, msg.id, ctx));
    }
    Some(row.into_any_element())
}

fn reaction_pill(
    index: usize,
    reaction: &Reaction,
    message_id: MessageId,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let reacted = !ctx.current_user_id.is_empty() && reaction.has_sender(ctx.current_user_id);
    let count_label = reaction.count_label.clone();
    let src = reaction.emoji_proxied.clone();
    let add_emoji_id = reaction.emoji_id.clone();
    let add_emoji = reaction.emoji.clone();
    let panel_emoji_id = reaction.emoji_id.clone();
    let panel_emoji = reaction.emoji.clone();
    let avatar_cache = ctx.avatar_cache.clone();

    let mut pill = div()
        .id(("reaction", index))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .h(px(24.))
        .min_w(px(48.))
        .pl(px(28.))
        .pr(px(8.))
        .rounded_md()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .text_color(theme.tokens.text_theme_primary)
        .on_click(move |_, _, cx| {
            MessagesStore::global(cx).update(cx, |store, cx| {
                store.add_reaction(
                    message_id,
                    add_emoji_id.to_string(),
                    add_emoji.to_string(),
                    cx,
                );
            });
        })
        .hoverable_tooltip(move |_window, cx| {
            cx.new(|cx| {
                UserReactionPanel::new(
                    message_id,
                    panel_emoji_id.clone(),
                    panel_emoji.clone(),
                    avatar_cache.clone(),
                    cx,
                )
            })
            .into()
        });
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

    let glyph = reaction.emoji.clone();
    let emoji_el = if src.is_empty() {
        div()
            .absolute()
            .left(px(5.))
            .child(glyph)
            .into_any_element()
    } else {
        img(src)
            .absolute()
            .left(px(5.))
            .size(px(16.))
            .object_fit(ObjectFit::ScaleDown)
            .with_fallback(emoji_error_fallback(px(16.), theme.text_muted))
            .into_any_element()
    };

    pill.child(emoji_el).child(count_label).into_any_element()
}

pub fn render_hover_actions(
    msg: &Message,
    combined: bool,
    has_reply: bool,
    is_different_day: bool,
    ctx: &RowCtx,
) -> AnyElement {
    if ctx.suppress_hover || ctx.hovered_row != Some(msg.id) {
        return div().into_any_element();
    }
    let theme = ctx.theme;
    let bg_hover = theme.bg_hover;
    let action = move |id: &'static str, icon: IconName, size: f32| {
        let mut svg_icon = Icon::new(icon)
            .size(px(size))
            .text_color(theme.text_secondary);
        if matches!(icon, IconName::Reply) {
            // Mirrors React's `rotate-180` on the toolbar's reply icon (the same
            // asset is used un-rotated for the "···" menu's reply item).
            svg_icon =
                svg_icon.with_transformation(Transformation::rotate(radians(std::f32::consts::PI)));
        }
        div()
            .id(id)
            .p_1()
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .child(svg_icon)
    };

    let (top, margin_top) = if is_different_day {
        (-8., 4.)
    } else if combined || has_reply {
        (-8., 0.)
    } else {
        (16., 0.)
    };

    let reply_id = msg.id;
    let react_id = msg.id;
    let react_host = ctx.video_host.clone();

    let is_topic_msg = msg.code == MessageCode::Topic;
    let is_poll_msg = msg.code == MessageCode::Poll;
    let sender_is_real = !msg.sender_id.is_empty() && msg.sender_id != "0";
    let is_own_message = ctx.current_user_id == msg.sender_id.as_str();

    let show_topic = ctx.clan_id.is_some_and(|c| !c.is_zero()) && !is_topic_msg && !is_poll_msg;
    let show_edit = is_own_message
        && msg.code != MessageCode::SendToken
        && msg.code.is_user_timeline()
        && !is_poll_msg
        && !msg.is_forwarded;
    let show_thread = ctx.channel_top_level
        && ctx.is_clan_owner
        && !is_poll_msg
        && ctx.channel_type != Some(ChannelType::Stream)
        && ctx.channel_type != Some(ChannelType::App);
    let show_coffee = !is_own_message && sender_is_real;

    let coming_soon = ctx.coming_soon.clone();

    let msg_id = msg.id;
    let edit_host = ctx.video_host.clone();
    let option_host = ctx.video_host.clone();

    let recent_emoji = (!ctx.emoji_recent.is_empty()).then(|| {
        let mut row = div().flex().flex_row().items_center().gap_0p5();
        for emoji in ctx.emoji_recent {
            let emoji_id = emoji.id.clone();
            let shortname = emoji.shortname.clone();
            let src = emoji.src.clone();
            let cell_id =
                SharedString::from(format!("recent-emoji-{}-{}", msg.row_anchor_id.0, emoji.id));
            let mut cell = div()
                .id(cell_id)
                .flex()
                .items_center()
                .justify_center()
                .p_1()
                .rounded_md()
                .cursor_pointer()
                .hover(move |s| s.bg(bg_hover))
                .on_click(move |_, _, cx| {
                    MessagesStore::global(cx).update(cx, |store, cx| {
                        store.add_reaction(msg_id, emoji_id.clone(), shortname.clone(), cx);
                    });
                });
            if !src.is_empty() {
                cell = cell.child(
                    img(src)
                        .size(px(20.))
                        .with_fallback(emoji_error_fallback(px(20.), theme.text_secondary)),
                );
            }
            row = row.child(cell);
        }
        row.child(
            div()
                .w(px(1.))
                .h(px(20.))
                .mx_1()
                .bg(theme.border)
                .opacity(0.5),
        )
    });

    div()
        .id(SharedString::from(format!(
            "hover-actions-{}",
            msg.row_anchor_id.0
        )))
        .absolute()
        .right(px(24.))
        .top(px(top))
        .mt(px(margin_top))
        .flex()
        .flex_row()
        .items_center()
        .gap_0p5()
        .p_0p5()
        .rounded_lg()
        .bg(theme.tokens.bg_theme_contexify)
        .children(recent_emoji)
        .when(show_topic, |d| {
            let coming_soon = coming_soon.clone();
            d.child(
                action("topic", IconName::TopicIcon, 24.).on_click(move |_, _, cx| {
                    let coming_soon = coming_soon.clone();
                    Shell::global(cx).update(cx, move |shell, cx| shell.info(coming_soon, cx));
                }),
            )
        })
        .child(
            action("react", IconName::Smile, 20.).on_click(move |_, window, cx| {
                let position = window.mouse_position();
                let _ = react_host.update(cx, |this, cx| {
                    this.open_reaction_picker(react_id, position, window, cx);
                });
            }),
        )
        .when(!is_own_message, |d| {
            d.child(
                action("reply", IconName::Reply, 20.).on_click(move |_, _, cx| {
                    MessagesStore::global(cx)
                        .update(cx, |store, cx| store.set_reply_to(reply_id, cx));
                }),
            )
        })
        .when(show_edit, |d| {
            d.child(
                action("edit", IconName::PenEdit, 20.).on_click(move |_, window, cx| {
                    let _ = edit_host.update(cx, |this, cx| {
                        this.begin_edit(msg_id, window, cx);
                    });
                }),
            )
        })
        .when(show_thread, |d| {
            let coming_soon = coming_soon.clone();
            d.child(
                action("thread", IconName::ThreadIcon, 20.).on_click(move |_, _, cx| {
                    let coming_soon = coming_soon.clone();
                    Shell::global(cx).update(cx, move |shell, cx| shell.info(coming_soon, cx));
                }),
            )
        })
        .when(show_coffee, |d| {
            let coming_soon = coming_soon.clone();
            d.child(
                action("give-coffee", IconName::DollarIconRightClick, 20.).on_click(
                    move |_, _, cx| {
                        let coming_soon = coming_soon.clone();
                        Shell::global(cx).update(cx, move |shell, cx| shell.info(coming_soon, cx));
                    },
                ),
            )
        })
        .child(
            action("option", IconName::ThreeDot, 20.).on_click(move |_, window, cx| {
                let position = window.mouse_position();
                let _ = option_host.update(cx, |this, cx| {
                    this.open_context_menu(msg_id, position, cx);
                });
            }),
        )
        .into_any_element()
}

pub fn render_date_divider(theme: &Theme, label: &str) -> AnyElement {
    let line_color = theme.tokens.border_color_primary;
    div()
        .id(SharedString::from(format!("date-sep-{}", label)))
        .w_full()
        .mt_5()
        .mb_2()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .relative()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .flex()
                        .items_center()
                        .child(div().w_full().h(px(1.)).bg(line_color)),
                )
                .child(
                    div()
                        .relative()
                        .px_4()
                        .rounded_lg()
                        .bg(theme.tokens.bg_primary)
                        .text_xs()
                        .line_height(rems(1.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(label.to_string()),
                ),
        )
        .into_any_element()
}
