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
    /// Installed by the Windows setup program or Windows Installer
    /// package, which left a `<slug>-installer.txt` marker. Updated by
    /// installing the release's package silently, or its setup program
    /// when it has no package.
    WindowsInstaller,
    /// A macOS app bundle from the disk image, outside Homebrew.
    MacBundle,
    /// A Linux AppImage, with [`UpdateConfig::appimage`] on. The
    /// installation's executable is the AppImage file itself (`$APPIMAGE`),
    /// not the program running from inside it, and an update replaces that
    /// file with the release's `<slug>-<version>-<arch>.AppImage`.
    ///
    /// Only a helper that knows this variant ever writes it, and only an
    /// app at least as new reads it back as a receipt.
    AppImage,
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
    /// An AppImage that the user does not own, or that sits in a folder
    /// the user cannot write to, so it cannot be replaced.
    NotWritable,
    /// No download exists for this operating system or processor.
    Platform,
    /// The installation could not be inspected.
    Unavailable(String),
    /// The release has no signed download for this kind of installation
    /// (an AppImage whose release doesn't list it in `checksums.txt`, say).
    /// The release is out; it just can't be installed from here.
    MissingAsset,
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
            Self::NotWritable => formatter.write_str(
                "This AppImage cannot be replaced from here. Move it to a folder of your own, such as ~/Applications, or use the download page.",
            ),
            Self::Platform => formatter
                .write_str("Use the download page for this operating system or architecture."),
            Self::Unavailable(reason) => {
                write!(formatter, "Cannot inspect this installation: {reason}")
            }
            Self::MissingAsset => formatter.write_str(
                "This release has no signed download for this installation. Use the download page.",
            ),
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
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
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
            if platform == Platform::Linux
                && config.appimage
                && let Some(appimage) = running_appimage(host, executable)
            {
                return detect_appimage(host, &appimage);
            }
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
            if marker(config, directory, "portable")
                && runs_the_portable_app(config, platform, executable)
            {
                found(Kind::Portable)
            } else {
                Err(Unsupported::NotPortable)
            }
        }
        Platform::Other => Err(Unsupported::Platform),
    }
}

/// The AppImage this process runs from: `$APPIMAGE`, when `$APPDIR` (the
/// AppImage's mount point or extracted folder) holds the running
/// executable. A program that an AppImage merely started inherits both
/// variables but runs from elsewhere, so it never takes the other
/// AppImage for its own.
fn running_appimage(host: &dyn Host, executable: &Path) -> Option<PathBuf> {
    let appimage = PathBuf::from(host.var("APPIMAGE")?);
    let appdir = PathBuf::from(host.var("APPDIR")?);
    if !appimage.is_absolute() || !appdir.is_absolute() {
        return None;
    }
    let appdir = appdir.canonicalize().unwrap_or(appdir);
    executable.starts_with(&appdir).then_some(appimage)
}

/// Whether the AppImage at `path` may replace itself: no package manager
/// owns it, it is not in a system directory, it is a regular file the user
/// owns and can write, in a folder the user can write to (the update is
/// staged there and renamed over it).
fn detect_appimage(host: &dyn Host, path: &Path) -> Result<Installation, Unsupported> {
    for (program, flag, manager) in [
        ("dpkg-query", "-S", PackageManager::Apt),
        ("rpm", "-qf", PackageManager::Dnf),
        ("pacman", "-Qo", PackageManager::Pacman),
    ] {
        if host.package_owner(program, flag, path) {
            return Err(Unsupported::SystemPackage(manager));
        }
    }
    let text = path.to_string_lossy();
    if ["/usr/", "/bin/", "/sbin/"]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return Err(Unsupported::SystemDirectory);
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|error| Unsupported::Unavailable(error.to_string()))?;
    if !metadata.is_file() {
        return Err(Unsupported::Unavailable(
            "The AppImage is not a regular file".into(),
        ));
    }
    let directory = path.parent().ok_or(Unsupported::NotWritable)?;
    let user = host
        .user_id()
        .ok_or_else(|| Unsupported::Unavailable("Cannot tell which user runs the app".into()))?;
    match (owner_and_mode(path), owner_and_mode(directory)) {
        (Some(file), Some(folder)) if may_replace(user, file, folder) => Ok(Installation {
            executable: path.to_owned(),
            kind: Kind::AppImage,
        }),
        _ => Err(Unsupported::NotWritable),
    }
}

/// The owner and permission bits of `path`, following symbolic links (the
/// folder may be one). Only Unix has them.
fn owner_and_mode(path: &Path) -> Option<(u32, u32)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path)
            .ok()
            .map(|metadata| (metadata.uid(), metadata.mode()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Whether `user` may replace a file with `file`'s owner and mode in a
/// folder with `folder`'s: the user owns the file and may write it, and
/// owns a writable folder or the folder is writable by everyone. Group
/// permissions are not considered, so a shared folder counts as read-only.
fn may_replace(user: u32, file: (u32, u32), folder: (u32, u32)) -> bool {
    let (file_owner, file_mode) = file;
    let (folder_owner, folder_mode) = folder;
    file_owner == user
        && file_mode & 0o200 != 0
        && ((folder_owner == user && folder_mode & 0o300 == 0o300) || folder_mode & 0o003 == 0o003)
}

/// Whether a portable copy is the app the archive installs. With
/// [`UpdateConfig::portable_executable`] set, the archive ships other
/// programs beside the app and its marker; only the app's own file name may
/// update itself, or a sibling would be replaced by the app. Without it,
/// any name is accepted, as before.
fn runs_the_portable_app(config: &UpdateConfig, platform: Platform, executable: &Path) -> bool {
    if config.portable_executable.is_none() {
        return true;
    }
    let expected = crate::release::portable_executable(config, platform);
    executable.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        if platform == Platform::Windows {
            name.eq_ignore_ascii_case(&expected)
        } else {
            name == expected.as_str()
        }
    })
}

/// Whether `<name>-<kind>.txt` in `directory` reads `<name>-<kind>-v1`, for
/// the slug or a legacy name.
fn marker(config: &UpdateConfig, directory: &Path, kind: &str) -> bool {
    // The same check that decides whether a portable copy keeps its data
    // next to it.
    config.names().any(|name| {
        fastframe_dirs::marker(directory, name, kind, |path| fs::read_to_string(path).ok())
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

    /// TonePush's archives carry the `tonepush` command-line tool beside the
    /// `tonepush-gui` editor and one marker. Only the editor updates itself.
    #[test]
    fn a_named_portable_executable_is_the_only_copy_that_updates() {
        const TONEPUSH: UpdateConfig = UpdateConfig {
            portable_executable: Some("tonepush-gui"),
            ..UpdateConfig::new("crmne/tonepush", "TonePush", "tonepush", "0.6.1")
        };
        let host = FakeHost::default();
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("tonepush-portable.txt"),
            "tonepush-portable-v1\n",
        )
        .unwrap();
        for (platform, app, sibling) in [
            (Platform::Linux, "tonepush-gui", "tonepush"),
            (Platform::Windows, "tonepush-gui.exe", "tonepush.exe"),
            (Platform::Windows, "TonePush-GUI.EXE", "TONEPUSH.EXE"),
        ] {
            let app = directory.path().join(app);
            assert_eq!(
                detect(&TONEPUSH, &host, platform, &app),
                Ok(Installation {
                    executable: app.clone(),
                    kind: Kind::Portable
                }),
                "{platform:?}"
            );
            assert_eq!(
                detect(&TONEPUSH, &host, platform, &directory.path().join(sibling)),
                Err(Unsupported::NotPortable),
                "{platform:?}"
            );
        }
        // Without the field any name next to the marker may update, as
        // before: a renamed ZapFast keeps updating.
        fs::write(
            directory.path().join("zapfast-portable.txt"),
            "zapfast-portable-v1\n",
        )
        .unwrap();
        let renamed = directory.path().join("zapfast-0.16");
        assert_eq!(
            linux(renamed.to_str().unwrap(), &host).unwrap().kind,
            Kind::Portable
        );
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
            (Kind::AppImage, "\"AppImage\""),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), name);
        }
    }

    #[cfg(unix)]
    const APPIMAGES: UpdateConfig = UpdateConfig {
        appimage: true,
        ..SPOTIFAST
    };

    /// An AppImage at `folder/Spotifast.AppImage`, mounted at
    /// `folder/.mount_spotif` with the app at `usr/bin/spotifast` inside,
    /// as the runtime starts it.
    #[cfg(unix)]
    struct MountedAppImage {
        folder: tempfile::TempDir,
        appimage: PathBuf,
        mount: PathBuf,
        executable: PathBuf,
    }

    #[cfg(unix)]
    impl MountedAppImage {
        fn new() -> Self {
            let folder = tempfile::tempdir().unwrap();
            let root = folder.path().canonicalize().unwrap();
            let appimage = root.join("Spotifast.AppImage");
            fs::write(&appimage, b"appimage").unwrap();
            let mount = root.join(".mount_spotif");
            let executable = mount.join("usr/bin/spotifast");
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            fs::write(&executable, b"app").unwrap();
            Self {
                folder,
                appimage,
                mount,
                executable,
            }
        }

        fn user(&self) -> u32 {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&self.appimage).unwrap().uid()
        }

        /// The variables the runtime sets for `appimage`, run by `user`.
        fn host_for(&self, appimage: &Path, user: Option<u32>) -> FakeHost {
            let host = FakeHost::default()
                .with_var("APPIMAGE", appimage.as_os_str())
                .with_var("APPDIR", self.mount.as_os_str());
            match user {
                Some(user) => host.with_user_id(user),
                None => host,
            }
        }

        /// As the runtime starts this AppImage for the user who owns it.
        fn host(&self) -> FakeHost {
            self.host_for(&self.appimage, Some(self.user()))
        }

        fn detect(
            &self,
            config: &UpdateConfig,
            host: &FakeHost,
        ) -> Result<Installation, Unsupported> {
            detect(config, host, Platform::Linux, &self.executable)
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_appimage_the_user_owns_updates_itself_as_the_file() {
        let mounted = MountedAppImage::new();
        let host = mounted.host();
        assert_eq!(
            mounted.detect(&APPIMAGES, &host),
            Ok(Installation {
                executable: mounted.appimage.clone(),
                kind: Kind::AppImage,
            })
        );
        // The package managers are asked about the AppImage, not the
        // program inside its mount.
        let appimage = mounted.appimage.display();
        assert_eq!(
            host.package_queries(),
            [
                format!("dpkg-query -S {appimage}"),
                format!("rpm -qf {appimage}"),
                format!("pacman -Qo {appimage}"),
            ]
        );
        // With the setting off an AppImage is refused as before 0.16.
        let off = UpdateConfig {
            appimage: false,
            ..SPOTIFAST
        };
        assert_eq!(
            mounted.detect(&off, &mounted.host()),
            Err(Unsupported::NotPortable)
        );
        // On by default.
        assert!(UpdateConfig::new("crmne/x", "X", "x", "1.0.0").appimage);
        // Unpacked with --appimage-extract-and-run, it is the same.
        let extracted = mounted
            .folder
            .path()
            .canonicalize()
            .unwrap()
            .join("appimage_extracted_00ff");
        let executable = extracted.join("usr/bin/spotifast");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"app").unwrap();
        let host = FakeHost::default()
            .with_var("APPIMAGE", mounted.appimage.as_os_str())
            .with_var("APPDIR", extracted.as_os_str())
            .with_user_id(mounted.user());
        assert_eq!(
            detect(&APPIMAGES, &host, Platform::Linux, &executable)
                .unwrap()
                .kind,
            Kind::AppImage
        );
        // AppImages are a Linux format.
        assert_eq!(
            detect(
                &APPIMAGES,
                &mounted.host(),
                Platform::Windows,
                &mounted.executable
            ),
            Err(Unsupported::NotPortable)
        );
    }

    /// A program an AppImage started inherits `$APPIMAGE` and `$APPDIR`,
    /// but runs from elsewhere: it must not replace the other AppImage.
    #[cfg(unix)]
    #[test]
    fn a_program_an_appimage_started_does_not_take_it_for_its_own() {
        let mounted = MountedAppImage::new();
        let portable = tempfile::tempdir().unwrap();
        let executable = portable.path().join("spotifast");
        fs::write(&executable, b"app").unwrap();
        assert_eq!(
            detect(&APPIMAGES, &mounted.host(), Platform::Linux, &executable),
            Err(Unsupported::NotPortable)
        );
        // With its marker it is the portable copy it is.
        fs::write(
            portable.path().join("spotifast-portable.txt"),
            "spotifast-portable-v1",
        )
        .unwrap();
        assert_eq!(
            detect(&APPIMAGES, &mounted.host(), Platform::Linux, &executable).unwrap(),
            Installation {
                executable,
                kind: Kind::Portable
            }
        );
        // A relative path is not one the runtime sets.
        let host = mounted.host_for(Path::new("Spotifast.AppImage"), Some(mounted.user()));
        assert_eq!(
            mounted.detect(&APPIMAGES, &host),
            Err(Unsupported::NotPortable)
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_appimage_someone_else_owns_or_cannot_write_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let mounted = MountedAppImage::new();
        let someone_else = mounted.host_for(&mounted.appimage, Some(mounted.user() + 1));
        assert_eq!(
            mounted.detect(&APPIMAGES, &someone_else),
            Err(Unsupported::NotWritable)
        );
        fs::set_permissions(&mounted.appimage, fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(
            mounted.detect(&APPIMAGES, &mounted.host()),
            Err(Unsupported::NotWritable)
        );
        fs::set_permissions(&mounted.appimage, fs::Permissions::from_mode(0o755)).unwrap();
        let folder = mounted.appimage.parent().unwrap();
        fs::set_permissions(folder, fs::Permissions::from_mode(0o555)).unwrap();
        let read_only = mounted.detect(&APPIMAGES, &mounted.host());
        fs::set_permissions(folder, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(read_only, Err(Unsupported::NotWritable));
        // A link to an AppImage is not the file to replace.
        let link = folder.join("link.AppImage");
        std::os::unix::fs::symlink(&mounted.appimage, &link).unwrap();
        assert!(matches!(
            mounted.detect(&APPIMAGES, &mounted.host_for(&link, Some(mounted.user()))),
            Err(Unsupported::Unavailable(_))
        ));
        // Without a user to compare with, nothing is replaced.
        assert!(matches!(
            mounted.detect(&APPIMAGES, &mounted.host_for(&mounted.appimage, None)),
            Err(Unsupported::Unavailable(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_packaged_or_system_appimage_is_left_to_its_owner() {
        let mounted = MountedAppImage::new();
        let host = mounted.host().owned_by("pacman");
        assert_eq!(
            mounted.detect(&APPIMAGES, &host),
            Err(Unsupported::SystemPackage(PackageManager::Pacman))
        );
        let host = mounted.host_for(
            Path::new("/usr/bin/spotifast.AppImage"),
            Some(mounted.user()),
        );
        assert_eq!(
            mounted.detect(&APPIMAGES, &host),
            Err(Unsupported::SystemDirectory)
        );
        let host = mounted
            .host()
            .with_var("FLATPAK_ID", "rocks.spotifast.Spotifast");
        assert_eq!(mounted.detect(&APPIMAGES, &host), Err(Unsupported::Flatpak));
    }

    #[test]
    fn replacing_needs_the_file_and_a_writable_folder() {
        let me = 1000;
        assert!(may_replace(me, (me, 0o755), (me, 0o755)));
        assert!(may_replace(me, (me, 0o700), (me, 0o700)));
        // Someone else's file, or one without write permission.
        assert!(!may_replace(me, (0, 0o777), (me, 0o755)));
        assert!(!may_replace(me, (me, 0o555), (me, 0o755)));
        // A folder the user cannot add to or rename in.
        assert!(!may_replace(me, (me, 0o755), (me, 0o555)));
        assert!(!may_replace(me, (me, 0o755), (me, 0o655)));
        assert!(!may_replace(me, (me, 0o755), (0, 0o755)));
        assert!(!may_replace(me, (me, 0o755), (0, 0o775)));
        // A folder open to everyone, such as /tmp with its sticky bit.
        assert!(may_replace(me, (me, 0o755), (0, 0o1777)));
    }
}
