use gpui::{Div, Pixels, Rgba, StyleRefinement, Window, WindowControlArea, prelude::*, px, rgb};

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
            WindowControlArea::Min,
            move |style| style.bg(hover),
        ))
        .child(window_control_button(
            color,
            icon_size,
            zoom_icon,
            WindowControlArea::Max,
            move |style| style.bg(hover),
        ))
        .child(window_control_button(
            color,
            icon_size,
            IconName::WindowClose,
            WindowControlArea::Close,
            |style| style.bg(rgb(CONTROL_CLOSE_HOVER)).text_color(gpui::white()),
        ))
}

fn window_control_button(
    color: Rgba,
    icon_size: Pixels,
    icon: IconName,
    area: WindowControlArea,
    hover: impl FnOnce(StyleRefinement) -> StyleRefinement + 'static,
) -> Div {
    control_button(color)
        .hover(hover)
        .window_control_area(area)
        .child(Icon::new(icon).size(icon_size).text_color(color))
}
