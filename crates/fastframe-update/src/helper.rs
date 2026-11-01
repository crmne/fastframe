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
use crate::{
    APPLY_UPDATE_FLAG, UPDATE_ERROR_FLAG, UPDATE_FAILURE_FLAG, UPDATE_RECEIPT_FLAG, UpdateConfig,
};

/// What a restored app is told.
pub(crate) const RESTORED: &str =
    "The update did not finish, so nothing changed. Restart to try again.";

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
        Kind::AppImage => {
            // The running executable lives inside the AppImage's mount,
            // which goes when the app quits, so the helper is a copy of
            // the AppImage itself.
            let helper = staged.file("helper");
            fs::copy(&staged.installation.executable, &helper)
                .context("Could not prepare the update helper")?;
            helper
        }
        Kind::Portable | Kind::WindowsInstaller => {
            // Windows locks a running executable, so the helper cannot be
            // the file it replaces.
            let helper = staged.file(if cfg!(windows) {
                "helper.exe"
            } else {
                "helper"
            });
            let running = host.current_exe()?;
            fs::copy(&running, &helper).context("Could not prepare the update helper")?;
            if cfg!(windows) {
                copy_libraries(&running, &staged.directory)
                    .context("Could not prepare the update helper")?;
            }
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
    // The helper runs in the staging folder, so nothing it loads or holds
    // open is a file in the app's folder that the update replaces.
    let mut child = host
        .spawn(
            &helper,
            &[APPLY_UPDATE_FLAG.into(), job.into_os_string()],
            &staged.directory,
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

/// Copies the DLLs beside the running `executable` into `directory`, beside
/// the helper's copy. Windows looks for a program's DLLs in its own folder,
/// then the system's, then the working directory, so without them the helper
/// either fails to start or loads the app's own DLLs from its working
/// directory and holds them while the installer has to replace them.
fn copy_libraries(executable: &Path, directory: &Path) -> Result<()> {
    let Some(folder) = executable.parent() else {
        return Ok(());
    };
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        let library = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"));
        if library && entry.file_type()?.is_file() {
            fs::copy(&path, directory.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Where a launched app runs: its own folder, as its shortcut starts it, and
/// never the helper's staging folder, which a process working there would
/// keep Windows from removing.
fn app_directory(executable: &Path) -> &Path {
    executable.parent().unwrap_or(executable)
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
    // `installed` is what `replace` put in place, when it got that far: an
    // update that renamed the app is undone under both names.
    let fail = |error: anyhow::Error,
                message: &str,
                arguments: &[String],
                installed: Option<&Path>|
     -> Result<()> {
        restore(config, &original, installed)?;
        let failure = format!("{message}: {error:#}");
        fs::write(original.file(RESULT), &failure)?;
        restart_with_error(host, &original.installation.executable, arguments, &failure)?;
        Err(error)
    };
    let executable = match replace(config, host, &original) {
        Ok(executable) => executable,
        Err(error) => return fail(error, "Update failed", &handoff.arguments, None),
    };
    if executable != original.installation.executable {
        // The relaunched app reads this same file as its receipt, so it has
        // to name the executable that is now installed.
        handoff.prepared.installation.executable = executable.clone();
        if let Err(error) = stage::rewrite_handoff(job, &handoff) {
            return fail(
                error,
                "Update failed",
                &handoff.arguments,
                Some(&executable),
            );
        }
    }
    let mut arguments: Vec<OsString> = handoff.arguments.iter().map(OsString::from).collect();
    arguments.push(UPDATE_RECEIPT_FLAG.into());
    arguments.push(job.as_os_str().to_owned());
    let started = original.file(STARTED);
    let timing = host.timing();
    let launch = (|| -> Result<()> {
        let mut child = host
            .spawn(&executable, &arguments, app_directory(&executable), None)
            .context("Could not launch the updated app")?;
        let start = Instant::now();
        loop {
            if started.is_file() {
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                bail!("The updated app exited before opening its window ({status})");
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
            Some(&executable),
        );
    }
    // The update is kept; the old name's launchers follow the new one.
    let platform = crate::detect::Platform::current();
    match original.installation.kind {
        Kind::Portable if executable != original.installation.executable => {
            crate::shortcuts::repoint(
                host,
                platform,
                &original.installation.executable,
                &executable,
            );
        }
        Kind::WindowsInstaller => {
            crate::rename::remove_legacy(config, host, platform, &executable);
        }
        _ => {}
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
            // A copy under a legacy name comes back under the slug.
            let installed =
                crate::rename::portable_target(config, target).unwrap_or_else(|| target.clone());
            // Windows cannot rename over an existing file.
            #[cfg(windows)]
            fs::remove_file(target).context("The app is still running or cannot be replaced")?;
            if let Err(error) = fs::rename(&staged.payload, &installed) {
                #[cfg(windows)]
                fs::copy(&backup, target).context("Could not restore the previous app")?;
                return Err(error).context("Could not replace the app");
            }
            if installed != *target {
                if let Err(error) = crate::rename::retire(target, &installed) {
                    crate::rename::undo_portable(target, &installed, &backup)
                        .context("Could not restore the previous app")?;
                    return Err(error).context("Could not rename the app");
                }
                return Ok(installed);
            }
        }
        Kind::AppImage => {
            stage::backup_current(target, &backup).context("Cannot back up the current app")?;
            // The staging folder sits beside the AppImage, on the same file
            // system, so one rename replaces it: whatever starts it
            // meanwhile gets the old file or the new one, never part of
            // either. It keeps the permissions the AppImage has now.
            let permissions = fs::metadata(target)
                .context("Cannot read the current app")?
                .permissions();
            fs::set_permissions(&staged.payload, permissions)
                .context("Could not prepare the update")?;
            fs::rename(&staged.payload, target).context("Could not replace the app")?;
        }
        Kind::WindowsInstaller => {
            stage::backup_current(target, &backup).context("Cannot back up the current app")?;
            let directory = target.parent().context("Missing installation directory")?;
            let log = staged.file(INSTALLER_LOG);
            let installed = if stage::is_msi(&staged.payload) {
                stage::msi_succeeded(host.run_msiexec(&stage::msi_command_line(
                    &staged.payload,
                    directory,
                    &log,
                ))?)
            } else {
                host.run_installer(
                    &staged.payload,
                    &stage::installer_arguments(directory, &log),
                )?
            };
            ensure!(
                installed,
                "The installer failed. See the update installer log."
            );
            // An app still running under a legacy name comes back under its
            // own, which the setup program installed beside it.
            if let Some(own) = crate::rename::own_executable(config, target) {
                return Ok(own);
            }
        }
    }
    Ok(target.clone())
}

/// Puts the previous version back, if a backup was taken. `installed` is the
/// executable `replace` returned, if it returned.
fn restore(config: &UpdateConfig, staged: &Staged, installed: Option<&Path>) -> Result<()> {
    if staged.installation.kind == Kind::MacBundle {
        return crate::macos::restore(config, staged, installed);
    }
    let backup = staged.file(PREVIOUS);
    let target = &staged.installation.executable;
    if staged.installation.kind == Kind::AppImage {
        // A rename, not a copy: the AppImage that failed may still be
        // running, and Linux refuses to write into a running executable.
        if backup.is_file() {
            fs::rename(&backup, target).context("Could not restore the previous app")?;
        }
        return Ok(());
    }
    if staged.installation.kind == Kind::Portable
        && let Some(renamed) = installed.filter(|installed| installed != target)
    {
        return crate::rename::undo_portable(target, renamed, &backup)
            .context("Could not restore the previous app");
    }
    if backup.is_file() {
        fs::copy(&backup, &staged.installation.executable)
            .context("Could not restore the previous app")?;
    }
    Ok(())
}

fn restart_with_error(
    host: &dyn Host,
    executable: &Path,
    arguments: &[String],
    failure: &str,
) -> Result<()> {
    let mut arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    arguments.push(UPDATE_ERROR_FLAG.into());
    arguments.push(RESTORED.into());
    arguments.push(UPDATE_FAILURE_FLAG.into());
    arguments.push(failure.into());
    host.spawn(executable, &arguments, app_directory(executable), None)
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
    // An AppImage runs from its mount; the installation is the file.
    let running = if staged.installation.kind == Kind::AppImage {
        PathBuf::from(
            host.var("APPIMAGE")
                .context("The updated app is not running from an AppImage")?,
        )
        .canonicalize()?
    } else {
        host.current_exe()?
    };
    ensure!(
        running == staged.installation.executable.canonicalize()?,
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
    // Earlier updates are over; this one's helper is still finishing.
    if let Some(parent) = staged.directory.parent() {
        stage::sweep(config, parent, Some(&staged.directory));
    }
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
        assert_eq!(
            spawned[0].directory, staged.directory,
            "the helper holds nothing in the app's folder"
        );
        assert!(staged.file(HELPER_LOG).is_file());
        let job = stage::read_handoff(&staged.file(JOB)).unwrap();
        assert_eq!(job.prepared, staged);
        assert_eq!(job.parent, std::process::id());
        assert_eq!(job.arguments, ["--verbose"]);
    }

    /// The helper's copy finds the app's DLLs beside it, so it neither fails
    /// to start nor loads them from the app's folder.
    #[test]
    fn the_helper_takes_copies_of_the_apps_libraries() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        let staging = root.path().join("staging");
        fs::create_dir_all(app.join("resources.dll")).unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(app.join("solco.exe"), b"app").unwrap();
        fs::write(app.join("webgpu_dawn.dll"), b"dawn").unwrap();
        fs::write(app.join("VCRUNTIME140.DLL"), b"runtime").unwrap();
        fs::write(app.join("solco-installer.txt"), b"marker").unwrap();
        copy_libraries(&app.join("solco.exe"), &staging).unwrap();
        let mut copied: Vec<_> = fs::read_dir(&staging)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        copied.sort();
        assert_eq!(copied, ["VCRUNTIME140.DLL", "webgpu_dawn.dll"]);
        assert_eq!(fs::read(staging.join("webgpu_dawn.dll")).unwrap(), b"dawn");
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
            Some(spawned[0].directory.as_path()),
            staged.installation.executable.parent(),
            "the app runs in its folder, not the staging folder"
        );
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

    /// The app is `zapfast-gui` beside a `zapfast` command-line tool: the
    /// helper installs and rolls back the app only, and the tool is left
    /// alone either way.
    #[test]
    fn a_named_portable_executable_is_installed_and_rolled_back_alone() {
        let config = UpdateConfig {
            portable_executable: Some("zapfast-gui"),
            ..ZAPFAST
        };
        for acknowledged in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let tool = root.path().join("zapfast");
            fs::write(&tool, b"old tool").unwrap();
            let executable = root.path().join("zapfast-gui");
            fs::write(&executable, b"old executable").unwrap();
            let installation = Installation {
                executable: executable.canonicalize().unwrap(),
                kind: Kind::Portable,
            };
            let directory = stage::staging(&config, &installation).unwrap();
            let payload = directory.join("zapfast-gui");
            fs::write(&payload, b"new executable").unwrap();
            let staged = Staged {
                sha256: stage::hash(&payload).unwrap(),
                installation,
                directory,
                payload,
                version: "0.17.0".into(),
            };
            let job = write_job(&staged);
            let host = FakeHost::default().on_spawn(if acknowledged {
                Behaviour::Acknowledge
            } else {
                Behaviour::Exit(true)
            });
            let result = run(&config, &host, &job);
            assert_eq!(result.is_ok(), acknowledged);
            assert_eq!(
                fs::read(&executable).unwrap(),
                if acknowledged {
                    &b"new executable"[..]
                } else {
                    &b"old executable"[..]
                }
            );
            assert_eq!(fs::read(&tool).unwrap(), b"old tool");
            for spawned in host.spawned() {
                assert_eq!(spawned.executable, staged.installation.executable);
            }
        }
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
                    RESTORED.into(),
                    "--update-failure".into(),
                    result.into()
                ]
            );
            assert_eq!(spawned[1].executable, staged.installation.executable);
            assert_eq!(
                Some(spawned[1].directory.as_path()),
                staged.installation.executable.parent()
            );
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

    /// A release with a Windows Installer package: `msiexec` installs it
    /// silently into the same folder, and a failed install, or a start that
    /// never comes, restores the previous app.
    #[test]
    fn a_windows_installer_package_runs_through_msiexec() {
        fn msi(root: &Path) -> Staged {
            let mut staged = staged(root);
            staged.installation.kind = Kind::WindowsInstaller;
            let payload = staged.file("zapfast-v0.17.0-x86_64-pc-windows-msvc.msi");
            fs::rename(&staged.payload, &payload).unwrap();
            staged.payload = payload;
            staged
        }
        for code in [Some(0), Some(3010)] {
            let root = tempfile::tempdir().unwrap();
            let staged = msi(root.path());
            let job = write_job(&staged);
            let host = FakeHost::default()
                .on_spawn(Behaviour::Acknowledge)
                .msiexec_exits_with(code);
            run(&ZAPFAST, &host, &job).unwrap();
            assert!(host.installers().is_empty(), "no setup program");
            let directory = root.path().canonicalize().unwrap();
            assert_eq!(
                host.msiexec_runs(),
                [stage::msi_command_line(
                    &staged.payload,
                    &directory,
                    &staged.file(INSTALLER_LOG)
                )]
            );
            assert_eq!(fs::read(staged.file(PREVIOUS)).unwrap(), b"old executable");
            assert_eq!(
                fs::read_to_string(staged.file(RESULT)).unwrap(),
                "Updated to 0.17.0"
            );
        }
        for (code, behaviour) in [
            (Some(1603), Behaviour::Acknowledge),
            (None, Behaviour::Acknowledge),
            (Some(0), Behaviour::Exit(false)),
        ] {
            let root = tempfile::tempdir().unwrap();
            let staged = msi(root.path());
            let job = write_job(&staged);
            let host = FakeHost::default()
                .on_spawn(behaviour)
                .msiexec_exits_with(code);
            assert!(run(&ZAPFAST, &host, &job).is_err(), "{code:?}");
            assert_eq!(
                fs::read(&staged.installation.executable).unwrap(),
                b"old executable"
            );
            let spawned = host.spawned();
            assert!(
                spawned
                    .last()
                    .unwrap()
                    .arguments
                    .contains(&OsString::from("--update-error"))
            );
        }
    }

    /// Spotifast #582: an installation still running as `fastpotify.exe`
    /// comes back as the `spotifast.exe` its setup program installed, and
    /// the old file goes once that start succeeds. A failed start restarts
    /// the old name, which still works.
    #[test]
    fn an_installer_update_moves_a_legacy_name_onto_the_slug() {
        const SPOTIFAST: UpdateConfig = UpdateConfig {
            legacy_names: &["fastpotify"],
            ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.3")
        };
        for acknowledges in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let legacy = root.join("fastpotify.exe");
            fs::write(&legacy, b"old executable").unwrap();
            // What the setup program leaves beside it.
            let own = root.join("spotifast.exe");
            fs::write(&own, b"new executable").unwrap();
            let installation = Installation {
                executable: legacy.clone(),
                kind: Kind::WindowsInstaller,
            };
            let directory = stage::staging(&SPOTIFAST, &installation).unwrap();
            let payload = directory.join("spotifast-v0.10.4-x86_64-pc-windows-msvc-setup.exe");
            fs::write(&payload, b"setup").unwrap();
            let staged = Staged {
                sha256: stage::hash(&payload).unwrap(),
                installation,
                directory,
                payload,
                version: "0.10.4".into(),
            };
            let job = write_job(&staged);
            let host = FakeHost::default().on_spawn(if acknowledges {
                Behaviour::Acknowledge
            } else {
                Behaviour::Exit(false)
            });
            let result = run(&SPOTIFAST, &host, &job);
            let spawned = host.spawned();
            assert_eq!(spawned[0].executable, own);
            if acknowledges {
                result.unwrap();
                assert_eq!(
                    stage::read_handoff(&job)
                        .unwrap()
                        .prepared
                        .installation
                        .executable,
                    own,
                    "the receipt names the executable that starts"
                );
                assert!(!legacy.exists(), "the old name is gone");
                assert!(own.is_file());
            } else {
                assert!(result.is_err());
                assert_eq!(spawned.len(), 2);
                assert_eq!(spawned[1].executable, legacy);
                assert_eq!(fs::read(&legacy).unwrap(), b"old executable");
            }
        }
    }

    /// A portable copy still called `fastpotify` updates to `spotifast`. On
    /// Unix the old name stays as a link for scripts; a failed start puts
    /// the old file back under the old name and removes the new one.
    #[test]
    fn a_portable_copy_moves_off_a_legacy_name() {
        const SPOTIFAST: UpdateConfig = UpdateConfig {
            legacy_names: &["fastpotify"],
            ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.3")
        };
        let extension = std::env::consts::EXE_SUFFIX;
        for acknowledges in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let legacy = root.join(format!("fastpotify{extension}"));
            fs::write(&legacy, b"old executable").unwrap();
            let installation = Installation {
                executable: legacy.clone(),
                kind: Kind::Portable,
            };
            let directory = stage::staging(&SPOTIFAST, &installation).unwrap();
            let payload = directory.join(format!("spotifast{extension}"));
            fs::write(&payload, b"new executable").unwrap();
            let staged = Staged {
                sha256: stage::hash(&payload).unwrap(),
                installation,
                directory,
                payload,
                version: "0.10.4".into(),
            };
            let job = write_job(&staged);
            let host = FakeHost::default().on_spawn(if acknowledges {
                Behaviour::Acknowledge
            } else {
                Behaviour::Exit(false)
            });
            let result = run(&SPOTIFAST, &host, &job);
            let own = root.join(format!("spotifast{extension}"));
            let spawned = host.spawned();
            assert_eq!(spawned[0].executable, own);
            if acknowledges {
                result.unwrap();
                assert_eq!(fs::read(&own).unwrap(), b"new executable");
                assert_eq!(
                    stage::read_handoff(&job)
                        .unwrap()
                        .prepared
                        .installation
                        .executable,
                    own
                );
                if cfg!(unix) {
                    assert_eq!(fs::read(&legacy).unwrap(), b"new executable", "a link");
                } else {
                    assert!(!legacy.exists());
                }
            } else {
                assert!(result.is_err());
                assert_eq!(spawned[1].executable, legacy);
                assert!(fs::symlink_metadata(&own).is_err());
                assert!(!fs::symlink_metadata(&legacy).unwrap().is_symlink());
                assert_eq!(fs::read(&legacy).unwrap(), b"old executable");
            }
        }
    }

    /// A portable copy keeps its settings in `<slug>-data` beside it (see
    /// `fastframe-dirs`). Installing, rolling back, moving off a legacy name
    /// and sweeping the staging folders all leave that folder as it was.
    #[test]
    fn a_portable_update_keeps_the_data_folder_beside_it() {
        const SPOTIFAST: UpdateConfig = UpdateConfig {
            legacy_names: &["fastpotify"],
            ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.3")
        };
        let extension = std::env::consts::EXE_SUFFIX;
        for (name, acknowledges) in [
            ("spotifast", true),
            ("spotifast", false),
            ("fastpotify", true),
            ("fastpotify", false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let data = root.join("spotifast-data");
            fs::create_dir_all(data.join("state")).unwrap();
            fs::write(data.join("state").join("session.json"), b"signed in").unwrap();
            fs::write(
                root.join("spotifast-portable.txt"),
                b"spotifast-portable-v1",
            )
            .unwrap();
            let executable = root.join(format!("{name}{extension}"));
            fs::write(&executable, b"old executable").unwrap();
            let installation = Installation {
                executable: executable.clone(),
                kind: Kind::Portable,
            };
            let directory = stage::staging(&SPOTIFAST, &installation).unwrap();
            let payload = directory.join(format!("spotifast{extension}"));
            fs::write(&payload, b"new executable").unwrap();
            let staged = Staged {
                sha256: stage::hash(&payload).unwrap(),
                installation,
                directory,
                payload,
                version: "0.10.4".into(),
            };
            let job = write_job(&staged);
            let host = FakeHost::default().on_spawn(if acknowledges {
                Behaviour::Acknowledge
            } else {
                Behaviour::Exit(false)
            });
            assert_eq!(run(&SPOTIFAST, &host, &job).is_ok(), acknowledges);
            stage::sweep(&SPOTIFAST, &root, None);
            assert!(!staged.directory.exists(), "{name} {acknowledges}");
            assert_eq!(
                fs::read(data.join("state").join("session.json")).unwrap(),
                b"signed in",
                "{name} {acknowledges}"
            );
            assert_eq!(
                fs::read(root.join("spotifast-portable.txt")).unwrap(),
                b"spotifast-portable-v1"
            );
        }
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

    /// A bundle still called `Fastpotify.app` comes back as `Spotifast.app`,
    /// and a failed start puts `Fastpotify.app` back.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_legacy_bundle_name_is_renamed_and_rolled_back() {
        use crate::testing::bundle;
        const SPOTIFAST: UpdateConfig = UpdateConfig {
            legacy_names: &["fastpotify"],
            macos: crate::MacConfig {
                bundle_ids: &["rocks.spotifast.Spotifast"],
                executable_names: &["fastpotify", "Spotifast"],
                legacy_bundle_names: &["Fastpotify.app"],
            },
            ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.3")
        };
        for acknowledges in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let legacy = root.join("Fastpotify.app");
            bundle(&legacy, "Spotifast", b"old executable", b"old metadata");
            let installation = Installation {
                executable: legacy.join("Contents/MacOS/Spotifast"),
                kind: Kind::MacBundle,
            };
            let directory = stage::staging(&SPOTIFAST, &installation).unwrap();
            let payload = directory.join("spotifast-v0.10.4-macos-universal.dmg");
            fs::write(&payload, b"disk image").unwrap();
            let staged = Staged {
                sha256: stage::hash(&payload).unwrap(),
                installation: installation.clone(),
                directory,
                payload: payload.clone(),
                version: "0.10.4".into(),
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
                .with_any_bundle("rocks.spotifast.Spotifast", "Spotifast", "0.10.4")
                .with_version_output("spotifast 0.10.4")
                .on_spawn(if acknowledges {
                    Behaviour::Acknowledge
                } else {
                    Behaviour::Exit(false)
                });
            let result = run(&SPOTIFAST, &host, &job);
            let spawned = host.spawned();
            let renamed = root.join("Spotifast.app/Contents/MacOS/Spotifast");
            assert_eq!(spawned[0].executable, renamed);
            if acknowledges {
                result.unwrap();
                assert_eq!(fs::read(&renamed).unwrap(), b"new executable");
                assert!(!legacy.exists(), "the old bundle name is gone");
            } else {
                assert!(result.is_err());
                assert_eq!(spawned[1].executable, installation.executable);
                assert_eq!(
                    fs::read(&installation.executable).unwrap(),
                    b"old executable"
                );
                assert!(!root.join("Spotifast.app").exists());
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
        let earlier = staged
            .directory
            .with_file_name(".zapfast-update-00000000000000ee");
        fs::create_dir(&earlier).unwrap();
        fs::write(earlier.join(RESULT), b"Updated to 0.16.0").unwrap();
        acknowledge(&NEXT, &host, &job).unwrap();
        assert_eq!(fs::read_to_string(staged.file(STARTED)).unwrap(), "0.17.0");
        assert!(!earlier.exists(), "an earlier update's folder is swept");
        assert!(
            staged.directory.is_dir(),
            "this update's helper still works there"
        );
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

    /// An installed AppImage and a staged, hashed replacement. The app runs
    /// from inside the AppImage's mount, so the running executable is
    /// another file.
    fn staged_appimage(root: &Path) -> Staged {
        let appimage = root.join("Spotifast.AppImage");
        fs::write(&appimage, b"old appimage").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&appimage, fs::Permissions::from_mode(0o750)).unwrap();
        }
        let installation = Installation {
            executable: appimage.canonicalize().unwrap(),
            kind: Kind::AppImage,
        };
        let directory = stage::staging(&ZAPFAST, &installation).unwrap();
        let payload = directory.join("zapfast-0.17.0-x86_64.AppImage");
        fs::write(&payload, b"new appimage").unwrap();
        Staged {
            sha256: stage::hash(&payload).unwrap(),
            installation,
            directory,
            payload,
            version: "0.17.0".into(),
        }
    }

    #[test]
    fn an_appimages_helper_is_a_copy_of_the_appimage() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged_appimage(root.path());
        let mounted = root.path().join(".mount_spotif/usr/bin/spotifast");
        fs::create_dir_all(mounted.parent().unwrap()).unwrap();
        fs::write(&mounted, b"the program inside").unwrap();
        let host = FakeHost::default()
            .with_current_exe(&mounted)
            .on_spawn(Behaviour::WriteReady);
        handoff(&ZAPFAST, &host, prepared(staged.clone()), Vec::new()).unwrap();
        let helper = staged.file("helper");
        assert_eq!(fs::read(&helper).unwrap(), b"old appimage");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&helper).unwrap().permissions().mode() & 0o777,
                0o750,
                "the helper runs"
            );
        }
        assert_eq!(host.spawned()[0].executable, helper);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_is_renamed_into_place_and_rolled_back_by_rename() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        for acknowledges in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let staged = staged_appimage(root.path());
            let target = &staged.installation.executable;
            let inode = fs::metadata(&staged.payload).unwrap().ino();
            let job = write_job(&staged);
            let host = FakeHost::default().on_spawn(if acknowledges {
                Behaviour::Acknowledge
            } else {
                Behaviour::Exit(false)
            });
            let result = run(&ZAPFAST, &host, &job);
            let spawned = host.spawned();
            assert_eq!(spawned[0].executable, *target, "the AppImage relaunches");
            assert_eq!(Some(spawned[0].directory.as_path()), target.parent());
            if acknowledges {
                result.unwrap();
                assert_eq!(fs::read(target).unwrap(), b"new appimage");
                assert_eq!(
                    fs::metadata(target).unwrap().ino(),
                    inode,
                    "the download itself, renamed over the AppImage"
                );
                assert_eq!(
                    fs::metadata(target).unwrap().permissions().mode() & 0o777,
                    0o750
                );
                assert_eq!(fs::read(staged.file(PREVIOUS)).unwrap(), b"old appimage");
                assert_eq!(
                    fs::read_to_string(staged.file(RESULT)).unwrap(),
                    "Updated to 0.17.0"
                );
            } else {
                assert!(result.is_err());
                assert_eq!(fs::read(target).unwrap(), b"old appimage");
                assert_eq!(
                    fs::metadata(target).unwrap().permissions().mode() & 0o777,
                    0o750
                );
                assert_eq!(spawned.len(), 2);
                assert_eq!(spawned[1].executable, *target);
                assert!(
                    spawned[1]
                        .arguments
                        .contains(&OsString::from("--update-error"))
                );
            }
            // Nothing but the AppImage and its staging folder.
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
        }
    }

    /// The relaunched app runs from the AppImage's mount: the receipt is
    /// for it when `$APPIMAGE` names the installed file.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_receipt_is_matched_by_its_file() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged_appimage(root.path());
        let job = write_job(&staged);
        let mounted = root.path().join("inside");
        fs::write(&mounted, b"x").unwrap();
        let without = FakeHost::default().with_current_exe(&mounted);
        assert!(acknowledge(&NEXT, &without, &job).is_err());
        let other = root.path().join("Other.AppImage");
        fs::write(&other, b"x").unwrap();
        let elsewhere = FakeHost::default()
            .with_current_exe(&mounted)
            .with_var("APPIMAGE", other.as_os_str());
        let error = acknowledge(&NEXT, &elsewhere, &job).unwrap_err();
        assert!(
            error.to_string().contains("different installation"),
            "{error:#}"
        );
        let host = FakeHost::default()
            .with_current_exe(&mounted)
            .with_var("APPIMAGE", staged.installation.executable.as_os_str());
        acknowledge(&NEXT, &host, &job).unwrap();
        assert_eq!(fs::read_to_string(staged.file(STARTED)).unwrap(), "0.17.0");
    }
}
