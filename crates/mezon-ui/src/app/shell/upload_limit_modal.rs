use gpui::{Context, FocusHandle, SharedString, Window, div, img, prelude::*, px, rgb, white};

use super::Shell;

pub(super) struct UploadLimitModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) title: SharedString,
    pub(super) content: SharedString,
}

impl Render for UploadLimitModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .relative()
            .w(px(400.))
            .h(px(240.))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .rounded_lg()
            .bg(rgb(0xEF4444))
            .child(
                div()
                    .w(px(360.))
                    .h(px(206.))
                    .rounded_lg()
                    .border_2()
                    .border_dashed()
                    .border_color(white())
                    .child(
                        div()
                            .mt(px(56.))
                            .flex()
                            .flex_col()
                            .child(
                                div().w_full().flex().justify_center().child(
                                    div()
                                        .mt(px(16.))
                                        .text_2xl()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_center()
                                        .text_color(white())
                                        .child(self.title.clone()),
                                ),
                            )
                            .child(
                                div().w_full().flex().justify_center().mt(px(16.)).child(
                                    div()
                                        .w(px(306.))
                                        .text_center()
                                        .text_color(white())
                                        .child(self.content.clone()),
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px(-144.))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        img("icons/file-and-folder.svg")
                            .w(px(250.))
                            .h(px(225.))
                            .flex_none(),
                    ),
            )
    }
}
