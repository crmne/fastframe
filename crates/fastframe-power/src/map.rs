//! What the system's raw values mean, kept apart from the readers that talk
//! to the OS so their tests run on every platform. Each reader uses only
//! its own platform's part.

/// Whether a power-profiles-daemon profile asks to save energy (Linux).
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
pub(crate) fn saves(profile: &str) -> bool {
    profile == "power-saver"
}

/// `kSCNetworkReachabilityFlagsReachable` (SCNetworkReachability.h).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const REACHABLE: u32 = 1 << 1;
/// `kSCNetworkReachabilityFlagsConnectionRequired`: a connection, such as a
/// VPN or a dial-up link, has to be made first.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const CONNECTION_REQUIRED: u32 = 1 << 2;

/// Whether SCNetworkReachability's flags say the internet can be reached
/// now (macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn reachable(flags: u32) -> bool {
    flags & REACHABLE != 0 && flags & CONNECTION_REQUIRED == 0
}

/// The session changes `WM_WTSSESSION_CHANGE` reports in its `wParam`
/// (WinUser.h), kept here so the session's state is read the same way on
/// every platform's tests. The Windows tests check them against windows-sys.
pub(crate) mod wts {
    #![cfg_attr(not(target_os = "windows"), allow(dead_code))]
    /// The session was connected to the console.
    pub(crate) const CONSOLE_CONNECT: u32 = 1;
    /// The session was disconnected from the console.
    pub(crate) const CONSOLE_DISCONNECT: u32 = 2;
    /// The session was connected to a remote client.
    pub(crate) const REMOTE_CONNECT: u32 = 3;
    /// The session was disconnected from a remote client.
    pub(crate) const REMOTE_DISCONNECT: u32 = 4;
    /// The session was locked.
    pub(crate) const SESSION_LOCK: u32 = 7;
    /// The session was unlocked.
    pub(crate) const SESSION_UNLOCK: u32 = 8;
}

/// The session's lock and its connection to a console or remote client, as
/// the session changes report them (Windows). They are kept apart because a
/// session switched back to the console is connected again but still shows
/// the lock screen until the user unlocks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) struct SessionState {
    locked: bool,
    disconnected: bool,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl SessionState {
    /// Unlocked and connected, as a session starts.
    pub(crate) const fn new() -> Self {
        Self {
            locked: false,
            disconnected: false,
        }
    }

    /// The state after a session change, or `None` for a change that says
    /// nothing about either.
    pub(crate) fn apply(self, code: u32) -> Option<Self> {
        match code {
            wts::SESSION_LOCK => Some(Self {
                locked: true,
                ..self
            }),
            wts::SESSION_UNLOCK => Some(Self {
                locked: false,
                ..self
            }),
            wts::CONSOLE_DISCONNECT | wts::REMOTE_DISCONNECT => Some(Self {
                disconnected: true,
                ..self
            }),
            wts::CONSOLE_CONNECT | wts::REMOTE_CONNECT => Some(Self {
                disconnected: false,
                ..self
            }),
            _ => None,
        }
    }

    /// Nobody can see the session: it is locked, or away from every console
    /// and remote client.
    pub(crate) fn hidden(self) -> bool {
        self.locked || self.disconnected
    }
}

/// What to do with the process's power throttling (Windows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) enum Throttle {
    /// Write this: on is EcoQoS, off hands the choice back to the system.
    Write(bool),
    /// Already as wanted.
    Leave,
    /// Someone else manages it (the Task Manager's efficiency mode, say):
    /// stop touching it for the rest of the run.
    GiveUp,
}

/// Decides, from what the app last wrote (`None`: nothing yet), the current
/// setting (`None`: not controlled, the system decides) and whether the
/// process runs at idle priority, how to get to `want`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn throttle(
    written: Option<bool>,
    current: Option<bool>,
    idle: bool,
    want: bool,
) -> Throttle {
    let expected = match written {
        None => None,
        Some(true) => Some(true),
        Some(false) => None,
    };
    if idle || current != expected {
        Throttle::GiveUp
    } else if written.unwrap_or(false) == want {
        Throttle::Leave
    } else {
        Throttle::Write(want)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_power_saver_profile_saves() {
        assert!(saves("power-saver"));
        assert!(!saves("balanced"));
        assert!(!saves("performance"));
    }

    #[test]
    fn reachable_needs_a_route_and_no_connection_to_make_first() {
        assert!(reachable(REACHABLE));
        assert!(!reachable(0), "no route");
        assert!(
            !reachable(REACHABLE | CONNECTION_REQUIRED),
            "a VPN or a dial-up link still to connect"
        );
        // kSCNetworkReachabilityFlagsIsDirect (1 << 17) and
        // kSCNetworkReachabilityFlagsIsLocalAddress (1 << 16) say nothing
        // about whether the internet is there.
        assert!(reachable(REACHABLE | 1 << 17 | 1 << 16));
    }

    #[test]
    fn eco_qos_follows_the_window_unless_someone_else_set_it() {
        // Nothing written yet, the system decides: write what is wanted.
        assert_eq!(throttle(None, None, false, true), Throttle::Write(true));
        assert_eq!(throttle(None, None, false, false), Throttle::Leave);
        // As last written: write only a change.
        assert_eq!(
            throttle(Some(true), Some(true), false, false),
            Throttle::Write(false)
        );
        assert_eq!(
            throttle(Some(true), Some(true), false, true),
            Throttle::Leave
        );
        assert_eq!(
            throttle(Some(false), None, false, true),
            Throttle::Write(true)
        );
        // Changed behind the app's back, or efficiency mode: hands off.
        assert_eq!(throttle(None, Some(true), false, true), Throttle::GiveUp);
        assert_eq!(
            throttle(Some(false), Some(true), false, false),
            Throttle::GiveUp
        );
        assert_eq!(throttle(Some(true), None, false, true), Throttle::GiveUp);
        assert_eq!(throttle(None, None, true, true), Throttle::GiveUp);
    }

    /// Whether nobody can see the session after each change in turn.
    fn hidden_after(codes: &[u32]) -> Vec<bool> {
        let mut state = SessionState::new();
        codes
            .iter()
            .map(|&code| {
                state = state.apply(code).unwrap_or(state);
                state.hidden()
            })
            .collect()
    }

    /// The lock and the connection are separate: with fast user switching,
    /// a locked session switched back to the console shows the lock screen
    /// until it is unlocked, and an RDP session that disconnects is not
    /// locked but nobody sees it either.
    #[test]
    fn lock_and_unlock_are_read_from_session_changes() {
        assert_eq!(
            hidden_after(&[
                wts::SESSION_LOCK,
                wts::CONSOLE_DISCONNECT,
                wts::CONSOLE_CONNECT,
                wts::SESSION_UNLOCK,
            ]),
            [true, true, true, false],
        );
        assert_eq!(
            hidden_after(&[wts::REMOTE_DISCONNECT, wts::REMOTE_CONNECT]),
            [true, false],
        );
        assert_eq!(
            SessionState::new().apply(5),
            None,
            "a logon changes nothing here"
        );
    }
}
