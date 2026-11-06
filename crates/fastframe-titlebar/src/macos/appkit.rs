//! The AppKit calls: moving the standard window buttons, reading the user
//! defaults, and a header's double-click.

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
    let left = |kind| {
        window
            .standardWindowButton(kind)
            .map(|b| b.frame().origin.x)
    };
    let spacing = super::spacing(
        left(NSWindowButton::CloseButton),
        left(NSWindowButton::MiniaturizeButton),
    );
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
            let size = button.frame().size;
            let (x, y) = super::button_origin(index, height, size.height, spacing);
            let mut origin = button.frame().origin;
            origin.x = x;
            origin.y = y;
            if button.frame().origin != origin {
                button.setFrameOrigin(origin);
            }
            if kind == NSWindowButton::ZoomButton {
                super::measured(x + size.width);
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

/// See [`super::title_bar_press`]. Ported from Spotifast, which shipped it.
pub(crate) fn title_bar_press() -> bool {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::{NSApplication, NSEventType};
    use objc2_foundation::NSObjectNSDelayedPerforming;

    let Some(mtm) = MainThreadMarker::new() else {
        return true;
    };
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
        return true;
    };
    if event.r#type() != NSEventType::LeftMouseDown || event.clickCount() != 2 {
        return true;
    }
    if let Some(window) = event.window(mtm) {
        // Performed after a delay of zero, so egui finishes this frame before
        // AppKit starts resizing the window.
        // SAFETY: both NSWindow selectors take one optional sender argument.
        unsafe {
            match super::double_click_action() {
                super::DoubleClick::Fill | super::DoubleClick::Nothing => {}
                super::DoubleClick::Minimize => window.performSelector_withObject_afterDelay(
                    sel!(performMiniaturize:),
                    None,
                    0.0,
                ),
                super::DoubleClick::Zoom => {
                    window.performSelector_withObject_afterDelay(sel!(performZoom:), None, 0.0)
                }
            }
        }
    }
    false
}
