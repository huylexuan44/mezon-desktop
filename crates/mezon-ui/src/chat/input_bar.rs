use std::sync::Arc;

use crate::chat::ReplyTarget;
// composer: use crate::chat::{MentionInput, ReplyTarget};
use crate::components::primitives::{Button, Icon, IconName, Input, InputState};
// composer: use crate::components::primitives::{Button, Icon, IconName};
use crate::theme::Theme;
use gpui::{App, ClickEvent, Window, div, prelude::*, px};

type SendHandler = Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>;

pub struct InputBar {
    input_state: Option<gpui::Entity<InputState>>,
    // composer: mention_input: Option<gpui::Entity<MentionInput>>,
    on_send: Option<SendHandler>,
    // composer: on_cancel_reply: Option<SendHandler>,
    replying_to: Option<ReplyTarget>,
}

impl Default for InputBar {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBar {
    pub fn new() -> Self {
        Self {
            input_state: None,
            // composer: mention_input: None,
            on_send: None,
            // composer: on_cancel_reply: None,
            replying_to: None,
        }
    }

    // composer: pub fn on_cancel_reply(mut self, handler: SendHandler) -> Self {
    // composer:     self.on_cancel_reply = Some(handler);
    // composer:     self
    // composer: }

    pub fn with_input(mut self, state: gpui::Entity<InputState>) -> Self {
        self.input_state = Some(state);
        self
    }
    // composer: pub fn with_mention_input(mut self, mention_input: gpui::Entity<MentionInput>) -> Self {
    // composer:     self.mention_input = Some(mention_input);
    // composer:     self
    // composer: }

    pub fn on_send(mut self, handler: SendHandler) -> Self {
        self.on_send = Some(handler);
        self
    }

    pub fn replying_to(mut self, target: Option<ReplyTarget>) -> Self {
        self.replying_to = target;
        self
    }

    // composer: take `&self` and add `let on_cancel = self.on_cancel_reply.clone();` before the div.
    fn reply_preview_bar(theme: &Theme, locale: &str, target: &ReplyTarget) -> impl IntoElement {
        div()
            .id("reply-preview-bar")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .bg(theme.bg_hover)
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .w(px(3.))
                    .h_full()
                    .min_h(px(20.))
                    .rounded(px(2.))
                    .bg(theme.border),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(format!(
                                "{} {}",
                                mezon_i18n::t(locale, "chat.replyingTo"),
                                target.sender_name
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(target.content_preview.clone()),
                    ),
            )
            .child(
                div().cursor_pointer().child(
                    Icon::new(IconName::Close)
                        .size_4()
                        .text_color(theme.text_muted),
                ),
            )
        // composer: clickable cancel button — replace the .child(...) above with:
        // composer: .child(
        // composer:     div()
        // composer:         .id("reply-cancel")
        // composer:         .cursor_pointer()
        // composer:         .on_click(move |_, window, cx| {
        // composer:             if let Some(handler) = &on_cancel {
        // composer:                 handler(window, cx);
        // composer:             }
        // composer:         })
        // composer:         .child(
        // composer:             Icon::new(IconName::Close)
        // composer:                 .size_4()
        // composer:                 .text_color(theme.text_muted),
        // composer:         ),
        // composer: )
    }

    pub fn render(&self, theme: &Theme, locale: &str) -> impl IntoElement {
        let on_send = self.on_send.clone();

        let on_click = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if let Some(ref handler) = on_send {
                handler(window, cx);
            }
        };

        div()
            .flex()
            .flex_col()
            .when_some(self.replying_to.as_ref(), |d, target| {
                d.child(Self::reply_preview_bar(theme, locale, target))
                // composer: d.child(self.reply_preview_bar(theme, locale, target))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.bg_primary)
                    .when_some(self.input_state.as_ref(), |d, state| {
                        d.child(div().flex_1().child(Input::new(state)))
                    })
                    // composer: .when_some(self.mention_input.clone(), |d, mention_input| {
                    // composer:     d.child(div().flex_1().child(mention_input))
                    // composer: })
                    .child(
                        Button::new("send-btn")
                            .label(mezon_i18n::t(locale, "chat.send"))
                            .on_click(on_click),
                    ),
            )
    }
}
