//! The staging folder beside the app, and the files in it that old and new
//! versions of an app read from each other.
//!
//! A staging folder is `.<slug>-update-<16 hex digits>` next to the
//! executable (next to the bundle on macOS), mode 0700 on Unix, created
//! fresh for every download and never reused. In it:
//!
//! | File | Written by | Meaning |
//! | --- | --- | --- |
//! | `<asset name>` | download | the verified package; kept for the installer and disk image |
//! | `<slug>` or `<slug>.exe` | download | the unpacked portable executable |
//! | `helper`, `helper.exe`, `helper.app` | handoff | the copy of the running app that installs |
//! | `handoff.json` | handoff, helper | the job, and later the relaunched app's receipt |
//! | `helper.log` | helper | the helper's standard error |
//! | `ready` | helper | the helper is watching the app |
//! | `previous` | helper | the backup that a rollback restores |
//! | `installer.log` | Windows installer | its log |
//! | `mounted-<16 hex digits>` | helper, download | a disk image mount point |
//! | `failed.app` | helper | a rolled-back macOS bundle |
//! | `started` | relaunched app | it opened its window |
//! | `result.txt` | helper | `Updated to <version>`, or why it failed |

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::detect::{Installation, Kind};
use crate::{UpdateConfig, hex};

/// The largest package or executable accepted: 2 GiB.
pub(crate) const LIMIT: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) const JOB: &str = "handoff.json";
pub(crate) const READY: &str = "ready";
pub(crate) const STARTED: &str = "started";
pub(crate) const RESULT: &str = "result.txt";
pub(crate) const PREVIOUS: &str = "previous";
pub(crate) const HELPER_LOG: &str = "helper.log";
pub(crate) const HELPER_APP: &str = "helper.app";
pub(crate) const FAILED_APP: &str = "failed.app";
pub(crate) const INSTALLER_LOG: &str = "installer.log";

/// Files whose presence means a staging folder was already used for an
/// attempt. A handoff refuses such a folder: a stale `ready` would report a
/// helper that is not watching, and a stale `started` would skip rollback.
pub(crate) const ATTEMPT_MARKERS: [&str; 10] = [
    JOB,
    READY,
    STARTED,
    RESULT,
    PREVIOUS,
    HELPER_LOG,
    HELPER_APP,
    FAILED_APP,
    "helper",
    "helper.exe",
];

/// What a download staged, as `handoff.json` records it. The field names and
/// order are the file format; see `tests/fixtures`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Staged {
    pub(crate) installation: Installation,
    pub(crate) directory: PathBuf,
    pub(crate) payload: PathBuf,
    pub(crate) sha256: String,
    pub(crate) version: String,
}

/// `handoff.json`. The field names and order are the file format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Handoff {
    pub(crate) prepared: Staged,
    pub(crate) parent: u32,
    pub(crate) arguments: Vec<String>,
}

/// A verified update, staged beside the app and ready for
/// [`Updater::handoff`](crate::Updater::handoff).
///
/// Only [`Updater::download`](crate::Updater::download) makes one that can
/// be installed, and a handoff consumes it. It cannot be saved and read
/// back: an update never installs from paths an app read out of a file.
#[derive(Debug)]
pub struct Prepared {
    pub(crate) staged: Staged,
    pub(crate) sample: bool,
}

impl Prepared {
    /// The version it installs.
    pub fn version(&self) -> &str {
        &self.staged.version
    }

    /// The installation it replaces.
    pub fn installation(&self) -> &Installation {
        &self.staged.installation
    }

    /// A stand-in for demos and interface tests. A handoff refuses it.
    pub fn sample(installation: Installation, version: &str) -> Self {
        let directory = installation
            .executable
            .with_file_name(".sample-update-0000000000000000");
        Self {
            staged: Staged {
                payload: directory.join("sample"),
                directory,
                installation,
                sha256: String::new(),
                version: version.to_owned(),
            },
            sample: true,
        }
    }

    /// Removes the staging folder, for an app that will not install it.
    pub fn discard(self) {
        if !self.sample {
            discard(&self.staged.directory);
        }
    }
}

/// A new, empty staging folder beside the installation.
pub(crate) fn staging(config: &UpdateConfig, installation: &Installation) -> Result<PathBuf> {
    let parent = installation
        .root(config)?
        .parent()
        .context("Missing installation directory")?;
    let directory = parent.join(format!(".{}-update-{}", config.slug, random_suffix()?));
    fs::create_dir(&directory).context("Cannot write to the installation directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory)
}

/// 16 lowercase hexadecimal digits.
pub(crate) fn random_suffix() -> Result<String> {
    let mut bytes = [0; 8];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("The system random number generator failed"))?;
    Ok(hex(&bytes))
}

/// Whether `name` is a staging folder name this app (or an earlier name of
/// it) writes.
pub(crate) fn is_staging_name(config: &UpdateConfig, name: &str) -> bool {
    config.names().any(|slug| {
        name.strip_prefix(&format!(".{slug}-update-"))
            .is_some_and(|suffix| {
                suffix.len() == 16
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    })
}

/// Whether the folder still holds nothing from an earlier attempt.
pub(crate) fn unused(directory: &Path) -> bool {
    ATTEMPT_MARKERS.iter().all(|name| {
        fs::symlink_metadata(directory.join(name))
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

/// Removes a staging folder, unless a disk image may still be mounted in
/// it: a failed detach leaves the volume there, and removing the folder
/// would walk into it.
pub(crate) fn discard(directory: &Path) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("mounted-")
            && fs::remove_dir(entry.path()).is_err()
        {
            return false;
        }
    }
    fs::remove_dir_all(directory).is_ok()
}

pub(crate) fn hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex(&hash.finalize()))
}

pub(crate) fn read_handoff(job: &Path) -> Result<Handoff> {
    let mut bytes = Vec::new();
    File::open(job)
        .context("Cannot open the update job")?
        .take(1024 * 1024)
        .read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).context("Invalid update job")
}

/// Writes a new job file. It must not exist yet.
pub(crate) fn create_handoff(job: &Path, handoff: &Handoff) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(job)?;
    serde_json::to_writer(&mut file, handoff)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

/// Rewrites the job the relaunched app reads as its receipt.
pub(crate) fn rewrite_handoff(job: &Path, handoff: &Handoff) -> Result<()> {
    fs::write(job, serde_json::to_vec(handoff)?)?;
    Ok(())
}

pub(crate) fn backup_current(target: &Path, backup: &Path) -> Result<()> {
    let mut source = File::open(target)?;
    let permissions = source.metadata()?.permissions();
    write_backup(&mut source, backup, permissions)
}

/// Rollback recognizes only `previous`. Publish that name after the complete
/// copy has been synced, so a failed copy cannot replace a working executable
/// with the partial backup it left behind.
pub(crate) fn write_backup(
    source: &mut impl Read,
    backup: &Path,
    permissions: fs::Permissions,
) -> Result<()> {
    ensure!(
        fs::symlink_metadata(backup)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "This update already has a backup"
    );
    let partial = backup.with_extension("partial");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)?;
    let result = (|| -> Result<()> {
        std::io::copy(source, &mut output)?;
        output.sync_all()?;
        fs::set_permissions(&partial, permissions)?;
        Ok(())
    })();
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    if let Err(error) = fs::rename(&partial, backup) {
        let _ = fs::remove_file(&partial);
        return Err(error.into());
    }
    Ok(())
}

/// A path Inno Setup accepts in `/DIR=` and `/LOG=`: no `\\?\` prefix.
pub(crate) fn installer_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned()
    }
}

/// The arguments for a silent Inno Setup upgrade in place.
pub(crate) fn installer_arguments(installation: &Path, log: &Path) -> Vec<String> {
    vec![
        "/VERYSILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/CLOSEAPPLICATIONS".into(),
        "/NORESTARTAPPLICATIONS".into(),
        format!("/DIR={}", installer_path(installation)),
        format!("/LOG={}", installer_path(log)),
    ]
}

impl Staged {
    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    /// Checks that the paths belong together, so a job file cannot point the
    /// helper or the receipt anywhere else.
    pub(crate) fn check_layout(&self, config: &UpdateConfig, job: &Path) -> Result<()> {
        ensure!(
            job.parent() == Some(self.directory.as_path()),
            "Invalid update job directory"
        );
        ensure!(
            self.payload.parent() == Some(self.directory.as_path()),
            "Invalid staged payload"
        );
        ensure!(
            self.directory
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| is_staging_name(config, name)),
            "Invalid update directory name"
        );
        ensure!(
            self.directory.parent() == self.installation.root(config)?.parent(),
            "Invalid installation directory"
        );
        ensure!(
            crate::version::is_plain_release(&self.version),
            "Invalid update version"
        );
        if self.installation.kind == Kind::MacBundle {
            ensure!(cfg!(target_os = "macos"), "This update is for macOS");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::ZAPFAST;

    fn portable(directory: &Path) -> Installation {
        Installation {
            executable: directory.join("zapfast"),
            kind: Kind::Portable,
        }
    }

    #[test]
    fn every_download_gets_a_fresh_private_folder() {
        let directory = tempfile::tempdir().unwrap();
        let installation = portable(directory.path());
        let first = staging(&ZAPFAST, &installation).unwrap();
        let second = staging(&ZAPFAST, &installation).unwrap();
        assert_ne!(first, second);
        for stage in [&first, &second] {
            assert_eq!(stage.parent(), Some(directory.path()));
            let name = stage.file_name().unwrap().to_str().unwrap();
            assert!(is_staging_name(&ZAPFAST, name), "{name}");
            assert!(unused(stage));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(stage).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
    }

    #[test]
    fn staging_names_accept_legacy_slugs_and_nothing_else() {
        assert!(is_staging_name(
            &ZAPFAST,
            ".zapfast-update-0123456789abcdef"
        ));
        assert!(is_staging_name(
            &ZAPFAST,
            ".fastsapp-update-0123456789abcdef"
        ));
        for name in [
            ".spotifast-update-0123456789abcdef",
            ".zapfast-update-0123456789ABCDEF",
            ".zapfast-update-0123",
            ".zapfast-update-0123456789abcdef0",
            "zapfast-update-0123456789abcdef",
            ".zapfast-pending",
        ] {
            assert!(!is_staging_name(&ZAPFAST, name), "{name}");
        }
    }

    #[test]
    fn any_marker_from_an_earlier_attempt_makes_a_folder_used() {
        for marker in ATTEMPT_MARKERS {
            let directory = tempfile::tempdir().unwrap();
            assert!(unused(directory.path()));
            fs::write(directory.path().join(marker), b"stale").unwrap();
            assert!(!unused(directory.path()), "{marker}");
        }
    }

    #[test]
    fn a_folder_with_a_mounted_volume_is_left_alone() {
        let directory = tempfile::tempdir().unwrap();
        let stage = directory.path().join(".zapfast-update-0123456789abcdef");
        let mount = stage.join("mounted-0123456789abcdef");
        fs::create_dir_all(&mount).unwrap();
        fs::write(mount.join("ZapFast.app"), b"still mounted").unwrap();
        assert!(!discard(&stage));
        assert_eq!(
            fs::read(mount.join("ZapFast.app")).unwrap(),
            b"still mounted"
        );
        fs::remove_file(mount.join("ZapFast.app")).unwrap();
        fs::write(stage.join("zapfast"), b"payload").unwrap();
        assert!(discard(&stage));
        assert!(!stage.exists());
    }

    #[test]
    fn an_interrupted_backup_is_never_available_to_rollback() {
        struct InterruptedCopy(bool);
        impl Read for InterruptedCopy {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::other("injected copy failure"));
                }
                self.0 = true;
                buffer[0] = b'p';
                Ok(1)
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("zapfast");
        fs::write(&target, b"working executable").unwrap();
        let backup = directory.path().join(PREVIOUS);
        let permissions = fs::metadata(&target).unwrap().permissions();
        assert!(write_backup(&mut InterruptedCopy(false), &backup, permissions).is_err());
        assert!(
            !backup.exists(),
            "rollback must not see the incomplete copy"
        );
        assert!(!backup.with_extension("partial").exists());
        assert_eq!(fs::read(&target).unwrap(), b"working executable");
        backup_current(&target, &backup).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"working executable");
        fs::write(&target, b"new executable").unwrap();
        assert!(backup_current(&target, &backup).is_err());
        assert_eq!(
            fs::read(&backup).unwrap(),
            b"working executable",
            "a retry keeps the known backup"
        );
    }

    #[test]
    fn installer_arguments_use_paths_inno_setup_accepts() {
        assert_eq!(
            installer_path(Path::new(r"\\?\C:\Users\test\ZapFast")),
            r"C:\Users\test\ZapFast"
        );
        assert_eq!(
            installer_path(Path::new(r"\\?\UNC\server\share\ZapFast")),
            r"\\server\share\ZapFast"
        );
        assert_eq!(
            installer_arguments(
                Path::new(r"\\?\C:\Apps\ZapFast"),
                Path::new(r"\\?\C:\Apps\.zapfast-update-0\installer.log")
            ),
            [
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/CLOSEAPPLICATIONS",
                "/NORESTARTAPPLICATIONS",
                r"/DIR=C:\Apps\ZapFast",
                r"/LOG=C:\Apps\.zapfast-update-0\installer.log",
            ]
        );
    }

    #[test]
    fn the_job_must_describe_one_staging_folder_beside_the_app() {
        let directory = tempfile::tempdir().unwrap();
        let installation = portable(directory.path());
        let stage = staging(&ZAPFAST, &installation).unwrap();
        let good = Staged {
            installation: installation.clone(),
            directory: stage.clone(),
            payload: stage.join("zapfast"),
            sha256: "0".repeat(64),
            version: "0.17.0".into(),
        };
        let job = stage.join(JOB);
        good.check_layout(&ZAPFAST, &job).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let foreign = elsewhere.path().join(".zapfast-update-0123456789abcdef");
        for bad in [
            Staged {
                payload: directory.path().join("zapfast"),
                ..good.clone()
            },
            Staged {
                directory: foreign.clone(),
                payload: foreign.join("zapfast"),
                ..good.clone()
            },
            Staged {
                directory: directory.path().join("staging"),
                payload: directory.path().join("staging/zapfast"),
                ..good.clone()
            },
            Staged {
                version: "0.17.0/../../x".into(),
                ..good.clone()
            },
        ] {
            let job = bad.directory.join(JOB);
            assert!(bad.check_layout(&ZAPFAST, &job).is_err(), "{bad:?}");
        }
        assert!(
            good.check_layout(&ZAPFAST, &directory.path().join(JOB))
                .is_err()
        );
    }

    #[test]
    fn samples_are_never_installed_or_discarded() {
        let directory = tempfile::tempdir().unwrap();
        let sample = Prepared::sample(portable(directory.path()), "99.0.0");
        assert_eq!(sample.version(), "99.0.0");
        assert!(sample.sample);
        sample.discard();
        assert!(directory.path().exists());
    }
}
