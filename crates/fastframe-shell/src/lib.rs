//! Keep an egui app running without a window, recreate the window on demand,
//! and bring back windows restored off-screen.
//!
//! A messaging client or a music player keeps working with its window closed:
//! the connection, the playback, the tray item and the notifications live on.
//! eframe ties the event loop to the window, so ZapFast and Spotifast run
//! `eframe::run_native` in a loop. Closing the window with "keep running" on
//! destroys it; the app's state survives in a slot, and a headless loop keeps
//! calling the app until the tray, a notification, or another launch asks for
//! the window again, when a new one is made. [`Shell`] is that loop.
//!
//! ```no_run
//! use fastframe_shell::{Closed, Headless, Resident, Shell, Waker};
//!
//! struct App { quit: bool, hide: bool, show: bool }
//!
//! impl Resident for App {
//!     fn closed(&self) -> Closed {
//!         if !self.quit && self.hide { Closed::Hide } else { Closed::Quit }
//!     }
//!     fn window_gone(&mut self) { self.hide = false; self.show = false; }
//!     fn headless_frame(&mut self, _ctx: &egui::Context) -> Headless {
//!         // Drain the tray, the network, the timers...
//!         if self.quit { Headless::Quit } else if self.show { Headless::Show } else { Headless::Wait }
//!     }
//!     fn shutdown(&mut self) {}
//! }
//!
//! struct Window { app: fastframe_shell::Held<App> }
//! impl eframe::App for Window {
//!     fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
//! }
//!
//! # fn run_native(_: &str, _: eframe::NativeOptions, _: eframe::AppCreator<'_>) -> eframe::Result<()> { Ok(()) }
//! let waker = Waker::default();
//! let app = App { quit: false, hide: false, show: false };
//! Shell::new(app, &waker).run(|lease| {
//!     run_native(
//!         "App",
//!         eframe::NativeOptions::default(),
//!         Box::new(move |cc| Ok(Box::new(Window { app: lease.take(&cc.egui_ctx) }))),
//!     )
//! })?;
//! # Ok::<(), eframe::Error>(())
//! ```
//!
//! The pieces:
//!
//! - [`Shell`] runs the loop around the app's own `run_native` call, starts
//!   hidden when asked, and idles between headless ticks through a function
//!   the app supplies (the tray crate's `idle`, which serves AppKit on macOS).
//! - [`Waker`] lets threads repaint whichever window exists, and does nothing
//!   while none does.
//! - [`window::recover_offscreen`] moves a window that opened on no connected
//!   monitor onto the primary one.
//!
//! Single-instance handling is not here: the apps use three different designs
//! (see the README).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub mod window;

/// How often the headless loop calls [`Resident::headless_frame`] while the
/// app waits in the background.
pub const HEADLESS_TICK: Duration = Duration::from_millis(150);

/// What an app that outlives its window tells the [`Shell`].
pub trait Resident {
    /// What to do once the window has closed.
    fn closed(&self) -> Closed;

    /// The window is gone and the app is about to run headless. Reset what
    /// asked for the window to close or reopen, and drop per-window state.
    fn window_gone(&mut self);

    /// One headless tick: do the work a window's frame would have done
    /// (drain the tray and backend events, run timers), then say whether to
    /// keep waiting, open a window, or quit.
    ///
    /// `ctx` is a context with no window behind it, reused for the whole
    /// headless stretch.
    fn headless_frame(&mut self, ctx: &egui::Context) -> Headless;

    /// Starts without a window, when the launch asked for that.
    ///
    /// Return `false` when there would be no way back to the window (no tray
    /// item, or "keep running" off): a window then opens as usual.
    ///
    /// Anything that waits for a window's first frame must be released here.
    /// ZapFast's backend waited for one before it opened its archive and
    /// connected, so a hidden start at login did nothing until the window
    /// was shown (ZapFast 5da9d38).
    fn start_hidden(&mut self) -> bool {
        false
    }

    /// The app is about to end: stop threads and save what must survive.
    fn shutdown(&mut self);
}

/// What happens after a window closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    /// End the app.
    Quit,
    /// Keep running without a window until [`Headless::Show`].
    Hide,
    /// Open a new window at once (a different kind of window, say).
    Reopen,
}

/// What a headless tick asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Headless {
    /// Keep running in the background.
    Wait,
    /// Open a window.
    Show,
    /// End the app.
    Quit,
}

/// The app's state between windows, and the loop that recreates them.
#[must_use = "the app does not run until `run` is called"]
pub struct Shell<A> {
    app: A,
    waker: Waker,
    start_hidden: bool,
    idle: fn(Duration),
}

impl<A: Resident> Shell<A> {
    /// A shell for `app`. `waker` is attached to each window as it is made
    /// and detached when it closes.
    pub fn new(app: A, waker: &Waker) -> Self {
        Self {
            app,
            waker: waker.clone(),
            start_hidden: false,
            idle: std::thread::sleep,
        }
    }

    /// Starts without a window if [`Resident::start_hidden`] agrees.
    pub fn start_hidden(mut self, hidden: bool) -> Self {
        self.start_hidden = hidden;
        self
    }

    /// Waits between headless ticks with `idle` instead of sleeping.
    ///
    /// On macOS a status item only answers while AppKit's event loop runs, so
    /// an app with a tray passes `fastframe_tray::idle` here.
    pub fn idle(mut self, idle: fn(Duration)) -> Self {
        self.idle = idle;
        self
    }

    /// Runs the app until it quits, then calls [`Resident::shutdown`].
    ///
    /// `open_window` makes one window and returns when it closes: it calls
    /// `eframe::run_native`, and its app creator calls [`Lease::take`] with
    /// the new context, keeping the [`Held`] app in its `eframe::App`. When
    /// that is dropped with the window, the app returns here.
    ///
    /// # Errors
    ///
    /// Whatever `open_window` returns. The app is then dropped without
    /// [`Resident::shutdown`], as `?` on `run_native` did before.
    pub fn run<E>(self, mut open_window: impl FnMut(Lease<A>) -> Result<(), E>) -> Result<(), E> {
        let Self {
            app,
            waker,
            start_hidden,
            idle,
        } = self;
        let slot = Rc::new(RefCell::new(Some(app)));
        let mut hidden_start = start_hidden;
        'windows: loop {
            let closed = if std::mem::take(&mut hidden_start)
                && with_app(&slot, Resident::start_hidden) == Some(true)
            {
                Closed::Hide
            } else {
                open_window(Lease {
                    slot: Rc::clone(&slot),
                    waker: waker.clone(),
                })?;
                waker.detach();
                match with_app(&slot, |app| app.closed()) {
                    Some(closed) => closed,
                    None => {
                        // The window never handed the app back (a leaked
                        // eframe app). There is nothing left to run.
                        log::error!("the window closed without returning the app state");
                        return Ok(());
                    }
                }
            };
            match closed {
                Closed::Quit => break,
                Closed::Reopen => continue,
                Closed::Hide => {}
            }

            let headless = egui::Context::default();
            with_app(&slot, Resident::window_gone);
            loop {
                match with_app(&slot, |app| app.headless_frame(&headless)) {
                    Some(Headless::Wait) => idle(HEADLESS_TICK),
                    Some(Headless::Show) => continue 'windows,
                    Some(Headless::Quit) | None => break 'windows,
                }
            }
        }
        if let Some(mut app) = slot.borrow_mut().take() {
            app.shutdown();
        }
        Ok(())
    }
}

fn with_app<A, T>(slot: &RefCell<Option<A>>, f: impl FnOnce(&mut A) -> T) -> Option<T> {
    slot.borrow_mut().as_mut().map(f)
}

/// The right to take the app into one window. See [`Shell::run`].
pub struct Lease<A> {
    slot: Rc<RefCell<Option<A>>>,
    waker: Waker,
}

impl<A> Lease<A> {
    /// Takes the app for the window whose context is `ctx`, attaching the
    /// [`Waker`] to it. Call it from eframe's app creator.
    ///
    /// # Panics
    ///
    /// If the app is already held elsewhere, which [`Shell`] never allows.
    pub fn take(self, ctx: &egui::Context) -> Held<A> {
        self.waker.attach(ctx);
        let app = self
            .slot
            .borrow_mut()
            .take()
            .expect("the shell hands out one lease per window");
        Held {
            app: Some(app),
            slot: self.slot,
        }
    }
}

/// The app, held by one window. Dropping it (with the window's
/// `eframe::App`) hands the app back to the [`Shell`].
pub struct Held<A> {
    app: Option<A>,
    slot: Rc<RefCell<Option<A>>>,
}

impl<A> std::ops::Deref for Held<A> {
    type Target = A;

    fn deref(&self) -> &A {
        self.app.as_ref().expect("held until dropped")
    }
}

impl<A> std::ops::DerefMut for Held<A> {
    fn deref_mut(&mut self) -> &mut A {
        self.app.as_mut().expect("held until dropped")
    }
}

impl<A> Drop for Held<A> {
    fn drop(&mut self) {
        if let Some(app) = self.app.take() {
            *self.slot.borrow_mut() = Some(app);
        }
    }
}

/// Repaints the current window from any thread, and does nothing while there
/// is no window.
///
/// Background threads and the tray hold this instead of an `egui::Context`,
/// because the context changes with every window and there is none while the
/// app runs headless.
#[derive(Clone, Default)]
pub struct Waker(Arc<Mutex<Option<egui::Context>>>);

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Waker")
            .field("attached", &self.context().is_some())
            .finish()
    }
}

impl Waker {
    /// Repaints `ctx` from now on. [`Lease::take`] does this for each window.
    pub fn attach(&self, ctx: &egui::Context) {
        *self.lock() = Some(ctx.clone());
    }

    /// Stops repainting: the window is gone. [`Shell`] does this when a
    /// window closes.
    pub fn detach(&self) {
        *self.lock() = None;
    }

    /// Asks the window, if there is one, for a new frame.
    pub fn wake(&self) {
        if let Some(ctx) = self.context() {
            ctx.request_repaint();
        }
    }

    /// Asks the window, if there is one, for a frame after `delay`.
    pub fn wake_after(&self, delay: Duration) {
        if let Some(ctx) = self.context() {
            ctx.request_repaint_after(delay);
        }
    }

    fn context(&self) -> Option<egui::Context> {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<egui::Context>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static IDLED: Cell<u32> = const { Cell::new(0) };
        /// What the last dropped `Script` was asked, in order.
        static CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    /// A scripted app that records what the shell asked of it.
    struct Script {
        calls: Vec<&'static str>,
        closes: Vec<Closed>,
        /// What the current window closed with.
        now: Closed,
        ticks: Vec<Headless>,
        can_hide: bool,
        released: bool,
    }

    impl Resident for Script {
        fn closed(&self) -> Closed {
            self.now
        }
        fn window_gone(&mut self) {
            self.calls.push("window_gone");
        }
        fn headless_frame(&mut self, _ctx: &egui::Context) -> Headless {
            self.calls.push("tick");
            if self.ticks.is_empty() {
                Headless::Quit
            } else {
                self.ticks.remove(0)
            }
        }
        fn start_hidden(&mut self) -> bool {
            self.calls.push("start_hidden");
            self.released = self.can_hide;
            self.can_hide
        }
        fn shutdown(&mut self) {
            self.calls.push("shutdown");
        }
    }

    fn script(closes: &[Closed], ticks: &[Headless], can_hide: bool) -> Script {
        Script {
            closes: closes.to_vec(),
            ticks: ticks.to_vec(),
            can_hide,
            now: Closed::Quit,
            calls: Vec::new(),
            released: false,
        }
    }

    impl Drop for Script {
        fn drop(&mut self) {
            CALLS.with(|calls| *calls.borrow_mut() = std::mem::take(&mut self.calls));
        }
    }

    fn count_idle(duration: Duration) {
        assert_eq!(duration, HEADLESS_TICK);
        IDLED.with(|idled| idled.set(idled.get() + 1));
    }

    fn calls() -> Vec<&'static str> {
        CALLS.with(|calls| calls.borrow().clone())
    }

    /// Opens "windows" that take the app, let `during` act on it, and hand it
    /// back. Returns how many windows opened.
    fn run(shell: Shell<Script>, mut during: impl FnMut(&mut Script)) -> u32 {
        IDLED.with(|idled| idled.set(0));
        let mut windows = 0;
        shell
            .idle(count_idle)
            .run(|lease| {
                windows += 1;
                let mut held = lease.take(&egui::Context::default());
                held.calls.push("window");
                during(&mut held);
                held.now = if held.closes.is_empty() {
                    Closed::Quit
                } else {
                    held.closes.remove(0)
                };
                Ok::<(), ()>(())
            })
            .unwrap();
        windows
    }

    #[test]
    fn a_closed_window_without_keep_running_quits() {
        let waker = Waker::default();
        assert_eq!(run(Shell::new(script(&[], &[], false), &waker), |_| {}), 1);
        assert_eq!(calls(), ["window", "shutdown"]);
    }

    #[test]
    fn hiding_runs_headless_until_asked_for_a_window_then_quits_with_it() {
        let waker = Waker::default();
        let script = script(
            &[Closed::Hide, Closed::Quit],
            &[Headless::Wait, Headless::Wait, Headless::Show],
            false,
        );
        assert_eq!(run(Shell::new(script, &waker), |_| {}), 2);
        assert_eq!(
            calls(),
            [
                "window",
                "window_gone",
                "tick",
                "tick",
                "tick",
                "window",
                "shutdown"
            ]
        );
        assert_eq!(IDLED.with(Cell::get), 2, "idle between waiting ticks only");
    }

    #[test]
    fn quitting_from_the_background_shuts_down_without_a_window() {
        let waker = Waker::default();
        let script = script(&[Closed::Hide], &[Headless::Wait, Headless::Quit], false);
        assert_eq!(run(Shell::new(script, &waker), |_| {}), 1);
        assert_eq!(
            calls(),
            ["window", "window_gone", "tick", "tick", "shutdown"]
        );
    }

    #[test]
    fn reopen_makes_the_next_window_at_once() {
        let waker = Waker::default();
        let script = script(&[Closed::Reopen, Closed::Quit], &[], false);
        assert_eq!(run(Shell::new(script, &waker), |_| {}), 2);
        assert_eq!(calls(), ["window", "window", "shutdown"]);
    }

    /// The ZapFast 5da9d38 case: a hidden start goes straight to the
    /// background, and the app's `start_hidden` is where it releases what
    /// would otherwise wait for a first frame.
    #[test]
    fn a_hidden_start_opens_no_window_and_lets_the_app_start_its_work() {
        let waker = Waker::default();
        let script = script(&[], &[Headless::Wait, Headless::Show], true);
        let mut released_before_window = None;
        let windows = run(Shell::new(script, &waker).start_hidden(true), |app| {
            released_before_window = Some(app.released);
        });
        assert_eq!(windows, 1, "the window opens only when asked for");
        assert_eq!(released_before_window, Some(true));
        assert_eq!(
            calls(),
            [
                "start_hidden",
                "window_gone",
                "tick",
                "tick",
                "window",
                "shutdown"
            ]
        );
    }

    #[test]
    fn a_hidden_start_without_a_way_back_opens_the_window() {
        let waker = Waker::default();
        let shell = Shell::new(script(&[], &[], false), &waker).start_hidden(true);
        assert_eq!(run(shell, |_| {}), 1);
        assert_eq!(calls(), ["start_hidden", "window", "shutdown"]);
    }

    #[test]
    fn a_window_error_ends_the_run_without_shutdown() {
        let waker = Waker::default();
        let result = Shell::new(script(&[], &[], false), &waker).run(|_lease| Err("no display"));
        assert_eq!(result, Err("no display"));
        assert!(!calls().contains(&"shutdown"));
    }

    #[test]
    fn a_leaked_window_ends_the_run() {
        let waker = Waker::default();
        let result = Shell::new(script(&[], &[], false), &waker).run(|lease| {
            std::mem::forget(lease.take(&egui::Context::default()));
            Ok::<(), ()>(())
        });
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn the_waker_follows_the_window_and_is_quiet_without_one() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let ctx = egui::Context::default();
        let seen = Arc::clone(&requests);
        ctx.set_request_repaint_callback(move |info| seen.lock().unwrap().push(info.delay));

        let waker = Waker::default();
        waker.wake();
        waker.wake_after(Duration::from_secs(1));
        assert!(requests.lock().unwrap().is_empty(), "no window, no repaint");

        let mut attached = None;
        Shell::new(script(&[], &[], false), &waker)
            .run(|lease| {
                let _held = lease.take(&ctx);
                attached = Some(format!("{waker:?}"));
                waker.wake_after(Duration::from_secs(1));
                waker.wake();
                Ok::<(), ()>(())
            })
            .unwrap();
        assert_eq!(attached.as_deref(), Some("Waker { attached: true }"));
        assert_eq!(format!("{waker:?}"), "Waker { attached: false }");
        {
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            // egui subtracts an estimated frame time from the delay.
            assert!(requests[0] > Duration::from_millis(900));
            assert_eq!(requests[1], Duration::ZERO);
        }

        waker.wake();
        assert_eq!(
            requests.lock().unwrap().len(),
            2,
            "detached when the window closed"
        );
    }
}
