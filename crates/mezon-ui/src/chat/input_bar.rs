use std::sync::Arc;

use crate::chat::ReplyTarget;
use crate::components::primitives::{Button, Icon, IconName, Input, InputState};
use crate::theme::Theme;
use gpui::{App, ClickEvent, SharedString, Window, div, prelude::*, px};

type SendHandler = Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>;

pub struct InputBar {
    input_state: Option<gpui::Entity<InputState>>,
    on_send: Option<SendHandler>,
    replying_to: Option<ReplyTarget>,
    typing_label: Option<SharedString>,
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
            on_send: None,
            replying_to: None,
            typing_label: None,
        }
    }

    pub fn typing_label(mut self, label: Option<SharedString>) -> Self {
        self.typing_label = label;
        self
    }

    pub fn with_input(mut self, state: gpui::Entity<InputState>) -> Self {
        self.input_state = Some(state);
        self
    }

    pub fn on_send(mut self, handler: SendHandler) -> Self {
        self.on_send = Some(handler);
        self
    }

    pub fn replying_to(mut self, target: Option<ReplyTarget>) -> Self {
        self.replying_to = target;
        self
    }

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
            })
            .child(
                div()
                    .mx_3()
                    .h(px(16.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(theme.text_primary)
                    .when_some(self.typing_label.as_ref(), |d, label| {
                        d.child(label.clone())
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .min_w_0()
                    .w_full()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.bg_primary)
                    .when_some(self.input_state.as_ref(), |d, state| {
                        d.child(div().flex_1().min_w_0().child(Input::new(state)))
                    })
                    .child(
                        Button::new("send-btn")
                            .label(mezon_i18n::t(locale, "chat.send"))
                            .on_click(on_click),
                    ),
            )
    }
}
