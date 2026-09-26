//! Colour palettes for egui apps: JSON palette files, a catalogue scanned
//! off the interface thread, the palettes the apps share, and following the
//! Omarchy desktop's theme.
//!
//! The app keeps its own palette type (its colours, its dark and light
//! defaults) and its mapping onto `egui::Visuals` and widgets. It implements
//! [`Palette`] so palette files can set its colours by name:
//!
//! ```
//! use egui::Color32;
//!
//! #[derive(Clone, Debug, PartialEq)]
//! struct Colors { dark: bool, window: Color32, accent: Color32 }
//!
//! impl fastframe_theme::Palette for Colors {
//!     fn base(base: fastframe_theme::Base) -> Self {
//!         match base {
//!             fastframe_theme::Base::Dark => Colors { dark: true, window: Color32::BLACK, accent: Color32::GREEN },
//!             fastframe_theme::Base::Light => Colors { dark: false, window: Color32::WHITE, accent: Color32::DARK_GREEN },
//!         }
//!     }
//!     fn set(&mut self, name: &str, color: Color32) -> bool {
//!         match name {
//!             "window" => self.window = color,
//!             "accent" => self.accent = color,
//!             _ => return false,
//!         }
//!         true
//!     }
//! }
//!
//! let palette: Colors =
//!     fastframe_theme::parse_palette(r##"{"base":"light","colors":{"accent":"#102030"}}"##)?;
//! assert!(!palette.dark);
//! assert_eq!(palette.accent, Color32::from_rgb(0x10, 0x20, 0x30));
//! # Ok::<(), String>(())
//! ```
//!
//! A palette file names a `base` (`dark`, the default, or `light`) and any
//! colours to override, as `#RRGGBB` or `#RRGGBBAA`. The sixteen names in
//! [`BASE_COLORS`] are the ones every app understands; an app may add its own
//! (ZapFast's chat and bubble colours) and derive them from the base ones when
//! a file leaves them out ([`Palette::derive`]).
//!
//! [`Catalog`] lists the palette files in a directory on a background thread,
//! adds the [`presets`] and, on Linux, the current Omarchy palette
//! ([`omarchy`]), and watches both for changes. [`Transition`] reveals a
//! change of colours from the middle of the window outwards.

use std::collections::{BTreeMap, BTreeSet};

use egui::Color32;

mod catalog;
pub mod omarchy;
pub mod presets;
mod transition;
#[cfg(target_os = "linux")]
mod watch;

pub use catalog::{
    Catalog, DesktopThemes, MAX_DIRECTORY_ENTRIES, MAX_FILE_BYTES, MAX_THEMES, Problem, Status,
    Waker,
};
pub use transition::{Reveal, Transition};

/// The colour names every app's palette understands: the sixteen interface
/// colours Spotifast defined and ZapFast adopted.
pub const BASE_COLORS: [&str; 16] = [
    "window",
    "panel",
    "surface",
    "surface_hover",
    "surface_active",
    "outline",
    "text",
    "secondary",
    "dim",
    "accent",
    "accent_hover",
    "on_accent",
    "danger",
    "warning",
    "overlay",
    "shadow",
];

/// The palette a file starts from before its overrides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Base {
    /// The app's dark palette. The default when a file names none.
    #[default]
    Dark,
    /// The app's light palette.
    Light,
}

/// An app's palette, as palette files set it.
pub trait Palette: Clone + PartialEq + Send + 'static {
    /// The app's own dark or light palette.
    fn base(base: Base) -> Self;

    /// Sets the colour called `name`. Returns `false` for a name this
    /// palette does not have, which makes the file invalid.
    fn set(&mut self, name: &str, color: Color32) -> bool;

    /// Fills colours the file left out from those it set, after every
    /// colour has been applied. `given` holds the names the file set.
    ///
    /// ZapFast derives its chat colours from a Spotifast palette's window,
    /// surface and accent this way. The default does nothing.
    fn derive(&mut self, given: &BTreeSet<&str>) {
        let _ = given;
    }
}

/// A palette file's contents, before they meet a palette type.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteFile {
    #[serde(default)]
    base: Base,
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

/// Reads a palette file: its base, then each colour it sets.
///
/// Fails on invalid JSON, an unknown field, a colour that is not `#RRGGBB`
/// or `#RRGGBBAA`, or a colour name the palette does not have. The error
/// names the colour.
pub fn parse_palette<P: Palette>(text: &str) -> Result<P, String> {
    let file: PaletteFile = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let mut palette = P::base(file.base);
    for (name, value) in &file.colors {
        let color =
            parse_color(value).ok_or_else(|| format!("{name}: expected #RRGGBB or #RRGGBBAA"))?;
        if !palette.set(name, color) {
            return Err(format!("unknown color: {name}"));
        }
    }
    let given: BTreeSet<&str> = file.colors.keys().map(String::as_str).collect();
    palette.derive(&given);
    Ok(palette)
}

/// Reads `#RRGGBB` or `#RRGGBBAA` (alpha not premultiplied).
#[must_use]
pub fn parse_color(value: &str) -> Option<Color32> {
    let hex = value.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    let [a, b, c, d] = value.to_be_bytes();
    Some(if hex.len() == 6 {
        Color32::from_rgb(b, c, d)
    } else {
        Color32::from_rgba_unmultiplied(a, b, c, d)
    })
}

/// A palette file, known by its filename in the themes directory.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomTheme<P> {
    /// The file name, such as `Nord.json` or [`omarchy::FILENAME`].
    pub filename: String,
    /// Its colours.
    pub palette: P,
}

/// The name a theme's filename is shown under: "Omarchy" for the live
/// desktop palette, and the filename without its `.json` otherwise.
#[must_use]
pub fn display_name(filename: &str) -> &str {
    if filename == omarchy::FILENAME {
        return "Omarchy";
    }
    let stem = filename.len().checked_sub(".json".len()).and_then(|at| {
        filename
            .get(at..)
            .filter(|extension| extension.eq_ignore_ascii_case(".json"))
            .and_then(|_| filename.get(..at))
    });
    stem.filter(|stem| !stem.is_empty()).unwrap_or(filename)
}

/// Reads a cached theme from settings, treating a damaged one as absent so
/// the rest of the settings still load.
///
/// For `#[serde(default, deserialize_with = "fastframe_theme::read_cached_theme")]`
/// on an `Option<CustomTheme<Palette>>` field.
pub fn read_cached_theme<'de, D, P>(deserializer: D) -> Result<Option<CustomTheme<P>>, D::Error>
where
    D: serde::Deserializer<'de>,
    P: serde::de::DeserializeOwned,
{
    use serde::Deserialize as _;
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    match serde_json::from_value(value) {
        Ok(theme) => Ok(Some(theme)),
        Err(error) => {
            log::warn!("ignoring an unreadable cached theme: {error}");
            Ok(None)
        }
    }
}

/// A palette type for the crate's tests: the sixteen base colours plus one
/// app colour derived from them, as ZapFast's `chat`.
#[cfg(test)]
pub(crate) mod test_palette {
    use super::{BASE_COLORS, Base, Palette};
    use egui::Color32;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    pub(crate) struct Colors {
        pub(crate) dark: bool,
        pub(crate) colors: BTreeMap<String, [u8; 4]>,
    }

    impl Colors {
        pub(crate) fn get(&self, name: &str) -> Color32 {
            let [r, g, b, a] = self.colors[name];
            Color32::from_rgba_premultiplied(r, g, b, a)
        }
    }

    impl Palette for Colors {
        fn base(base: Base) -> Self {
            let (dark, fill) = match base {
                Base::Dark => (true, Color32::BLACK),
                Base::Light => (false, Color32::WHITE),
            };
            let colors = BASE_COLORS
                .iter()
                .chain(&["chat"])
                .map(|name| ((*name).to_owned(), fill.to_array()))
                .collect();
            Self { dark, colors }
        }

        fn set(&mut self, name: &str, color: Color32) -> bool {
            match self.colors.get_mut(name) {
                Some(slot) => {
                    *slot = color.to_array();
                    true
                }
                None => false,
            }
        }

        fn derive(&mut self, given: &BTreeSet<&str>) {
            if given.contains("window") && !given.contains("chat") {
                let window = self.colors["window"];
                self.colors.insert("chat".into(), window);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_palette::Colors;
    use super::*;

    #[test]
    fn overrides_inherit_the_base_and_support_alpha() {
        let palette: Colors =
            parse_palette(r##"{"base":"light","colors":{"text":"#ebdbb2","shadow":"#00000080"}}"##)
                .unwrap();
        assert!(!palette.dark);
        assert_eq!(palette.get("window"), Color32::WHITE);
        assert_eq!(palette.get("text"), Color32::from_rgb(235, 219, 178));
        assert_eq!(palette.get("shadow"), Color32::from_black_alpha(128));
        assert_eq!(
            parse_palette::<Colors>("{}").unwrap(),
            Colors::base(Base::Dark)
        );
    }

    #[test]
    fn invalid_palettes_are_rejected() {
        for text in [
            r##"{"colors":{"text":"#fff"}}"##,
            r##"{"colors":{"text":"#zzzzzz"}}"##,
            r##"{"colors":{"text":"ffffff"}}"##,
            r##"{"colors":{"typo":"#ffffff"}}"##,
            r#"{"base":"system"}"#,
            r#"{"typo":true}"#,
            "not json",
        ] {
            assert!(parse_palette::<Colors>(text).is_err(), "{text}");
        }
        assert_eq!(
            parse_palette::<Colors>(r##"{"colors":{"typo":"#ffffff"}}"##).unwrap_err(),
            "unknown color: typo"
        );
    }

    #[test]
    fn app_colours_are_derived_only_when_the_file_leaves_them_out() {
        let derived: Colors = parse_palette(r##"{"colors":{"window":"#102030"}}"##).unwrap();
        assert_eq!(derived.get("chat"), Color32::from_rgb(0x10, 0x20, 0x30));
        let explicit: Colors =
            parse_palette(r##"{"colors":{"window":"#102030","chat":"#405060"}}"##).unwrap();
        assert_eq!(explicit.get("chat"), Color32::from_rgb(0x40, 0x50, 0x60));
    }

    #[test]
    fn colours_parse_with_and_without_alpha() {
        assert_eq!(parse_color("#0a0b0c"), Some(Color32::from_rgb(10, 11, 12)));
        assert_eq!(
            parse_color("#0a0b0c80"),
            Some(Color32::from_rgba_unmultiplied(10, 11, 12, 128))
        );
        for bad in [
            "", "#", "#12345", "#1234567", "0a0b0c", "#0a0b0g", "#+a0b0c",
        ] {
            assert_eq!(parse_color(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_damaged_cache_reads_as_absent() {
        #[derive(serde::Deserialize)]
        struct Settings {
            #[serde(default, deserialize_with = "read_cached_theme")]
            theme: Option<CustomTheme<Colors>>,
        }
        let read = |text: &str| serde_json::from_str::<Settings>(text).unwrap().theme;
        assert_eq!(read(r#"{"theme":{"filename":"x.json"}}"#), None);
        assert_eq!(read(r#"{"theme":null}"#), None);
        assert_eq!(read("{}"), None);
        let theme = CustomTheme {
            filename: "x.json".to_owned(),
            palette: Colors::base(Base::Light),
        };
        let text = format!(r#"{{"theme":{}}}"#, serde_json::to_string(&theme).unwrap());
        assert_eq!(read(&text), Some(theme));
    }

    #[test]
    fn the_live_desktop_palette_has_a_name() {
        assert_eq!(display_name("omarchy.json"), "Omarchy");
        assert_eq!(display_name("Nord.json"), "Nord");
        assert_eq!(display_name("Rose Pine Dawn.json"), "Rose Pine Dawn");
        assert_eq!(display_name("mine.JSON"), "mine");
        assert_eq!(display_name(".json"), ".json", "nothing left to show");
        assert_eq!(display_name("notes"), "notes");
    }
}
