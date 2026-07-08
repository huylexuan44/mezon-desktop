use gpui::{
    CursorStyle, Decorations, Div, MouseButton, ResizeEdge, Tiling, TitlebarOptions, Window,
    WindowDecorations, div, prelude::*, px,
};

#[cfg(any(target_os = "linux", target_os = "windows"))]
use gpui::Rgba;

use crate::theme::Theme;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

/// Whether the app paints its own title bar with custom window controls.
/// macOS uses native traffic lights; Linux and Windows use client-side decorations.
pub const HAS_CUSTOM_TITLE_BAR: bool = cfg!(any(target_os = "linux", target_os = "windows"));
pub const APP_NAME: &str = "Mezon";

/// React `SidebarMenu`: `mt-[21px]` on desktop + sticky `pt-3` (12px).
pub const MACOS_TITLEBAR_INSET: f32 = 21.0;
pub const CLAN_SIDEBAR_STICKY_TOP: f32 = 12.0;

pub const MACOS_TRAFFIC_LIGHT_BUTTON_SIZE: f32 = 12.0;
pub const MACOS_TRAFFIC_LIGHT_GAP: f32 = 8.0;
pub const MACOS_TRAFFIC_LIGHT_PADDING_X: f32 = 12.0;
pub const MACOS_TRAFFIC_LIGHT_PADDING_Y: f32 = 12.0;

pub const NAV_ARROW_ICON_SIZE: f32 = 20.0;
pub const NAV_ARROW_BUTTON_PADDING: f32 = 4.0;
pub const APP_HEADER_HEIGHT: f32 = 50.0;
pub const RESIZE_BORDER_SIZE: f32 = 8.0;

#[cfg(target_os = "macos")]
pub const CLAN_SIDEBAR_HEADER_TOP: f32 = MACOS_TITLEBAR_INSET + CLAN_SIDEBAR_STICKY_TOP;
#[cfg(not(target_os = "macos"))]
pub const CLAN_SIDEBAR_HEADER_TOP: f32 = CLAN_SIDEBAR_STICKY_TOP;

/// Render the platform window controls (minimize / maximize / close).
/// Returns an empty element on macOS, where the native traffic lights are used.
pub fn render_controls(theme: &Theme, window: &Window) -> impl IntoElement {
    platform::render_controls(theme, window)
}

pub fn window_title_options() -> TitlebarOptions {
    #[cfg(target_os = "macos")]
    {
        use gpui::{point, px};

        TitlebarOptions {
            title: None,
            appears_transparent: true,
            // Hide native traffic lights; we paint custom controls like the React app.
            traffic_light_position: Some(point(px(-100.), px(-100.))),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        TitlebarOptions {
            title: Some(APP_NAME.into()),
            appears_transparent: true,
            ..Default::default()
        }
    }
}

pub fn main_window_decorations() -> Option<WindowDecorations> {
    #[cfg(target_os = "linux")]
    {
        Some(WindowDecorations::Client)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
pub fn linux_app_id() -> Option<String> {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        Some("mezon".into())
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        None
    }
}

pub fn is_edge_resizable() -> bool {
    cfg!(target_os = "linux")
}

pub fn render_resize_edges(window: &mut Window) -> impl IntoElement {
    if !is_edge_resizable() || window.is_maximized() {
        return div().id("window-resize-edges-disabled");
    }

    let decorations = window.window_decorations();
    let tiling = match decorations {
        Decorations::Client { tiling } => tiling,
        Decorations::Server => Tiling::default(),
    };

    if matches!(decorations, Decorations::Client { .. }) {
        window.set_client_inset(px(RESIZE_BORDER_SIZE));
    }

    let border = px(RESIZE_BORDER_SIZE);
    let mut layer = div()
        .id("window-resize-edges")
        .absolute()
        .top_0()
        .left_0()
        .size_full();

    if !tiling.top {
        layer = layer.child(resize_strip(
            ResizeEdge::Top,
            div().absolute().top_0().left_0().right_0().h(border),
            CursorStyle::ResizeUp,
        ));
    }
    if !tiling.bottom {
        layer = layer.child(resize_strip(
            ResizeEdge::Bottom,
            div().absolute().bottom_0().left_0().right_0().h(border),
            CursorStyle::ResizeDown,
        ));
    }
    if !tiling.left {
        layer = layer.child(resize_strip(
            ResizeEdge::Left,
            div().absolute().top_0().bottom_0().left_0().w(border),
            CursorStyle::ResizeLeft,
        ));
    }
    if !tiling.right {
        layer = layer.child(resize_strip(
            ResizeEdge::Right,
            div().absolute().top_0().bottom_0().right_0().w(border),
            CursorStyle::ResizeRight,
        ));
    }

    if !tiling.top && !tiling.left {
        layer = layer.child(resize_strip(
            ResizeEdge::TopLeft,
            div().absolute().top_0().left_0().size(border),
            CursorStyle::ResizeUpLeftDownRight,
        ));
    }
    if !tiling.top && !tiling.right {
        layer = layer.child(resize_strip(
            ResizeEdge::TopRight,
            div().absolute().top_0().right_0().size(border),
            CursorStyle::ResizeUpRightDownLeft,
        ));
    }
    if !tiling.bottom && !tiling.left {
        layer = layer.child(resize_strip(
            ResizeEdge::BottomLeft,
            div().absolute().bottom_0().left_0().size(border),
            CursorStyle::ResizeUpRightDownLeft,
        ));
    }
    if !tiling.bottom && !tiling.right {
        layer = layer.child(resize_strip(
            ResizeEdge::BottomRight,
            div().absolute().bottom_0().right_0().size(border),
            CursorStyle::ResizeUpLeftDownRight,
        ));
    }

    layer
}

fn resize_strip(edge: ResizeEdge, strip: Div, cursor: CursorStyle) -> Div {
    strip
        .cursor(cursor)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            window.start_window_resize(edge);
        })
}

/// Invisible full-width strip at the top of the window for macOS window dragging
pub fn render_app_drag_header() -> impl IntoElement {
    #[cfg(target_os = "macos")]
    {
        use gpui::px;

        window_drag_handle(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(APP_HEADER_HEIGHT)),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        div().hidden()
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) const CONTROL_BUTTON_SIZE: f32 = 28.0;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) const CONTROL_ICON_SIZE: f32 = 12.0;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) const CONTROL_CLOSE_HOVER: u32 = 0xc42b1c;

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) fn control_button(color: Rgba) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(CONTROL_BUTTON_SIZE))
        .rounded_full()
        .cursor_pointer()
        .text_color(color)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) fn controls_row() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .h_full()
}

pub fn window_drag_handle(header: Div) -> Div {
    #[cfg(target_os = "macos")]
    {
        header.on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count >= 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        header
    }
}
