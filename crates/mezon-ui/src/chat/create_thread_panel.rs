use gpui::{AnyElement, ClickEvent, Entity, FontWeight, div, prelude::*, px};

use crate::chat::layout::ChatLayout;
use crate::components::primitives::{Icon, IconName, Input, InputState, h_flex, v_flex};
use crate::theme::Theme;

const PANEL_WIDTH: f32 = 510.;

pub fn render_create_thread_panel(
    thread_name_input: Entity<InputState>,
    message_input: Entity<InputState>,
    name_error: Option<&str>,
    locale: &str,
    theme: &Theme,
    layout: Entity<ChatLayout>,
) -> AnyElement {
    let tokens = &theme.tokens;
    let cancel_layout = layout.clone();
    let cancel_footer_layout = cancel_layout.clone();
    let send_layout = layout.clone();
    let name_input = thread_name_input.clone();
    let msg_input = message_input.clone();

    let error_label = name_error.map(|key| match key {
        "thread_name_too_short" => mezon_i18n::t(
            locale,
            "channelTopbar.createThread.toast.threadNameTooShort",
        ),
        "thread_name_exists" => {
            mezon_i18n::t(locale, "channelTopbar.createThread.toast.threadNameExists")
        }
        "initial_message_required" => mezon_i18n::t(
            locale,
            "channelTopbar.createThread.toast.initialMessageRequired",
        ),
        other => other,
    });

    v_flex()
        .w(px(PANEL_WIDTH))
        .min_w(px(PANEL_WIDTH))
        .flex_shrink_0()
        .h_full()
        .overflow_hidden()
        .border_l_1()
        .border_color(tokens.border_primary)
        .bg(tokens.theme_setting_primary)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .px_4()
                .h(px(48.))
                .border_b_1()
                .border_color(tokens.border_primary)
                .bg(tokens.theme_setting_nav)
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::ThreadIcon)
                                .size_4()
                                .text_color(tokens.text_theme_primary),
                        )
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(tokens.text_theme_primary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "channelTopbar.createThread.newThread",
                                )),
                        ),
                )
                .child(
                    div()
                        .id("create-thread-close")
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::Close)
                                .size_4()
                                .text_color(tokens.text_theme_primary),
                        )
                        .on_click(move |_: &ClickEvent, _window, cx| {
                            cancel_layout.update(cx, |layout, cx| layout.close_create_thread(cx));
                        }),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .gap_4()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(tokens.text_theme_primary)
                        .child(mezon_i18n::t(
                            locale,
                            "channelTopbar.createThread.threadName",
                        )),
                )
                .child(
                    Input::new(&thread_name_input)
                        .w_full()
                        .text_sm()
                        .text_color(tokens.text_theme_primary),
                )
                .when_some(error_label, |this, err| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.status_dnd)
                            .child(err.to_string()),
                    )
                })
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(tokens.text_theme_primary)
                        .child(mezon_i18n::t(
                            locale,
                            "channelTopbar.createThread.inviteMessage",
                        )),
                )
                .child(
                    Input::new(&message_input)
                        .w_full()
                        .text_sm()
                        .text_color(tokens.text_theme_primary),
                ),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .p_4()
                .border_t_1()
                .border_color(tokens.border_primary)
                .child(
                    div()
                        .id("create-thread-cancel")
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(tokens.text_theme_primary)
                        .hover(|s| s.bg(tokens.bg_hover))
                        .child(mezon_i18n::t(locale, "common.cancel"))
                        .on_click(move |_: &ClickEvent, _window, cx| {
                            cancel_footer_layout
                                .update(cx, |layout, cx| layout.close_create_thread(cx));
                        }),
                )
                .child(
                    div()
                        .id("create-thread-send")
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.brand)
                        .cursor_pointer()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(gpui::white())
                        .hover(|s| s.bg(theme.brand_hover))
                        .child(mezon_i18n::t(locale, "channelTopbar.threads.createThread"))
                        .on_click(move |_: &ClickEvent, window, cx| {
                            let name = name_input.read(cx).value().to_string();
                            let message = msg_input.read(cx).value().to_string();
                            send_layout.update(cx, |layout, cx| {
                                layout.submit_create_thread(name, message, window, cx);
                            });
                        }),
                ),
        )
        .into_any_element()
}
