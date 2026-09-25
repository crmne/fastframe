//! The macOS menu-bar item, the Dock reopen handler, and the headless AppKit
//! pump.
//!
//! AppKit allows status items on the main thread only, and only while its
//! event loop runs, so the item is made with the first window, and [`pump`]
//! runs the loop while no window exists.

// The Objective-C runtime is FFI, and `define_class!` expands to unsafe code.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::CString;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, MethodImplementation, NSObject, Sel};
use objc2::{Encode, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSObjectNSDelayedPerforming, NSPoint};

use crate::native::Item;
use crate::{Config, Event, Router};

thread_local! {
    /// The status item, which only the main thread may touch.
    static ITEM: RefCell<Option<Item>> = const { RefCell::new(None) };
    /// Where a Dock click asks for the window.
    static REOPEN: RefCell<Option<Router>> = const { RefCell::new(None) };
}

pub(crate) struct Host {
    /// What the item needs, until the first window lets it be made.
    pending: Option<(Config, Router)>,
}

impl Host {
    /// Keeps the settings until AppKit runs with the first window.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the same signature as the other platforms' hosts"
    )]
    pub(crate) fn start(config: Config, router: Router) -> Result<Self, String> {
        Ok(Self {
            pending: Some((config, router)),
        })
    }

    pub(crate) fn set_label(&mut self, id: &str, label: String) {
        if let Some((config, _)) = &mut self.pending {
            crate::set_label(&mut config.menu, id, label);
            return;
        }
        ITEM.with(|slot| {
            if let Some(item) = slot.borrow().as_ref() {
                item.set_label(id, &label);
            }
        });
    }

    /// Makes the item if this is the first window, and brings the app
    /// forward.
    pub(crate) fn attach(&mut self) {
        if let Some((config, router)) = self.pending.take() {
            create(&config, router);
        }
        if ITEM.with(|slot| slot.borrow().is_some()) {
            activate();
        }
    }
}

/// Makes the item, once, on the main thread.
fn create(config: &Config, router: Router) {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("the status item can only be made on the main thread");
        return;
    };
    REOPEN.with(|slot| *slot.borrow_mut() = Some(router.clone()));
    install_reopen_handler(&NSApplication::sharedApplication(mtm));
    match crate::native::build(config, router) {
        Ok(item) => ITEM.with(|slot| *slot.borrow_mut() = Some(item)),
        Err(error) => log::info!("no status item: {error}"),
    }
}

fn activate() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    #[allow(deprecated, reason = "its replacement needs macOS 14")]
    app.activateIgnoringOtherApps(true);
}

/// A Dock click, asking for the app back.
///
/// `has_visible_windows` is not the question it sounds like: a window
/// sitting in the Dock counts as visible, which is exactly the case that
/// needs help, so the flag is not consulted (Spotifast ec75951). Asking for
/// a window that is already up costs a focus and nothing else. The wake
/// matters too: a minimized window draws no frames, so without one nobody
/// would read the event.
fn request_reopen(_has_visible_windows: bool) -> Bool {
    REOPEN.with(|slot| {
        if let Some(router) = slot.borrow().as_ref() {
            router.send(Event::Show);
        }
    });
    Bool::YES
}

extern "C-unwind" fn application_should_handle_reopen(
    _delegate: *mut AnyObject,
    _selector: Sel,
    _application: *mut NSApplication,
    has_visible_windows: Bool,
) -> Bool {
    request_reopen(has_visible_windows.as_bool())
}

/// Adds `applicationShouldHandleReopen:hasVisibleWindows:` to winit's
/// application delegate, unless it already answers it.
fn install_reopen_handler(app: &NSApplication) {
    let Some(delegate) = app.delegate() else {
        log::warn!("the macOS application delegate is unavailable");
        return;
    };
    let delegate: &AnyObject = AsRef::<AnyObject>::as_ref(&*delegate);
    let class = delegate.class();
    let selector = sel!(applicationShouldHandleReopen:hasVisibleWindows:);
    if class.responds_to(selector) {
        return;
    }
    let implementation: extern "C-unwind" fn(
        *mut AnyObject,
        Sel,
        *mut NSApplication,
        Bool,
    ) -> Bool = application_should_handle_reopen;
    let Ok(types) = CString::new(format!("{}@:@{}", Bool::ENCODING, Bool::ENCODING)) else {
        return;
    };
    // SAFETY: the implementation's signature matches the type encoding, and
    // the selector is not yet on the class, so nothing is replaced.
    let installed = unsafe {
        objc2::ffi::class_addMethod(
            std::ptr::from_ref::<AnyClass>(class).cast_mut(),
            selector,
            implementation.__imp(),
            types.as_ptr(),
        )
    };
    if !installed.as_bool() {
        log::warn!("the macOS Dock reopen handler could not be installed");
    }
}

define_class!(
    /// Ends a headless `-[NSApplication run]` when its time slice is over.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "FastframeTrayHeadlessStop"]
    struct HeadlessStop;

    impl HeadlessStop {
        #[unsafe(method(stopRun:))]
        fn stop_run(&self, _sender: Option<&AnyObject>) {
            let app = NSApplication::sharedApplication(self.mtm());
            app.stop(None);
            // `stop:` takes effect after the next event, and a timer is not
            // one, so post a no-op event for `run` to return on.
            if let Some(event) =
                NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                    NSEventType::ApplicationDefined,
                    NSPoint::new(0.0, 0.0),
                    NSEventModifierFlags::empty(),
                    0.0,
                    0,
                    None,
                    0,
                    0,
                    0,
                )
            {
                app.postEvent_atStart(&event, true);
            }
        }
    }
);

impl HeadlessStop {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `NSObject`'s designated initializer, called once.
        unsafe { msg_send![super(this), init] }
    }
}

/// Runs AppKit's event loop for `duration` while no window exists.
///
/// `-[NSApplication run]` catches an Objective-C exception raised while an
/// event is handled and reports it, as it does while a window is open. A
/// hand-written `nextEventMatchingMask:`/`sendEvent:` loop let such an
/// exception unwind into Rust, which aborts the process (ZapFast #199,
/// 655509b).
pub(crate) fn pump(duration: Duration) {
    let Some(mtm) = MainThreadMarker::new() else {
        std::thread::sleep(duration);
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let stop = HeadlessStop::new(mtm);
    // SAFETY: `stopRun:` is defined above and accepts a nil sender.
    unsafe {
        stop.performSelector_withObject_afterDelay(sel!(stopRun:), None, duration.as_secs_f64());
    }
    app.run();
    // Another `stop:` may end the run early; a leftover request must not stop
    // the next window's event loop.
    // SAFETY: `stop` is the target the request was scheduled on.
    unsafe { NSObject::cancelPreviousPerformRequestsWithTarget(&stop) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A Dock click asks for the window whatever AppKit says about visible
    /// ones, and wakes the app so a minimized window reads it.
    #[test]
    fn a_dock_click_asks_for_the_window_and_for_a_frame() {
        let (sender, events) = std::sync::mpsc::channel();
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let router = Router::new(
            sender,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            &[],
        );
        REOPEN.with(|slot| *slot.borrow_mut() = Some(router));

        assert!(request_reopen(true).as_bool());
        assert_eq!(
            events.try_recv(),
            Ok(Event::Show),
            "a window in the Dock reports as visible"
        );
        assert_eq!(woken.load(Ordering::SeqCst), 1);

        assert!(request_reopen(false).as_bool());
        assert_eq!(events.try_recv(), Ok(Event::Show));
        assert_eq!(woken.load(Ordering::SeqCst), 2);
        REOPEN.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn labels_set_before_the_item_exists_are_kept_for_it() {
        let (sender, _events) = std::sync::mpsc::channel();
        let menu = vec![crate::MenuItem::action("play", "Play")];
        let router = Router::new(sender, Arc::new(|| {}), &menu);
        let config = Config {
            id: "spotifast",
            title: "Spotifast".into(),
            icon: |size| vec![0; size * size * 4],
            template_icon: None,
            menu,
        };
        let mut host = Host::start(config, router).unwrap();
        host.set_label("play", "Pause".into());
        let (config, _) = host.pending.as_ref().unwrap();
        assert_eq!(config.menu[0], crate::MenuItem::action("play", "Pause"));
    }
}
