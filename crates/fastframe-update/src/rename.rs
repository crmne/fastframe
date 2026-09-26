//! Moving an installation off a legacy name.
//!
//! An app renamed from one of [`UpdateConfig::legacy_names`] may still run
//! under that name: a launcher from before the rename starts it, and every
//! helper relaunches the executable it was started from. Each update moves
//! it onto the slug:
//!
//! - Windows installer: the setup program installs the slug beside the old
//!   executable. The helper relaunches the slug, and the old executable goes
//!   once nothing needs it: after that start succeeds, or when the app starts
//!   under its slug with no update in progress.
//! - Portable copy: the update is written under the slug. On Unix the old
//!   name stays as a link to it, for scripts that name it.
//! - macOS: the bundle comes back as `<app_name>.app` (see `macos`).
//!
//! The launchers that named the old path are repointed (see `shortcuts`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::UpdateConfig;
use crate::detect::{self, Kind, Platform};
use crate::host::Host;
use crate::stage::{self, JOB, RESULT};

/// How long an update without a result counts as running. An older helper
/// may still be about to relaunch the legacy executable; one that died
/// leaves its folder behind, which must not keep the file forever.
const IN_PROGRESS: Duration = Duration::from_secs(10 * 60);

/// Whether `executable` is named after one of the legacy names.
fn is_legacy(config: &UpdateConfig, executable: &Path) -> bool {
    executable
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| {
            config
                .legacy_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(stem))
        })
}

/// `name` beside `executable`, with its extension.
fn sibling(executable: &Path, name: &str) -> PathBuf {
    let mut sibling = executable.with_file_name(name);
    if let Some(extension) = executable.extension() {
        sibling.set_extension(extension);
    }
    sibling
}

/// The slug's executable beside `executable`, when `executable` has a
/// legacy name and the setup program installed the slug next to it.
pub(crate) fn own_executable(config: &UpdateConfig, executable: &Path) -> Option<PathBuf> {
    let own = sibling(executable, config.slug);
    (is_legacy(config, executable) && own.is_file()).then_some(own)
}

/// Where a portable copy under a legacy name is updated to: the slug beside
/// it. `None` keeps the name: the copy is the slug already, the archive
/// names its app another way, or something else has the slug's name.
pub(crate) fn portable_target(config: &UpdateConfig, executable: &Path) -> Option<PathBuf> {
    let own = sibling(executable, config.slug);
    (config.portable_executable.is_none()
        && is_legacy(config, executable)
        && fs::symlink_metadata(&own).is_err())
    .then_some(own)
}

/// Retires the legacy name once `own` is in place: on Unix it becomes a
/// link to `own`, elsewhere it is deleted.
pub(crate) fn retire(legacy: &Path, own: &Path) -> io::Result<()> {
    if fs::symlink_metadata(legacy).is_ok() {
        fs::remove_file(legacy)?;
    }
    #[cfg(unix)]
    {
        let name = own.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        std::os::unix::fs::symlink(name, legacy)?;
    }
    #[cfg(not(unix))]
    let _ = own;
    Ok(())
}

/// Undoes a portable rename: `own` goes, and `legacy` is the backup again.
pub(crate) fn undo_portable(legacy: &Path, own: &Path, backup: &Path) -> io::Result<()> {
    if fs::symlink_metadata(own).is_ok() {
        fs::remove_file(own)?;
    }
    if fs::symlink_metadata(legacy).is_ok_and(|metadata| metadata.is_symlink()) {
        fs::remove_file(legacy)?;
    }
    if fs::symlink_metadata(legacy).is_err() {
        fs::copy(backup, legacy)?;
    }
    Ok(())
}

/// Repoints the launchers of the legacy executables beside `executable`,
/// which must be the slug's own, and deletes them. Returns the files it
/// deleted; one still running stays.
pub(crate) fn remove_legacy(
    config: &UpdateConfig,
    host: &dyn Host,
    platform: Platform,
    executable: &Path,
) -> Vec<PathBuf> {
    let own = executable
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case(config.slug));
    if !own {
        return Vec::new();
    }
    config
        .legacy_names
        .iter()
        .map(|name| sibling(executable, name))
        .filter(|legacy| legacy.is_file())
        .filter_map(|legacy| {
            crate::shortcuts::repoint(host, platform, &legacy, executable);
            fs::remove_file(&legacy).is_ok().then_some(legacy)
        })
        .collect()
}

/// At startup: an app installed by its setup program and running under its
/// slug retires the legacy executables beside it, unless an update is being
/// installed there.
pub(crate) fn tidy(config: &UpdateConfig, host: &dyn Host, platform: Platform) -> Vec<PathBuf> {
    if platform != Platform::Windows || config.legacy_names.is_empty() {
        return Vec::new();
    }
    let Ok(executable) = host.current_exe() else {
        return Vec::new();
    };
    let Ok(installation) = detect::detect(config, host, platform, &executable) else {
        return Vec::new();
    };
    let Some(directory) = executable.parent() else {
        return Vec::new();
    };
    if installation.kind != Kind::WindowsInstaller || updating(config, directory) {
        return Vec::new();
    }
    remove_legacy(config, host, platform, &executable)
}

/// Whether a staging folder in `directory` holds a recent job without a
/// result: a helper may be about to relaunch a legacy executable.
fn updating(config: &UpdateConfig, directory: &Path) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return true;
    };
    let now = SystemTime::now();
    entries.flatten().any(|entry| {
        let folder = entry.path();
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| stage::is_staging_name(config, name))
            && !folder.join(RESULT).exists()
            && fs::metadata(folder.join(JOB))
                .and_then(|job| job.modified())
                .is_ok_and(|modified| {
                    now.duration_since(modified)
                        .map_or(true, |age| age < IN_PROGRESS)
                })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;

    const SPOTIFAST: UpdateConfig = UpdateConfig {
        legacy_names: &["fastpotify"],
        ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.3")
    };

    /// A setup program's folder with both executables and its marker.
    fn installed(root: &Path) -> (PathBuf, PathBuf) {
        fs::write(
            root.join("spotifast-installer.txt"),
            "spotifast-installer-v1\n",
        )
        .unwrap();
        let own = root.join("spotifast.exe");
        let legacy = root.join("fastpotify.exe");
        fs::write(&own, b"app").unwrap();
        fs::write(&legacy, b"app").unwrap();
        (own.canonicalize().unwrap(), legacy.canonicalize().unwrap())
    }

    #[test]
    fn a_legacy_name_leads_to_the_installed_slug() {
        let root = tempfile::tempdir().unwrap();
        let (own, legacy) = installed(root.path());
        assert_eq!(own_executable(&SPOTIFAST, &legacy), Some(own.clone()));
        let shouting = legacy.with_file_name("FastPotify.exe");
        assert_eq!(own_executable(&SPOTIFAST, &shouting), Some(own.clone()));
        assert_eq!(own_executable(&SPOTIFAST, &own), None, "already the slug");
        fs::remove_file(&own).unwrap();
        assert_eq!(own_executable(&SPOTIFAST, &legacy), None, "nothing to run");
    }

    #[test]
    fn only_the_slug_removes_legacy_executables() {
        let root = tempfile::tempdir().unwrap();
        let (own, legacy) = installed(root.path());
        let host = FakeHost::default();
        let windows = Platform::Windows;
        assert!(remove_legacy(&SPOTIFAST, &host, windows, &legacy).is_empty());
        assert!(legacy.is_file());
        assert!(host.repointed_shortcuts().is_empty());
        assert_eq!(
            remove_legacy(&SPOTIFAST, &host, windows, &own),
            std::slice::from_ref(&legacy)
        );
        assert!(!legacy.exists());
        assert!(own.is_file());
        assert!(remove_legacy(&SPOTIFAST, &host, windows, &own).is_empty());
    }

    #[test]
    fn startup_tidies_an_installer_folder_on_windows_only() {
        let root = tempfile::tempdir().unwrap();
        let (own, legacy) = installed(root.path());
        let host = FakeHost::default()
            .with_current_exe(&own)
            .with_var("APPDATA", root.path().join("roaming").as_os_str());
        for platform in [Platform::Linux, Platform::MacOs] {
            assert!(tidy(&SPOTIFAST, &host, platform).is_empty());
        }
        let as_legacy = FakeHost::default().with_current_exe(&legacy);
        assert!(tidy(&SPOTIFAST, &as_legacy, Platform::Windows).is_empty());
        assert!(legacy.is_file());
        assert_eq!(
            tidy(&SPOTIFAST, &host, Platform::Windows),
            std::slice::from_ref(&legacy)
        );
        assert!(!legacy.exists());
        let repointed = host.repointed_shortcuts();
        assert_eq!(repointed.len(), 1, "its shortcuts first");
        assert_eq!((&repointed[0].1, &repointed[0].2), (&legacy, &own));
    }

    /// A portable copy moves when it updates, not when it starts.
    #[test]
    fn startup_leaves_a_portable_folder_to_the_helper() {
        let root = tempfile::tempdir().unwrap();
        let (own, legacy) = installed(root.path());
        fs::remove_file(root.path().join("spotifast-installer.txt")).unwrap();
        fs::write(
            root.path().join("spotifast-portable.txt"),
            "spotifast-portable-v1\n",
        )
        .unwrap();
        let host = FakeHost::default().with_current_exe(&own);
        assert!(tidy(&SPOTIFAST, &host, Platform::Windows).is_empty());
        assert!(legacy.is_file());
    }

    /// An older helper relaunches the legacy name after its installer runs.
    /// Starting the app from the Start menu meanwhile must not delete it.
    #[test]
    fn a_running_update_keeps_the_legacy_executable_until_it_is_old() {
        let root = tempfile::tempdir().unwrap();
        let (own, legacy) = installed(root.path());
        let folder = root.path().join(".fastpotify-update-0123456789abcdef");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join(JOB), b"{}").unwrap();
        let host = FakeHost::default().with_current_exe(&own);
        assert!(tidy(&SPOTIFAST, &host, Platform::Windows).is_empty());
        assert!(legacy.is_file());

        fs::write(folder.join(RESULT), b"Updated to 0.10.3").unwrap();
        assert!(!updating(&SPOTIFAST, root.path()), "finished");
        fs::remove_file(folder.join(RESULT)).unwrap();
        fs::File::options()
            .write(true)
            .open(folder.join(JOB))
            .unwrap()
            .set_modified(SystemTime::now() - IN_PROGRESS - Duration::from_secs(1))
            .unwrap();
        assert!(!updating(&SPOTIFAST, root.path()), "abandoned");
        assert_eq!(tidy(&SPOTIFAST, &host, Platform::Windows), [legacy]);
    }
    #[test]
    fn a_portable_copy_under_a_legacy_name_updates_to_the_slug() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("fastpotify");
        fs::write(&legacy, b"old").unwrap();
        let own = root.path().join("spotifast");
        assert_eq!(portable_target(&SPOTIFAST, &legacy), Some(own.clone()));
        assert_eq!(portable_target(&SPOTIFAST, &own), None, "already the slug");
        let named = UpdateConfig {
            portable_executable: Some("spotifast-gui"),
            ..SPOTIFAST
        };
        assert_eq!(portable_target(&named, &legacy), None, "its own naming");
        fs::write(&own, b"someone else's").unwrap();
        assert_eq!(portable_target(&SPOTIFAST, &legacy), None, "taken");
    }

    #[test]
    fn a_retired_name_is_a_link_on_unix_and_gone_elsewhere_and_can_come_back() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("fastpotify");
        let own = root.path().join("spotifast");
        let backup = root.path().join("previous");
        fs::write(&legacy, b"old").unwrap();
        fs::write(&backup, b"old").unwrap();
        fs::write(&own, b"new").unwrap();
        retire(&legacy, &own).unwrap();
        if cfg!(unix) {
            assert_eq!(fs::read_link(&legacy).unwrap(), Path::new("spotifast"));
            assert_eq!(fs::read(&legacy).unwrap(), b"new");
        } else {
            assert!(fs::symlink_metadata(&legacy).is_err());
        }
        undo_portable(&legacy, &own, &backup).unwrap();
        assert!(fs::symlink_metadata(&own).is_err());
        assert!(!fs::symlink_metadata(&legacy).unwrap().is_symlink());
        assert_eq!(fs::read(&legacy).unwrap(), b"old");
    }
}
