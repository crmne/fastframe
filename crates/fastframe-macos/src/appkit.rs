//! The AppKit calls: moving the standard window buttons and reading the
//! user defaults.

// AppKit's view hierarchy is FFI.
#![allow(unsafe_code)]

use objc2_app_kit::{NSView, NSWindowButton};
use objc2_foundation::{NSString, NSUserDefaults};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Moves the standard buttons of `frame`'s window into a bar `height`
/// AppKit points tall.
pub(crate) fn align(frame: &eframe::Frame, height: f64) {
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: eframe supplies a live NSView, and this runs on its main thread.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let Some(window) = view.window() else {
        return;
    };
    let Some(close) = window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    // SAFETY: the buttons and their parent views belong to this live window,
    // and AppKit is used only from eframe's main-thread callback.
    let Some(parent) = (unsafe { close.superview() }) else {
        return;
    };
    // SAFETY: as above.
    let Some(container) = (unsafe { parent.superview() }) else {
        return;
    };
    // The container spans the bar at the top of the window (AppKit's origin
    // is at the bottom), and the buttons' parent fills it.
    let mut rect = container.frame();
    rect.size.height = height;
    rect.origin.y = window.frame().size.height - height;
    if container.frame() != rect {
        container.setFrame(rect);
    }
    let mut parent_rect = parent.frame();
    parent_rect.origin.y = 0.0;
    parent_rect.size.height = height;
    if parent.frame() != parent_rect {
        parent.setFrame(parent_rect);
    }
    for (index, kind) in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(button) = window.standardWindowButton(kind) {
            let (x, y) = crate::button_origin(index, height, button.frame().size.height);
            let mut origin = button.frame().origin;
            origin.x = x;
            origin.y = y;
            if button.frame().origin != origin {
                button.setFrameOrigin(origin);
            }
        }
    }
}

/// `AppleActionOnDoubleClick`, and the older `AppleMiniaturizeOnDoubleClick`.
pub(crate) fn double_click_defaults() -> (Option<String>, bool) {
    let defaults = NSUserDefaults::standardUserDefaults();
    let action = defaults
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|action| action.to_string());
    let minimize = defaults.boolForKey(&NSString::from_str("AppleMiniaturizeOnDoubleClick"));
    (action, minimize)
}
