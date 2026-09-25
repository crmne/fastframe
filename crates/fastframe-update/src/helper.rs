//! Handing a staged update to the helper, the helper itself, and the
//! relaunched app's acknowledgement.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};

use crate::detect::Kind;
use crate::host::Host;
use crate::stage::{
    self, HELPER_LOG, Handoff, INSTALLER_LOG, JOB, PREVIOUS, Prepared, READY, RESULT, STARTED,
    Staged,
};
use crate::{APPLY_UPDATE_FLAG, UPDATE_ERROR_FLAG, UPDATE_RECEIPT_FLAG, UpdateConfig};

/// What a restored app is told. Old helpers send the same text.
pub(crate) const RESTORED: &str =
    "The update could not start. The previous version has been restored.";

/// Starts the helper and returns once it is watching this process. The app
/// must quit soon after: the helper waits one minute.
pub(crate) fn handoff(
    config: &UpdateConfig,
    host: &dyn Host,
    prepared: Prepared,
    arguments: Vec<String>,
) -> Result<()> {
    ensure!(!prepared.sample, "A sample update cannot be installed");
    let staged = prepared.staged;
    ensure!(
        stage::unused(&staged.directory),
        "This update was already tried. Download it again."
    );
    ensure!(
        stage::hash(&staged.payload)? == staged.sha256,
        "The staged update changed. Download it again."
    );
    let helper = match staged.installation.kind {
        Kind::MacBundle => crate::macos::helper(config, host, &staged)?,
        Kind::Portable | Kind::WindowsInstaller => {
            // Windows locks a running executable, so the helper cannot be
            // the file it replaces.
            let helper = staged.file(if cfg!(windows) {
                "helper.exe"
            } else {
                "helper"
            });
            fs::copy(host.current_exe()?, &helper)
                .context("Could not prepare the update helper")?;
            helper
        }
    };
    let job = staged.file(JOB);
    stage::create_handoff(
        &job,
        &Handoff {
            prepared: staged.clone(),
            parent: std::process::id(),
            arguments,
        },
    )?;
    let log = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged.file(HELPER_LOG))?;
    let mut child = host
        .spawn(
            &helper,
            &[APPLY_UPDATE_FLAG.into(), job.into_os_string()],
            Some(log),
        )
        .context("Cannot start the update helper")?;
    let timing = host.timing();
    let ready = staged.file(READY);
    let start = Instant::now();
    while start.elapsed() < timing.helper_ready {
        if ready.exists() {
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            bail!(
                "The update helper exited before it was ready. See {}",
                staged.file(HELPER_LOG).display()
            );
        }
        std::thread::sleep(timing.poll);
    }
    child.kill();
    bail!("The update helper did not start. Try again.")
}

/// The helper: waits for the app to exit, replaces it, relaunches it with a
/// receipt and waits for its acknowledgement. Any failure after the app has
/// exited restores the previous version and restarts it with an error.
pub(crate) fn run(config: &UpdateConfig, host: &dyn Host, job: &Path) -> Result<()> {
    let mut handoff = stage::read_handoff(job)?;
    // Kept aside for rollback: replacing can rename the executable inside a
    // macOS bundle, and a rollback restores the previous bundle, which still
    // has the old name.
    let original = handoff.prepared.clone();
    original.check_layout(config, job)?;
    ensure!(
        stage::hash(&original.payload)? == original.sha256,
        "The staged update checksum changed"
    );
    for stale in [READY, STARTED, RESULT, PREVIOUS] {
        ensure!(
            !original.file(stale).exists(),
            "This update was already tried. Download it again."
        );
    }
    host.wait_for_parent(handoff.parent, &original.file(READY))?;
    // From here on something may have changed on disk, so every failure
    // rolls back to `original` and restarts the previous app.
    let fail = |error: anyhow::Error, message: &str, arguments: &[String]| -> Result<()> {
        restore(config, &original)?;
        fs::write(original.file(RESULT), format!("{message}: {error:#}"))?;
        restart_with_error(host, &original.installation.executable, arguments)?;
        Err(error)
    };
    let executable = match replace(config, host, &original) {
        Ok(executable) => executable,
        Err(error) => return fail(error, "Update failed", &handoff.arguments),
    };
    if executable != original.installation.executable {
        // The relaunched app reads this same file as its receipt, so it has
        // to name the executable that is now installed.
        handoff.prepared.installation.executable = executable.clone();
        if let Err(error) = stage::rewrite_handoff(job, &handoff) {
            return fail(error, "Update failed", &handoff.arguments);
        }
    }
    let mut arguments: Vec<OsString> = handoff.arguments.iter().map(OsString::from).collect();
    arguments.push(UPDATE_RECEIPT_FLAG.into());
    arguments.push(job.as_os_str().to_owned());
    let started = original.file(STARTED);
    let timing = host.timing();
    let launch = (|| -> Result<()> {
        let mut child = host
            .spawn(&executable, &arguments, None)
            .context("Could not launch the updated app")?;
        let start = Instant::now();
        loop {
            if started.is_file() {
                return Ok(());
            }
            if child.try_wait()?.is_some() {
                bail!("The updated app exited before opening its window");
            }
            if start.elapsed() >= timing.app_start {
                child.kill();
                bail!("The updated app did not open its window within one minute");
            }
            std::thread::sleep(timing.poll);
        }
    })();
    if let Err(error) = launch {
        return fail(
            error,
            "Update failed; restored the previous app",
            &handoff.arguments,
        );
    }
    fs::write(
        original.file(RESULT),
        format!("Updated to {}", original.version),
    )?;
    Ok(())
}

/// Replaces the installation and returns the executable to relaunch.
fn replace(config: &UpdateConfig, host: &dyn Host, staged: &Staged) -> Result<PathBuf> {
    ensure!(
        stage::hash(&staged.payload)? == staged.sha256,
        "The staged update checksum changed"
    );
    let target = &staged.installation.executable;
    let backup = staged.file(PREVIOUS);
    match staged.installation.kind {
        Kind::MacBundle => return crate::macos::replace(config, host, staged),
        Kind::Portable => {
            ensure!(!backup.exists(), "This update was already applied");
            stage::backup_current(target, &backup).context("Cannot back up the current app")?;
            // Windows cannot rename over an existing file.
            #[cfg(windows)]
            fs::remove_file(target).context("The app is still running or cannot be replaced")?;
            if let Err(error) = fs::rename(&staged.payload, target) {
                #[cfg(windows)]
                fs::copy(&backup, target).context("Could not restore the previous app")?;
                return Err(error).context("Could not replace the app");
            }
        }
        Kind::WindowsInstaller => {
            stage::backup_current(target, &backup).context("Cannot back up the current app")?;
            let directory = target.parent().context("Missing installation directory")?;
            ensure!(
                host.run_installer(
                    &staged.payload,
                    &stage::installer_arguments(directory, &staged.file(INSTALLER_LOG)),
                )?,
                "The installer failed. See the update installer log."
            );
        }
    }
    Ok(target.clone())
}

/// Puts the previous version back, if a backup was taken.
fn restore(config: &UpdateConfig, staged: &Staged) -> Result<()> {
    if staged.installation.kind == Kind::MacBundle {
        return crate::macos::restore(config, staged);
    }
    let backup = staged.file(PREVIOUS);
    if backup.is_file() {
        fs::copy(&backup, &staged.installation.executable)
            .context("Could not restore the previous app")?;
    }
    Ok(())
}

fn restart_with_error(host: &dyn Host, executable: &Path, arguments: &[String]) -> Result<()> {
    let mut arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    arguments.push(UPDATE_ERROR_FLAG.into());
    arguments.push(RESTORED.into());
    host.spawn(executable, &arguments, None)
        .context("Could not restart the previous app")?;
    Ok(())
}

/// Tells the waiting helper that the relaunched app is up, by writing
/// `started` next to the receipt. Accepts receipts written by older helpers
/// too, with the same checks.
pub(crate) fn acknowledge(config: &UpdateConfig, host: &dyn Host, job: &Path) -> Result<()> {
    let handoff = stage::read_handoff(job)?;
    let staged = &handoff.prepared;
    staged
        .check_layout(config, job)
        .context("Invalid update receipt")?;
    ensure!(
        host.current_exe()? == staged.installation.executable.canonicalize()?,
        "The receipt belongs to a different installation"
    );
    ensure!(
        config.current_version == staged.version,
        "The updated app reports the wrong version"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged.file(STARTED))
        .context("The update was already acknowledged")?;
    file.write_all(config.current_version.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Installation;
    use crate::testing::{Behaviour, FakeHost};
    use crate::tests::ZAPFAST;

    const NEXT: UpdateConfig = UpdateConfig {
        current_version: "0.17.0",
        ..ZAPFAST
    };

    /// An installed portable app and a staged, hashed replacement.
    fn staged(root: &Path) -> Staged {
        let executable = root.join("zapfast");
        fs::write(&executable, b"old executable").unwrap();
        let installation = Installation {
            executable: executable.canonicalize().unwrap(),
            kind: Kind::Portable,
        };
        let directory = stage::staging(&ZAPFAST, &installation).unwrap();
        let payload = directory.join("zapfast");
        fs::write(&payload, b"new executable").unwrap();
        Staged {
            sha256: stage::hash(&payload).unwrap(),
            installation,
            directory,
            payload,
            version: "0.17.0".into(),
        }
    }

    fn prepared(staged: Staged) -> Prepared {
        Prepared {
            staged,
            sample: false,
        }
    }

    fn write_job(staged: &Staged) -> PathBuf {
        let job = staged.file(JOB);
        stage::create_handoff(
            &job,
            &Handoff {
                prepared: staged.clone(),
                parent: 4242,
                arguments: vec!["--verbose".into()],
            },
        )
        .unwrap();
        job
    }

    #[test]
    fn handoff_copies_itself_writes_the_job_and_waits_for_ready() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(root.path());
        let running = root.path().join("running-app");
        fs::write(&running, b"running app").unwrap();
        let host = FakeHost::default()
            .with_current_exe(&running)
            .on_spawn(Behaviour::WriteReady);
        handoff(
            &ZAPFAST,
            &host,
            prepared(staged.clone()),
            vec!["--verbose".into()],
        )
        .unwrap();
        let helper = staged.file(if cfg!(windows) {
            "helper.exe"
        } else {
            "helper"
        });
        assert_eq!(fs::read(&helper).unwrap(), b"running app");
        let spawned = host.spawned();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].executable, helper);
        assert_eq!(
            spawned[0].arguments,
            [OsString::from("--apply-update"), staged.file(JOB).into()]
        );
        assert!(spawned[0].logged, "the helper's errors go to helper.log");
        assert!(staged.file(HELPER_LOG).is_file());
        let job = stage::read_handoff(&staged.file(JOB)).unwrap();
        assert_eq!(job.prepared, staged);
        assert_eq!(job.parent, std::process::id());
        assert_eq!(job.arguments, ["--verbose"]);
    }

    #[test]
    fn handoff_reports_a_helper_that_dies_or_never_gets_ready() {
        for (behaviour, message) in [
            (Behaviour::Exit(false), "exited before it was ready"),
            (Behaviour::Hang, "did not start"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let staged = staged(root.path());
            let host = FakeHost::default()
                .with_current_exe(&staged.installation.executable)
                .on_spawn(behaviour);
            let error = handoff(&ZAPFAST, &host, prepared(staged), Vec::new()).unwrap_err();
            assert!(error.to_string().contains(message), "{error:#}");
        }
    }

    #[test]
    fn handoff_into_a_used_folder_cannot_report_success() {
        // PR #185: a stale `ready` would make handoff return before a helper
        // watches the app; a stale `started` would skip rollback.
        for marker in [READY, STARTED, RESULT, PREVIOUS, JOB, HELPER_LOG] {
            let root = tempfile::tempdir().unwrap();
            let staged = staged(root.path());
            fs::write(staged.file(marker), b"stale").unwrap();
            let host = FakeHost::default()
                .with_current_exe(&staged.installation.executable)
                .on_spawn(Behaviour::WriteReady);
            let error = handoff(&ZAPFAST, &host, prepared(staged), Vec::new()).unwrap_err();
            assert!(
                error.to_string().contains("already tried"),
                "{marker}: {error:#}"
            );
            assert!(host.spawned().is_empty(), "{marker}");
        }
    }

    #[test]
    fn handoff_refuses_a_changed_payload_and_samples() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(root.path());
        fs::write(&staged.payload, b"tampered").unwrap();
        let host = FakeHost::default().on_spawn(Behaviour::WriteReady);
        assert!(handoff(&ZAPFAST, &host, prepared(staged.clone()), Vec::new()).is_err());
        let sample = Prepared::sample(staged.installation.clone(), "0.17.0");
        assert!(handoff(&ZAPFAST, &host, sample, Vec::new()).is_err());
        assert!(host.spawned().is_empty());
    }

    #[test]
    fn the_helper_installs_relaunches_with_a_receipt_and_records_success() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(root.path());
        let job = write_job(&staged);
        let host = FakeHost::default().on_spawn(Behaviour::Acknowledge);
        run(&ZAPFAST, &host, &job).unwrap();
        assert_eq!(
            fs::read(&staged.installation.executable).unwrap(),
            b"new executable"
        );
        assert_eq!(fs::read(staged.file(PREVIOUS)).unwrap(), b"old executable");
        assert!(staged.file(READY).is_file());
        assert_eq!(
            fs::read_to_string(staged.file(RESULT)).unwrap(),
            "Updated to 0.17.0"
        );
        assert_eq!(host.waited_for(), [4242]);
        let spawned = host.spawned();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].executable, staged.installation.executable);
        assert_eq!(
            spawned[0].arguments,
            [
                OsString::from("--verbose"),
                "--update-receipt".into(),
                job.clone().into()
            ]
        );
        assert!(!spawned[0].logged);
    }

    #[test]
    fn an_app_that_never_acknowledges_is_rolled_back_and_told_why() {
        for (behaviour, message) in [
            (Behaviour::Exit(true), "exited before opening its window"),
            (Behaviour::Hang, "within one minute"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let staged = staged(root.path());
            let job = write_job(&staged);
            let host = FakeHost::default().on_spawn(behaviour);
            let error = run(&ZAPFAST, &host, &job).unwrap_err();
            assert!(error.to_string().contains(message), "{error:#}");
            assert_eq!(
                fs::read(&staged.installation.executable).unwrap(),
                b"old executable"
            );
            let result = fs::read_to_string(staged.file(RESULT)).unwrap();
            assert!(
                result.starts_with("Update failed; restored the previous app: "),
                "{result}"
            );
            let spawned = host.spawned();
            assert_eq!(spawned.len(), 2);
            assert_eq!(
                spawned[1].arguments,
                [
                    OsString::from("--verbose"),
                    "--update-error".into(),
                    RESTORED.into()
                ]
            );
            assert_eq!(spawned[1].executable, staged.installation.executable);
        }
    }

    #[test]
    fn a_failed_replacement_restarts_the_untouched_app() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(root.path());
        let job = write_job(&staged);
        // Something already sits where the backup goes... but not a marker
        // run() checks for: a partial file blocks the backup's create_new.
        fs::write(staged.file("previous.partial"), b"left over").unwrap();
        let host = FakeHost::default().on_spawn(Behaviour::Acknowledge);
        assert!(run(&ZAPFAST, &host, &job).is_err());
        assert_eq!(
            fs::read(&staged.installation.executable).unwrap(),
            b"old executable"
        );
        assert!(
            fs::read_to_string(staged.file(RESULT))
                .unwrap()
                .starts_with("Update failed: ")
        );
        let spawned = host.spawned();
        assert_eq!(spawned.len(), 1, "only the restart");
        assert!(
            spawned[0]
                .arguments
                .contains(&OsString::from("--update-error"))
        );
    }

    #[test]
    fn the_windows_installer_runs_silently_into_the_same_folder() {
        let root = tempfile::tempdir().unwrap();
        let mut staged = staged(root.path());
        staged.installation.kind = Kind::WindowsInstaller;
        let job = write_job(&staged);
        let host = FakeHost::default().on_spawn(Behaviour::Acknowledge);
        run(&ZAPFAST, &host, &job).unwrap();
        let installs = host.installers();
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].0, staged.payload);
        assert!(installs[0].1.contains(&"/VERYSILENT".to_owned()));
        assert!(installs[0].1.contains(&format!(
            "/DIR={}",
            stage::installer_path(root.path().canonicalize().unwrap().as_path())
        )));
        assert_eq!(fs::read(staged.file(PREVIOUS)).unwrap(), b"old executable");
        let host = FakeHost::default()
            .on_spawn(Behaviour::Acknowledge)
            .failing_installer();
        let root = tempfile::tempdir().unwrap();
        let mut staged = self::staged(root.path());
        staged.installation.kind = Kind::WindowsInstaller;
        let job = write_job(&staged);
        let error = run(&ZAPFAST, &host, &job).unwrap_err();
        assert!(error.to_string().contains("installer failed"), "{error:#}");
        assert_eq!(
            fs::read(&staged.installation.executable).unwrap(),
            b"old executable"
        );
    }

    #[test]
    fn the_helper_refuses_a_job_from_a_used_or_foreign_folder() {
        for marker in [READY, STARTED, RESULT, PREVIOUS] {
            let root = tempfile::tempdir().unwrap();
            let staged = staged(root.path());
            let job = write_job(&staged);
            fs::write(staged.file(marker), b"stale").unwrap();
            let host = FakeHost::default().on_spawn(Behaviour::Acknowledge);
            assert!(run(&ZAPFAST, &host, &job).is_err(), "{marker}");
            assert!(host.waited_for().is_empty(), "{marker}");
            assert_eq!(
                fs::read(&staged.installation.executable).unwrap(),
                b"old executable"
            );
        }
        // A job that points the payload outside its folder.
        let root = tempfile::tempdir().unwrap();
        let mut staged = staged(root.path());
        let outside = root.path().join("elsewhere");
        fs::write(&outside, b"new executable").unwrap();
        staged.payload = outside;
        let job = write_job(&staged);
        let host = FakeHost::default();
        assert!(run(&ZAPFAST, &host, &job).is_err());
        assert!(host.waited_for().is_empty());
    }

    /// Spotifast #538 through the helper: the download renames the bundle's
    /// executable, the receipt names the new one, and a rollback restarts
    /// the old one. Bundles are only accepted on macOS.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_renamed_bundle_executable_is_relaunched_and_rolled_back_by_its_old_name() {
        use crate::testing::bundle;
        const SPOTIFAST: UpdateConfig = UpdateConfig {
            legacy_names: &["fastpotify"],
            macos: crate::MacConfig {
                bundle_ids: &["rocks.spotifast.Spotifast"],
                executable_names: &["fastpotify", "Spotifast"],
                legacy_bundle_names: &["Fastpotify.app"],
            },
            ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.1")
        };
        for acknowledges in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let app = root.join("Spotifast.app");
            bundle(&app, "fastpotify", b"old executable", b"old metadata");
            let installation = Installation {
                executable: app.join("Contents/MacOS/fastpotify"),
                kind: Kind::MacBundle,
            };
            let directory = stage::staging(&SPOTIFAST, &installation).unwrap();
            let payload = directory.join("spotifast-v0.11.0-macos-universal.dmg");
            fs::write(&payload, b"disk image").unwrap();
            let staged = Staged {
                sha256: stage::hash(&payload).unwrap(),
                installation: installation.clone(),
                directory,
                payload: payload.clone(),
                version: "0.11.0".into(),
            };
            let job = write_job(&staged);
            let host = FakeHost::default()
                .with_image(&payload, |volume| {
                    bundle(
                        &volume.join("Spotifast.app"),
                        "Spotifast",
                        b"new executable",
                        b"new metadata",
                    );
                })
                .with_any_bundle("rocks.spotifast.Spotifast", "Spotifast", "0.11.0")
                .with_version_output("spotifast 0.11.0")
                .on_spawn(if acknowledges {
                    Behaviour::Acknowledge
                } else {
                    Behaviour::Exit(false)
                });
            let result = run(&SPOTIFAST, &host, &job);
            let spawned = host.spawned();
            if acknowledges {
                result.unwrap();
                let renamed = app.join("Contents/MacOS/Spotifast");
                assert_eq!(spawned[0].executable, renamed);
                assert_eq!(
                    stage::read_handoff(&job)
                        .unwrap()
                        .prepared
                        .installation
                        .executable,
                    renamed,
                    "the receipt names the installed executable"
                );
                assert_eq!(fs::read(&renamed).unwrap(), b"new executable");
            } else {
                assert!(result.is_err());
                assert_eq!(spawned.len(), 2);
                assert_eq!(spawned[1].executable, installation.executable);
                assert_eq!(
                    fs::read(&installation.executable).unwrap(),
                    b"old executable"
                );
                assert_eq!(
                    fs::read(staged.file("failed.app/Contents/MacOS/Spotifast")).unwrap(),
                    b"new executable"
                );
            }
        }
    }

    #[test]
    fn a_receipt_is_acknowledged_once_by_the_installed_version() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(root.path());
        let job = write_job(&staged);
        let host = FakeHost::default().with_current_exe(&staged.installation.executable);
        let error = acknowledge(&ZAPFAST, &host, &job).unwrap_err();
        assert!(error.to_string().contains("wrong version"), "{error:#}");
        acknowledge(&NEXT, &host, &job).unwrap();
        assert_eq!(fs::read_to_string(staged.file(STARTED)).unwrap(), "0.17.0");
        assert!(acknowledge(&NEXT, &host, &job).is_err(), "only once");
        let other = root.path().join("other-copy");
        fs::write(&other, b"x").unwrap();
        let host = FakeHost::default().with_current_exe(&other);
        fs::remove_file(staged.file(STARTED)).unwrap();
        let error = acknowledge(&NEXT, &host, &job).unwrap_err();
        assert!(
            error.to_string().contains("different installation"),
            "{error:#}"
        );
        assert!(!staged.file(STARTED).exists());
    }
}
