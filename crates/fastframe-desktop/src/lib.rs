//! Who the app is to the desktop, and what the desktop says.
//!
//! - [`desktop_entry`]: the name of the app's desktop entry, as the desktop
//!   knows it, for MPRIS, notifications and the window's app id, which must
//!   all agree. Inside a Flatpak that is the sandbox's app id.
//! - [`is_gnome`]: whether the session is GNOME, which handles some requests
//!   its own way (it turns a window's request for attention into a notice).
//! - [`setting`]: one of the desktop's settings, such as GNOME's clock format
//!   or window button layout, read through the desktop portal. That works
//!   inside a Flatpak too, where running `gsettings` does not, and needs no
//!   process.
//! - [`macos_notifications_allowed`]: checks the real application bundle
//!   before asking macOS for notification permission, off the UI thread.
//!
//! ```no_run
//! let entry = fastframe_desktop::desktop_entry("zapfast");
//! let twelve_hour = fastframe_desktop::setting("org.gnome.desktop.interface", "clock-format")
//!     .is_some_and(|format| format == "12h");
//! if fastframe_desktop::is_gnome() { /* skip the attention request */ }
//! # let _ = (entry, twelve_hour);
//! ```

#[cfg(any(target_os = "macos", test))]
mod notifications;

/// Whether this macOS application can send native notifications.
///
/// Checks the process's actual bundle once, then requests authorization on
/// every call. macOS prompts only if permission has not been decided yet;
/// later calls observe changes made in System Settings without a restart.
/// An unpackaged executable stays silent. No application lookup, substitute
/// bundle identity or AppleScript is used.
///
/// Call from a worker only when the app intends to notify, not during
/// startup or when its notification preference is disabled. The permission
/// request can block waiting for the user. A running native event loop is
/// needed by the notification framework. Delivery and click handling remain
/// the app's responsibility, through `mac-usernotifications`.
///
/// Returns `false` on other platforms, without a bundle, after denial, or
/// when the native check or request fails. This is a macOS guard, not a
/// cross-platform notification-permission query.
#[must_use]
pub fn macos_notifications_allowed() -> bool {
    #[cfg(target_os = "macos")]
    {
        notifications::allowed()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The desktop entry the desktop knows the app by, for `own` (the entry's
/// name without `.desktop`): the sandbox's app id inside a Flatpak, since the
/// entry is exported under it there, and `own` elsewhere.
#[must_use]
pub fn desktop_entry(own: &str) -> String {
    let flatpak_id = std::env::var("FLATPAK_ID").ok();
    desktop_entry_for(flatpak_id.as_deref(), own).to_owned()
}

fn desktop_entry_for<'a>(flatpak_id: Option<&'a str>, own: &'a str) -> &'a str {
    flatpak_id.filter(|id| !id.is_empty()).unwrap_or(own)
}

/// Whether the session is GNOME, from `XDG_CURRENT_DESKTOP`.
#[must_use]
pub fn is_gnome() -> bool {
    names_gnome(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref())
}

/// Whether `desktops`, an `XDG_CURRENT_DESKTOP` value (a colon-separated
/// list such as `ubuntu:GNOME`), names GNOME.
fn names_gnome(desktops: Option<&str>) -> bool {
    desktops.is_some_and(|desktops| {
        desktops
            .split(':')
            .any(|name| name.trim().eq_ignore_ascii_case("GNOME"))
    })
}

/// A string setting of the desktop's, such as
/// `("org.gnome.desktop.interface", "clock-format")` or
/// `("org.gnome.desktop.wm.preferences", "button-layout")`, read through the
/// desktop portal's Settings interface.
///
/// `None` off Linux, without a session bus or a portal, or when the portal
/// does not have the key. The call gives up after half a second, so a portal
/// that is still starting cannot hold the app up for long. GNOME's portal and
/// the GTK one serve GNOME's own schemas, and other desktops' portals serve
/// what they choose.
#[must_use]
pub fn setting(namespace: &str, key: &str) -> Option<String> {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        portal::read_string(namespace, key)
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = (namespace, key);
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod portal {
    use std::time::Duration;

    use zbus::blocking::{Proxy, connection};
    use zbus::zvariant::{OwnedValue, Value};

    pub(crate) fn read_string(namespace: &str, key: &str) -> Option<String> {
        let connection = connection::Builder::session()
            .ok()?
            .method_timeout(Duration::from_millis(500))
            .build()
            .ok()?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )
        .ok()?;
        // `ReadOne` (portal version 2), or the older `Read`, which wraps the
        // value in a second variant.
        let value: OwnedValue = proxy
            .call("ReadOne", &(namespace, key))
            .or_else(|_| proxy.call("Read", &(namespace, key)))
            .ok()?;
        string(&value)
    }

    pub(super) fn string(value: &Value<'_>) -> Option<String> {
        match value {
            Value::Str(text) => Some(text.as_str().to_owned()),
            Value::Value(inner) => string(inner),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_entry_is_the_flatpak_id_inside_a_sandbox() {
        assert_eq!(
            desktop_entry_for(Some("com.getsolco.Solco"), "solco"),
            "com.getsolco.Solco"
        );
        assert_eq!(desktop_entry_for(None, "solco"), "solco");
        assert_eq!(desktop_entry_for(Some(""), "solco"), "solco");
    }

    #[test]
    fn gnome_is_found_in_a_list_of_desktops() {
        assert!(names_gnome(Some("GNOME")));
        assert!(names_gnome(Some("ubuntu:GNOME")));
        assert!(names_gnome(Some("gnome-classic:gnome")));
        assert!(!names_gnome(Some("Hyprland")));
        assert!(!names_gnome(Some("GNOME-Flashback-ish")));
        assert!(!names_gnome(None));
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn a_setting_is_read_through_either_variant_wrapping() {
        use zbus::zvariant::Value;
        let plain = Value::from("12h");
        let wrapped = Value::Value(Box::new(Value::from("12h")));
        assert_eq!(portal::string(&plain).as_deref(), Some("12h"));
        assert_eq!(portal::string(&wrapped).as_deref(), Some("12h"));
        assert_eq!(portal::string(&Value::from(3u32)), None);
    }
}
