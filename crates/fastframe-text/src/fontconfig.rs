//! Fontconfig's rendering settings for a family (Linux and other Unix).
//!
//! No fontconfig binding is common to the apps' dependency trees, and the C
//! library would add a build dependency, so `read` runs
//! `fc-match -f '%{hintstyle}|%{hinting}|%{antialias}' <family>` and parses
//! its one line with [`parse_fc_match`]. When `fc-match` is not installed, or
//! fails, the reader has no answer. `rgba` is not asked for: egui renders
//! grayscale only.
//!
//! `hintstyle` is fontconfig's integer: 0 none, 1 slight, 2 medium, 3 full.
//! `hinting` false means no hinting whatever the style. A field fontconfig
//! leaves empty keeps its default.

use crate::{Hinting, TextRendering};

/// The `fc-match` format `read` asks for and [`parse_fc_match`] expects.
pub const FORMAT: &str = "%{hintstyle}|%{hinting}|%{antialias}";

/// Maps fontconfig's `hintstyle` (`0`..`3`, or the `hintslight` style
/// constants) to a hinting level.
#[must_use]
pub fn parse_hintstyle(value: &str) -> Option<Hinting> {
    match value.trim() {
        "0" | "hintnone" => Some(Hinting::None),
        "1" | "hintslight" => Some(Hinting::Slight),
        "2" | "hintmedium" => Some(Hinting::Medium),
        "3" | "hintfull" => Some(Hinting::Full),
        _ => None,
    }
}

/// Parses a fontconfig boolean as `fc-match` prints it (`True`, `False`;
/// `DontCare` and anything else is no answer).
#[must_use]
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Parses `fc-match` output in [`FORMAT`].
///
/// Returns `None` when no field holds a known value.
#[must_use]
pub fn parse_fc_match(output: &str) -> Option<TextRendering> {
    let line = output.lines().next()?;
    let mut fields = line.split('|');
    let hintstyle = fields.next().and_then(parse_hintstyle);
    let hinting = fields.next().and_then(parse_bool);
    let antialias = fields.next().and_then(parse_bool);
    if hintstyle.is_none() && hinting.is_none() && antialias.is_none() {
        return None;
    }
    let default = TextRendering::default();
    let hinting = match hinting {
        Some(false) => Hinting::None,
        _ => hintstyle.unwrap_or(default.hinting),
    };
    Some(TextRendering {
        hinting,
        antialias: antialias.unwrap_or(default.antialias),
        ..default
    })
}

/// Asks fontconfig how `family` is rendered, through `fc-match`.
///
/// Returns `None` when `fc-match` is missing, fails, or prints nothing
/// useful.
#[cfg(unix)]
#[must_use]
pub fn read(family: &str) -> Option<TextRendering> {
    let output = std::process::Command::new("fc-match")
        .arg("-f")
        .arg(FORMAT)
        .arg(family)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_fc_match(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hintstyles() {
        assert_eq!(parse_hintstyle("0"), Some(Hinting::None));
        assert_eq!(parse_hintstyle("1"), Some(Hinting::Slight));
        assert_eq!(parse_hintstyle("2"), Some(Hinting::Medium));
        assert_eq!(parse_hintstyle("3"), Some(Hinting::Full));
        assert_eq!(parse_hintstyle("hintslight"), Some(Hinting::Slight));
        assert_eq!(parse_hintstyle("4"), None);
        assert_eq!(parse_hintstyle(""), None);
    }

    #[test]
    fn booleans() {
        assert_eq!(parse_bool("True"), Some(true));
        assert_eq!(parse_bool("False"), Some(false));
        assert_eq!(parse_bool("DontCare"), None);
        assert_eq!(parse_bool(""), None);
    }

    #[test]
    fn slight_hinting_on_this_desktop() {
        // hintstyle=hintslight, hinting on, grayscale antialiasing.
        assert_eq!(
            parse_fc_match("1|True|True"),
            Some(TextRendering::default())
        );
    }

    #[test]
    fn full_hinting_without_antialiasing() {
        let got = parse_fc_match("3|True|False").unwrap();
        assert_eq!(got.hinting, Hinting::Full);
        assert!(!got.antialias);
    }

    #[test]
    fn hinting_off_overrides_the_style() {
        let got = parse_fc_match("3|False|True").unwrap();
        assert_eq!(got.hinting, Hinting::None);
    }

    #[test]
    fn missing_fields_keep_their_defaults() {
        let got = parse_fc_match("|True|").unwrap();
        assert_eq!(got, TextRendering::default());
        let got = parse_fc_match("2").unwrap();
        assert_eq!(got.hinting, Hinting::Medium);
        assert!(got.antialias);
        let got = parse_fc_match("||False").unwrap();
        assert_eq!(got.hinting, Hinting::Slight);
        assert!(!got.antialias);
    }

    #[test]
    fn extra_fields_are_ignored() {
        // An older format that also asked for rgba.
        assert_eq!(
            parse_fc_match("1|True|True|1\n"),
            Some(TextRendering::default())
        );
    }

    #[test]
    fn nothing_useful_is_no_answer() {
        assert_eq!(parse_fc_match(""), None);
        assert_eq!(parse_fc_match("||"), None);
        assert_eq!(parse_fc_match("Fontconfig error"), None);
    }
}
