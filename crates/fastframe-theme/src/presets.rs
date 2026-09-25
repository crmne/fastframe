//! The palettes the apps share, embedded so every installation has them.
//!
//! Eight palettes in the sixteen base colours (Catppuccin, Catppuccin Latte,
//! Nord, Ristretto, Rosé Pine, Rosé Pine Moon, Rosé Pine Dawn, Tokyo Night).
//! They set only [`crate::BASE_COLORS`] names, so any app's palette reads
//! them; an app with more colours derives the rest ([`crate::Palette::derive`]).
//!
//! A user file with the same name in the themes directory overrides one.

use crate::{CustomTheme, Palette, parse_palette};

/// Every shared palette: its filename and its JSON.
pub const FILES: &[(&str, &str)] = &[
    (
        "Catppuccin Latte.json",
        include_str!("../themes/Catppuccin Latte.json"),
    ),
    ("Catppuccin.json", include_str!("../themes/Catppuccin.json")),
    ("Nord.json", include_str!("../themes/Nord.json")),
    ("Ristretto.json", include_str!("../themes/Ristretto.json")),
    (
        "Rose Pine Dawn.json",
        include_str!("../themes/Rose Pine Dawn.json"),
    ),
    (
        "Rose Pine Moon.json",
        include_str!("../themes/Rose Pine Moon.json"),
    ),
    ("Rose Pine.json", include_str!("../themes/Rose Pine.json")),
    (
        "Tokyo Night.json",
        include_str!("../themes/Tokyo Night.json"),
    ),
];

/// Every shared palette read into the app's palette type.
///
/// # Panics
///
/// If the app's palette rejects one of the base colour names: every
/// [`Palette`] must accept all of [`crate::BASE_COLORS`].
pub fn themes<P: Palette>() -> impl Iterator<Item = CustomTheme<P>> {
    FILES.iter().map(|(filename, text)| CustomTheme {
        filename: (*filename).to_owned(),
        palette: parse_palette(text)
            .unwrap_or_else(|error| panic!("the shared palette {filename} is invalid: {error}")),
    })
}

/// Whether `filename` names a shared palette.
#[must_use]
pub fn contains(filename: &str) -> bool {
    FILES.iter().any(|(name, _)| *name == filename)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_palette::Colors;

    #[test]
    fn every_shared_palette_uses_only_base_colours() {
        let themes: Vec<CustomTheme<Colors>> = themes().collect();
        assert_eq!(themes.len(), 8);
        for theme in &themes {
            assert!(contains(&theme.filename));
            assert_eq!(
                theme.palette.dark,
                !matches!(
                    theme.filename.as_str(),
                    "Catppuccin Latte.json" | "Rose Pine Dawn.json"
                ),
                "{}",
                theme.filename
            );
            assert_eq!(
                theme.palette.get("chat"),
                theme.palette.get("window"),
                "app colours are derived: {}",
                theme.filename
            );
        }
        assert!(!contains("omarchy.json"));
    }

    #[test]
    fn rose_pine_dawn_hovered_primary_buttons_keep_readable_content() {
        fn luminance(color: egui::Color32) -> f64 {
            let linear = [color.r(), color.g(), color.b()].map(|channel| {
                let value = f64::from(channel) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            });
            0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
        }
        let dawn = themes::<Colors>()
            .find(|theme| theme.filename == "Rose Pine Dawn.json")
            .unwrap()
            .palette;
        let foreground = luminance(dawn.get("on_accent"));
        let background = luminance(dawn.get("accent_hover"));
        let contrast = (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05);
        assert!(contrast >= 4.5, "hover contrast is only {contrast:.2}:1");
    }
}
