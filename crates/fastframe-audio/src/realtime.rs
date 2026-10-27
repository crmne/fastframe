//! Real-time scheduling for the thread the output's callback runs on, on
//! Linux.
//!
//! On macOS the callback runs on CoreAudio's real-time I/O thread, and on
//! Windows the output joins MMCSS (`mmcss.rs`). On Linux cpal calls back from
//! a thread of its own (PipeWire's main loop, the PulseAudio or ALSA
//! worker) with ordinary priority, unless built with its `realtime-dbus`
//! feature, which links the C libdbus. With a 512-frame buffer the callback
//! has about 10 ms each period; on a busy machine an ordinary thread waits
//! longer than that, and the device plays silence (crmne/spotifast#785).
//!
//! So on the first callback of each stream on a device that waits for the
//! hardware ([`plan::realtime_eligible`], cpal's own rule) the output hands
//! that thread's id to a short-lived helper, which asks RTKit
//! (`org.freedesktop.RealtimeKit1` on the system bus, which PipeWire uses
//! for its own threads) to make it real-time, or failing that to raise its
//! nice level. The callback itself never waits for D-Bus. Best effort:
//! without RTKit (a minimal system, a Flatpak sandbox) or when RTKit
//! refuses, the outcome is logged once and the thread carries on as it was.
//!
//! A real-time thread that never waits uses up `RLIMIT_RTTIME` and the
//! kernel kills the whole process (0.10.0 did this on ALSA's `null` PCM).
//! So every callback also checks, window by window, how much of the time its
//! thread ran ([`plan::SpinWindow`]); one that ran for most of a window
//! drops back to ordinary scheduling itself, which any thread may do, long
//! before the limit, and the helper does not promote it afterwards. No
//! signal handler is installed.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use crate::plan;

/// The real-time priority asked for, within RTKit's maximum: below the
/// PipeWire daemon's own threads, as for Firefox and cpal.
const PRIORITY: u32 = 10;
/// The nice level asked for when real-time is refused, within RTKit's
/// minimum: PipeWire's own fallback.
const NICE: i32 = -11;

/// Hands the callback thread to the helper once, and keeps it from spinning
/// at a raised priority.
pub(crate) struct Promoter {
    sender: Option<SyncSender<libc::pid_t>>,
    /// "output" or "input", for the log.
    label: &'static str,
    /// Set once the thread was seen spinning: the helper must not promote it.
    spinning: Arc<AtomicBool>,
    window: plan::SpinWindow,
}

impl Promoter {
    /// For a stream on a device that waits for the hardware, starts the
    /// helper, which waits for the callback's first call and ends with the
    /// stream if that never comes. For any other, only the spin guard.
    pub(crate) fn start(eligible: bool, device: &str, direction: plan::Direction) -> Self {
        let label = direction.label();
        let spinning = Arc::new(AtomicBool::new(false));
        let mut promoter = Self {
            sender: None,
            label,
            spinning: Arc::clone(&spinning),
            window: plan::SpinWindow::default(),
        };
        if !eligible {
            static REPORTED: AtomicBool = AtomicBool::new(false);
            if !REPORTED.swap(true, Ordering::Relaxed) {
                log::info!(
                    "audio {label}: callback thread keeps ordinary priority: {device} may not wait \
                     for the hardware"
                );
            }
            return promoter;
        }
        let (sender, receiver) = sync_channel(1);
        let started = std::thread::Builder::new()
            .name("audio-realtime".into())
            .spawn(move || promote_from(&receiver, &spinning, label));
        match started {
            Ok(_) => promoter.sender = Some(sender),
            Err(error) => {
                log::debug!("audio {label}: cannot start the real-time helper: {error}");
            }
        }
        promoter
    }

    /// Called on every callback. The first sends the thread's id; each one
    /// reads the thread's CPU time and, at the end of a window in which the
    /// thread hardly waited, takes back any raised priority. A few system
    /// calls and a send on a channel with room, so it never blocks.
    pub(crate) fn on_callback(&mut self) {
        if let Some(sender) = self.sender.take() {
            // SAFETY: `gettid` takes nothing and returns the calling thread's id.
            let thread = unsafe { libc::gettid() };
            let _ = sender.try_send(thread);
        }
        if self.window.observe(Instant::now(), thread_cpu_time()) {
            self.spinning.store(true, Ordering::Release);
            if raised() {
                lower();
                static REPORTED: AtomicBool = AtomicBool::new(false);
                if !REPORTED.swap(true, Ordering::Relaxed) {
                    log::warn!(
                        "audio {}: the callback thread never waits for this device; it \
                         keeps ordinary priority",
                        self.label
                    );
                }
            }
        }
    }
}

fn promote_from(receiver: &Receiver<libc::pid_t>, spinning: &AtomicBool, label: &str) {
    let Ok(thread) = receiver.recv() else {
        return;
    };
    if already_real_time(thread) || spinning.load(Ordering::Acquire) {
        return;
    }
    let outcome = rtkit::promote(thread, PRIORITY, NICE);
    static REPORTED: AtomicBool = AtomicBool::new(false);
    let first = !REPORTED.swap(true, Ordering::Relaxed);
    match outcome {
        Ok(Promoted::RealTime(priority)) => {
            if first {
                log::info!("audio {label}: callback thread real-time at priority {priority}");
            }
        }
        Ok(Promoted::Nice(nice)) => {
            if first {
                log::info!("audio {label}: callback thread not real-time; raised to nice {nice}");
            }
        }
        Err(error) => {
            if first {
                log::info!("audio {label}: callback thread keeps ordinary priority: {error}");
            }
        }
    }
}

/// What RTKit granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Promoted {
    RealTime(u32),
    Nice(i32),
}

/// Whether the thread already runs under a real-time policy (a JACK
/// callback, say), which needs nothing more.
fn already_real_time(thread: libc::pid_t) -> bool {
    // SAFETY: reads the scheduling policy of a thread of this process.
    let policy = unsafe { libc::sched_getscheduler(thread) };
    policy == libc::SCHED_FIFO || policy == libc::SCHED_RR
}

/// The CPU time the calling thread has used.
fn thread_cpu_time() -> Duration {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes the calling thread's CPU clock into a valid timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
        return Duration::ZERO;
    }
    Duration::new(
        u64::try_from(time.tv_sec).unwrap_or(0),
        u32::try_from(time.tv_nsec).unwrap_or(0),
    )
}

/// Whether the calling thread runs real-time or above ordinary nice.
fn raised() -> bool {
    // SAFETY: plain reads of the calling thread's own scheduling; `gettid`
    // with PRIO_PROCESS names only this thread on Linux.
    unsafe {
        let policy = libc::sched_getscheduler(0);
        if policy == libc::SCHED_FIFO || policy == libc::SCHED_RR {
            return true;
        }
        *libc::__errno_location() = 0;
        let nice = libc::getpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t);
        nice < 0 && *libc::__errno_location() == 0
    }
}

/// Puts the calling thread back to ordinary scheduling at nice 0. Lowering
/// one's own priority needs no privilege.
fn lower() {
    let param = libc::sched_param { sched_priority: 0 };
    // SAFETY: changes only the calling thread's own policy and nice value.
    unsafe {
        libc::sched_setscheduler(0, libc::SCHED_OTHER, &param);
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 0);
    }
}
/// The `RLIMIT_RTTIME` RTKit requires before it grants real-time: the
/// process's limit, lowered to at most `max` microseconds. `None` when the
/// current one already fits. A nonpositive RTKit budget or an existing
/// zero limit refuses promotion: either would let RTKit grant real-time
/// with no CPU budget, so the kernel could kill the whole process.
pub(crate) fn rttime_limit(
    current: (u64, u64),
    max: i64,
) -> Result<Option<(u64, u64)>, &'static str> {
    let max = u64::try_from(max)
        .ok()
        .filter(|max| *max > 0)
        .ok_or("RTKit: RTTimeUSecMax must be positive")?;
    let (soft, hard) = current;
    if soft == 0 || hard == 0 {
        return Err("RLIMIT_RTTIME: zero CPU budget; refusing real-time");
    }
    Ok((hard > max || soft > max).then(|| (soft.min(max), hard.min(max))))
}

/// The priority to ask for: ours, within RTKit's maximum, at least 1.
pub(crate) fn priority_within(wanted: u32, max: i32) -> u32 {
    wanted.min(u32::try_from(max).unwrap_or(0)).max(1)
}

mod rtkit {
    use super::{Promoted, priority_within, rttime_limit};

    const SERVICE: &str = "org.freedesktop.RealtimeKit1";
    const PATH: &str = "/org/freedesktop/RealtimeKit1";

    pub(super) fn promote(
        thread: libc::pid_t,
        priority: u32,
        nice: i32,
    ) -> Result<Promoted, String> {
        let connection = zbus::blocking::Connection::system()
            .map_err(|error| format!("no system bus: {error}"))?;
        let proxy = zbus::blocking::Proxy::new(&connection, SERVICE, PATH, SERVICE)
            .map_err(|error| format!("no RTKit: {error}"))?;
        let thread = u64::try_from(thread).map_err(|_| "a negative thread id".to_owned())?;
        let real_time = (|| {
            let max_priority: i32 = proxy
                .get_property("MaxRealtimePriority")
                .map_err(|error| format!("RTKit: {error}"))?;
            let max_rttime: i64 = proxy
                .get_property("RTTimeUSecMax")
                .map_err(|error| format!("RTKit: {error}"))?;
            limit_rttime(max_rttime)?;
            let priority = priority_within(priority, max_priority);
            proxy
                .call_method("MakeThreadRealtime", &(thread, priority))
                .map_err(|error| format!("RTKit refused real-time: {error}"))?;
            Ok::<_, String>(priority)
        })();
        match real_time {
            Ok(priority) => Ok(Promoted::RealTime(priority)),
            Err(refused) => {
                let min_nice: i32 = proxy.get_property("MinNiceLevel").unwrap_or(nice);
                let nice = nice.max(min_nice);
                proxy
                    .call_method("MakeThreadHighPriority", &(thread, nice))
                    .map_err(|error| format!("{refused}; and a higher priority: {error}"))?;
                Ok(Promoted::Nice(nice))
            }
        }
    }

    /// RTKit grants real-time only to a process whose `RLIMIT_RTTIME` is at
    /// most its own maximum, so that a runaway real-time thread is stopped
    /// rather than freezing the machine. PipeWire's clients set it too.
    fn limit_rttime(max: i64) -> Result<(), String> {
        let mut current = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: reads this process's limit into a valid struct.
        if unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut current) } != 0 {
            return Err(format!(
                "RLIMIT_RTTIME: {}",
                std::io::Error::last_os_error()
            ));
        }
        let Some((soft, hard)) = rttime_limit((current.rlim_cur, current.rlim_max), max)? else {
            return Ok(());
        };
        let limit = libc::rlimit {
            rlim_cur: soft,
            rlim_max: hard,
        };
        // SAFETY: lowers this process's own limit, which needs no privilege.
        if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &limit) } != 0 {
            return Err(format!(
                "RLIMIT_RTTIME: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rttime_limit_comes_down_to_what_rtkit_allows() {
        // Unlimited, as a process starts: both come down to RTKit's 200 ms.
        assert_eq!(
            rttime_limit((u64::MAX, u64::MAX), 200_000),
            Ok(Some((200_000, 200_000)))
        );
        // Already within it (PipeWire's own client set it): left alone.
        assert_eq!(rttime_limit((100_000, 200_000), 200_000), Ok(None));
        assert_eq!(
            rttime_limit((50_000, 400_000), 200_000),
            Ok(Some((50_000, 200_000)))
        );
    }

    #[test]
    fn a_nonpositive_rtkit_budget_refuses_real_time() {
        for max in [0, -1, i64::MIN] {
            assert!(
                rttime_limit((u64::MAX, u64::MAX), max).is_err(),
                "invalid RTKit budget {max} must not lower the process limit"
            );
        }
    }

    #[test]
    fn an_existing_zero_limit_refuses_real_time() {
        // Even a valid RTKit maximum cannot repair an inherited hard limit
        // of zero without privilege. A zero soft limit is unsafe too.
        for current in [(0, 0), (0, 200_000), (0, u64::MAX)] {
            assert!(
                rttime_limit(current, 200_000).is_err(),
                "zero limit {current:?} must not permit real-time promotion"
            );
        }
    }

    #[test]
    fn the_priority_stays_within_rtkits_maximum() {
        assert_eq!(priority_within(10, 20), 10);
        assert_eq!(priority_within(10, 5), 5);
        assert_eq!(priority_within(10, 0), 1);
        assert_eq!(priority_within(10, -1), 1);
    }

    #[test]
    fn a_stream_that_never_calls_back_lets_its_helper_go() {
        // The helper waits on the channel; dropping the promoter ends it.
        let promoter = Promoter::start(true, "hw:CARD=PCH,DEV=0", plan::Direction::Output);
        drop(promoter);
    }

    /// 0.10.0 regression: a callback on ALSA's `null` PCM never waits. Its
    /// thread is not handed to RTKit (no helper, no D-Bus), the guard sees it
    /// spin, and it stays at ordinary priority, so nothing can use up
    /// `RLIMIT_RTTIME` and kill the test process.
    #[test]
    fn a_callback_that_never_waits_is_not_made_real_time() {
        std::thread::spawn(|| {
            let mut promoter = Promoter::start(false, "null", plan::Direction::Input);
            assert!(
                promoter.sender.is_none(),
                "no helper for a device that may spin"
            );
            let started = Instant::now();
            while started.elapsed() < plan::SPIN_WINDOW * 3 {
                promoter.on_callback();
            }
            assert!(
                promoter.spinning.load(Ordering::Acquire),
                "the guard saw it spin"
            );
            assert!(!raised());
        })
        .join()
        .expect("the spinning callback thread finishes");
    }
}
