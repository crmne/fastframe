//! The palettes the apps share, embedded so every installation has them.
//!
//! Eight palettes in the sixteen base colours (Catppuccin, Catppuccin Latte,
//! Nord, Ristretto, Rosé Pine, Rosé Pine Moon, Rosé Pine Dawn, Tokyo Night).
//! They set only [`crate::BASE_COLORS`] names, so any app's palette reads
//! them; an app with more colours derives the rest ([`crate::Palette::derive`]).
//!
//! A user file with the same name in the themes directory overrides one.
//!
//! [`write_examples`] keeps a copy of each in the themes directory's
//! `examples` folder, for people to read and copy from. The catalogue never
//! loads that folder, so the copies never freeze a palette: a later version
//! replaces them, and copying one into the themes directory makes it the
//! user's own.

use std::io;
use std::path::Path;

use crate::{CustomTheme, Palette, parse_palette};

/// The folder inside the themes directory that holds the examples.
pub const EXAMPLES: &str = "examples";

/// What the examples folder says about itself.
const EXAMPLES_README: &str = "\
These are the palettes the app ships with, as palette files.

This folder is rewritten each time the app starts, so edits here do not
last and the app never loads these files as themes. To make one your own,
copy it into the themes folder (the folder above this one), give it a new
name if you like, and change its colours. It then appears in the theme
picker; a copy with the same name as a shipped palette replaces it.

Each file names a base (dark or light) and any colours to change, as
#RRGGBB or #RRGGBBAA.
";

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

/// Writes each shared palette, and a README, into the `examples` folder of
/// `directory`, creating both folders when missing. A file whose contents
/// are already right is left alone, anything else goes through a temporary
/// file and a rename, and a symbolic link is never followed. Returns how
/// many files were written.
///
/// # Errors
///
/// When a folder or file cannot be created or written, or `examples` is
/// something other than a folder.
pub fn write_examples(directory: &Path) -> io::Result<usize> {
    let examples = directory.join(EXAMPLES);
    match std::fs::symlink_metadata(&examples) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(io::Error::other(format!(
                "{} is not a folder",
                examples.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&examples)?;
        }
        Err(error) => return Err(error),
    }
    let mut written = 0;
    for (name, contents) in FILES
        .iter()
        .copied()
        .chain(std::iter::once(("README.txt", EXAMPLES_README)))
    {
        let path = examples.join(name);
        let current = std::fs::symlink_metadata(&path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .and_then(|_| std::fs::read(&path).ok());
        if current.as_deref() == Some(contents.as_bytes()) {
            continue;
        }
        let temporary = examples.join(format!(".{name}.tmp"));
        let replaced =
            std::fs::write(&temporary, contents).and_then(|()| std::fs::rename(&temporary, &path));
        if replaced.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        replaced?;
        written += 1;
    }
    Ok(written)
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
    #[test]
    fn the_examples_are_written_once_and_restored_when_edited() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(write_examples(root.path()).unwrap(), FILES.len() + 1);
        let examples = root.path().join(EXAMPLES);
        for (name, contents) in FILES {
            assert_eq!(
                std::fs::read_to_string(examples.join(name)).unwrap(),
                *contents
            );
        }
        assert!(examples.join("README.txt").is_file());
        assert_eq!(write_examples(root.path()).unwrap(), 0, "nothing to change");

        let nord = examples.join("Nord.json");
        std::fs::write(&nord, "{}").unwrap();
        assert_eq!(write_examples(root.path()).unwrap(), 1);
        assert!(std::fs::read_to_string(&nord).unwrap().contains("colors"));
        assert!(std::fs::read_dir(&examples).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn the_examples_never_follow_a_link() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), root.path().join(EXAMPLES)).unwrap();
        assert!(write_examples(root.path()).is_err());
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);

        let root = tempfile::tempdir().unwrap();
        let examples = root.path().join(EXAMPLES);
        std::fs::create_dir(&examples).unwrap();
        let target = elsewhere.path().join("target.json");
        std::fs::write(&target, "mine").unwrap();
        std::os::unix::fs::symlink(&target, examples.join("Nord.json")).unwrap();
        write_examples(root.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "mine");
        assert!(
            !std::fs::symlink_metadata(examples.join("Nord.json"))
                .unwrap()
                .is_symlink()
        );
    }
}
