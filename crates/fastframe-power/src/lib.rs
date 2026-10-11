//! The device's power, session and network state for an egui app.
//!
//! [`Power`] starts a thin reader per platform that writes what the system
//! reports into shared [`Conditions`] (energy saver, battery, display, lock,
//! the network's reach and cost) and queues [`Event`]s (sleep, wake, the end
//! of the session), then calls the app's wake function. The app reads both
//! in its logic pass, which runs with or without a window, and decides for
//! itself what to do with them: draw less, poll less, hold a download, save
//! before sleeping.
//!
//! [`Activity`] keeps work the user is waiting for (audio playing behind
//! another app's window) out of App Nap on macOS.
//!
//! ```no_run
//! use fastframe_power::{Event, Power};
//!
//! let power = Power::spawn(|| {
//!     // Wake the app: the conditions changed, or an event is waiting.
//! });
//!
//! // In the logic pass:
//! let conditions = power.conditions();
//! let animate = !conditions.saver && !conditions.display_off;
//! for event in power.events() {
//!     match event {
//!         Event::Suspending(ack) | Event::EndingSession(ack) => {
//!             // Save, then:
//!             ack.done();
//!         }
//!         Event::Resumed => { /* reconnect what the sleep broke */ }
//!     }
//! }
//! # let _ = animate;
//! ```

use std::sync::{Arc, Condvar, Mutex, PoisonError};
#[cfg(any(target_os = "windows", target_os = "linux", test))]
use std::time::Duration;
use std::time::SystemTime;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod map;
#[cfg(test)]
mod tests;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use self::linux::Reader;
#[cfg(target_os = "macos")]
use self::macos::Reader;
#[cfg(target_os = "windows")]
use self::windows::Reader;

/// What the system says about power, the display, the session and the
/// network. A platform that cannot tell leaves the default, which is the
/// unconstrained case: an app that reads these never holds anything back
/// because a platform says nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conditions {
    /// The user asked the system to save energy: Energy Saver or battery
    /// saver on Windows, Low Power Mode on macOS, the power-saver profile on
    /// Linux.
    pub saver: bool,
    /// Running on battery (Windows).
    pub on_battery: bool,
    /// Every display is off (Windows).
    pub display_off: bool,
    /// The session is locked, or switched away from or disconnected
    /// (Windows).
    pub locked: bool,
    /// The system reports a way to the internet (Windows and macOS).
    pub online: bool,
    /// The connection is metered or charged by use, roaming, or over its
    /// data limit (Windows).
    pub metered: bool,
}

impl Default for Conditions {
    fn default() -> Self {
        Self {
            saver: false,
            on_battery: false,
            display_off: false,
            locked: false,
            online: true,
            metered: false,
        }
    }
}

/// Something the system is about to do, or has just done.
#[derive(Debug)]
pub enum Event {
    /// The system is about to sleep. Save, then [`Ack::done`].
    Suspending(Ack),
    /// The system woke up. A connection that was open across the sleep can
    /// still look established while nothing arrives through it, so this is
    /// the moment to reconnect it, rather than waiting for a timeout or
    /// guessing the sleep from the wall clock running ahead of
    /// [`Instant`](std::time::Instant). Reported on Linux and Windows, not
    /// yet on macOS.
    Resumed,
    /// The user is signing out or shutting down. Save, then [`Ack::done`].
    /// Reported on Windows.
    EndingSession(Ack),
}

/// Holds back the system, for a bounded time, until the app has handled an
/// event the system will not wait long for.
///
/// The reader waits at most 1.5 seconds before sleep and 3 seconds before
/// the end of the session, then lets the system go on whether or not the
/// app answered. `Ack::default()` makes one for an [`Event`] a test sends
/// with [`Power::send`].
#[derive(Clone, Default)]
pub struct Ack(Arc<(Mutex<bool>, Condvar)>);

/// Events are logged; the lock and the condition variable inside say
/// nothing worth reading there.
impl std::fmt::Debug for Ack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ack")
    }
}

impl Ack {
    /// The app has done what the event asked.
    pub fn done(&self) {
        let (done, ring) = &*self.0;
        *done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        ring.notify_all();
    }

    /// Whether the app has answered, for an app's tests to check that it
    /// answers the events it is sent.
    pub fn is_done(&self) -> bool {
        *self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until [`done`](Self::done) or `timeout`, whichever is first.
    /// Returns whether the app answered in time.
    #[cfg(any(target_os = "windows", target_os = "linux", test))]
    fn wait(&self, timeout: Duration) -> bool {
        let (done, ring) = &*self.0;
        let done = done.lock().unwrap_or_else(PoisonError::into_inner);
        let (done, _) = ring
            .wait_timeout_while(done, timeout, |done| !*done)
            .unwrap_or_else(PoisonError::into_inner);
        *done
    }
}

/// How long a reader waits for the app to save before the system sleeps.
/// Windows gives a suspend callback about two seconds.
#[cfg(any(target_os = "windows", target_os = "linux"))]
const SUSPEND_ACK: Duration = Duration::from_millis(1500);
/// How long a reader holds back the end of the session for the app to save.
/// Windows waits about five seconds before it lists the app as blocking.
#[cfg(target_os = "windows")]
const END_SESSION_ACK: Duration = Duration::from_secs(3);

/// Run by a reader itself as the system is about to sleep.
type Hook = Arc<dyn Fn() + Send + Sync>;

/// What the readers write and the app reads.
struct Shared {
    conditions: Mutex<Conditions>,
    events: Mutex<Vec<Event>>,
    wake: Box<dyn Fn() + Send + Sync>,
    on_suspending: Mutex<Option<Hook>>,
    /// When the system last said it was going to sleep, by the wall clock,
    /// which keeps counting through it. A macOS reader never sets it, since
    /// it reports no sleep yet.
    suspended_at: Mutex<Option<SystemTime>>,
}

impl Shared {
    fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            conditions: Mutex::new(Conditions::default()),
            events: Mutex::new(Vec::new()),
            wake: Box::new(wake),
            on_suspending: Mutex::new(None),
            suspended_at: Mutex::new(None),
        }
    }

    /// Applies `change` and wakes the app if it changed anything.
    fn update(&self, change: impl FnOnce(&mut Conditions)) {
        let mut conditions = self
            .conditions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let before = *conditions;
        change(&mut conditions);
        let after = *conditions;
        drop(conditions);
        if after != before {
            log::info!("power and session: {after:?}");
            (self.wake)();
        }
    }

    /// What cannot wait for the app's logic pass as the system is about to
    /// sleep: notes the time and runs the app's hook.
    fn begin_suspending(&self) {
        *self
            .suspended_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(SystemTime::now());
        // Run outside the lock, so a hook that sets another, or a call from
        // elsewhere meanwhile, does not wait on it.
        let hook = self
            .on_suspending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        // A panicking hook must not end a reader thread, or unwind through a
        // Windows callback, which aborts: the event is still queued and the
        // reader goes on.
        if let Some(hook) = hook
            && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook())).is_err()
        {
            log::warn!("power and session: the suspending hook panicked");
        }
    }

    /// Queues `event` and wakes the app.
    fn push(&self, event: Event) {
        log::info!("power and session: {event:?}");
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        (self.wake)();
    }

    /// Queues the event `make` builds and waits up to `timeout` for the app
    /// to answer it.
    #[cfg(any(target_os = "windows", target_os = "linux", test))]
    fn push_and_wait(&self, make: impl FnOnce(Ack) -> Event, timeout: Duration) {
        let ack = Ack::default();
        let event = make(ack.clone());
        // The app's logic pass may not run in time while the display goes
        // off, so this happens here, before the event waits for it.
        if matches!(event, Event::Suspending(_)) {
            self.begin_suspending();
        }
        self.push(event);
        if !ack.wait(timeout) {
            log::warn!("power and session: the app did not answer within {timeout:?}");
        }
    }
}

/// Watches the system's power, session and network state for the app.
///
/// Readers run on threads of their own (Linux, the Windows session window)
/// or on the system's own threads (Windows callbacks, macOS's main dispatch
/// queue), so a slow or absent service never holds up the app.
pub struct Power {
    shared: Arc<Shared>,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    _reader: Option<Reader>,
    #[cfg(target_os = "windows")]
    eco_qos: windows::EcoQos,
}

impl Power {
    /// Starts the platform's readers. `wake` runs on a reader's thread
    /// whenever the conditions change or an event is queued; it should ask
    /// for the app's next logic pass (a repaint, or a headless tick), and
    /// return at once.
    ///
    /// What a platform cannot report keeps its default, and a reader that
    /// cannot start is logged and left out: the app still gets a `Power`.
    pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared::new(wake));
        Self {
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            _reader: Reader::start(Arc::clone(&shared))
                .inspect_err(|error| log::warn!("power and session state unavailable: {error}"))
                .ok(),
            #[cfg(target_os = "windows")]
            eco_qos: windows::EcoQos::default(),
            shared,
        }
    }

    /// Conditions that only change through
    /// [`set_conditions`](Self::set_conditions), with no reader: for tests,
    /// demo runs, and an app that does not want the system's state. The
    /// device's own state (Low Power Mode on a Mac, say) never leaks in.
    pub fn fixed(conditions: Conditions) -> Self {
        let shared = Arc::new(Shared::new(|| {}));
        *shared
            .conditions
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = conditions;
        Self {
            shared,
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            _reader: None,
            #[cfg(target_os = "windows")]
            eco_qos: windows::EcoQos::default(),
        }
    }

    /// The latest conditions.
    ///
    /// On macOS, Low Power Mode is read here, one message to NSProcessInfo,
    /// rather than followed through a notification, so call this from the
    /// logic pass and a change shows up on the next one. A change found here
    /// wakes the app like any other.
    pub fn conditions(&self) -> Conditions {
        #[cfg(target_os = "macos")]
        if self._reader.is_some() {
            macos::read_low_power(&self.shared);
        }
        *self
            .shared
            .conditions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The events since the last call, oldest first. Call it in every logic
    /// pass, window or not: a reader holds the system back until the app
    /// answers an [`Ack`], or for a short while at most.
    pub fn events(&self) -> Vec<Event> {
        std::mem::take(
            &mut *self
                .shared
                .events
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Tells the system whether the app runs where nobody can see it (no
    /// window shown, the display off or the session locked), so the work it
    /// still does can run on efficient cores at efficient clocks: EcoQoS on
    /// Windows. Microsoft asks not to throttle a window in the foreground,
    /// so turn it off again as soon as one is shown.
    ///
    /// If something else manages the process's throttling (the Task
    /// Manager's efficiency mode, say), it is left alone for the rest of the
    /// run. Only a watching `Power` touches the process; on Linux and macOS,
    /// and on a fixed `Power`, this does nothing.
    pub fn set_background(&mut self, background: bool) {
        #[cfg(target_os = "windows")]
        if self._reader.is_some() {
            self.eco_qos.set(background);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = background;
    }

    /// Has the reader run `hook` itself as the system is about to sleep,
    /// before the app's logic pass gets to [`Event::Suspending`], which it
    /// may not do in time while the display goes off. For work that must
    /// not wait, such as pausing playback. A later call replaces the hook.
    ///
    /// The hook runs on a reader's thread, or on the system's own for a
    /// Windows callback, while the system waits, so keep it short: one that
    /// blocks holds back the sleep and the events the reader reports after
    /// it. A panic in it is logged and the sleep is still queued, where
    /// panics unwind; under `panic = "abort"` it aborts the process as any
    /// panic would. Do not capture the `Power` itself, which would keep it
    /// from ever being dropped.
    ///
    /// Run on Linux and Windows, where a reader reports the sleep; macOS
    /// reports none yet, so there only [`send`](Self::send) runs it.
    pub fn on_suspending(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self
            .shared
            .on_suspending
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(hook));
    }

    /// When the system last said it was going to sleep, by the wall clock,
    /// taken once. The reader notes it as the system says so, before it
    /// waits up to 1.5 seconds for the app's answer, however late the
    /// app's logic pass reads the event. On waking, the time since then
    /// tells how long the machine slept, give or take that wait, where
    /// [`Instant`](std::time::Instant) cannot: it keeps counting through
    /// sleep on Windows. The wall clock can also be set back meanwhile, so
    /// handle [`SystemTime::duration_since`] failing. `None` before any
    /// sleep. A macOS reader reports none yet, so there only a sleep
    /// delivered with [`send`](Self::send) sets it.
    pub fn take_suspended_at(&self) -> Option<SystemTime> {
        self.shared
            .suspended_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Replaces the conditions and wakes the app if they changed, as a
    /// reader would: for tests and demo runs on a [`fixed`](Self::fixed)
    /// `Power`. On a watching one the readers overwrite what they follow
    /// with the next change they see.
    pub fn set_conditions(&self, conditions: Conditions) {
        self.shared.update(|current| *current = conditions);
    }

    /// Queues `event` for [`events`](Self::events) and wakes the app, as a
    /// reader would: for tests and demo runs. For [`Event::Suspending`] it
    /// also notes the time and runs the [`on_suspending`](Self::on_suspending)
    /// hook first, as a reader does. Nothing waits for its [`Ack`].
    pub fn send(&self, event: Event) {
        if matches!(event, Event::Suspending(_)) {
            self.shared.begin_suspending();
        }
        self.shared.push(event);
    }
}

/// Keeps the app's work out of App Nap on macOS while it is held, and ends
/// when dropped.
///
/// macOS slows down an app whose windows are covered, hidden or on another
/// space while another app is in front: its timers fire late and its threads
/// get less time. Hold an `Activity` while the app does work the user is
/// waiting for that has to keep pace, above all audio playing or loading
/// while the app is not frontmost, and drop it on pause, stop, an output
/// error or a disconnection, so a paused app naps again. It asks for a
/// user-initiated activity that still lets the system sleep when idle and
/// the display turn off; it does not keep the Mac awake.
///
/// Keep it in an `Option<Activity>` and begin one only when there is none,
/// so repeated logic passes hold the same activity rather than stacking new
/// ones. Linux and Windows have no App Nap: there, an `Activity` does
/// nothing.
/// <https://developer.apple.com/documentation/foundation/processinfo/activityoptions/userinitiatedallowingidlesystemsleep>
#[must_use = "the activity ends when it is dropped"]
pub struct Activity {
    #[cfg(target_os = "macos")]
    _token: macos::Activity,
}

impl Activity {
    /// Begins an activity. `reason` is what the system shows for it (in
    /// `pmset -g assertions`, say), such as "Playing audio".
    pub fn begin(reason: &str) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                _token: macos::Activity::begin(reason),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reason;
            Self {}
        }
    }
}

impl std::fmt::Debug for Activity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Activity")
    }
}
