//! Live changes to the desktop's font settings (feature `watch`).
//!
//! On Linux a background thread listens for the desktop portal's
//! `SettingChanged` signal on the font keys, reads the settings again with
//! [`crate::detect`], and hands the new [`TextRendering`] to a callback when
//! it differs from the last one. The app then applies it on its interface
//! thread (`apply_to` and `ctx.set_fonts`, then `apply_to_visuals`) and
//! requests a repaint.
//!
//! Edits to fontconfig files are not watched. On macOS and Windows there is
//! nothing to watch yet and [`watch`] returns [`WatchError::Unsupported`].

use crate::TextRendering;
use std::fmt;

/// Why watching could not start.
#[derive(Debug)]
#[non_exhaustive]
pub enum WatchError {
    /// This platform has no source of live font settings.
    Unsupported,
    /// The session bus or the desktop portal could not be reached.
    Bus(String),
    /// The watching thread could not be started.
    Thread(std::io::Error),
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("font settings cannot be watched on this platform"),
            Self::Bus(error) => write!(f, "desktop portal unavailable: {error}"),
            Self::Thread(error) => write!(f, "cannot start the font settings thread: {error}"),
        }
    }
}

impl std::error::Error for WatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Thread(error) => Some(error),
            _ => None,
        }
    }
}

/// Whether a `SettingChanged` signal concerns the font settings.
#[must_use]
pub fn is_font_setting(namespace: &str, key: &str) -> bool {
    use crate::portal::{ANTIALIASING_KEY, HINTING_KEY, NAMESPACE};
    namespace == NAMESPACE && (key == HINTING_KEY || key == ANTIALIASING_KEY)
}

/// Keeps the last rendering handed out, and hands out only changes.
#[derive(Debug)]
pub struct Changes {
    last: TextRendering,
}

impl Changes {
    /// Starts from the rendering the app already applied.
    #[must_use]
    pub fn new(current: TextRendering) -> Self {
        Self { last: current }
    }

    /// Returns `next` if it differs from the last one, and remembers it.
    pub fn update(&mut self, next: TextRendering) -> Option<TextRendering> {
        (next != self.last).then(|| {
            self.last = next;
            next
        })
    }
}

/// Calls `on_change` on a background thread whenever the desktop's font
/// settings change from `current` (normally the value the app applied at
/// startup from [`crate::detect`]).
///
/// The thread lives as long as the process; the connection is made before
/// this returns, so a missing portal is reported here.
///
/// # Errors
///
/// [`WatchError::Unsupported`] off Linux, [`WatchError::Bus`] without a
/// session bus or portal, [`WatchError::Thread`] if the thread cannot start.
#[cfg(target_os = "linux")]
pub fn watch<F>(current: TextRendering, on_change: F) -> Result<(), WatchError>
where
    F: Fn(TextRendering) + Send + 'static,
{
    use crate::portal::{NAMESPACE, bus};
    use zbus::zvariant::OwnedValue;

    let bus_error = |error: zbus::Error| WatchError::Bus(error.to_string());
    let connection = bus::connect().map_err(bus_error)?;
    let proxy = bus::settings(&connection).map_err(bus_error)?;
    let signals = proxy
        .receive_signal_with_args("SettingChanged", &[(0, NAMESPACE)])
        .map_err(bus_error)?;
    std::thread::Builder::new()
        .name("fastframe-text-watch".to_owned())
        .spawn(move || {
            // The proxy owns the match rule; keep it alive with the iterator.
            let _proxy = proxy;
            let mut changes = Changes::new(current);
            for message in signals {
                let Ok((namespace, key, _value)) =
                    message.body().deserialize::<(String, String, OwnedValue)>()
                else {
                    continue;
                };
                if !is_font_setting(&namespace, &key) {
                    continue;
                }
                if let Some(next) = changes.update(crate::detect()) {
                    on_change(next);
                }
            }
        })
        .map_err(WatchError::Thread)?;
    Ok(())
}

/// Font settings cannot be watched on this platform.
///
/// # Errors
///
/// Always [`WatchError::Unsupported`].
#[cfg(not(target_os = "linux"))]
pub fn watch<F>(current: TextRendering, on_change: F) -> Result<(), WatchError>
where
    F: Fn(TextRendering) + Send + 'static,
{
    let _ = (current, on_change);
    Err(WatchError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hinting;

    #[test]
    fn only_font_keys_in_the_interface_namespace() {
        assert!(is_font_setting(
            "org.gnome.desktop.interface",
            "font-hinting"
        ));
        assert!(is_font_setting(
            "org.gnome.desktop.interface",
            "font-antialiasing"
        ));
        assert!(!is_font_setting(
            "org.gnome.desktop.interface",
            "color-scheme"
        ));
        assert!(!is_font_setting(
            "org.freedesktop.appearance",
            "font-hinting"
        ));
    }

    #[test]
    fn changes_report_only_differences() {
        let mut changes = Changes::new(TextRendering::default());
        assert_eq!(changes.update(TextRendering::default()), None);
        let none = TextRendering {
            hinting: Hinting::None,
            ..TextRendering::default()
        };
        assert_eq!(changes.update(none), Some(none));
        assert_eq!(changes.update(none), None);
        assert_eq!(
            changes.update(TextRendering::default()),
            Some(TextRendering::default())
        );
    }
}
