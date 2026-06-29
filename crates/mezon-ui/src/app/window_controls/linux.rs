use gpui::{Div, MouseButton, Pixels, Rgba, StyleRefinement, Window, prelude::*, px, rgb};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

use super::{CONTROL_CLOSE_HOVER, control_button, controls_row};

pub fn render_controls(theme: &Theme, window: &Window) -> impl IntoElement {
    let hover = theme.bg_hover;
    let color = theme.text_secondary;
    let icon_size = px(super::CONTROL_ICON_SIZE);
    let zoom_icon = if window.is_maximized() {
        IconName::WindowRestore
    } else {
        IconName::WindowMaximize
    };

    controls_row()
        .child(window_control_button(
            color,
            icon_size,
            IconName::WindowMinimize,
            move |style| style.bg(hover),
            |window| window.minimize_window(),
        ))
        .child(window_control_button(
            color,
            icon_size,
            zoom_icon,
            move |style| style.bg(hover),
            |window| window.zoom_window(),
        ))
        .child(window_control_button(
            color,
            icon_size,
            IconName::WindowClose,
            |style| style.bg(rgb(CONTROL_CLOSE_HOVER)).text_color(gpui::white()),
            |window| window.remove_window(),
        ))
}

fn window_control_button(
    color: Rgba,
    icon_size: Pixels,
    icon: IconName,
    hover: impl FnOnce(StyleRefinement) -> StyleRefinement + 'static,
    on_click: impl Fn(&mut Window) + 'static,
) -> Div {
    control_button(color)
        .hover(hover)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            on_click(window);
        })
        .child(Icon::new(icon).size(icon_size).text_color(color))
}
