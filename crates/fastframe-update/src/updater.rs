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

/// What the latest check learned about this app's AppImage: the release it
/// read and whether that release signs the AppImage. Per app, feed and
/// AppImage name, and process-wide, since apps make an `Updater` for each
/// call.
static APPIMAGES: std::sync::Mutex<Vec<(String, String, bool)>> = std::sync::Mutex::new(Vec::new());

fn appimages() -> std::sync::MutexGuard<'static, Vec<(String, String, bool)>> {
    APPIMAGES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn appimage_key(config: &UpdateConfig, source: &Source) -> String {
    format!(
        "{}|{}|{}|{source:?}|{:?}",
        config.repository,
        config.slug,
        config.appimage_name.unwrap_or(config.slug),
        download::trusted(config)
    )
}

/// Records what a check learned, or forgets it (`None`): no newer release,
/// or one whose listing couldn't be read.
fn remember_appimage(key: &str, learned: Option<(&str, bool)>) {
    let mut known = appimages();
    known.retain(|(k, _, _)| k != key);
    if let Some((version, listed)) = learned {
        known.push((key.to_owned(), version.to_owned(), listed));
    }
}

fn appimage_unsigned(key: &str) -> bool {
    appimages().iter().any(|(k, _, listed)| k == key && !listed)
}

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
    ///
    /// For a copy running from an AppImage, it also reads whether that
    /// release signs this AppImage, so that [`Updater::installation`] then
    /// refuses the copy with [`Unsupported::MissingAsset`] when it doesn't,
    /// as it refused AppImages before they updated themselves, instead of a
    /// download that fails.
    pub fn check(&self) -> Result<Option<Release>> {
        self.require_publisher_key()?;
        let release = release::newer(&self.config, self.transport.as_ref(), &self.source)?;
        match &release {
            Some(release) => {
                self.learn_appimage(release, Platform::current(), std::env::consts::ARCH)
            }
            None => remember_appimage(&appimage_key(&self.config, &self.source), None),
        }
        Ok(release)
    }

    /// Records whether `release` signs this copy's AppImage, when this copy
    /// is one. A failure to find out records nothing: the download then
    /// says what went wrong, as before.
    fn learn_appimage(&self, release: &Release, platform: Platform, arch: &str) {
        if !self.config.appimage || platform != Platform::Linux {
            return;
        }
        let Ok(executable) = self.host.current_exe() else {
            return;
        };
        let appimage = detect::detect(&self.config, self.host.as_ref(), platform, &executable)
            .is_ok_and(|installation| installation.kind == Kind::AppImage);
        if !appimage {
            return;
        }
        let listed = download::appimage_listed(
            &self.config,
            self.transport.as_ref(),
            &self.source,
            platform,
            arch,
            &release.version,
        )
        .ok();
        remember_appimage(
            &appimage_key(&self.config, &self.source),
            listed.map(|listed| (release.version.as_str(), listed)),
        );
    }

    /// How this copy was installed, or why it cannot update itself. Reads
    /// files next to the executable and, on Linux, asks dpkg, rpm and pacman
    /// whether they own it.
    ///
    /// An AppImage whose latest checked release doesn't sign it (see
    /// [`Updater::check`]) is refused with [`Unsupported::MissingAsset`].
    pub fn installation(&self) -> Result<Installation, Unsupported> {
        let executable = self
            .host
            .current_exe()
            .map_err(|error| Unsupported::Unavailable(error.to_string()))?;
        let installation = self.installation_at(&executable)?;
        if installation.kind == Kind::AppImage
            && appimage_unsigned(&appimage_key(&self.config, &self.source))
        {
            return Err(Unsupported::MissingAsset);
        }
        Ok(installation)
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
        if !release.verified_by(&self.config, &self.source) {
            return Err(Unsupported::Unavailable(
                "The release was verified with another key or source".into(),
            ));
        }
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
        let appimage = installation.kind == Kind::AppImage;
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
        .inspect_err(|error| {
            // Marks the release the last check read as unsigned, so the next
            // `installation` refuses it; without a check nothing is recorded.
            // Only for the release the last check read, in one step so a
            // check in between isn't overwritten.
            if appimage && error.downcast_ref::<Unsupported>() == Some(&Unsupported::MissingAsset) {
                let key = appimage_key(&self.config, &self.source);
                let mut known = appimages();
                if let Some(entry) = known
                    .iter_mut()
                    .find(|(k, version, _)| *k == key && *version == release.version)
                {
                    entry.2 = false;
                }
            }
        })
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

    /// An app that bumps fastframe and calls only `check`, `installation`
    /// and `download`, as ZapFast and TonePush do, running from an AppImage
    /// its releases don't sign: it hears "can't update here", as before
    /// AppImages updated themselves, never a failed download. One whose
    /// release signs it updates itself.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_app_using_only_the_old_calls_is_refused_an_unsigned_appimage_not_failed() {
        use crate::testing::{FakeHost, FakeTransport, fixture_key, fixture_key_hex};
        use std::fs;
        use std::os::unix::fs::MetadataExt;

        fn run(repository: &'static str, sign_appimage: bool) -> Result<Installation, Unsupported> {
            let folder = tempfile::tempdir().unwrap();
            let root = folder.path().canonicalize().unwrap();
            let appimage = root.join("ZapFast.AppImage");
            fs::write(&appimage, b"appimage").unwrap();
            let mount = root.join(".mount_zapfas");
            let executable = mount.join("usr/bin/zapfast");
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            fs::write(&executable, b"app").unwrap();
            let user = fs::metadata(&appimage).unwrap().uid();
            let host = FakeHost::default()
                .with_current_exe(&executable)
                .with_var("APPIMAGE", appimage.as_os_str())
                .with_var("APPDIR", mount.as_os_str())
                .with_user_id(user);

            let arch = std::env::consts::ARCH;
            let version = "0.17.0";
            let asset = |name: &str| {
                format!("https://github.com/{repository}/releases/download/v{version}/{name}")
            };
            let portable = format!("zapfast-v{version}-{arch}-unknown-linux-gnu.tar.gz");
            let image = format!("zapfast-{version}-{arch}.AppImage");
            let mut manifest = format!("{}  {portable}\n", "a".repeat(64));
            if sign_appimage {
                manifest.push_str(&format!("{}  {image}\n", "b".repeat(64)));
            }
            let signature = fixture_key().sign(manifest.as_bytes());
            // The AppImage is attached either way, as packaging.yml attaches
            // it after signing.
            let metadata = serde_json::json!({
                "tag_name": format!("v{version}"),
                "assets": [
                    {"name": portable, "size": 10, "browser_download_url": asset(&portable)},
                    {"name": image, "size": 10, "browser_download_url": asset(&image)},
                    {"name": "checksums.txt", "size": manifest.len(), "browser_download_url": asset("checksums.txt")},
                    {"name": "checksums.txt.sig", "size": 64, "browser_download_url": asset("checksums.txt.sig")},
                ],
            });
            let latest = serde_json::json!({
                "tag_name": format!("v{version}"),
                "html_url": format!("https://github.com/{repository}/releases/tag/v{version}"),
            });
            let transport = FakeTransport::default()
                .serve(
                    &format!("https://api.github.com/repos/{repository}/releases/latest"),
                    latest.to_string().as_bytes(),
                )
                .serve(
                    &format!("https://api.github.com/repos/{repository}/releases/tags/v{version}"),
                    metadata.to_string().as_bytes(),
                )
                .serve(&asset("checksums.txt"), manifest.as_bytes())
                .serve(&asset("checksums.txt.sig"), signature.as_ref());
            let config = UpdateConfig {
                publisher_key: Some(fixture_key_hex()),
                ..UpdateConfig::new(repository, "ZapFast", "zapfast", "0.16.0")
            };
            let updater = Updater::new(config, transport).with_host(host);
            let release = updater.check().unwrap().unwrap();
            assert_eq!(release.version, version);
            // A new updater, as apps make one per call.
            let result = updater.clone().installation();
            if result.is_err() {
                let error = updater.download(&release, |_, _| {}).unwrap_err();
                assert_eq!(
                    error.downcast_ref::<Unsupported>(),
                    Some(&Unsupported::MissingAsset)
                );
            }
            result
        }

        if !matches!(std::env::consts::ARCH, "x86_64" | "aarch64") {
            return;
        }
        assert_eq!(
            run("crmne/zapfast-unsigned-appimage", false),
            Err(Unsupported::MissingAsset)
        );
        assert_eq!(
            run("crmne/zapfast-signed-appimage", true).unwrap().kind,
            Kind::AppImage
        );
    }

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
            &crate::tests::ZAPFAST,
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
            &crate::tests::ZAPFAST,
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
        let msi = VerifiedRelease::listing(
            release.clone(),
            &["zapfast-v0.17.0-x86_64-pc-windows-msvc.msi"],
            &crate::tests::ZAPFAST,
        );
        assert!(
            updater
                .check_listed(&msi, installer.clone(), Platform::Windows, "x86_64")
                .is_ok()
        );
        // Verified with another key: not this updater's.
        let keyed = crate::UpdateConfig {
            publisher_key: Some(crate::testing::fixture_key_hex()),
            ..crate::tests::ZAPFAST
        };
        let other = VerifiedRelease::listing(
            release,
            &["zapfast-v0.17.0-x86_64-pc-windows-msvc.msi"],
            &keyed,
        );
        assert!(matches!(
            updater.check_listed(&other, installer.clone(), Platform::Windows, "x86_64"),
            Err(Unsupported::Unavailable(_))
        ));
        // Verified from another source: not this updater's either.
        let elsewhere = Updater::new(
            crate::tests::ZAPFAST,
            crate::testing::FakeTransport::default(),
        )
        .with_source(Source::local("http://127.0.0.1:8799").unwrap());
        assert!(matches!(
            elsewhere.check_listed(&msi, installer, Platform::Windows, "x86_64"),
            Err(Unsupported::Unavailable(_))
        ));
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
