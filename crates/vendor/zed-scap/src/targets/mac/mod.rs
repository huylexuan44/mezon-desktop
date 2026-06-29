use anyhow::{Context as _, Result};
use cocoa::appkit::{NSApp, NSScreen};
use cocoa::base::{id, nil};
use cocoa::foundation::{NSString, NSUInteger};
use core_graphics_helmer_fork::display::{CGDirectDisplayID, CGDisplay, CGMainDisplayID};
use core_graphics_helmer_fork::window::CGWindowID;
use objc::{msg_send, sel, sel_impl};
use screencapturekit::sc_shareable_content::SCShareableContent;

use super::{Display, Target};

fn get_display_name(display_id: CGDirectDisplayID) -> String {
    unsafe {
        let screens: id = NSScreen::screens(nil);
        let count: u64 = msg_send![screens, count];

        for i in 0..count {
            let screen: id = msg_send![screens, objectAtIndex: i];
            let device_description: id = msg_send![screen, deviceDescription];
            let display_id_number: id = msg_send![device_description, objectForKey: NSString::alloc(nil).init_str("NSScreenNumber")];
            let display_id_number: u32 = msg_send![display_id_number, unsignedIntValue];

            if display_id_number == display_id {
                let localized_name: id = msg_send![screen, localizedName];
                let name: *const i8 = msg_send![localized_name, UTF8String];
                return std::ffi::CStr::from_ptr(name)
                    .to_string_lossy()
                    .into_owned();
            }
        }

        format!("Unknown Display {}", display_id)
    }
}

pub fn get_all_targets() -> Result<Vec<Target>> {
    let mut targets: Vec<Target> = Vec::new();

    let content = SCShareableContent::current();

    for display in content.displays {
        let id: CGDirectDisplayID = display.display_id;
        let raw_handle = CGDisplay::new(id);
        let title = get_display_name(id);

        let target = Target::Display(super::Display {
            id,
            title,
            raw_handle,
        });

        targets.push(target);
    }

    for window in content.windows {
        if window.title.is_some() {
            let id = window.window_id;
            let title = window.title.context("Window title not found")?;
            let raw_handle: CGWindowID = id;

            let target = Target::Window(super::Window {
                id,
                title,
                raw_handle,
            });
            targets.push(target);
        }
    }

    Ok(targets)
}

pub fn get_main_display() -> Result<Display> {
    let id = unsafe { CGMainDisplayID() };
    let title = get_display_name(id);

    Ok(Display {
        id,
        title,
        raw_handle: CGDisplay::new(id),
    })
}

fn shareable_window(window_id: u32) -> Option<(u64, u64)> {
    SCShareableContent::current()
        .windows
        .into_iter()
        .find(|window| window.window_id == window_id)
        .map(|window| (window.width as u64, window.height as u64))
}

fn window_backing_scale_factor(window_id: u32) -> f64 {
    unsafe {
        let ns_app: id = NSApp();
        let ns_window: id =
            msg_send![ns_app, windowWithWindowNumber: window_id as NSUInteger];
        if ns_window != nil {
            return msg_send![ns_window, backingScaleFactor];
        }
    }
    unsafe {
        let screen: id = NSScreen::mainScreen(nil);
        if screen != nil {
            return msg_send![screen, backingScaleFactor];
        }
    }
    2.0
}

pub fn get_scale_factor(target: &Target) -> f64 {
    match target {
        Target::Window(window) => window_backing_scale_factor(window.id),
        Target::Display(display) => {
            let mode = display.raw_handle.display_mode().unwrap();
            (mode.pixel_width() / mode.width()) as f64
        }
    }
}

pub fn get_target_dimensions(target: &Target) -> (u64, u64) {
    match target {
        Target::Window(window) => shareable_window(window.id).unwrap_or((1280, 720)),
        Target::Display(display) => {
            let mode = display.raw_handle.display_mode().unwrap();
            (mode.width(), mode.height())
        }
    }
}
