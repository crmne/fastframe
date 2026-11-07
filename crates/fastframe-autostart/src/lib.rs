//! Start an app when the person logs in, with its window or hidden in the
//! tray.
//!
//! Each platform keeps its own record of what starts at login, and the person
//! can change it there too (System Settings, Task Manager, GNOME's
//! background apps). [`Autostart::state`] reads that record back, so an app
//! never keeps a copy that could disagree with it.
//!
//! - **Linux**: an XDG autostart entry, `$XDG_CONFIG_HOME/autostart/<id>.desktop`,
//!   replaced atomically. Inside a Flatpak, which cannot write the host's
//!   autostart folder, the background portal's `RequestBackground` registers
//!   the app instead, and the same entry in the sandbox's own configuration
//!   folder records what was asked, since the portal has no way to read it
//!   back.
//! - **macOS 13 and later**: `SMAppService`. Starting with the window is the
//!   main app as a login item ("Open at Login" in System Settings). Starting
//!   hidden is a launch agent the bundle carries, because a login item cannot
//!   pass arguments: [`macos_agent_plist`] is its text, at
//!   `Contents/Library/LaunchAgents/` [`macos_agent_plist_name`]. The agent runs
//!   `open`, which launches the app through Launch Services and exits, so
//!   unregistering the agent (which kills its running process) never kills the
//!   app. An executable outside an `.app` bundle (`cargo run`) and macOS 12 or
//!   earlier report [`State::Unsupported`].
//! - **Windows**: a value named after the app under
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, the quoted
//!   executable and the hidden argument. An entry turned off in Task
//!   Manager's Startup apps reads as [`State::Off`], and turning it on again
//!   here clears that.
//!
//! Every call reads or writes files, the registry or D-Bus, and the portal
//! may wait for the person to answer a dialog: call [`Autostart::state`] and
//! [`Autostart::set`] off the interface thread.
//!
//! ```no_run
//! use fastframe_autostart::{Autostart, Start, State};
//!
//! let autostart = Autostart::new("spotifast", "Spotifast");
//! // On a worker thread:
//! match autostart.state() {
//!     State::Unsupported => { /* hide the setting */ }
//!     State::Off => {}
//!     State::On(Start::Window) | State::NeedsApproval(Start::Window) => {}
//!     State::On(Start::Hidden) | State::NeedsApproval(Start::Hidden) => {}
//! }
//! let now = autostart.set(Some(Start::Hidden))?;
//! # let _ = now;
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! A hidden start passes [`START_HIDDEN`] (or the app's own argument, from
//! [`Autostart::hidden_argument`]); the app hands it to
//! `fastframe_shell::Shell::start_hidden`, whose `Resident::start_hidden` opens
//! the window after all when there is no tray to come back from.

use std::io;

mod command;
mod desktop;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod plist;
#[cfg(windows)]
mod windows;

/// The argument a hidden start passes, unless the app names its own with
/// [`Autostart::hidden_argument`].
pub const START_HIDDEN: &str = "--start-hidden";

/// How the app starts at login.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Start {
    /// With its window, as when the person opens it.
    Window,
    /// Without a window, in the tray: the command line carries the hidden
    /// argument.
    Hidden,
}

/// What the platform has registered for the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// This installation cannot start at login: an executable outside an
    /// `.app` bundle or macOS 12 and earlier, no home folder, or another
    /// operating system.
    Unsupported,
    /// Nothing starts the app at login, or the person turned it off in the
    /// platform's own settings.
    Off,
    /// The app starts at login, this way.
    On(Start),
    /// Registered this way, but the person has to allow it in System
    /// Settings, Login Items, before it starts (macOS).
    NeedsApproval(Start),
}

impl State {
    /// How the app is registered to start, approved or not.
    #[must_use]
    pub fn start(self) -> Option<Start> {
        match self {
            Self::On(start) | Self::NeedsApproval(start) => Some(start),
            Self::Unsupported | Self::Off => None,
        }
    }
}

/// The app's registration to start at login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Autostart {
    id: String,
    name: String,
    hidden_argument: String,
}

impl Autostart {
    /// The registration of the app known to the desktop as `id` (its desktop
    /// entry's name without `.desktop`, which is also its icon's name) and to
    /// people as `name` (the Windows Run value, the entry's `Name`).
    ///
    /// macOS goes by the running bundle's identifier instead of `id`.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            hidden_argument: START_HIDDEN.to_owned(),
        }
    }

    /// Starts hidden with `argument` instead of [`START_HIDDEN`]. On macOS
    /// the bundled agent's text, from [`macos_agent_plist`], must name the
    /// same argument.
    #[must_use]
    pub fn hidden_argument(mut self, argument: &str) -> Self {
        argument.clone_into(&mut self.hidden_argument);
        self
    }

    /// What starts the app at login now, as the platform has it.
    ///
    /// Blocks on the file system, the registry or `SMAppService`: call it off
    /// the interface thread.
    #[must_use]
    pub fn state(&self) -> State {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            linux::state(self)
        }
        #[cfg(target_os = "macos")]
        {
            macos::state(self)
        }
        #[cfg(windows)]
        {
            windows::state(self)
        }
        #[cfg(not(any(any(target_os = "linux", target_os = "freebsd"), target_os = "macos", windows)))]
        {
            State::Unsupported
        }
    }

    /// Starts the app at login this way, or not at all with `None`, for the
    /// running executable. Returns the state read back afterwards, which on
    /// macOS can be [`State::NeedsApproval`].
    ///
    /// Blocks, and inside a Flatpak waits for the portal, which can ask the
    /// person first: call it off the interface thread.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] where [`state`](Self::state) is
    /// [`State::Unsupported`] (and on macOS for a hidden start when the
    /// bundle carries no agent), [`io::ErrorKind::PermissionDenied`] when
    /// the person or the portal refused, and the platform's own error when
    /// writing the entry fails.
    pub fn set(&self, start: Option<Start>) -> io::Result<State> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            linux::set(self, start)
        }
        #[cfg(target_os = "macos")]
        {
            macos::set(self, start)
        }
        #[cfg(windows)]
        {
            windows::set(self, start)
        }
        #[cfg(not(any(any(target_os = "linux", target_os = "freebsd"), target_os = "macos", windows)))]
        {
            let _ = start;
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }

    /// The command line for `start`: `executable`, then the hidden argument
    /// for a hidden start.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn command(&self, executable: String, start: Start) -> Vec<String> {
        let mut command = vec![executable];
        if start == Start::Hidden {
            command.push(self.hidden_argument.clone());
        }
        command
    }
}

/// The file name of the launch agent that starts the app hidden, which the
/// bundle carries in `Contents/Library/LaunchAgents/`: `<bundle id>.autostart.plist`.
#[must_use]
pub fn macos_agent_plist_name(bundle_id: &str) -> String {
    format!("{bundle_id}.autostart.plist")
}

/// The launch agent that starts the app with `bundle_id` hidden at login, by
/// running `open -g -b <bundle id> --args <hidden argument>`.
///
/// Put it in the bundle as `Contents/Library/LaunchAgents/`
/// [`macos_agent_plist_name`] before signing. `open` launches the app through
/// Launch Services and exits at once, so the agent's job never is the app:
/// `SMAppService` kills a running agent when it is unregistered, and that
/// must not be the app the person is using.
#[must_use]
pub fn macos_agent_plist(bundle_id: &str, hidden_argument: &str) -> String {
    plist::agent(bundle_id, hidden_argument)
}

/// Where a bundle at `bundle` carries the agent [`macos_agent_plist_name`]
/// names. Whether that file is there is how to tell that the bundle can start
/// hidden: `SMAppService` reports an agent it never registered as not found
/// too, so its status cannot say.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn macos_bundled_agent(bundle: &std::path::Path, bundle_id: &str) -> std::path::PathBuf {
    bundle
        .join("Contents/Library/LaunchAgents")
        .join(macos_agent_plist_name(bundle_id))
}

/// What `SMAppService` says about one service, the main app's or the agent's.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceStatus {
    NotRegistered,
    Enabled,
    RequiresApproval,
    /// No record of the service: a missing plist, but also an agent that was
    /// never registered.
    NotFound,
}

/// The state the main app's login item and the hidden agent make together.
/// The agent wins if both are registered, since `set` registers the new one
/// before it removes the old.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn service_state(main: ServiceStatus, agent: ServiceStatus) -> State {
    use ServiceStatus::{Enabled, RequiresApproval};
    match (main, agent) {
        (_, Enabled) => State::On(Start::Hidden),
        (Enabled, _) => State::On(Start::Window),
        (_, RequiresApproval) => State::NeedsApproval(Start::Hidden),
        (RequiresApproval, _) => State::NeedsApproval(Start::Window),
        _ => State::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registration_knows_how_it_starts() {
        assert_eq!(State::On(Start::Hidden).start(), Some(Start::Hidden));
        assert_eq!(
            State::NeedsApproval(Start::Window).start(),
            Some(Start::Window)
        );
        assert_eq!(State::Off.start(), None);
        assert_eq!(State::Unsupported.start(), None);
    }

    #[test]
    fn a_hidden_start_adds_the_apps_own_argument() {
        let autostart = Autostart::new("cww-app", "Chat with Work").hidden_argument("--background");
        assert_eq!(
            autostart.command("/usr/bin/cww-app".into(), Start::Hidden),
            ["/usr/bin/cww-app", "--background"]
        );
        assert_eq!(
            autostart.command("/usr/bin/cww-app".into(), Start::Window),
            ["/usr/bin/cww-app"]
        );
        assert_eq!(
            Autostart::new("spotifast", "Spotifast").command("spotifast".into(), Start::Hidden),
            ["spotifast", START_HIDDEN]
        );
    }

    #[test]
    fn the_services_say_together_how_the_app_starts() {
        use ServiceStatus::*;
        assert_eq!(service_state(NotRegistered, NotRegistered), State::Off);
        assert_eq!(service_state(NotRegistered, NotFound), State::Off);
        assert_eq!(service_state(NotFound, NotFound), State::Off);
        assert_eq!(
            service_state(Enabled, NotFound),
            State::On(Start::Window),
            "a bundle without the agent still opens its window at login"
        );
        assert_eq!(
            service_state(NotRegistered, Enabled),
            State::On(Start::Hidden)
        );
        assert_eq!(
            service_state(Enabled, Enabled),
            State::On(Start::Hidden),
            "halfway through a switch, the new one wins"
        );
        assert_eq!(
            service_state(RequiresApproval, NotRegistered),
            State::NeedsApproval(Start::Window)
        );
        assert_eq!(
            service_state(NotRegistered, RequiresApproval),
            State::NeedsApproval(Start::Hidden)
        );
        assert_eq!(
            service_state(Enabled, RequiresApproval),
            State::On(Start::Window),
            "an approved login item beats an agent still waiting"
        );
    }

    /// The agent is looked for where the bundle carries it, by name.
    #[test]
    fn the_bundled_agent_is_inside_the_bundle() {
        assert_eq!(
            macos_bundled_agent(
                std::path::Path::new("/Applications/Spotifast.app"),
                "rocks.spotifast.Spotifast"
            ),
            std::path::Path::new(
                "/Applications/Spotifast.app/Contents/Library/LaunchAgents/rocks.spotifast.Spotifast.autostart.plist"
            )
        );
    }

    #[test]
    fn the_agent_is_named_after_the_bundle() {
        assert_eq!(
            macos_agent_plist_name("rocks.spotifast.Spotifast"),
            "rocks.spotifast.Spotifast.autostart.plist"
        );
    }
}
