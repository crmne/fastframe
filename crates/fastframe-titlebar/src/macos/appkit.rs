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
            let (x, y) = super::button_origin(index, height, button.frame().size.height);
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

/// Each window's latest left mouse-down. A header press takes it, and the
/// window then remembers it was taken until its next mouse-down.
#[derive(Default)]
struct MouseDowns(
    std::collections::HashMap<isize, Option<objc2::rc::Retained<objc2_app_kit::NSEvent>>>,
);

/// What [`MouseDowns::take`] found for a window.
#[derive(Debug)]
enum Kept {
    /// The press, now taken.
    Press(objc2::rc::Retained<objc2_app_kit::NSEvent>),
    /// Already taken by another strip or egui pass.
    Taken,
    /// No mouse-down seen in that window yet.
    Unknown,
}

impl MouseDowns {
    fn record(&mut self, event: &objc2_app_kit::NSEvent) {
        use objc2::Message;

        self.0.insert(event.windowNumber(), Some(event.retain()));
    }

    /// `window`'s press, once.
    fn take(&mut self, window: isize) -> Kept {
        match self.0.get_mut(&window) {
            Some(kept) => kept.take().map_or(Kept::Taken, Kept::Press),
            None => Kept::Unknown,
        }
    }
}

thread_local! {
    static MONITOR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static MOUSE_DOWNS: std::cell::RefCell<MouseDowns> = std::cell::RefCell::default();
}

/// The window number of `frame`'s window.
pub(crate) fn window_number(frame: &eframe::Frame) -> Option<isize> {
    let handle = frame.window_handle().ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: eframe supplies a live NSView, and this runs on its main thread.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    Some(view.window()?.windowNumber())
}

/// See [`super::record_presses`].
pub(crate) fn record_presses() {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventMask};

    if MONITOR.replace(true) {
        return;
    }
    let block = RcBlock::new(|event: NonNull<NSEvent>| {
        // SAFETY: AppKit hands a local monitor a live event.
        let event_ref = unsafe { event.as_ref() };
        MOUSE_DOWNS.with_borrow_mut(|downs| {
            // Forget presses in windows that have since closed.
            if let Some(mtm) = MainThreadMarker::new() {
                let app = NSApplication::sharedApplication(mtm);
                downs
                    .0
                    .retain(|window, _| app.windowWithWindowNumber(*window).is_some());
            }
            downs.record(event_ref);
        });
        event.as_ptr()
    });
    // SAFETY: the handler runs on the main thread and returns the event it
    // was given, unchanged; it never consumes input.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::LeftMouseDown, &block)
    };
    // One monitor for the application's lifetime; AppKit keeps its handler.
    std::mem::forget(monitor);
}

/// See [`super::title_bar_press`]. Ported from Spotifast, which shipped it.
pub(crate) fn title_bar_press(window: Option<isize>, down: bool) -> bool {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::{NSApplication, NSEventType};
    use objc2_foundation::NSObjectNSDelayedPerforming;

    let (event, kept) = match window {
        // Taken, so another pass or another header strip cannot act on it again.
        Some(window) => match MOUSE_DOWNS.with_borrow_mut(|downs| downs.take(window)) {
            Kept::Press(event) => (event, true),
            // Another strip or egui pass already acted on this press: a
            // drag from a later event would follow a zoom, say.
            Kept::Taken => return false,
            Kept::Unknown => return down,
        },
        // A viewport `align_traffic_lights` never named (a deferred one, say)
        // cannot tell its press from another window's, so it keeps reading
        // the current event, as before.
        None => {
            let Some(mtm) = MainThreadMarker::new() else {
                return down;
            };
            match NSApplication::sharedApplication(mtm).currentEvent() {
                Some(event) if event.r#type() == NSEventType::LeftMouseDown => (event, false),
                _ => return down,
            }
        }
    };
    let Some(window) = MainThreadMarker::new().and_then(|mtm| event.window(mtm)) else {
        return false;
    };
    if event.clickCount() != 2 {
        if !kept {
            return down;
        }
        if down {
            // AppKit needs the mouse-down itself. `StartDrag` would pass the
            // current event, which is often a later pressure or drag event.
            window.performWindowDragWithEvent(&event);
        }
        return false;
    }
    // Performed after a delay of zero, so egui finishes this frame before
    // AppKit starts resizing the window.
    // SAFETY: both NSWindow selectors take one optional sender argument.
    unsafe {
        match super::double_click_action() {
            super::DoubleClick::Fill | super::DoubleClick::Nothing => {}
            super::DoubleClick::Minimize => {
                window.performSelector_withObject_afterDelay(sel!(performMiniaturize:), None, 0.0)
            }
            super::DoubleClick::Zoom => {
                window.performSelector_withObject_afterDelay(sel!(performZoom:), None, 0.0)
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::NSPoint;

    use super::{Kept, MOUSE_DOWNS, MouseDowns, title_bar_press};

    // A native value only: never posted.
    fn mouse_down(window: isize, clicks: isize, time: f64) -> Retained<NSEvent> {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDown,
            NSPoint::new(100., 10.),
            NSEventModifierFlags::empty(),
            time,
            window,
            None,
            1,
            clicks,
            0.,
        )
        .expect("mouse event")
    }

    #[test]
    fn a_window_takes_only_its_own_press_once() {
        let mut downs = MouseDowns::default();
        downs.record(&mouse_down(7, 2, 1.));
        downs.record(&mouse_down(8, 1, 2.));
        downs.record(&mouse_down(7, 1, 3.));
        assert!(matches!(downs.take(7), Kept::Press(e) if e.clickCount() == 1));
        assert!(matches!(downs.take(7), Kept::Taken));
        assert!(matches!(downs.take(9), Kept::Unknown));
        assert!(matches!(downs.take(8), Kept::Press(e) if e.clickCount() == 1));
        downs.record(&mouse_down(7, 2, 4.));
        assert!(matches!(downs.take(7), Kept::Press(e) if e.clickCount() == 2));
    }

    #[test]
    fn a_kept_press_is_used_once() {
        MOUSE_DOWNS.with_borrow_mut(|downs| downs.record(&mouse_down(0, 2, 1.)));
        // It has no window here, so nothing is performed, and no drag either.
        assert!(!title_bar_press(Some(0), true));
        // Another pass or strip on the same press does not start a drag.
        assert!(!title_bar_press(Some(0), true));
        // A window with no mouse-down seen falls back to `StartDrag`.
        assert!(title_bar_press(Some(1), true));
        assert!(!title_bar_press(Some(1), false));
    }
}
