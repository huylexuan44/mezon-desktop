use gpui::{AnyElement, Entity, Pixels, Rgba, SharedString, div, img, prelude::*, px};
use mezon_store::Message;

use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size};
use crate::image_cache::LruImageCache;
use crate::theme::Theme;

const DEFAULT_DISPLAY_NAME_COLOR: u32 = 0x17ac86;

pub enum MessageAttachmentView {
    Image {
        src: SharedString,
        width: Pixels,
        height: Pixels,
        label: String,
    },
    File {
        label: String,
    },
}

fn attachment_box(label: String, bg: Rgba, border: Rgba, color: Rgba) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(240.))
        .h(px(120.))
        .rounded_md()
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_xs()
        .text_color(color)
        .child(label)
        .into_any_element()
}

pub struct MessageRow<'a> {
    message: &'a Message,
    combined: bool,
    reply: bool,
    theme: &'a Theme,
    reply_label: SharedString,
    avatar_src: Option<SharedString>,
    attachments: Vec<MessageAttachmentView>,
    suppress_hover: bool,
    avatar_image_cache: Option<Entity<LruImageCache>>,
}

impl<'a> MessageRow<'a> {
    pub fn new(
        message: &'a Message,
        theme: &'a Theme,
        _current_user_id: &str,
        reply_label: SharedString,
    ) -> Self {
        Self {
            message,
            combined: false,
            reply: false,
            theme,
            reply_label,
            avatar_src: None,
            attachments: Vec::new(),
            suppress_hover: false,
            avatar_image_cache: None,
        }
    }

    pub fn attachments(mut self, views: Vec<MessageAttachmentView>) -> Self {
        self.attachments = views;
        self
    }

    pub fn avatar_src(mut self, src: impl Into<SharedString>) -> Self {
        let src = src.into();
        self.avatar_src = if src.is_empty() { None } else { Some(src) };
        self
    }

    pub fn combined(mut self, combined: bool) -> Self {
        self.combined = combined;
        self
    }

    pub fn reply(mut self, reply: bool) -> Self {
        self.reply = reply;
        self
    }

    pub fn suppress_hover(mut self, suppress_hover: bool) -> Self {
        self.suppress_hover = suppress_hover;
        self
    }

    pub fn avatar_image_cache(mut self, cache: Entity<LruImageCache>) -> Self {
        self.avatar_image_cache = Some(cache);
        self
    }

    pub fn render(&self) -> impl IntoElement {
        crate::trace_render!("MessageRow {}", self.message.id);
        let msg = self.message;
        let theme = self.theme;
        let time = msg.timestamp_label.clone();

        let display_name = &msg.sender_name;

        let mut avatar = Avatar::new().name(display_name).with_size(Size::Small);
        if let Some(cache) = self.avatar_image_cache.clone() {
            avatar = avatar.image_cache(cache);
        }
        if let Some(src) = self.avatar_src.as_deref().filter(|s| !s.is_empty()) {
            avatar = avatar.src(src);
            if !msg.avatar_url.is_empty() && msg.avatar_url != *src {
                avatar = avatar.fallback_src(msg.avatar_url.as_str());
            }
        } else if !msg.avatar_url.is_empty() {
            avatar = avatar.src(msg.avatar_url.as_str());
        }

        let name_row = div()
            .flex()
            .flex_row()
            .items_baseline()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(gpui::rgb(DEFAULT_DISPLAY_NAME_COLOR))
                    .child(display_name.clone()),
            )
            .child(div().text_xs().text_color(theme.text_muted).child(time));

        let content = div()
            .text_sm()
            .text_color(theme.text_secondary)
            .child(msg.content.clone());

        let reply_placeholder = div()
            .id("reply-placeholder")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .mb_1()
            .child(
                Icon::new(IconName::ReplyCorner)
                    .size_4()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.reply_label.clone()),
            );

        let content_area = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .pl(px(16.))
            .child(if self.reply {
                reply_placeholder.into_any_element()
            } else {
                div().into_any_element()
            })
            .child(if !self.combined {
                name_row.into_any_element()
            } else {
                div().into_any_element()
            })
            .child(content)
            .when(!msg.reactions.is_empty(), |d| {
                let mut counts: Vec<(String, usize)> = Vec::new();
                for emoji in &msg.reactions {
                    if let Some(entry) = counts.iter_mut().find(|(e, _)| e == emoji) {
                        entry.1 += 1;
                    } else {
                        counts.push((emoji.clone(), 1));
                    }
                }
                let bg = theme.bg_tertiary;
                let text_color = theme.text_muted;
                d.child(div().flex().flex_row().flex_wrap().gap_1().mt_1().children(
                    counts.into_iter().enumerate().map(|(i, (emoji, count))| {
                        let label = if count > 1 {
                            format!("{} {}", emoji, count)
                        } else {
                            emoji
                        };
                        div()
                            .id(SharedString::from(format!("reaction-{}-{}", msg.id, i)))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(bg)
                            .text_xs()
                            .text_color(text_color)
                            .child(label)
                    }),
                ))
            })
            .when(!self.attachments.is_empty(), |d| {
                d.child(div().flex().flex_col().gap_2().mt_1().children(
                    self.attachments.iter().enumerate().map(|(i, view)| {
                        match view {
                            MessageAttachmentView::Image {
                                src,
                                width,
                                height,
                                label,
                            } => {
                                if src.is_empty() {
                                    let box_bg = theme.bg_tertiary;
                                    attachment_box(
                                        label.clone(),
                                        box_bg,
                                        theme.border,
                                        theme.text_muted,
                                    )
                                    .into_any_element()
                                } else {
                                    // A stable id makes the img stateful, which is
                                    // what lets GPUI advance animated GIF/WebP
                                    // frames (a stateless img only ever shows the
                                    // first frame).
                                    img(src.clone())
                                        .id(SharedString::from(format!("msg-img-{}-{}", msg.id, i)))
                                        .w(*width)
                                        .h(*height)
                                        .max_w(px(400.))
                                        .rounded_md()
                                        .into_any_element()
                                }
                            }
                            MessageAttachmentView::File { label } => div()
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
                                .child(label.clone())
                                .into_any_element(),
                        }
                    }),
                ))
            });

        let hover_bg = theme.bg_hover;
        div()
            .id(SharedString::from(format!("msg-row-{}", msg.id)))
            .flex()
            .flex_row()
            .items_start()
            .w_full()
            .px_4()
            .py(px(2.))
            .when(!self.combined, |d| d.mt(px(10.)).pt_3())
            .when(!self.suppress_hover, |d| d.hover(|s| s.bg(hover_bg)))
            .child({
                if self.combined {
                    div().w(px(40.)).flex_shrink_0().into_any_element()
                } else {
                    div()
                        .w(px(40.))
                        .flex_shrink_0()
                        .id(SharedString::from(format!("msg-avatar-{}", msg.sender_id)))
                        .child(avatar)
                        .into_any_element()
                }
            })
            .child(content_area)
    }
}
