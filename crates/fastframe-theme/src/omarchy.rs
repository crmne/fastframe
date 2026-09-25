//! Following the [Omarchy](https://omarchy.org) desktop's theme.
//!
//! Omarchy keeps the current theme in `~/.local/state/omarchy/current/theme`
//! and renders each app's template (`<slug>.json.tpl`) into it when the theme
//! changes. On a Linux desktop with Omarchy configured, the [`crate::Catalog`]
//! reads that palette as [`FILENAME`] and follows it live:
//!
//! 1. the rendered `<slug>.json` in the current theme, when Omarchy has one;
//! 2. otherwise the user's template in `~/.config/omarchy/themed`, or the
//!    app's own, rendered here from `omarchy-theme-color --all` (so source and
//!    portable builds follow too, before any package installed a template).
//!
//! A native package can also ship `share/<slug>/omarchy/<slug>.json.tpl` and
//! the hook `<slug>-theme` ([`hook_script`]). The first launch then installs
//! both for the user (never replacing existing files), so Omarchy renders the
//! palette itself and the hook asks a running app to reload. Packaging files
//! stay in each app (`contrib/omarchy/`).

/// The filename the live Omarchy palette is listed under.
pub const FILENAME: &str = "omarchy.json";

/// The Omarchy template for the sixteen base colours
/// ([`crate::BASE_COLORS`]), as Spotifast ships it. An app with more colours
/// ships its own template with those added.
pub const BASE_TEMPLATE: &str = include_str!("../templates/base.json.tpl");

/// The Omarchy `theme-set` hook an app ships as `contrib/omarchy/<slug>-theme`.
///
/// It copies the palette Omarchy rendered for the app into the app's themes
/// directory as [`FILENAME`], atomically, and asks a running instance to
/// reload with `<slug> reload-themes` (a stopped app stays stopped). Apps can
/// test their shipped copy against this text so it cannot drift.
#[must_use]
pub fn hook_script(slug: &str) -> String {
    let variable = slug.to_ascii_uppercase().replace('-', "_");
    format!(
        r#"#!/bin/bash
# Install with: omarchy hook install theme-set contrib/omarchy/{slug}-theme
set -euo pipefail

# These overrides also let the hook be tested without changing the desktop.
theme_dir=${{{variable}_OMARCHY_THEME_DIR:-$HOME/.local/state/omarchy/current/theme}}
themes_dir=${{{variable}_THEMES_DIR:-${{XDG_CONFIG_HOME:-$HOME/.config}}/{slug}/themes}}
source_file=$theme_dir/{slug}.json

# A theme without a generated palette leaves the last accepted colors alone.
[[ -f $source_file && ! -L $source_file ]] || exit 0
mkdir -p -- "$themes_dir"
temporary=$(mktemp "$themes_dir/.omarchy.XXXXXX")
trap 'rm -f -- "$temporary"' EXIT
cp -- "$source_file" "$temporary"
chmod 644 "$temporary"
mv -f -- "$temporary" "$themes_dir/{FILENAME}"

# This command only contacts an existing instance. A stopped app stays stopped.
# The next launch reads the file if no instance is running or predates reload.
{slug} reload-themes >/dev/null 2>&1 || true
"#
    )
}

/// Renders an Omarchy template with the colours `omarchy-theme-color --all`
/// printed (one `name<TAB>#rrggbb` per line), for the first use before
/// Omarchy has rendered it itself.
///
/// Supports the placeholders the apps' templates use: `{{ name }}` and
/// `{{ mix from to N% }}` (N% of the way from `from` to `to`, rounded as
/// Omarchy does). The result must be a valid palette for `P`.
pub fn render_seed<P: crate::Palette>(template: &str, colors: &str) -> Result<String, String> {
    let colors: std::collections::BTreeMap<_, _> = colors
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect();
    let get = |key: &str| {
        colors
            .get(key)
            .copied()
            .ok_or_else(|| format!("missing Omarchy color {key:?}"))
    };
    let rgb = |value: &str| -> Result<u32, String> {
        value
            .strip_prefix('#')
            .filter(|hex| hex.len() == 6)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .ok_or_else(|| "expected an RGB color".to_owned())
    };
    let mut output = String::new();
    let mut rest = template;
    while let Some((before, token)) = rest.split_once("{{") {
        output.push_str(before);
        let (token, after) = token
            .split_once("}}")
            .ok_or("incomplete palette placeholder")?;
        let words: Vec<_> = token.split_whitespace().collect();
        match words.as_slice() {
            [key] => output.push_str(get(key)?),
            ["mix", from, to, percent] => {
                let percent: u32 = percent
                    .strip_suffix('%')
                    .and_then(|p| p.parse().ok())
                    .filter(|p| *p <= 100)
                    .ok_or("unsupported palette mix")?;
                let (from, to) = (rgb(get(from)?)?, rgb(get(to)?)?);
                output.push('#');
                for shift in [16, 8, 0] {
                    let value = (((from >> shift) & 255) * (100 - percent)
                        + ((to >> shift) & 255) * percent
                        + 50)
                        / 100;
                    output.push_str(&format!("{value:02x}"));
                }
            }
            _ => return Err("unsupported palette placeholder".into()),
        }
        rest = after;
    }
    output.push_str(rest);
    crate::parse_palette::<P>(&output)?;
    Ok(output)
}

#[cfg(target_os = "linux")]
pub(crate) use setup::Setup;

#[cfg(target_os = "linux")]
mod setup {
    use std::fs;
    use std::io::{self, Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use crate::{CustomTheme, Palette};

    /// Files Omarchy hands over are small; anything larger is not a palette.
    const LIMIT: u64 = 64 * 1024;

    /// The user's Omarchy set-up, and the app's packaged assets if any.
    #[derive(Clone, Debug)]
    pub(crate) struct Setup {
        pub(crate) slug: &'static str,
        pub(crate) template: &'static str,
        pub(crate) assets: PathBuf,
        pub(crate) home: PathBuf,
    }

    impl Setup {
        /// The set-up for the running executable: packaged assets in
        /// `<prefix>/share/<slug>/omarchy` beside `<prefix>/bin`.
        pub(crate) fn discover(slug: &'static str, template: &'static str) -> Option<Self> {
            let executable = std::env::current_exe().ok()?;
            Some(Self {
                slug,
                template,
                assets: executable
                    .parent()?
                    .parent()?
                    .join("share")
                    .join(slug)
                    .join("omarchy"),
                home: directories::BaseDirs::new()?.home_dir().to_path_buf(),
            })
        }

        /// A package shipped the template and hook, and Omarchy is set up.
        pub(crate) fn available(&self) -> bool {
            self.assets.is_dir()
                && self.home.join(".config/omarchy").is_dir()
                && self.watch_directory().join("theme").is_dir()
        }

        /// Omarchy is set up for this user, packaged or not.
        pub(crate) fn active(&self) -> bool {
            self.home.join(".config/omarchy").is_dir() && self.watch_directory().is_dir()
        }

        /// Where Omarchy switches the current theme. Watched recursively,
        /// because Omarchy replaces the `theme` directory as a whole.
        pub(crate) fn watch_directory(&self) -> PathBuf {
            self.home.join(".local/state/omarchy/current")
        }

        /// The current Omarchy palette.
        pub(crate) fn current_theme<P: Palette>(&self) -> io::Result<CustomTheme<P>> {
            self.current_theme_with(read_colors)
        }

        pub(crate) fn current_theme_with<P: Palette>(
            &self,
            colors: impl FnOnce(&Path) -> io::Result<String>,
        ) -> io::Result<CustomTheme<P>> {
            let current = self.watch_directory().join("theme");
            let rendered = current.join(format!("{}.json", self.slug));
            let text = if rendered.is_file() {
                read_small(&rendered)?
            } else {
                let custom = self
                    .home
                    .join(".config/omarchy/themed")
                    .join(format!("{}.json.tpl", self.slug));
                let template = if custom.is_file() {
                    read_small(&custom)?
                } else {
                    self.template.to_owned()
                };
                super::render_seed::<P>(&template, &colors(&current.join("colors.toml"))?)
                    .map_err(io::Error::other)?
            };
            Ok(CustomTheme {
                filename: super::FILENAME.into(),
                palette: crate::parse_palette(&text).map_err(io::Error::other)?,
            })
        }

        /// Installs the packaged template and hook for the user and seeds the
        /// palette file, without replacing anything that exists. Does nothing
        /// unless a package shipped them and Omarchy is set up.
        pub(crate) fn install<P: Palette>(&self, themes: &Path) -> io::Result<()> {
            if !self.available() {
                return Ok(());
            }
            let config = self.home.join(".config/omarchy");
            let current = self.watch_directory().join("theme");
            let slug = self.slug;
            let template = read_small(&self.assets.join(format!("{slug}.json.tpl")))?;
            let hook = read_small(&self.assets.join(format!("{slug}-theme")))?;
            let template_path = config.join("themed").join(format!("{slug}.json.tpl"));
            create_only(&template_path, template.as_bytes(), 0o644)?;
            create_only(
                &config
                    .join("hooks/theme-set.d")
                    .join(format!("{slug}-theme")),
                hook.as_bytes(),
                0o755,
            )?;

            let destination = themes.join(super::FILENAME);
            if fs::symlink_metadata(&destination).is_ok() {
                return Ok(());
            }
            // Seed the current palette without reapplying the desktop theme.
            // Later changes come from Omarchy's own renderer and the hook.
            let rendered = current.join(format!("{slug}.json"));
            let palette = if rendered.is_file() {
                read_small(&rendered)?
            } else {
                super::render_seed::<P>(
                    &read_small(&template_path)?,
                    &read_colors(&current.join("colors.toml"))?,
                )
                .map_err(io::Error::other)?
            };
            crate::parse_palette::<P>(&palette).map_err(io::Error::other)?;
            create_only(&destination, palette.as_bytes(), 0o644)
        }
    }

    /// Omarchy's current colours, from its own tool.
    fn read_colors(path: &Path) -> io::Result<String> {
        let output = Command::new("omarchy-theme-color")
            .arg("--file")
            .arg(path)
            .arg("--all")
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() || output.stdout.len() as u64 > LIMIT {
            return Err(io::Error::other(
                "Omarchy's current colors could not be read",
            ));
        }
        String::from_utf8(output.stdout).map_err(io::Error::other)
    }

    fn read_small(path: &Path) -> io::Result<String> {
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            return Err(io::Error::other("Omarchy theme file exceeds 64 KiB"));
        }
        String::from_utf8(bytes).map_err(io::Error::other)
    }

    /// Publishes a complete file only when the destination does not exist.
    /// A concurrent user edit, or even a broken symbolic link, is preserved.
    pub(super) fn create_only(destination: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(".fastframe-setup-{}", unique()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            match fs::hard_link(&temporary, destination) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                result => result,
            }
        })();
        let _ = fs::remove_file(temporary);
        result
    }

    /// A name no other process or call uses at the same time: the process,
    /// the time, and a counter.
    fn unique() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        format!(
            "{}-{nanos:x}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::test_palette::Colors;
        use std::os::unix::fs::{PermissionsExt, symlink};

        const FIXTURES: [(&str, &str); 2] = [
            (
                include_str!("../tests/fixtures/omarchy/catppuccin.tsv"),
                include_str!("../tests/fixtures/omarchy/catppuccin.json"),
            ),
            (
                include_str!("../tests/fixtures/omarchy/catppuccin-latte.tsv"),
                include_str!("../tests/fixtures/omarchy/catppuccin-latte.json"),
            ),
        ];

        fn setup(root: &Path) -> Setup {
            Setup {
                slug: "app",
                template: crate::omarchy::BASE_TEMPLATE,
                assets: root.join("package/share/app/omarchy"),
                home: root.join("user"),
            }
        }

        #[test]
        fn following_needs_no_packaged_assets_or_user_hooks() {
            let root = tempfile::tempdir().unwrap();
            let setup = setup(root.path());
            let config = setup.home.join(".config/omarchy");
            let current = setup.watch_directory().join("theme");
            fs::create_dir_all(&config).unwrap();
            fs::create_dir_all(&current).unwrap();
            assert!(setup.active());
            assert!(!setup.available());
            for (colors, expected) in FIXTURES {
                let theme: CustomTheme<Colors> = setup
                    .current_theme_with(|path| {
                        assert_eq!(path, current.join("colors.toml"));
                        Ok(colors.into())
                    })
                    .unwrap();
                assert_eq!(theme.filename, "omarchy.json");
                assert_eq!(theme.palette, crate::parse_palette(expected).unwrap());
            }
            assert_eq!(
                fs::read_dir(&config).unwrap().count(),
                0,
                "following installs no desktop files"
            );
            fs::remove_dir(&current).unwrap();
            assert!(
                setup.active(),
                "keep the cached palette while Omarchy replaces its theme directory"
            );
        }

        #[test]
        fn omarchys_rendering_and_the_users_template_win_over_the_apps() {
            let root = tempfile::tempdir().unwrap();
            let setup = setup(root.path());
            let current = setup.watch_directory().join("theme");
            let themed = setup.home.join(".config/omarchy/themed");
            fs::create_dir_all(&current).unwrap();
            fs::create_dir_all(&themed).unwrap();
            fs::write(themed.join("app.json.tpl"), r#"{"base":"light"}"#).unwrap();
            let theme: CustomTheme<Colors> =
                setup.current_theme_with(|_| Ok(String::new())).unwrap();
            assert!(!theme.palette.dark, "the user's template");
            fs::write(
                current.join("app.json"),
                r##"{"colors":{"text":"#010203"}}"##,
            )
            .unwrap();
            let theme: CustomTheme<Colors> = setup
                .current_theme_with(|_| panic!("no colours are needed"))
                .unwrap();
            assert_eq!(theme.palette.get("text"), egui::Color32::from_rgb(1, 2, 3));
        }

        #[test]
        fn setup_requires_both_a_package_and_an_omarchy_desktop() {
            let root = tempfile::tempdir().unwrap();
            let setup = setup(root.path());
            let themes = root.path().join("profile/themes");
            setup.install::<Colors>(&themes).unwrap();
            assert!(!setup.home.exists());
            fs::create_dir_all(&setup.assets).unwrap();
            setup.install::<Colors>(&themes).unwrap();
            assert!(!setup.home.exists());
            assert!(!themes.exists());
        }

        #[test]
        fn setup_is_per_user_and_preserves_customizations() {
            let root = tempfile::tempdir().unwrap();
            let setup = setup(root.path());
            let config = setup.home.join(".config/omarchy");
            let current = setup.watch_directory().join("theme");
            let themes = root.path().join("profile/themes");
            for path in [&setup.assets, &config, &current, &themes] {
                fs::create_dir_all(path).unwrap();
            }
            let hook = crate::omarchy::hook_script("app");
            fs::write(
                setup.assets.join("app.json.tpl"),
                crate::omarchy::BASE_TEMPLATE,
            )
            .unwrap();
            fs::write(setup.assets.join("app-theme"), &hook).unwrap();
            fs::write(current.join("app.json"), "{}").unwrap();
            let settings = root.path().join("profile/settings.json");
            fs::write(&settings, "existing preferences").unwrap();
            setup.install::<Colors>(&themes).unwrap();
            let template = config.join("themed/app.json.tpl");
            let installed_hook = config.join("hooks/theme-set.d/app-theme");
            assert_eq!(
                fs::read_to_string(&template).unwrap(),
                crate::omarchy::BASE_TEMPLATE
            );
            assert_eq!(fs::read_to_string(&installed_hook).unwrap(), hook);
            let mode = fs::metadata(&installed_hook).unwrap().permissions().mode();
            assert_ne!(mode & 0o100, 0, "the hook is executable");
            assert_eq!(
                fs::read_to_string(themes.join("omarchy.json")).unwrap(),
                "{}"
            );

            fs::write(&template, "user template").unwrap();
            fs::write(&installed_hook, "user hook").unwrap();
            fs::write(themes.join("omarchy.json"), "user palette").unwrap();
            setup.install::<Colors>(&themes).unwrap();
            assert_eq!(fs::read_to_string(&template).unwrap(), "user template");
            assert_eq!(fs::read_to_string(&installed_hook).unwrap(), "user hook");
            assert_eq!(
                fs::read_to_string(themes.join("omarchy.json")).unwrap(),
                "user palette"
            );
            assert_eq!(
                fs::read_to_string(&settings).unwrap(),
                "existing preferences"
            );
            assert_eq!(fs::read_dir(template.parent().unwrap()).unwrap().count(), 1);
            assert_eq!(
                fs::read_dir(installed_hook.parent().unwrap())
                    .unwrap()
                    .count(),
                1
            );
            assert_eq!(fs::read_dir(&themes).unwrap().count(), 1);
        }

        #[test]
        fn publishing_never_replaces_an_existing_or_broken_link() {
            let root = tempfile::tempdir().unwrap();
            let outside = root.path().join("keep");
            fs::write(&outside, "unchanged").unwrap();
            for target in [outside.clone(), root.path().join("missing")] {
                let link = root.path().join("theme.json");
                symlink(&target, &link).unwrap();
                create_only(&link, b"replacement", 0o644).unwrap();
                assert_eq!(fs::read_link(&link).unwrap(), target);
                fs::remove_file(link).unwrap();
            }
            assert_eq!(fs::read_to_string(outside).unwrap(), "unchanged");
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_palette::Colors;

    #[test]
    fn the_first_palette_matches_omarchys_renderer_in_light_and_dark_themes() {
        for (colors, expected) in [
            (
                include_str!("../tests/fixtures/omarchy/catppuccin.tsv"),
                include_str!("../tests/fixtures/omarchy/catppuccin.json"),
            ),
            (
                include_str!("../tests/fixtures/omarchy/catppuccin-latte.tsv"),
                include_str!("../tests/fixtures/omarchy/catppuccin-latte.json"),
            ),
        ] {
            let actual = render_seed::<Colors>(BASE_TEMPLATE, colors).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&actual).unwrap(),
                serde_json::from_str::<serde_json::Value>(expected).unwrap()
            );
        }
    }

    #[test]
    fn broken_templates_and_colours_are_refused() {
        assert!(render_seed::<Colors>(BASE_TEMPLATE, "mode\tdark\n").is_err());
        assert!(render_seed::<Colors>("{{ missing }}", "").is_err());
        assert!(render_seed::<Colors>("{{", "").is_err());
        assert!(render_seed::<Colors>("{{ mix a b 101% }}", "a\t#000000\nb\t#ffffff").is_err());
        assert!(render_seed::<Colors>("{{ blend a b }}", "a\t#000000\nb\t#ffffff").is_err());
        assert!(
            render_seed::<Colors>(r#"{"colors":{"typo":"{{ a }}"}}"#, "a\t#000000").is_err(),
            "the result must be a palette the app reads"
        );
    }

    #[test]
    fn mixing_rounds_like_omarchy() {
        let rendered = render_seed::<Colors>(
            r#"{"colors":{"window":"{{ mix a b 50% }}","text":"{{ mix a b 0% }}"}}"#,
            "a\t#000000\nb\t#ffffff\n",
        )
        .unwrap();
        assert_eq!(
            rendered,
            r##"{"colors":{"window":"#808080","text":"#000000"}}"##
        );
    }

    #[test]
    fn the_hook_is_the_one_the_apps_ship() {
        assert_eq!(
            hook_script("zapfast"),
            include_str!("../tests/fixtures/omarchy/zapfast-theme")
        );
        assert_eq!(
            hook_script("spotifast"),
            include_str!("../tests/fixtures/omarchy/spotifast-theme")
        );
        assert!(hook_script("rekord-flash").contains("${REKORD_FLASH_THEMES_DIR:-"));
    }
}
