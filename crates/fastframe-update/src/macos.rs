//! macOS app bundles: where they are, whose they are, and how one replaces
//! another. The file handling runs everywhere; the macOS tools (PlistBuddy,
//! codesign, spctl, hdiutil, ditto) are reached through [`Host`], so the
//! tests run on every platform.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::UpdateConfig;
use crate::detect::{Installation, Unsupported};
use crate::host::Host;
use crate::stage::{FAILED_APP, HELPER_APP, PREVIOUS, Staged};

/// `CFBundleExecutable` names this app may have.
fn executable_names(config: &UpdateConfig) -> Vec<&'static str> {
    if config.macos.executable_names.is_empty() {
        vec![config.slug]
    } else {
        config.macos.executable_names.to_vec()
    }
}

/// `<app_name>.app` first, then the legacy bundle names.
fn bundle_names(config: &UpdateConfig) -> Vec<String> {
    std::iter::once(format!("{}.app", config.app_name))
        .chain(
            config
                .macos
                .legacy_bundle_names
                .iter()
                .map(|name| (*name).to_owned()),
        )
        .collect()
}

/// The `.app` folder an executable sits in, when it sits in
/// `<name>.app/Contents/MacOS/<one of the executable names>`.
pub(crate) fn bundle_root<'a>(config: &UpdateConfig, executable: &'a Path) -> Option<&'a Path> {
    let root = executable.ancestors().nth(3)?;
    (root.extension().is_some_and(|extension| extension == "app")
        && executable_names(config)
            .iter()
            .any(|name| root.join("Contents/MacOS").join(name) == executable))
    .then_some(root)
}

/// Where a bundle's own `CFBundleExecutable` says its executable lives.
/// Never a literal name: a release can rename it.
fn executable_path(host: &dyn Host, bundle: &Path) -> Result<PathBuf> {
    Ok(bundle
        .join("Contents/MacOS")
        .join(host.plist(bundle, "CFBundleExecutable")?))
}

fn identity(config: &UpdateConfig, host: &dyn Host, bundle: &Path) -> Result<bool> {
    Ok(config
        .macos
        .bundle_ids
        .contains(&host.plist(bundle, "CFBundleIdentifier")?.as_str())
        && executable_names(config).contains(&host.plist(bundle, "CFBundleExecutable")?.as_str())
        && host.plist(bundle, "CFBundlePackageType")? == "APPL")
}

pub(crate) fn detect(
    config: &UpdateConfig,
    host: &dyn Host,
    executable: &Path,
) -> Result<(), Unsupported> {
    if executable.starts_with("/Volumes")
        || executable
            .components()
            .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return Err(Unsupported::MoveToApplications);
    }
    let bundle = bundle_root(config, executable).ok_or(Unsupported::MoveToApplications)?;
    let prefixes = [
        Some(PathBuf::from("/opt/homebrew")),
        Some(PathBuf::from("/usr/local")),
        host.var("HOMEBREW_PREFIX").map(PathBuf::from),
    ];
    let names = bundle_names(config);
    for prefix in prefixes.into_iter().flatten() {
        if config
            .names()
            .any(|cask| cask_owns(&prefix.join("Caskroom").join(cask), bundle, &names))
        {
            return Err(Unsupported::Homebrew);
        }
    }
    match identity(config, host, bundle) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Unsupported::ForeignBundle),
        Err(error) => Err(Unsupported::Unavailable(format!("{error:#}"))),
    }
}

/// Whether a Homebrew cask folder links to this bundle. Casks move the app
/// into /Applications and keep a link to it in each version folder.
pub(crate) fn cask_owns(cask: &Path, bundle: &Path, names: &[String]) -> bool {
    let Ok(bundle) = bundle.canonicalize() else {
        return false;
    };
    fs::read_dir(cask).is_ok_and(|versions| {
        versions.flatten().any(|version| {
            names.iter().any(|name| {
                version
                    .path()
                    .join(name)
                    .canonicalize()
                    .is_ok_and(|installed| installed == bundle)
            })
        })
    })
}

/// The `TeamIdentifier=` line of `codesign --display --verbose=4`.
pub(crate) fn team_identifier(display: &str) -> Option<String> {
    display
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .filter(|value| *value != "not set")
        .map(str::to_owned)
}

/// Checks a candidate bundle before it may replace the installation: same
/// identity, the expected version, and when the running app is signed by a
/// team, the same team and Gatekeeper's approval. Then asks the executable
/// for its version.
fn validate(
    config: &UpdateConfig,
    host: &dyn Host,
    bundle: &Path,
    installation: &Installation,
    version: &str,
) -> Result<()> {
    ensure!(
        identity(config, host, bundle)?,
        "The download is not a {} app bundle",
        config.app_name
    );
    ensure!(
        host.plist(bundle, "CFBundleShortVersionString")? == version,
        "The app bundle has the wrong version"
    );
    let incoming = host.signing_team(bundle)?;
    let current = bundle_root(config, &installation.executable).context("Missing app bundle")?;
    if let Some(current) = host.signing_team(current)? {
        ensure!(
            incoming.as_deref() == Some(current.as_str()),
            "The update was signed by a different publisher"
        );
        ensure!(
            host.assess(bundle)?,
            "macOS could not approve this update for launch"
        );
    }
    crate::download::check_version(config, host, &executable_path(host, bundle)?, version)
}

/// A mounted disk image, detached when dropped.
struct Mounted<'a> {
    host: &'a dyn Host,
    mountpoint: PathBuf,
}

/// A new mount point beside the image. A failed detach can leave an earlier
/// attempt mounted; never traverse that volume or reuse its folder.
fn mountpoint(image: &Path) -> Result<PathBuf> {
    let mount = image
        .parent()
        .context("Missing update directory")?
        .join(format!("mounted-{}", crate::stage::random_suffix()?));
    fs::create_dir(&mount)?;
    Ok(mount)
}

impl<'a> Mounted<'a> {
    fn open(host: &'a dyn Host, image: &Path) -> Result<Self> {
        let mountpoint = mountpoint(image)?;
        if let Err(error) = host.attach(image, &mountpoint) {
            let _ = fs::remove_dir(&mountpoint);
            return Err(error);
        }
        Ok(Self { host, mountpoint })
    }

    fn bundle(&self, config: &UpdateConfig) -> Result<PathBuf> {
        image_bundle(config, &self.mountpoint)
    }
}

impl Drop for Mounted<'_> {
    fn drop(&mut self) {
        if self.host.detach(&self.mountpoint) {
            let _ = fs::remove_dir(&self.mountpoint);
        }
    }
}

/// The app bundle at the top of a disk image: `<app_name>.app` or a legacy
/// name, never a symbolic link.
fn image_bundle(config: &UpdateConfig, root: &Path) -> Result<PathBuf> {
    for name in bundle_names(config) {
        let bundle = root.join(name);
        match fs::symlink_metadata(&bundle) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir(),
                    "The disk image has an invalid app bundle"
                );
                return Ok(bundle);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    bail!("The disk image has no {} app bundle", config.app_name)
}

/// Checks the bundle inside a freshly downloaded disk image.
pub(crate) fn validate_download(
    config: &UpdateConfig,
    host: &dyn Host,
    image: &Path,
    installation: &Installation,
    version: &str,
) -> Result<()> {
    let mounted = Mounted::open(host, image)?;
    validate(
        config,
        host,
        &mounted.bundle(config)?,
        installation,
        version,
    )
}

/// Replaces the bundle and returns the executable now installed, which a
/// release may have renamed. The installation's own executable stays the
/// right one for a rollback, which restores the previous bundle.
pub(crate) fn replace(config: &UpdateConfig, host: &dyn Host, staged: &Staged) -> Result<PathBuf> {
    let target =
        bundle_root(config, &staged.installation.executable).context("Missing app bundle")?;
    let backup = staged.file(PREVIOUS);
    let candidate = staged.file(&format!("{}.app", config.app_name));
    ensure!(
        !backup.exists() && !candidate.exists(),
        "This update was already applied"
    );
    let mounted = Mounted::open(host, &staged.payload)?;
    let source = mounted.bundle(config)?;
    validate(config, host, &source, &staged.installation, &staged.version)?;
    host.copy_bundle(&source, &candidate)
        .context("Could not copy the downloaded app bundle")?;
    validate(
        config,
        host,
        &candidate,
        &staged.installation,
        &staged.version,
    )?;
    drop(mounted);
    fs::rename(target, &backup).context("Cannot back up the current app bundle")?;
    if let Err(error) = fs::rename(&candidate, target) {
        fs::rename(&backup, target).context("Could not restore the previous app bundle")?;
        return Err(error).context("Could not replace the app bundle");
    }
    executable_path(host, target)
}

/// A signed executable is sealed to its bundle's Info.plist and resources,
/// and macOS kills a copy taken out of the bundle. The helper therefore runs
/// from a copy of the whole running bundle, which the update never moves:
/// replacing and restoring the installed bundle cannot pull the helper's
/// own code out from under it.
pub(crate) fn helper(config: &UpdateConfig, host: &dyn Host, staged: &Staged) -> Result<PathBuf> {
    let source =
        bundle_root(config, &staged.installation.executable).context("Missing app bundle")?;
    let bundle = staged.file(HELPER_APP);
    ensure!(!bundle.exists(), "The update helper was already prepared");
    host.copy_bundle(source, &bundle)
        .context("Could not prepare the update helper")?;
    let name = staged
        .installation
        .executable
        .file_name()
        .context("Missing app executable")?;
    Ok(bundle.join("Contents/MacOS").join(name))
}

/// Puts the previous bundle back, moving a failed one aside.
pub(crate) fn restore(config: &UpdateConfig, staged: &Staged) -> Result<()> {
    let backup = staged.file(PREVIOUS);
    if backup.is_dir() {
        let target =
            bundle_root(config, &staged.installation.executable).context("Missing app bundle")?;
        if target.exists() {
            fs::rename(target, staged.file(FAILED_APP))
                .context("Could not move the failed update aside")?;
        }
        fs::rename(backup, target).context("Could not restore the previous app bundle")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Kind;
    use crate::testing::{FakeHost, bundle};
    use crate::tests::ZAPFAST;

    pub(crate) const SPOTIFAST: UpdateConfig = UpdateConfig {
        legacy_names: &["fastpotify"],
        macos: crate::MacConfig {
            bundle_ids: &["rocks.spotifast.Spotifast", "me.paolino.fastpotify"],
            executable_names: &["fastpotify", "Spotifast"],
            legacy_bundle_names: &["Fastpotify.app"],
        },
        ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.1")
    };

    #[test]
    fn only_the_expected_bundle_layout_is_accepted() {
        assert_eq!(
            bundle_root(
                &ZAPFAST,
                Path::new("/Applications/ZapFast.app/Contents/MacOS/zapfast")
            ),
            Some(Path::new("/Applications/ZapFast.app"))
        );
        for path in [
            "/Applications/ZapFast.app/zapfast",
            "/tmp/Contents/MacOS/zapfast",
            "/usr/local/bin/zapfast",
            "/Applications/ZapFast.app/Contents/MacOS/other",
        ] {
            assert_eq!(bundle_root(&ZAPFAST, Path::new(path)), None, "{path}");
        }
        let host = FakeHost::default();
        for path in [
            "/Volumes/ZapFast/ZapFast.app/Contents/MacOS/zapfast",
            "/private/var/folders/x/AppTranslocation/y/ZapFast.app/Contents/MacOS/zapfast",
            "/Applications/zapfast",
        ] {
            assert_eq!(
                detect(&ZAPFAST, &host, Path::new(path)),
                Err(Unsupported::MoveToApplications),
                "{path}"
            );
        }
    }

    #[test]
    fn a_renamed_executable_updates_alongside_the_old_one() {
        // Spotifast #538: installs run "fastpotify"; later releases rename it
        // to "Spotifast". Both resolve to their bundle.
        for executable in ["Spotifast", "fastpotify"] {
            assert_eq!(
                bundle_root(
                    &SPOTIFAST,
                    &Path::new("/Applications/Spotifast.app/Contents/MacOS").join(executable)
                ),
                Some(Path::new("/Applications/Spotifast.app"))
            );
        }
        assert_eq!(
            bundle_root(
                &SPOTIFAST,
                Path::new("/Applications/Spotifast.app/Contents/MacOS/spotifast")
            ),
            None,
            "the Linux and Windows command name is not the bundle's executable"
        );
    }

    #[test]
    fn identity_accepts_listed_ids_and_executables_but_nothing_else() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Spotifast.app");
        for id in ["rocks.spotifast.Spotifast", "me.paolino.fastpotify"] {
            for executable in ["fastpotify", "Spotifast"] {
                let host = FakeHost::default().with_bundle(&app, id, executable, "0.10.1");
                assert!(
                    identity(&SPOTIFAST, &host, &app).unwrap(),
                    "{id} {executable}"
                );
            }
        }
        for (id, executable) in [
            ("rocks.spotifast.Spotifast", "SomeOtherName"),
            ("com.example.other", "Spotifast"),
        ] {
            let host = FakeHost::default().with_bundle(&app, id, executable, "0.10.1");
            assert!(!identity(&SPOTIFAST, &host, &app).unwrap());
        }
        let host = FakeHost::default().with_bundle(
            &app,
            "rocks.spotifast.Spotifast",
            "Spotifast",
            "0.10.1",
        );
        assert_eq!(
            executable_path(&host, &app).unwrap(),
            app.join("Contents/MacOS/Spotifast")
        );
    }

    // Symbolic links need privileges on Windows.
    #[cfg(unix)]
    #[test]
    fn homebrew_casks_own_their_bundle_across_the_rename() {
        let root = tempfile::tempdir().unwrap();
        let names = bundle_names(&SPOTIFAST);
        for name in ["Fastpotify.app", "Spotifast.app"] {
            let installed = root.path().join("Applications").join(name);
            let version = root.path().join("Caskroom/fastpotify/0.8.0");
            fs::create_dir_all(&installed).unwrap();
            fs::create_dir_all(&version).unwrap();
            symlink(&installed, &version.join(name));
            assert!(cask_owns(
                &root.path().join("Caskroom/fastpotify"),
                &installed,
                &names
            ));
            assert!(!cask_owns(
                &root.path().join("Caskroom/unrelated"),
                &installed,
                &names
            ));
        }
    }

    // Symbolic links need privileges on Windows.
    #[cfg(unix)]
    #[test]
    fn a_cask_link_does_not_claim_other_copies() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("Applications/ZapFast.app");
        let cask = root.path().join("Caskroom/zapfast");
        let copy = root.path().join("dev/ZapFast.app");
        for path in [&installed, &copy, &cask.join("0.7.1")] {
            fs::create_dir_all(path).unwrap();
        }
        symlink(&installed, &cask.join("0.7.1/ZapFast.app"));
        let names = bundle_names(&ZAPFAST);
        assert!(cask_owns(&cask, &installed, &names));
        assert!(!cask_owns(&cask, &copy, &names));
    }

    // Symbolic links need privileges on Windows.
    #[cfg(unix)]
    #[test]
    fn a_cask_under_homebrew_prefix_refuses_the_update() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("Applications/ZapFast.app");
        let executable = installed.join("Contents/MacOS/zapfast");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        let prefix = root.path().join("brew");
        for cask in ["zapfast", "fastsapp"] {
            let version = prefix.join("Caskroom").join(cask).join("1.0");
            fs::create_dir_all(&version).unwrap();
            symlink(&installed, &version.join("ZapFast.app"));
            let host = FakeHost::default()
                .with_var("HOMEBREW_PREFIX", &prefix)
                .with_bundle(&installed, "me.paolino.fastsapp", "zapfast", "0.16.3");
            assert_eq!(
                detect(&ZAPFAST, &host, &executable),
                Err(Unsupported::Homebrew),
                "{cask}"
            );
            fs::remove_dir_all(prefix.join("Caskroom")).unwrap();
        }
        let host = FakeHost::default()
            .with_var("HOMEBREW_PREFIX", &prefix)
            .with_bundle(&installed, "me.paolino.fastsapp", "zapfast", "0.16.3");
        assert_eq!(detect(&ZAPFAST, &host, &executable), Ok(()));
        let host = FakeHost::default().with_bundle(&installed, "com.example", "zapfast", "0.16.3");
        assert_eq!(
            detect(&ZAPFAST, &host, &executable),
            Err(Unsupported::ForeignBundle)
        );
    }

    // Symbolic links need privileges on Windows.
    #[cfg(unix)]
    #[test]
    fn disk_images_prefer_the_new_bundle_name_and_never_follow_links() {
        let root = tempfile::tempdir().unwrap();
        assert!(image_bundle(&ZAPFAST, root.path()).is_err());
        let legacy = root.path().join("FastsApp.app");
        fs::create_dir(&legacy).unwrap();
        assert_eq!(image_bundle(&ZAPFAST, root.path()).unwrap(), legacy);
        let app = root.path().join("ZapFast.app");
        fs::create_dir(&app).unwrap();
        assert_eq!(image_bundle(&ZAPFAST, root.path()).unwrap(), app);
        fs::remove_dir(&app).unwrap();
        symlink(&legacy, &app);
        assert!(image_bundle(&ZAPFAST, root.path()).is_err());
    }

    #[test]
    fn another_mount_attempt_leaves_the_previous_volume_alone() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("update.dmg");
        let first = mountpoint(&image).unwrap();
        fs::write(first.join("still-mounted"), b"existing volume").unwrap();
        let second = mountpoint(&image).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read(first.join("still-mounted")).unwrap(),
            b"existing volume"
        );
    }

    #[test]
    fn a_failed_detach_keeps_the_mount_point() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("update.dmg");
        let host = FakeHost::default().failing_detach();
        let mounted = Mounted::open(&host, &image).unwrap();
        let mountpoint = mounted.mountpoint.clone();
        drop(mounted);
        assert!(mountpoint.is_dir());
        let host = FakeHost::default();
        let mounted = Mounted::open(&host, &image).unwrap();
        let mountpoint = mounted.mountpoint.clone();
        drop(mounted);
        assert!(!mountpoint.exists());
    }

    fn installed(root: &Path) -> Staged {
        let app = root.join("ZapFast.app");
        bundle(&app, "zapfast", b"old executable", b"old metadata");
        let installation = Installation {
            executable: app.join("Contents/MacOS/zapfast"),
            kind: Kind::MacBundle,
        };
        let directory = crate::stage::staging(&ZAPFAST, &installation).unwrap();
        Staged {
            payload: directory.join("zapfast-v0.17.0-macos-universal.dmg"),
            directory,
            installation,
            sha256: String::new(),
            version: "0.17.0".into(),
        }
    }

    #[test]
    fn the_helper_runs_from_a_whole_copy_of_the_bundle() {
        let root = tempfile::tempdir().unwrap();
        let staged = installed(root.path());
        assert_eq!(staged.directory.parent(), Some(root.path()));
        let host = FakeHost::default();
        let executable = helper(&ZAPFAST, &host, &staged).unwrap();
        assert_eq!(executable, staged.file("helper.app/Contents/MacOS/zapfast"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
        assert_eq!(
            fs::read(staged.file("helper.app/Contents/Info.plist")).unwrap(),
            b"old metadata"
        );
        assert!(helper(&ZAPFAST, &host, &staged).is_err());
    }

    #[test]
    fn replacing_swaps_whole_bundles_and_reports_a_renamed_executable() {
        let root = tempfile::tempdir().unwrap();
        let staged = installed(root.path());
        let app = root.path().join("ZapFast.app");
        let host = FakeHost::default()
            .with_image(&staged.payload, |volume| {
                bundle(
                    &volume.join("ZapFast.app"),
                    "zapfast",
                    b"new executable",
                    b"new metadata",
                );
            })
            .with_any_bundle("me.paolino.fastsapp", "zapfast", "0.17.0")
            .with_version_output("zapfast 0.17.0");
        let executable = replace(&ZAPFAST, &host, &staged).unwrap();
        assert_eq!(executable, app.join("Contents/MacOS/zapfast"));
        assert_eq!(fs::read(&executable).unwrap(), b"new executable");
        assert_eq!(
            fs::read(staged.file("previous/Contents/MacOS/zapfast")).unwrap(),
            b"old executable"
        );
        assert!(replace(&ZAPFAST, &host, &staged).is_err(), "applied once");
        restore(&ZAPFAST, &staged).unwrap();
        assert_eq!(
            fs::read(app.join("Contents/MacOS/zapfast")).unwrap(),
            b"old executable"
        );
        assert_eq!(
            fs::read(app.join("Contents/Info.plist")).unwrap(),
            b"old metadata"
        );
        assert_eq!(
            fs::read(staged.file("failed.app/Contents/Info.plist")).unwrap(),
            b"new metadata"
        );
        assert!(!staged.file(PREVIOUS).exists());
    }

    #[test]
    fn a_download_from_another_publisher_is_refused_before_anything_moves() {
        let root = tempfile::tempdir().unwrap();
        let staged = installed(root.path());
        let app = root.path().join("ZapFast.app");
        let host = FakeHost::default()
            .with_image(&staged.payload, |volume| {
                bundle(
                    &volume.join("ZapFast.app"),
                    "zapfast",
                    b"new executable",
                    b"new metadata",
                );
            })
            .with_any_bundle("me.paolino.fastsapp", "zapfast", "0.17.0")
            .with_version_output("zapfast 0.17.0")
            .with_team(&app, "TEAMONE")
            .with_default_team("TEAMTWO");
        let error = replace(&ZAPFAST, &host, &staged).unwrap_err();
        assert!(
            error.to_string().contains("different publisher"),
            "{error:#}"
        );
        assert_eq!(
            fs::read(app.join("Contents/MacOS/zapfast")).unwrap(),
            b"old executable"
        );
        assert!(!staged.file(PREVIOUS).exists());
    }

    #[test]
    fn team_identifiers_are_read_from_codesign_output() {
        assert_eq!(
            team_identifier("Executable=/x\nTeamIdentifier=ABCDE12345\nSealed"),
            Some("ABCDE12345".into())
        );
        assert_eq!(team_identifier("TeamIdentifier=not set\n"), None);
        assert_eq!(team_identifier(""), None);
    }

    #[cfg(unix)]
    fn symlink(original: &Path, link: &Path) {
        std::os::unix::fs::symlink(original, link).unwrap();
    }
}
