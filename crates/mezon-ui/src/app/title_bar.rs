use crate::app::window_controls;
use crate::theme::ActiveTheme;
use gpui::{
    Context, Entity, FontWeight, MouseButton, Subscription, Window, WindowControlArea, div,
    prelude::*,
};
use mezon_store::Settings;
use ui::{px, utils::ROUNDED_BORDER_WINDOW};

pub struct TitleBar {
    _bounds_observer: Option<Subscription>,
}

impl TitleBar {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self {
            _bounds_observer: None,
        }
    }

    fn ensure_bounds_observer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._bounds_observer.is_some() {
            return;
        }

        self._bounds_observer = Some(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_bounds_observer(window, cx);
        let theme = cx.theme();

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h_8()
            .bg(theme.title_bar_bg)
            .rounded_tl(px(ROUNDED_BORDER_WINDOW))
            .rounded_tr(px(ROUNDED_BORDER_WINDOW))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .h_full()
                    .when(cfg!(target_os = "windows"), |bar| {
                        bar.window_control_area(WindowControlArea::Drag)
                    })
                    .when(cfg!(target_os = "linux"), |bar| {
                        bar.on_mouse_down(MouseButton::Left, |event, window, _| {
                            if event.click_count >= 2 {
                                window.zoom_window();
                            } else {
                                window.start_window_move();
                            }
                        })
                    })
                    .child(
                        div().flex().items_center().px_3().child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_secondary)
                                .child(window_controls::APP_NAME),
                        ),
                    ),
            )
            .child(window_controls::render_controls(theme, window))
    }
}
