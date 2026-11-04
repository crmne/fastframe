//! The app's handle on the updater.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

#[cfg(doc)]
use crate::MacTarget;
use crate::detect::{self, Installation, Kind, Platform, Unsupported};
use crate::download::{self, Inputs, VerifiedRelease};
use crate::host::{Host, OsHost};
use crate::managed::{self, ManagedUpdate};
use crate::release::{self, Release};
use crate::stage::Prepared;
use crate::transport::{Source, Transport};
use crate::{UpdateConfig, helper};

/// Checks for, downloads and installs updates. Cheap to clone; every call
/// blocks, so run them off the interface thread.
#[derive(Clone)]
pub struct Updater {
    config: UpdateConfig,
    transport: Arc<dyn Transport>,
    source: Source,
    host: Arc<dyn Host>,
}

impl std::fmt::Debug for Updater {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Updater")
            .field("config", &self.config)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl Updater {
    /// An updater for GitHub releases that uses the app's HTTP client.
    pub fn new(config: UpdateConfig, transport: impl Transport + 'static) -> Self {
        Self {
            config,
            transport: Arc::new(transport),
            source: Source::github(),
            host: Arc::new(OsHost),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_host(mut self, host: impl Host + 'static) -> Self {
        self.host = Arc::new(host);
        self
    }

    /// Takes releases from `source` instead of GitHub.
    pub fn with_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }

    /// The configuration.
    pub fn config(&self) -> &UpdateConfig {
        &self.config
    }

    /// Where releases come from.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// The newest release, when it is newer than
    /// [`UpdateConfig::current_version`].
    pub fn check(&self) -> Result<Option<Release>> {
        self.require_publisher_key()?;
        release::newer(&self.config, self.transport.as_ref(), &self.source)
    }

    /// How this copy was installed, or why it cannot update itself. Reads
    /// files next to the executable and, on Linux, asks dpkg, rpm and pacman
    /// whether they own it.
    pub fn installation(&self) -> Result<Installation, Unsupported> {
        let executable = self
            .host
            .current_exe()
            .map_err(|error| Unsupported::Unavailable(error.to_string()))?;
        self.installation_at(&executable)
    }

    /// How the executable at `executable` (canonicalized) was installed, by
    /// the same rules as [`Updater::installation`]. For packaging checks and
    /// diagnostics; updates only ever replace the running app.
    pub fn installation_at(&self, executable: &Path) -> Result<Installation, Unsupported> {
        self.installation_on(executable, Platform::current(), std::env::consts::ARCH)
    }

    /// [`Updater::installation_at`] for a given system and processor. A Mac
    /// without a disk image for its processor ([`MacTarget::Arm64Only`](crate::MacTarget::Arm64Only) on
    /// x86_64) is refused first, so the app can say so before downloading.
    fn installation_on(
        &self,
        executable: &Path,
        platform: Platform,
        arch: &str,
    ) -> Result<Installation, Unsupported> {
        if platform == Platform::MacOs {
            release::target(platform, arch, self.config.mac_target)?;
        }
        detect::detect(&self.config, self.host.as_ref(), platform, executable)
    }

    /// Checks `release` without downloading it: its `checksums.txt` must
    /// verify with the publisher key and list a download of that version
    /// for this system. A copy that can't install a release announces it
    /// only after this, so it trusts the same signature a download would,
    /// not only the answer to "which release is the latest". Needs
    /// [`UpdateConfig::publisher_key`].
    pub fn verify(&self, release: &Release) -> Result<VerifiedRelease> {
        self.require_publisher_key()?;
        download::verify(
            &self.config,
            self.transport.as_ref(),
            &self.source,
            Platform::current(),
            std::env::consts::ARCH,
            release,
        )
    }

    /// Whether this copy can install `release`: [`Updater::installation`],
    /// and the signed `checksums.txt` lists this installation's download.
    /// [`Unsupported::MissingAsset`] when it doesn't (an AppImage whose
    /// release has none, say): tell people the release is out instead.
    pub fn installable(&self, release: &VerifiedRelease) -> Result<Installation, Unsupported> {
        let installation = self.installation()?;
        self.check_listed(
            release,
            installation,
            Platform::current(),
            std::env::consts::ARCH,
        )
    }

    fn check_listed(
        &self,
        release: &VerifiedRelease,
        installation: Installation,
        platform: Platform,
        arch: &str,
    ) -> Result<Installation, Unsupported> {
        let version = &release.release().version;
        let target = release::target(platform, arch, self.config.mac_target)?;
        let name = release::asset_name(&self.config, version, target, installation.kind, platform);
        let msi = release::msi_name(&release::stem(&self.config, version, target));
        let listed = release.lists(&name)
            || (installation.kind == Kind::WindowsInstaller && release.lists(&msi));
        if listed {
            Ok(installation)
        } else {
            Err(Unsupported::MissingAsset)
        }
    }

    /// For a copy a package manager owns, the tool that updates it, the
    /// package's name and the command for it, for the app to show in its
    /// own words. `None` when the copy updates itself, or no package
    /// manager owns it. Asks the package managers on Linux (`pacman -Qqo`,
    /// `dpkg-query -S`, `rpm -qf`) and reads `/etc/os-release`, Nix's
    /// profile manifest or Cargo's install list; never runs the command.
    pub fn managed_update(&self) -> Option<ManagedUpdate> {
        let executable = self.host.current_exe().ok()?;
        let reason = self.installation_at(&executable).err()?;
        managed::describe(
            &reason,
            &executable,
            self.host.as_ref(),
            Platform::current(),
        )
    }

    /// Downloads and verifies `release` into a new staging folder beside
    /// the app. `progress` receives bytes received and the published size.
    pub fn download(&self, release: &Release, progress: impl FnMut(u64, u64)) -> Result<Prepared> {
        self.require_publisher_key()?;
        let installation = self.installation()?;
        download::download(
            &Inputs {
                config: &self.config,
                transport: self.transport.as_ref(),
                source: &self.source,
                host: self.host.as_ref(),
                platform: Platform::current(),
                arch: std::env::consts::ARCH,
            },
            installation,
            release,
            progress,
        )
    }

    /// The update an earlier run of this app downloaded and did not install,
    /// checked again and ready for [`Updater::handoff`]. Lets an app download
    /// in the background and install at its next start, before it opens
    /// anything, with the same rollback as any update.
    ///
    /// Every download keeps what it verified in its staging folder: the
    /// staged payload's hash, the release's `checksums.txt` and its
    /// signature. A kept update is used only when its folder was never
    /// handed to a helper, it is for this installation, its version is newer
    /// than [`UpdateConfig::current_version`], its payload hashes as it did,
    /// the signature still verifies against the configured keys, and the
    /// package's listed checksum (macOS image, Windows installer, AppImage)
    /// or the executable's `--version` (portable copy, AppImage) checks out.
    /// When several are kept, the newest is taken; the next download or
    /// acknowledged update removes the rest. `Ok(None)` when none is kept. A
    /// kept update that fails a check is removed and the error says why.
    pub fn kept(&self) -> Result<Option<Prepared>> {
        let installation = self.installation()?;
        download::kept(
            &Inputs {
                config: &self.config,
                transport: self.transport.as_ref(),
                source: &self.source,
                host: self.host.as_ref(),
                platform: Platform::current(),
                arch: std::env::consts::ARCH,
            },
            &installation,
        )
    }

    /// An app's own feed ([`Source::feed`]) is only used with a publisher
    /// key: otherwise whoever controls the server controls the update.
    fn require_publisher_key(&self) -> Result<()> {
        anyhow::ensure!(
            !self.source.needs_publisher_key() || self.config.publisher_key.is_some(),
            "An update feed needs a publisher key: set UpdateConfig::publisher_key"
        );
        Ok(())
    }

    /// Starts the helper and returns once it is watching this process. Quit
    /// the app right after: the helper waits one minute for it to exit, then
    /// installs and relaunches it with `arguments` (for example
    /// `--verbose`) followed by the receipt flag.
    pub fn handoff(&self, prepared: Prepared, arguments: Vec<String>) -> Result<()> {
        helper::handoff(&self.config, self.host.as_ref(), prepared, arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_without_this_copys_signed_download_is_announced_not_installed() {
        let updater = Updater::new(
            crate::tests::ZAPFAST,
            crate::testing::FakeTransport::default(),
        );
        let release = Release {
            version: "0.17.0".into(),
            url: String::new(),
        };
        let appimage = Installation {
            executable: "/home/me/ZapFast.AppImage".into(),
            kind: Kind::AppImage,
        };
        let portable_only = VerifiedRelease::listing(
            release.clone(),
            &["zapfast-v0.17.0-x86_64-unknown-linux-gnu.tar.gz"],
        );
        assert_eq!(
            updater.check_listed(&portable_only, appimage.clone(), Platform::Linux, "x86_64"),
            Err(Unsupported::MissingAsset)
        );
        let with_appimage = VerifiedRelease::listing(
            release.clone(),
            &[
                "zapfast-v0.17.0-x86_64-unknown-linux-gnu.tar.gz",
                "zapfast-0.17.0-x86_64.AppImage",
            ],
        );
        assert_eq!(
            updater.check_listed(&with_appimage, appimage, Platform::Linux, "x86_64"),
            Ok(Installation {
                executable: "/home/me/ZapFast.AppImage".into(),
                kind: Kind::AppImage,
            })
        );
        // The Windows installer takes its .msi or its setup program.
        let installer = Installation {
            executable: "C:/Users/me/AppData/Local/Programs/ZapFast/zapfast.exe".into(),
            kind: Kind::WindowsInstaller,
        };
        let msi =
            VerifiedRelease::listing(release, &["zapfast-v0.17.0-x86_64-pc-windows-msvc.msi"]);
        assert!(
            updater
                .check_listed(&msi, installer, Platform::Windows, "x86_64")
                .is_ok()
        );
    }

    #[test]
    fn a_package_install_says_how_to_update_it() {
        let updater = Updater::new(
            crate::tests::ZAPFAST,
            crate::testing::FakeTransport::default(),
        )
        .with_host(crate::testing::FakeHost::default().with_current_exe(
            std::path::Path::new("/opt/homebrew/Cellar/zapfast/0.16.3/bin/zapfast"),
        ));
        let update = updater.managed_update().unwrap();
        assert_eq!(update.manager, crate::Manager::Homebrew);
        assert_eq!(update.command.as_deref(), Some("brew upgrade zapfast"));
    }
    use crate::detect::Kind;
    use crate::testing::{FakeHost, FakeTransport};
    use crate::tests::ZAPFAST;

    #[test]
    fn the_updater_checks_through_the_apps_transport_and_source() {
        let transport = FakeTransport::default().serve(
            "http://127.0.0.1:9/latest.json",
            br#"{"tag_name":"v0.17.0","html_url":"http://127.0.0.1:9/notes"}"#,
        );
        let updater = Updater::new(ZAPFAST, transport)
            .with_source(Source::local("http://127.0.0.1:9").unwrap());
        assert!(!updater.source().is_github());
        assert_eq!(updater.config().slug, "zapfast");
        assert_eq!(
            updater.check().unwrap(),
            Some(Release {
                version: "0.17.0".into(),
                url: "http://127.0.0.1:9/notes".into()
            })
        );
    }

    #[test]
    fn an_update_feed_is_only_used_with_a_publisher_key() {
        let latest = "https://updates.example.com/app/latest.json";
        let listing = br#"{"tag_name":"v0.17.0","html_url":"https://updates.example.com/app"}"#;
        let feed = Source::feed("https://updates.example.com/app").unwrap();

        let unsigned = UpdateConfig {
            publisher_key: None,
            ..ZAPFAST
        };
        let updater = Updater::new(unsigned, FakeTransport::default().serve(latest, listing))
            .with_source(feed.clone());
        let error = updater.check().unwrap_err().to_string();
        assert!(error.contains("publisher key"), "{error}");
        let release = Release {
            version: "0.17.0".into(),
            url: "https://updates.example.com/app".into(),
        };
        let error = updater
            .download(&release, |_, _| {})
            .unwrap_err()
            .to_string();
        assert!(error.contains("publisher key"), "{error}");

        let signed = Updater::new(ZAPFAST, FakeTransport::default().serve(latest, listing))
            .with_source(feed);
        assert_eq!(signed.check().unwrap(), Some(release));
    }

    #[test]
    fn the_updater_inspects_its_own_executable() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join(if cfg!(windows) {
            "zapfast.exe"
        } else {
            "zapfast"
        });
        std::fs::write(&executable, b"app").unwrap();
        let updater = Updater::new(ZAPFAST, FakeTransport::default())
            .with_host(FakeHost::default().with_current_exe(&executable));
        if cfg!(any(target_os = "linux", windows)) {
            assert_eq!(updater.installation(), Err(Unsupported::NotPortable));
            std::fs::write(
                directory.path().join("zapfast-portable.txt"),
                "zapfast-portable-v1\n",
            )
            .unwrap();
            assert_eq!(updater.installation().unwrap().kind, Kind::Portable);
        }
        let without =
            Updater::new(ZAPFAST, FakeTransport::default()).with_host(FakeHost::default());
        assert!(matches!(
            without.installation(),
            Err(Unsupported::Unavailable(_))
        ));
    }

    #[test]
    fn an_apple_silicon_only_app_refuses_an_x86_64_mac_up_front() {
        let arm64_only = UpdateConfig {
            mac_target: crate::MacTarget::Arm64Only,
            ..ZAPFAST
        };
        let executable = Path::new("/Applications/ZapFast.app/Contents/MacOS/zapfast");
        let updater =
            Updater::new(arm64_only, FakeTransport::default()).with_host(FakeHost::default());
        assert_eq!(
            updater.installation_on(executable, Platform::MacOs, "x86_64"),
            Err(Unsupported::Platform)
        );
        // On Apple silicon, and for the default universal image, the
        // bundle rules decide as before.
        for (config, arch) in [(arm64_only, "aarch64"), (ZAPFAST, "x86_64")] {
            let updater =
                Updater::new(config, FakeTransport::default()).with_host(FakeHost::default());
            assert_ne!(
                updater.installation_on(executable, Platform::MacOs, arch),
                Err(Unsupported::Platform),
                "{arch}"
            );
        }
        // Other systems are not affected.
        let updater =
            Updater::new(arm64_only, FakeTransport::default()).with_host(FakeHost::default());
        assert_ne!(
            updater.installation_on(Path::new("/opt/zapfast/zapfast"), Platform::Linux, "x86_64"),
            Err(Unsupported::Platform)
        );
    }

    #[test]
    fn a_download_for_an_unsupported_copy_touches_nothing() {
        let transport = FakeTransport::default();
        let updater = Updater::new(ZAPFAST, transport).with_host(FakeHost::default());
        let release = Release {
            version: "0.17.0".into(),
            url: String::new(),
        };
        assert!(updater.download(&release, |_, _| {}).is_err());
    }
}
