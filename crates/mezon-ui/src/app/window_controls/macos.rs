use block::ConcreteBlock;
use gpui::{App, Window, WindowHandle, div, prelude::*, px, rgb};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{Once, OnceLock};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

use super::{
    MACOS_TRAFFIC_LIGHT_BUTTON_SIZE, MACOS_TRAFFIC_LIGHT_GAP, MACOS_TRAFFIC_LIGHT_PADDING_X,
    MACOS_TRAFFIC_LIGHT_PADDING_Y,
};

const NS_KEY_DOWN_MASK: u64 = 1 << 10;
const NS_COMMAND_KEY_MASK: u64 = 1 << 20;
const NS_ALTERNATE_KEY_MASK: u64 = 1 << 19;
const NS_CONTROL_KEY_MASK: u64 = 1 << 18;
static INSTALL_SHORTCUTS: Once = Once::new();

#[allow(dead_code)]
struct EventMonitor(*mut Object);
unsafe impl Send for EventMonitor {}
unsafe impl Sync for EventMonitor {}

static EVENT_MONITOR: OnceLock<EventMonitor> = OnceLock::new();

pub fn render_controls(_theme: &Theme, _window: &Window) -> impl IntoElement {
    let zoom_icon = IconName::MacOSMaximizeIcon;

    div()
        .id("macos-window-controls")
        .absolute()
        .top_0()
        .left_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(MACOS_TRAFFIC_LIGHT_GAP))
        .pl(px(MACOS_TRAFFIC_LIGHT_PADDING_X))
        .pr(px(8.))
        .py(px(MACOS_TRAFFIC_LIGHT_PADDING_Y))
        .child(macos_traffic_button(
            "macos-close",
            rgb(0xff5f57),
            IconName::MacOSCloseIcon,
            |window, _| hide_window(window),
        ))
        .child(macos_traffic_button(
            "macos-minimize",
            rgb(0xffbd2e),
            IconName::MacOSMinimizeIcon,
            |window, _| window.minimize_window(),
        ))
        .child(macos_traffic_button(
            "macos-zoom",
            rgb(0x28ca42),
            zoom_icon,
            |window, _| window.zoom_window(),
        ))
}

fn macos_traffic_button(
    id: &'static str,
    background: gpui::Rgba,
    icon: IconName,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .group("macos-traffic")
        .flex()
        .items_center()
        .justify_center()
        .size(px(MACOS_TRAFFIC_LIGHT_BUTTON_SIZE))
        .rounded_full()
        .bg(background)
        .cursor_pointer()
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(
            div()
                .opacity(0.)
                .group_hover("macos-traffic", |s| s.opacity(100.))
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .child(Icon::new(icon).size(px(8.)).text_color(gpui::black())),
        )
}

pub fn hide_window(window: &Window) {
    let Some(view) = appkit_view(window) else {
        return;
    };
    with_ns_window(view, |ns_window| unsafe {
        order_out(ns_window);
    });
}

pub fn hide_active_window(cx: &mut App) {
    update_active_window(cx, |window| hide_window(window));
}

pub fn minimize_active_window(cx: &mut App) {
    update_active_window(cx, |window| window.minimize_window());
}

pub fn configure_window<V: 'static>(cx: &mut App, handle: WindowHandle<V>) {
    if let Err(error) = cx.update_window(handle.into(), |_, window, cx| {
        window.on_window_should_close(cx, |window, _| {
            hide_window(window);
            false
        });

        if let Some(view) = appkit_view(window) {
            disable_fullscreen(view);
        }
    }) {
        tracing::warn!("Failed to configure window: {error}");
    }
}

pub fn install_shortcuts(cx: &App) {
    INSTALL_SHORTCUTS.call_once(|| {
        let async_app = cx.to_async();
        let handler = ConcreteBlock::new(move |event: *mut Object| -> *mut Object {
            #[cfg(debug_assertions)]
            unsafe {
                let kc: u16 = msg_send![event, keyCode];
                if matches!(kc, 123 | 124 | 125 | 126) {
                    eprintln!("[gpui_monitor] arrow keyCode={kc} (123=L 124=R 125=Down 126=Up)");
                }
            }
            if unsafe { handle_shortcut(event, &async_app) } {
                std::ptr::null_mut()
            } else {
                event
            }
        });
        let handler = handler.copy();

        unsafe {
            let monitor: *mut Object = msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: NS_KEY_DOWN_MASK handler: &*handler];
            if monitor.is_null() {
                tracing::warn!("Failed to install macOS shortcut monitor");
            } else {
                let _ = EVENT_MONITOR.set(EventMonitor(monitor));
            }
        }
    });
}

unsafe fn handle_shortcut(event: *mut Object, async_app: &gpui::AsyncApp) -> bool {
    let flags: u64 = msg_send![event, modifierFlags];
    if flags & NS_COMMAND_KEY_MASK == 0 {
        return false;
    }
    if flags & (NS_ALTERNATE_KEY_MASK | NS_CONTROL_KEY_MASK) != 0 {
        return false;
    }

    let chars: *mut Object = msg_send![event, charactersIgnoringModifiers];
    if chars.is_null() {
        return false;
    }
    let len: u64 = msg_send![chars, length];
    if len == 0 {
        return false;
    }
    let key: u16 = msg_send![chars, characterAtIndex: 0usize];
    let Ok(key) = u8::try_from(key) else {
        return false;
    };

    match key.to_ascii_lowercase() {
        b'w' => {
            unsafe {
                order_out(key_window());
            }
            true
        }
        b'm' => {
            unsafe {
                miniaturize(key_window());
            }
            true
        }
        b'h' => {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![app, hide: std::ptr::null::<Object>()];
            true
        }
        b'q' => {
            async_app.update(|cx| cx.quit());
            true
        }
        _ => false,
    }
}

fn update_active_window(cx: &mut App, f: impl FnOnce(&mut Window)) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    let _ = handle.update(cx, |_, window, _| f(window));
}

unsafe fn key_window() -> *mut Object {
    let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
    msg_send![app, keyWindow]
}

unsafe fn order_out(ns_window: *mut Object) {
    if ns_window.is_null() {
        return;
    }
    let _: () = msg_send![ns_window, orderOut: std::ptr::null::<Object>()];
}

unsafe fn miniaturize(ns_window: *mut Object) {
    if ns_window.is_null() {
        return;
    }
    let _: () = msg_send![ns_window, miniaturize: std::ptr::null::<Object>()];
}

fn appkit_view(window: &Window) -> Option<*mut std::ffi::c_void> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    Some(appkit.ns_view.as_ptr())
}

fn with_ns_window(native_view: *mut std::ffi::c_void, f: impl FnOnce(*mut Object)) {
    if native_view.is_null() {
        return;
    }

    unsafe {
        let native_view = native_view.cast::<Object>();
        let ns_window: *mut Object = msg_send![native_view, window];
        if ns_window.is_null() {
            return;
        }
        f(ns_window);
    }
}

fn disable_fullscreen(native_view: *mut std::ffi::c_void) {
    const FULLSCREEN_PRIMARY: u64 = 1 << 7;
    const FULLSCREEN_AUXILIARY: u64 = 1 << 8;
    const FULLSCREEN_NONE: u64 = 1 << 9;
    const FULLSCREEN_STYLE_MASK: u64 = 1 << 14;

    with_ns_window(native_view, |window| unsafe {
        let style_mask: u64 = msg_send![window, styleMask];
        if style_mask & FULLSCREEN_STYLE_MASK != 0 {
            let _: () = msg_send![window, toggleFullScreen: std::ptr::null::<Object>()];
        }

        let current: u64 = msg_send![window, collectionBehavior];
        let behavior = (current & !(FULLSCREEN_PRIMARY | FULLSCREEN_AUXILIARY)) | FULLSCREEN_NONE;
        let _: () = msg_send![window, setCollectionBehavior: behavior];
    });
}
