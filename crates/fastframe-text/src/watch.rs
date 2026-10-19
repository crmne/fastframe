//! Live changes to the desktop's font settings (feature `watch`).
//!
//! On Linux a background thread listens for the desktop portal's
//! `SettingChanged` signal on the font keys, reads the settings again with
//! [`crate::detect`], and hands the new [`TextRendering`] to a callback when
//! it differs from the last one. The app then applies it on its interface
//! thread (`apply_to` and `ctx.set_fonts`, then `apply_to_visuals`) and
//! requests a repaint.
//!
//! [`watch_text_scale`] follows the desktop's text size the same way, on a
//! thread of its own, with the value the signal carries.
//!
//! Edits to fontconfig files are not watched. On macOS and Windows there is
//! nothing to watch yet and both return [`WatchError::Unsupported`].

use crate::TextRendering;
use std::fmt;

/// Why watching could not start.
#[derive(Debug)]
#[non_exhaustive]
pub enum WatchError {
    /// This platform has no source of live font settings. For the text
    /// size, also an X11 session, where the desktop already applies it.
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

/// The text size after a `SettingChanged` signal, when the signal concerns
/// it and `read` gives a usable one that differs from `last`. The value is
/// read rather than taken from the signal, so a signal that waited while a
/// newer value was read cannot bring an older size back.
#[cfg(any(target_os = "linux", test))]
fn text_scale_change(
    last: f32,
    namespace: &str,
    key: &str,
    read: impl FnOnce() -> Option<f64>,
) -> Option<f32> {
    use crate::portal::{NAMESPACE, TEXT_SCALING_KEY, parse_text_scale};
    if namespace != NAMESPACE || key != TEXT_SCALING_KEY {
        return None;
    }
    read()
        .and_then(parse_text_scale)
        .filter(|&scale| scale != last)
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
            // The proxy owns the match rule, so it lives as long as the
            // iterator; it also reads the size.
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

/// Calls `on_change` on a background thread whenever the desktop's text
/// size changes from `current` (normally the value from
/// [`crate::text_scale`]).
///
/// A value outside the usable range is ignored, so the last size stays. The
/// thread lives as long as the process; the connection is made before this
/// returns, so a missing portal is reported here.
///
/// # Errors
///
/// [`WatchError::Unsupported`] off Linux and under X11 (see
/// [`crate::text_scale`]), [`WatchError::Bus`] without a session bus or
/// portal, [`WatchError::Thread`] if the thread cannot start.
#[cfg(target_os = "linux")]
pub fn watch_text_scale<F>(current: f32, on_change: F) -> Result<(), WatchError>
where
    F: Fn(f32) + Send + 'static,
{
    use crate::portal::{NAMESPACE, TEXT_SCALING_KEY, bus};
    use zbus::zvariant::OwnedValue;

    if !crate::wayland_session(&|name| std::env::var_os(name)) {
        return Err(WatchError::Unsupported);
    }
    let bus_error = |error: zbus::Error| WatchError::Bus(error.to_string());
    let connection = bus::connect().map_err(bus_error)?;
    let proxy = bus::settings(&connection).map_err(bus_error)?;
    let signals = proxy
        .receive_signal_with_args("SettingChanged", &[(0, NAMESPACE)])
        .map_err(bus_error)?;
    std::thread::Builder::new()
        .name("fastframe-text-scale-watch".to_owned())
        .spawn(move || {
            // The proxy owns the match rule, so it lives as long as the
            // iterator; it also reads the size.
            let read = || bus::read_text_scale(&proxy);
            let mut last = current;
            // A change before the subscription sent no signal this thread
            // sees, so read once now that later ones will arrive.
            if let Some(next) = text_scale_change(last, NAMESPACE, TEXT_SCALING_KEY, read) {
                last = next;
                on_change(next);
            }
            for message in signals {
                let Ok((namespace, key, _value)) =
                    message.body().deserialize::<(String, String, OwnedValue)>()
                else {
                    continue;
                };
                if let Some(next) = text_scale_change(last, &namespace, &key, read) {
                    last = next;
                    on_change(next);
                }
            }
        })
        .map_err(WatchError::Thread)?;
    Ok(())
}

/// The text size cannot be watched on this platform.
///
/// # Errors
///
/// Always [`WatchError::Unsupported`].
#[cfg(not(target_os = "linux"))]
pub fn watch_text_scale<F>(current: f32, on_change: F) -> Result<(), WatchError>
where
    F: Fn(f32) + Send + 'static,
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

    #[test]
    fn a_new_text_size_is_reported() {
        let interface = "org.gnome.desktop.interface";
        let key = "text-scaling-factor";
        assert_eq!(
            text_scale_change(1.0, interface, key, || Some(1.25)),
            Some(1.25)
        );
        assert_eq!(
            text_scale_change(1.25, interface, key, || Some(1.0)),
            Some(1.0)
        );
    }

    #[test]
    fn the_same_or_an_unusable_text_size_is_not() {
        let interface = "org.gnome.desktop.interface";
        let key = "text-scaling-factor";
        assert_eq!(text_scale_change(1.25, interface, key, || Some(1.25)), None);
        assert_eq!(text_scale_change(1.25, interface, key, || Some(0.0)), None);
        assert_eq!(text_scale_change(1.25, interface, key, || Some(9.0)), None);
        assert_eq!(text_scale_change(1.25, interface, key, || None), None);
    }

    #[test]
    fn a_change_the_watch_missed_is_reported() {
        // The startup read: 1.25 was set before the subscription.
        assert_eq!(
            text_scale_change(
                1.0,
                crate::portal::NAMESPACE,
                crate::portal::TEXT_SCALING_KEY,
                || { Some(1.25) }
            ),
            Some(1.25)
        );
    }

    #[test]
    fn other_settings_are_not_read() {
        let read = || -> Option<f64> { panic!("read for another setting") };
        assert_eq!(
            text_scale_change(1.0, "org.gnome.desktop.interface", "color-scheme", read),
            None
        );
    }

    #[test]
    fn other_settings_are_not_the_text_size() {
        assert_eq!(
            text_scale_change(1.0, "org.gnome.desktop.interface", "font-hinting", || Some(
                1.5
            )),
            None
        );
        assert_eq!(
            text_scale_change(
                1.0,
                "org.freedesktop.appearance",
                "text-scaling-factor",
                || Some(1.5)
            ),
            None
        );
    }
}
