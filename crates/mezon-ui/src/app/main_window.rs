use gpui::{AnyWindowHandle, App, AppContext, Global};

struct MainWindowHandle(AnyWindowHandle);
impl Global for MainWindowHandle {}

pub fn register_main_window(handle: AnyWindowHandle, cx: &mut App) {
    cx.set_global(MainWindowHandle(handle));
}

pub fn activate_main_window(cx: &mut App) {
    let Some(handle) = cx.try_global::<MainWindowHandle>().map(|g| g.0) else {
        return;
    };
    let _ = cx.update_window(handle, |_, window, _| window.activate_window());
}
