//! The palettes the apps share, embedded so every installation has them.
//!
//! Eight palettes in the sixteen base colours (Catppuccin, Catppuccin Latte,
//! Nord, Ristretto, Rosé Pine, Rosé Pine Moon, Rosé Pine Dawn, Tokyo Night).
//! They set only [`crate::BASE_COLORS`] names, so any app's palette reads
//! them; an app with more colours derives the rest ([`crate::Palette::derive`]).
//!
//! [`install`] writes each into the app's themes directory the first time
//! it runs, as ordinary palette files people can read, change, copy from or
//! delete. From then on they are the user's: never rewritten, and a deleted
//! one stays deleted. A palette added in a later version is installed once
//! when it arrives.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use crate::{CustomTheme, Palette, parse_palette};

/// The file in the themes directory that lists the shared palettes already
/// installed there, one filename per line.
pub const INSTALLED: &str = ".installed-palettes";

/// The folder fastframe-theme 0.1.4 kept copies of the palettes in, before
/// they went into the themes directory itself.
const OLD_EXAMPLES: &str = "examples";
/// What that folder's README said.
const OLD_EXAMPLES_README: &str = "\
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

/// Installs the shared palettes into `directory`, creating it when
/// missing: each palette not yet recorded in [`INSTALLED`] is written, unless
/// something already has its name, and recorded. Files are written through a
/// temporary file and a rename, and nothing is written through a symbolic
/// link. Returns the filenames recorded as installed, now and before.
///
/// # Errors
///
/// When the directory, a palette or the record cannot be written.
pub fn install(directory: &Path) -> io::Result<BTreeSet<String>> {
    std::fs::create_dir_all(directory)?;
    remove_old_examples(directory);
    let record = directory.join(INSTALLED);
    let mut installed: BTreeSet<String> = std::fs::read_to_string(&record)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let before = installed.len();
    for (name, contents) in FILES {
        if installed.contains(*name) {
            continue;
        }
        let path = directory.join(name);
        // Anything already there, even a broken link, is the user's.
        if std::fs::symlink_metadata(&path).is_err() {
            write_new(directory, name, contents)?;
        }
        installed.insert((*name).to_owned());
    }
    if installed.len() != before {
        let list: String = installed.iter().map(|name| format!("{name}\n")).collect();
        write_new(directory, INSTALLED, &list)?;
    }
    Ok(installed)
}

/// Writes `contents` to `name` in `directory` through a temporary file.
fn write_new(directory: &Path, name: &str, contents: &str) -> io::Result<()> {
    let temporary = directory.join(format!(".{name}.tmp"));
    let written = std::fs::write(&temporary, contents)
        .and_then(|()| std::fs::rename(&temporary, directory.join(name)));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// Removes the examples folder fastframe-theme 0.1.4 wrote, when it holds
/// nothing but the files it wrote, unchanged.
fn remove_old_examples(directory: &Path) {
    let examples = directory.join(OLD_EXAMPLES);
    if !std::fs::symlink_metadata(&examples).is_ok_and(|metadata| metadata.is_dir()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&examples) else {
        return;
    };
    let ours = |name: &str, path: &Path| {
        let expected = FILES
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, contents)| *contents)
            .or((name == "README.txt").then_some(OLD_EXAMPLES_README));
        expected.is_some_and(|expected| {
            std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
                && std::fs::read(path).is_ok_and(|bytes| bytes == expected.as_bytes())
        })
    };
    let files: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    if files.iter().all(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| ours(name, path))
    }) {
        for path in &files {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(&examples);
    }
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
    fn the_palettes_are_installed_once_and_then_belong_to_the_user() {
        let root = tempfile::tempdir().unwrap();
        let themes = root.path().join("themes");
        let installed = install(&themes).unwrap();
        assert_eq!(installed.len(), FILES.len());
        for (name, contents) in FILES {
            assert_eq!(
                std::fs::read_to_string(themes.join(name)).unwrap(),
                *contents
            );
        }
        // An edited palette is left as it is, and a deleted one stays gone.
        std::fs::write(themes.join("Nord.json"), "mine").unwrap();
        std::fs::remove_file(themes.join("Tokyo Night.json")).unwrap();
        install(&themes).unwrap();
        assert_eq!(
            std::fs::read_to_string(themes.join("Nord.json")).unwrap(),
            "mine"
        );
        assert!(!themes.join("Tokyo Night.json").exists());
        assert!(std::fs::read_dir(&themes).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn a_palette_new_to_the_record_is_installed_but_never_over_a_users_file() {
        let root = tempfile::tempdir().unwrap();
        // A record from a version that shipped only Nord.
        std::fs::write(root.path().join(INSTALLED), "Nord.json\n").unwrap();
        std::fs::write(root.path().join("Catppuccin.json"), "mine").unwrap();
        let installed = install(root.path()).unwrap();
        assert!(!root.path().join("Nord.json").exists(), "deleted before");
        assert_eq!(
            std::fs::read_to_string(root.path().join("Catppuccin.json")).unwrap(),
            "mine"
        );
        assert!(root.path().join("Rose Pine.json").is_file());
        assert_eq!(installed.len(), FILES.len());
        let record = std::fs::read_to_string(root.path().join(INSTALLED)).unwrap();
        assert_eq!(record.lines().count(), FILES.len());
    }

    #[test]
    fn the_old_examples_folder_goes_only_when_it_is_untouched() {
        let root = tempfile::tempdir().unwrap();
        let examples = root.path().join(OLD_EXAMPLES);
        std::fs::create_dir(&examples).unwrap();
        for (name, contents) in FILES {
            std::fs::write(examples.join(name), contents).unwrap();
        }
        std::fs::write(examples.join("README.txt"), OLD_EXAMPLES_README).unwrap();
        install(root.path()).unwrap();
        assert!(!examples.exists());

        let root = tempfile::tempdir().unwrap();
        let examples = root.path().join(OLD_EXAMPLES);
        std::fs::create_dir(&examples).unwrap();
        std::fs::write(examples.join("Nord.json"), "edited").unwrap();
        install(root.path()).unwrap();
        assert!(examples.join("Nord.json").is_file(), "someone's work stays");
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_link() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let target = elsewhere.path().join("target.json");
        std::os::unix::fs::symlink(&target, root.path().join("Nord.json")).unwrap();
        install(root.path()).unwrap();
        assert!(!target.exists());
    }
}
