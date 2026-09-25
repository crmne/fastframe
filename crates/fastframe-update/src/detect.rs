//! Whether this copy of the app may replace itself.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::UpdateConfig;
use crate::host::Host;

/// How the app was installed, which decides the download and how it is
/// applied. The variant names are part of `handoff.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// An executable unpacked from the `.tar.gz` (Linux) or `.zip`
    /// (Windows) download, next to a `<slug>-portable.txt` marker.
    Portable,
    /// Installed by the Windows setup program, which left a
    /// `<slug>-installer.txt` marker. Updated by running the new setup
    /// program silently.
    WindowsInstaller,
    /// A macOS app bundle from the disk image, outside Homebrew.
    MacBundle,
}

/// An installation that can update itself. The shape is part of
/// `handoff.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    /// The running executable, canonicalized.
    pub executable: PathBuf,
    /// How it was installed.
    pub kind: Kind,
}

impl Installation {
    /// What an update replaces: the bundle on macOS, otherwise the
    /// executable.
    pub(crate) fn root(&self, config: &UpdateConfig) -> anyhow::Result<&Path> {
        if self.kind == Kind::MacBundle {
            return crate::macos::bundle_root(config, &self.executable)
                .ok_or_else(|| anyhow::anyhow!(Unsupported::MoveToApplications));
        }
        Ok(&self.executable)
    }
}

/// A system package manager that owns the executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PackageManager {
    /// Debian and Ubuntu packages (`dpkg-query -S`).
    Apt,
    /// Fedora and openSUSE packages (`rpm -qf`).
    Dnf,
    /// Arch packages, including the AUR (`pacman -Qo`).
    Pacman,
}

impl PackageManager {
    fn name(self) -> &'static str {
        match self {
            Self::Apt => "apt",
            Self::Dnf => "dnf",
            Self::Pacman => "pacman",
        }
    }
}

/// Why this copy cannot update itself. The `Display` text is a sentence for
/// the user; match on the variant to word it in the app's own language.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unsupported {
    /// Installed from Flathub or another Flatpak remote.
    Flatpak,
    /// Installed from the Snap Store.
    Snap,
    /// Built with `cargo install`.
    Cargo,
    /// Installed with Nix.
    Nix,
    /// Installed with Homebrew, as a formula or a cask.
    Homebrew,
    /// Owned by a system package manager.
    SystemPackage(PackageManager),
    /// In a system directory, but no package manager claims it.
    SystemDirectory,
    /// Neither a portable nor an installer marker sits next to the
    /// executable, so it is not a copy from the download page.
    NotPortable,
    /// A macOS app that runs from the disk image, from a translocated
    /// quarantine copy, or outside an app bundle.
    MoveToApplications,
    /// A macOS bundle whose identifier or executable is not this app's.
    ForeignBundle,
    /// No download exists for this operating system or processor.
    Platform,
    /// The installation could not be inspected.
    Unavailable(String),
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Flatpak => formatter
                .write_str("Update this installation through your software center or flatpak update."),
            Self::Snap => formatter.write_str("Update this installation with snap refresh."),
            Self::Cargo => formatter.write_str("Update this installation with cargo install."),
            Self::Nix => formatter.write_str("Update this installation with Nix."),
            Self::Homebrew => formatter.write_str("Update this installation with Homebrew."),
            Self::SystemPackage(manager) => write!(
                formatter,
                "Update this installation through {} or your software center.",
                manager.name()
            ),
            Self::SystemDirectory => formatter.write_str(
                "This installation is in a system directory. Use your package manager or the download page.",
            ),
            Self::NotPortable => formatter.write_str(
                "This installation does not identify itself as a portable download. Use the download page to install an update-enabled build.",
            ),
            Self::MoveToApplications => {
                formatter.write_str("Move the app to Applications, then open it to update.")
            }
            Self::ForeignBundle => {
                formatter.write_str("This app bundle is not a release of this app.")
            }
            Self::Platform => formatter
                .write_str("Use the download page for this operating system or architecture."),
            Self::Unavailable(reason) => {
                write!(formatter, "Cannot inspect this installation: {reason}")
            }
        }
    }
}

impl std::error::Error for Unsupported {}

/// The operating system whose rules apply. Detection takes it as a value so
/// every platform's rules are tested everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Linux,
    Windows,
    MacOs,
    Other,
}

impl Platform {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

/// Decides how `executable` was installed.
pub(crate) fn detect(
    config: &UpdateConfig,
    host: &dyn Host,
    platform: Platform,
    executable: &Path,
) -> Result<Installation, Unsupported> {
    let path = executable.to_string_lossy().replace('\\', "/");
    let lower = path.to_lowercase();
    if host.var("FLATPAK_ID").is_some() || path.starts_with("/app/") {
        return Err(Unsupported::Flatpak);
    }
    if host.var("SNAP").is_some() || path.starts_with("/snap/") {
        return Err(Unsupported::Snap);
    }
    if lower.contains("/.cargo/") {
        return Err(Unsupported::Cargo);
    }
    if path.starts_with("/nix/") {
        return Err(Unsupported::Nix);
    }
    if lower.contains("/cellar/") || lower.contains("/caskroom/") {
        return Err(Unsupported::Homebrew);
    }
    let found = |kind| {
        Ok(Installation {
            executable: executable.to_owned(),
            kind,
        })
    };
    match platform {
        Platform::MacOs => {
            crate::macos::detect(config, host, executable)?;
            found(Kind::MacBundle)
        }
        Platform::Linux | Platform::Windows => {
            if platform == Platform::Linux {
                for (program, flag, manager) in [
                    ("dpkg-query", "-S", PackageManager::Apt),
                    ("rpm", "-qf", PackageManager::Dnf),
                    ("pacman", "-Qo", PackageManager::Pacman),
                ] {
                    if host.package_owner(program, flag, executable) {
                        return Err(Unsupported::SystemPackage(manager));
                    }
                }
                if ["/usr/", "/bin/", "/sbin/"]
                    .iter()
                    .any(|prefix| path.starts_with(prefix))
                {
                    return Err(Unsupported::SystemDirectory);
                }
            }
            let directory = executable.parent().ok_or(Unsupported::NotPortable)?;
            if platform == Platform::Windows
                && windows_installer(config, host, executable, directory)
            {
                return found(Kind::WindowsInstaller);
            }
            if marker(config, directory, "portable") {
                found(Kind::Portable)
            } else {
                Err(Unsupported::NotPortable)
            }
        }
        Platform::Other => Err(Unsupported::Platform),
    }
}

/// Whether `<name>-<kind>.txt` in `directory` reads `<name>-<kind>-v1`, for
/// the slug or a legacy name.
fn marker(config: &UpdateConfig, directory: &Path, kind: &str) -> bool {
    config.names().any(|name| {
        fs::read_to_string(directory.join(format!("{name}-{kind}.txt")))
            .is_ok_and(|value| value.trim() == format!("{name}-{kind}-v1"))
    })
}

fn windows_installer(
    config: &UpdateConfig,
    host: &dyn Host,
    executable: &Path,
    directory: &Path,
) -> bool {
    if marker(config, directory, "installer") {
        return true;
    }
    // Installs from before the marker: the setup program's default folder,
    // with its uninstaller.
    let Some(base) = host.var("LOCALAPPDATA").map(PathBuf::from) else {
        return false;
    };
    let default = format!("Programs/{}/{}.exe", config.app_name, config.slug);
    std::iter::once(default.as_str())
        .chain(config.legacy_windows_installs.iter().copied())
        .any(|relative| {
            base.join(relative)
                .canonicalize()
                .is_ok_and(|installed| installed == executable)
        })
        && directory.join("unins000.exe").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;
    use crate::tests::ZAPFAST;

    const SPOTIFAST: UpdateConfig = UpdateConfig {
        legacy_names: &["fastpotify"],
        legacy_windows_installs: &["Programs/Fastpotify/fastpotify.exe"],
        ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.1")
    };

    fn linux(path: &str, host: &FakeHost) -> Result<Installation, Unsupported> {
        detect(&ZAPFAST, host, Platform::Linux, Path::new(path))
    }

    #[test]
    fn package_managed_paths_are_refused_on_every_platform() {
        let host = FakeHost::default();
        for (path, expected) in [
            ("/app/bin/zapfast", Unsupported::Flatpak),
            ("/snap/zapfast/12/bin/zapfast", Unsupported::Snap),
            ("/home/test/.cargo/bin/zapfast", Unsupported::Cargo),
            (
                "C:\\Users\\test\\.cargo\\bin\\zapfast.exe",
                Unsupported::Cargo,
            ),
            ("/nix/store/abc-zapfast/bin/zapfast", Unsupported::Nix),
            (
                "/opt/homebrew/Cellar/zapfast/1.0/bin/zapfast",
                Unsupported::Homebrew,
            ),
            (
                "/opt/homebrew/Caskroom/zapfast/1.0/ZapFast.app/Contents/MacOS/zapfast",
                Unsupported::Homebrew,
            ),
        ] {
            for platform in [Platform::Linux, Platform::Windows, Platform::MacOs] {
                assert_eq!(
                    detect(&ZAPFAST, &host, platform, Path::new(path)),
                    Err(expected.clone()),
                    "{path} on {platform:?}"
                );
            }
        }
    }

    #[test]
    fn sandbox_variables_mark_flatpak_and_snap() {
        let flatpak = FakeHost::default().with_var("FLATPAK_ID", "me.paolino.ZapFast");
        assert_eq!(
            linux("/home/test/zapfast", &flatpak),
            Err(Unsupported::Flatpak)
        );
        let snap = FakeHost::default().with_var("SNAP", "/snap/zapfast/1");
        assert_eq!(linux("/home/test/zapfast", &snap), Err(Unsupported::Snap));
    }

    #[test]
    fn linux_package_managers_are_asked_in_order() {
        for (program, manager) in [
            ("dpkg-query", PackageManager::Apt),
            ("rpm", PackageManager::Dnf),
            ("pacman", PackageManager::Pacman),
        ] {
            let host = FakeHost::default().owned_by(program);
            assert_eq!(
                linux("/opt/zapfast/zapfast", &host),
                Err(Unsupported::SystemPackage(manager))
            );
            assert_eq!(
                Unsupported::SystemPackage(manager).to_string(),
                format!(
                    "Update this installation through {} or your software center.",
                    manager.name()
                )
            );
        }
        let host = FakeHost::default();
        linux("/opt/zapfast/zapfast", &host).unwrap_err();
        assert_eq!(
            host.package_queries(),
            [
                "dpkg-query -S /opt/zapfast/zapfast",
                "rpm -qf /opt/zapfast/zapfast",
                "pacman -Qo /opt/zapfast/zapfast"
            ]
        );
        for path in [
            "/usr/bin/zapfast",
            "/usr/local/bin/zapfast",
            "/bin/zapfast",
            "/sbin/zapfast",
        ] {
            assert_eq!(linux(path, &host), Err(Unsupported::SystemDirectory));
        }
    }

    #[test]
    fn package_managers_are_not_asked_on_windows() {
        let host = FakeHost::default().owned_by("dpkg-query");
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("zapfast.exe");
        fs::write(
            directory.path().join("zapfast-portable.txt"),
            "zapfast-portable-v1\n",
        )
        .unwrap();
        assert_eq!(
            detect(&ZAPFAST, &host, Platform::Windows, &executable)
                .unwrap()
                .kind,
            Kind::Portable
        );
        assert!(host.package_queries().is_empty());
    }

    #[test]
    fn a_portable_copy_is_recognized_by_its_marker() {
        let host = FakeHost::default();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("zapfast");
        let path = executable.to_str().unwrap();
        assert_eq!(linux(path, &host), Err(Unsupported::NotPortable));
        let marker = directory.path().join("zapfast-portable.txt");
        fs::write(&marker, "zapfast-portable-v2\n").unwrap();
        assert_eq!(linux(path, &host), Err(Unsupported::NotPortable));
        fs::write(&marker, "zapfast-portable-v1\n").unwrap();
        assert_eq!(
            linux(path, &host),
            Ok(Installation {
                executable: executable.clone(),
                kind: Kind::Portable
            })
        );
        // The file the release packages ship.
        fs::write(
            &marker,
            include_str!("../tests/fixtures/zapfast-portable.txt"),
        )
        .unwrap();
        assert_eq!(linux(path, &host).unwrap().kind, Kind::Portable);
    }

    #[test]
    fn legacy_markers_keep_renamed_installs_updating() {
        let host = FakeHost::default();
        for (file, contents) in [
            (
                "spotifast-portable.txt",
                include_str!("../tests/fixtures/spotifast-portable.txt"),
            ),
            (
                "fastpotify-portable.txt",
                include_str!("../tests/fixtures/fastpotify-portable.txt"),
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let executable = directory.path().join("fastpotify");
            fs::write(directory.path().join(file), contents).unwrap();
            assert_eq!(
                detect(&SPOTIFAST, &host, Platform::Linux, &executable)
                    .unwrap()
                    .kind,
                Kind::Portable,
                "{file}"
            );
        }
        // A marker for another app's name does not count.
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("zapfast-portable.txt"),
            "zapfast-portable-v1",
        )
        .unwrap();
        assert_eq!(
            detect(
                &SPOTIFAST,
                &host,
                Platform::Linux,
                &directory.path().join("spotifast")
            ),
            Err(Unsupported::NotPortable)
        );
    }

    #[test]
    fn the_windows_installer_marker_wins_over_the_portable_one() {
        let host = FakeHost::default();
        for (file, contents) in [
            (
                "spotifast-installer.txt",
                include_str!("../tests/fixtures/spotifast-installer.txt"),
            ),
            (
                "fastpotify-installer.txt",
                include_str!("../tests/fixtures/fastpotify-installer.txt"),
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let executable = directory.path().join("spotifast.exe");
            fs::write(directory.path().join(file), contents).unwrap();
            fs::write(
                directory.path().join("spotifast-portable.txt"),
                "spotifast-portable-v1",
            )
            .unwrap();
            assert_eq!(
                detect(&SPOTIFAST, &host, Platform::Windows, &executable)
                    .unwrap()
                    .kind,
                Kind::WindowsInstaller,
                "{file}"
            );
            // Linux has no installer.
            assert_eq!(
                detect(&SPOTIFAST, &host, Platform::Linux, &executable)
                    .unwrap()
                    .kind,
                Kind::Portable
            );
        }
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("zapfast-installer.txt"),
            include_str!("../tests/fixtures/zapfast-installer.txt"),
        )
        .unwrap();
        assert_eq!(
            detect(
                &ZAPFAST,
                &host,
                Platform::Windows,
                &directory.path().join("zapfast.exe")
            )
            .unwrap()
            .kind,
            Kind::WindowsInstaller
        );
    }

    #[test]
    fn installs_from_before_the_marker_are_found_by_location_and_uninstaller() {
        let local = tempfile::tempdir().unwrap();
        let host = FakeHost::default().with_var("LOCALAPPDATA", local.path());
        for (config, relative) in [
            (ZAPFAST, "Programs/ZapFast/zapfast.exe"),
            (SPOTIFAST, "Programs/Spotifast/spotifast.exe"),
            (SPOTIFAST, "Programs/Fastpotify/fastpotify.exe"),
        ] {
            let executable = local.path().join(relative);
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            fs::write(&executable, b"app").unwrap();
            let executable = executable.canonicalize().unwrap();
            assert_eq!(
                detect(&config, &host, Platform::Windows, &executable),
                Err(Unsupported::NotPortable),
                "no uninstaller yet"
            );
            fs::write(executable.with_file_name("unins000.exe"), b"").unwrap();
            assert_eq!(
                detect(&config, &host, Platform::Windows, &executable)
                    .unwrap()
                    .kind,
                Kind::WindowsInstaller,
                "{relative}"
            );
        }
        let elsewhere = tempfile::tempdir().unwrap();
        fs::write(elsewhere.path().join("unins000.exe"), b"").unwrap();
        assert_eq!(
            detect(
                &ZAPFAST,
                &host,
                Platform::Windows,
                &elsewhere.path().join("zapfast.exe")
            ),
            Err(Unsupported::NotPortable)
        );
    }

    #[test]
    fn other_systems_have_no_download() {
        assert_eq!(
            detect(
                &ZAPFAST,
                &FakeHost::default(),
                Platform::Other,
                Path::new("/home/test/zapfast")
            ),
            Err(Unsupported::Platform)
        );
    }

    #[test]
    fn handoff_kind_names_are_stable() {
        for (kind, name) in [
            (Kind::Portable, "\"Portable\""),
            (Kind::WindowsInstaller, "\"WindowsInstaller\""),
            (Kind::MacBundle, "\"MacBundle\""),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), name);
        }
    }
}
