//! Application icons and badges that outlive individual windows, and whether
//! the macOS Dock shows the app at all.
//!
//! Extracted from ZapFast's Dock and taskbar badge implementations. See the
//! crate README for platform support and the Windows per-window overlay.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // AppKit icon setters, with safety notes at each call.
mod macos;
#[cfg(any(target_os = "windows", test))]
mod overlay;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // COM and the window subclass require native calls.
mod windows;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

/// The application-wide count. Keep this alive when closing a window.
///
/// macOS draws the number on the Dock tile. Linux publishes the Unity Launcher
/// API for compatible launchers. Windows stores the count for [`Taskbar`].
pub struct Badge(platform::Badge);

impl Badge {
    /// Creates a badge for a desktop entry, without the `.desktop` suffix.
    ///
    /// Resolve Flatpak identities with `fastframe_desktop::desktop_entry`.
    /// The name is only used on Linux. Construction does not contact the OS.
    pub fn new(desktop_entry: impl Into<String>) -> Self {
        Self(platform::Badge::new(desktop_entry.into()))
    }

    /// Shows a count, or clears the badge at zero. Call on the UI thread.
    /// Repeated values do not publish again. Linux caps the count at 9999.
    pub fn set(&mut self, count: u32) {
        self.0.set(count);
    }

    /// The last accepted count, or `None` before the first update.
    pub fn count(&self) -> Option<u32> {
        self.0.count()
    }
}

#[cfg(target_os = "windows")]
pub use windows::Taskbar;

/// A no-op overlay on platforms where [`Badge`] updates the launcher directly.
#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct Taskbar;

#[cfg(not(target_os = "windows"))]
impl Taskbar {
    /// Does nothing here; only Windows needs a per-window overlay.
    pub fn show(
        &mut self,
        _window: &impl raw_window_handle::HasWindowHandle,
        _count: u32,
        _description: &str,
    ) -> Option<std::time::Instant> {
        None
    }

    /// Does nothing here; only Windows caches the last applied overlay.
    pub fn reset(&mut self) {}
}

/// Why the unbundled macOS application icon could not be installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconError {
    /// AppKit requires the application's main thread.
    NotMainThread,
    /// AppKit could not decode the supplied image.
    InvalidImage,
}

impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotMainThread => "the Dock icon must be installed on the main thread",
            Self::InvalidImage => "AppKit could not decode the Dock icon",
        })
    }
}

impl std::error::Error for IconError {}

/// Installs an embedded PNG or ICNS as the Dock icon of an unbundled macOS app.
///
/// Call once on the main thread before opening the first window. This replaces
/// the generic executable icon in `cargo run`, and survives window destruction.
/// Returns `true` when installed, `false` for a packaged `.app` (whose branding
/// is preserved) or on other platforms. Does not change the executable's name.
pub fn set_icon(encoded_image: &[u8]) -> Result<bool, IconError> {
    #[cfg(target_os = "macos")]
    return macos::set_icon(encoded_image);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = encoded_image;
        Ok(false)
    }
}

/// Shows or hides the application's macOS Dock icon, while the app runs.
///
/// Hidden, the app runs as a menu-bar app (AppKit's `Accessory` activation
/// policy): no Dock icon, no entry in the Cmd-Tab switcher and no menu bar of
/// its own, while its windows, their keyboard shortcuts (including the main
/// menu's key equivalents) and its status item keep working. Shown, it is an
/// ordinary app again (`Regular`). Keep a status item, such as
/// `fastframe-tray`'s, while the icon is hidden: it is the way back to a
/// closed window.
///
/// Call on the main thread. An app that was active is activated again after
/// the switch, so its window stays in front with the focus. To start without
/// a Dock icon, call this before the event loop starts (before
/// `eframe::run_native`). winit sets `Regular` once AppKit finishes launching
/// an unbundled run (`cargo run`), so call it again once the first window
/// exists; a packaged `.app` keeps the early choice. `Info.plist`'s
/// `LSUIElement` cannot change while the app runs, so this is the switch to
/// use.
///
/// Returns whether the policy changed: `false` when it was already so, off
/// the main thread (logged), when AppKit refused (logged), and on Windows
/// and Linux, which have no Dock.
pub fn set_dock_icon_visible(visible: bool) -> bool {
    #[cfg(target_os = "macos")]
    return macos::set_dock_icon_visible(visible);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = visible;
        false
    }
}

/// AppKit's activation policies, as far as the Dock icon is concerned.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// An ordinary app, in the Dock.
    Regular,
    /// A menu-bar app, out of the Dock.
    Accessory,
    /// Anything else (`Prohibited`, as an unbundled run starts).
    Other,
}

/// What showing or hiding the Dock icon has to do.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Switch {
    /// The policy to set.
    policy: Policy,
    /// Whether to activate the app again afterwards, because it was active.
    reactivate: bool,
}

/// The switch from `current` to the Dock icon `visible`, or `None` when the
/// policy already matches.
#[cfg(any(target_os = "macos", test))]
fn dock_switch(visible: bool, current: Policy, active: bool) -> Option<Switch> {
    let policy = if visible {
        Policy::Regular
    } else {
        Policy::Accessory
    };
    (current != policy).then_some(Switch {
        policy,
        reactivate: active,
    })
}

#[cfg(any(target_os = "macos", test))]
fn is_app_bundle(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|extension| extension == "app")
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "macos", target_os = "windows")))]
mod platform {
    #[derive(Default)]
    pub(super) struct Badge(Option<u32>);
    impl Badge {
        pub(super) fn new(_: String) -> Self {
            Self::default()
        }
        pub(super) fn set(&mut self, count: u32) {
            self.0 = Some(count);
        }
        pub(super) fn count(&self) -> Option<u32> {
            self.0
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_real_app_bundles_keep_their_packaged_icon() {
        use std::path::Path;
        assert!(super::is_app_bundle(Path::new(
            "/Applications/Spotifast.app"
        )));
        assert!(!super::is_app_bundle(Path::new("/code/target/release")));
        assert!(!super::is_app_bundle(Path::new("/code/app")));
    }

    #[test]
    fn hiding_the_dock_icon_makes_a_menu_bar_app_and_keeps_it_in_front() {
        use super::{Policy, Switch, dock_switch};
        assert_eq!(
            dock_switch(false, Policy::Regular, true),
            Some(Switch {
                policy: Policy::Accessory,
                reactivate: true
            })
        );
        assert_eq!(
            dock_switch(true, Policy::Accessory, true),
            Some(Switch {
                policy: Policy::Regular,
                reactivate: true
            })
        );
    }

    #[test]
    fn before_launch_the_policy_is_set_without_activating() {
        use super::{Policy, Switch, dock_switch};
        // An unbundled run starts `Prohibited`; nothing is active yet.
        assert_eq!(
            dock_switch(false, Policy::Other, false),
            Some(Switch {
                policy: Policy::Accessory,
                reactivate: false
            })
        );
        assert_eq!(
            dock_switch(true, Policy::Other, false),
            Some(Switch {
                policy: Policy::Regular,
                reactivate: false
            })
        );
    }

    #[test]
    fn a_matching_policy_is_left_alone() {
        use super::{Policy, dock_switch};
        assert_eq!(dock_switch(true, Policy::Regular, true), None);
        assert_eq!(dock_switch(false, Policy::Accessory, false), None);
    }

    #[test]
    fn there_is_no_dock_to_change_off_macos() {
        if !cfg!(target_os = "macos") {
            assert!(!super::set_dock_icon_visible(false));
            assert!(!super::set_dock_icon_visible(true));
        }
    }
}
