//! The desktop portal's font settings (Linux).
//!
//! `xdg-desktop-portal` exposes the desktop's settings over D-Bus through
//! `org.freedesktop.portal.Settings`. GNOME and desktops that follow its
//! schema publish, in the `org.gnome.desktop.interface` namespace:
//!
//! - `font-hinting`: `none`, `slight`, `medium` or `full`;
//! - `font-antialiasing`: `none`, `grayscale` or `rgba`.
//!
//! The portal says nothing about sub-pixel positioning; GTK 4 always uses
//! it, so the default (on) stands. The parsers here are pure and available on
//! every platform; only [`read`] talks to the bus, and only on Linux.

use crate::{Hinting, TextRendering};

/// The settings namespace holding the font keys.
pub const NAMESPACE: &str = "org.gnome.desktop.interface";
/// The hinting key in [`NAMESPACE`].
pub const HINTING_KEY: &str = "font-hinting";
/// The antialiasing key in [`NAMESPACE`].
pub const ANTIALIASING_KEY: &str = "font-antialiasing";

/// Parses a `font-hinting` value.
#[must_use]
pub fn parse_hinting(value: &str) -> Option<Hinting> {
    match value.trim() {
        "none" => Some(Hinting::None),
        "slight" => Some(Hinting::Slight),
        "medium" => Some(Hinting::Medium),
        "full" => Some(Hinting::Full),
        _ => None,
    }
}

/// Parses a `font-antialiasing` value. `rgba` (sub-pixel) counts as
/// antialiased: egui renders grayscale only.
#[must_use]
pub fn parse_antialiasing(value: &str) -> Option<bool> {
    match value.trim() {
        "none" => Some(false),
        "grayscale" | "rgba" => Some(true),
        _ => None,
    }
}

/// Builds a rendering from the two keys as read from the portal.
///
/// Returns `None` when neither key holds a known value, so the next source
/// gets its turn; a missing or unknown key keeps its default.
#[must_use]
pub fn from_settings(hinting: Option<&str>, antialiasing: Option<&str>) -> Option<TextRendering> {
    let hinting = hinting.and_then(parse_hinting);
    let antialias = antialiasing.and_then(parse_antialiasing);
    if hinting.is_none() && antialias.is_none() {
        return None;
    }
    let default = TextRendering::default();
    Some(TextRendering {
        hinting: hinting.unwrap_or(default.hinting),
        antialias: antialias.unwrap_or(default.antialias),
        ..default
    })
}

/// Asks the session bus's desktop portal for the font settings.
///
/// Returns `None` when there is no session bus, no portal, or no font keys.
/// Each call gives up after about a second, so a portal that is being
/// activated cannot stall startup for long.
#[cfg(target_os = "linux")]
#[must_use]
pub fn read() -> Option<TextRendering> {
    let connection = bus::connect().ok()?;
    let proxy = bus::settings(&connection).ok()?;
    let hinting = bus::read_string(&proxy, HINTING_KEY);
    let antialiasing = bus::read_string(&proxy, ANTIALIASING_KEY);
    from_settings(hinting.as_deref(), antialiasing.as_deref())
}

#[cfg(target_os = "linux")]
pub(crate) mod bus {
    use std::time::Duration;
    use zbus::blocking::{Connection, Proxy, connection};
    use zbus::zvariant::{OwnedValue, Value};

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const INTERFACE: &str = "org.freedesktop.portal.Settings";

    pub(crate) fn connect() -> zbus::Result<Connection> {
        connection::Builder::session()?
            .method_timeout(Duration::from_secs(1))
            .build()
    }

    pub(crate) fn settings(connection: &Connection) -> zbus::Result<Proxy<'static>> {
        Proxy::new(connection, DESTINATION, PATH, INTERFACE)
    }

    /// Reads one string key, through `ReadOne` (portal version 2) or the
    /// older `Read`, which wraps the value in a second variant.
    pub(crate) fn read_string(proxy: &Proxy<'_>, key: &str) -> Option<String> {
        let args = (super::NAMESPACE, key);
        let value: OwnedValue = proxy
            .call("ReadOne", &args)
            .or_else(|_| proxy.call("Read", &args))
            .ok()?;
        string(&value)
    }

    pub(crate) fn string(value: &Value<'_>) -> Option<String> {
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
    fn hinting_values() {
        assert_eq!(parse_hinting("none"), Some(Hinting::None));
        assert_eq!(parse_hinting("slight"), Some(Hinting::Slight));
        assert_eq!(parse_hinting("medium"), Some(Hinting::Medium));
        assert_eq!(parse_hinting("full"), Some(Hinting::Full));
        assert_eq!(parse_hinting("hintfull"), None);
        assert_eq!(parse_hinting(""), None);
    }

    #[test]
    fn antialiasing_values() {
        assert_eq!(parse_antialiasing("none"), Some(false));
        assert_eq!(parse_antialiasing("grayscale"), Some(true));
        assert_eq!(parse_antialiasing("rgba"), Some(true));
        assert_eq!(parse_antialiasing("bgr"), None);
    }

    #[test]
    fn both_keys() {
        let got = from_settings(Some("full"), Some("none")).unwrap();
        assert_eq!(got.hinting, Hinting::Full);
        assert!(!got.antialias);
        assert!(got.subpixel_positioning);
    }

    #[test]
    fn carmines_desktop_is_the_default() {
        assert_eq!(
            from_settings(Some("slight"), Some("grayscale")),
            Some(TextRendering::default())
        );
    }

    #[test]
    fn one_key_keeps_the_other_default() {
        let got = from_settings(Some("none"), None).unwrap();
        assert_eq!(got.hinting, Hinting::None);
        assert!(got.antialias);
        let got = from_settings(Some("bogus"), Some("none")).unwrap();
        assert_eq!(got.hinting, Hinting::Slight);
        assert!(!got.antialias);
    }

    #[test]
    fn no_known_key_is_no_answer() {
        assert_eq!(from_settings(None, None), None);
        assert_eq!(from_settings(Some("bogus"), Some("bogus")), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn values_unwrap_nested_variants() {
        use zbus::zvariant::Value;
        let plain = Value::from("slight");
        assert_eq!(bus::string(&plain).as_deref(), Some("slight"));
        let nested = Value::Value(Box::new(Value::from("full")));
        assert_eq!(bus::string(&nested).as_deref(), Some("full"));
        assert_eq!(bus::string(&Value::from(3u32)), None);
    }
}
