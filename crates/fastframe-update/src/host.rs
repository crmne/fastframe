//! Everything that talks to the operating system beyond reading and writing
//! files: environment variables, other programs, processes. Tests replace it
//! with a fake, so the update logic runs on every platform without spawning
//! anything.

use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

/// A process the updater started.
pub(crate) trait Child: Send {
    /// `Some(success)` once it has exited.
    fn try_wait(&mut self) -> Result<Option<bool>>;
    /// Stops it and reaps it.
    fn kill(&mut self);
}

/// How long the helper waits for each step, and how often it looks.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    /// For the helper to report that it is watching the app.
    pub(crate) helper_ready: Duration,
    /// For the app to exit after the handoff.
    pub(crate) parent_exit: Duration,
    /// For the relaunched app to acknowledge its start.
    pub(crate) app_start: Duration,
    /// For a downloaded executable to answer `--version`.
    pub(crate) version_probe: Duration,
    /// Between looks.
    pub(crate) poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            helper_ready: Duration::from_secs(10),
            parent_exit: Duration::from_secs(60),
            app_start: Duration::from_secs(60),
            version_probe: Duration::from_secs(10),
            poll: Duration::from_millis(50),
        }
    }
}

pub(crate) trait Host: Send + Sync {
    fn timing(&self) -> Timing {
        Timing::default()
    }
    /// The running executable, canonicalized.
    fn current_exe(&self) -> std::io::Result<PathBuf>;
    fn var(&self, key: &str) -> Option<OsString>;
    /// Whether `program flag path` succeeds: the package manager owns `path`.
    fn package_owner(&self, program: &str, flag: &str, path: &Path) -> bool;
    /// Writes the archive member `entry` to the new file `destination`.
    fn extract(&self, archive: &Path, entry: &str, destination: &Path) -> Result<()>;
    /// What `executable --version` printed, trimmed.
    fn version_output(&self, executable: &Path) -> Result<String>;
    /// Starts `executable` with `arguments`. With a `log`, standard input and
    /// output are closed and standard error goes to the log; without one the
    /// child inherits them.
    fn spawn(
        &self,
        executable: &Path,
        arguments: &[OsString],
        log: Option<File>,
    ) -> Result<Box<dyn Child>>;
    /// Writes `ready` once process `parent` is being watched, then returns
    /// when it has exited.
    fn wait_for_parent(&self, parent: u32, ready: &Path) -> Result<()>;
    /// Runs an installer to completion; whether it succeeded.
    fn run_installer(&self, installer: &Path, arguments: &[String]) -> Result<bool>;
    /// Windows: points the `.lnk` shortcuts in `folders` and below whose
    /// target is `old` at `new`.
    fn repoint_windows_shortcuts(&self, folders: &[PathBuf], old: &Path, new: &Path) -> Result<()>;

    // macOS tools.
    /// A key of the bundle's Info.plist.
    fn plist(&self, bundle: &Path, key: &str) -> Result<String>;
    /// Verifies the bundle's code signature and returns its team, if signed
    /// by one.
    fn signing_team(&self, bundle: &Path) -> Result<Option<String>>;
    /// Whether Gatekeeper would let the bundle run.
    fn assess(&self, bundle: &Path) -> Result<bool>;
    /// Mounts a disk image read-only at `mountpoint`.
    fn attach(&self, image: &Path, mountpoint: &Path) -> Result<()>;
    /// Unmounts; whether that worked.
    fn detach(&self, mountpoint: &Path) -> bool;
    /// Copies a bundle with its signature, extended attributes and links.
    fn copy_bundle(&self, source: &Path, destination: &Path) -> Result<()>;
}

/// The real operating system.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OsHost;

mod os {
    use std::ffi::OsString;
    use std::fs::{self, File, OpenOptions};
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::Instant;

    use anyhow::{Context, Result, bail, ensure};

    use super::{Child, Host, OsHost};
    use crate::stage::LIMIT;

    /// Keeps a console window from flashing up on Windows.
    fn hidden(command: &mut Command) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(not(windows))]
        let _ = command;
    }

    struct Process(std::process::Child);

    impl Child for Process {
        fn try_wait(&mut self) -> Result<Option<bool>> {
            Ok(self.0.try_wait()?.map(|status| status.success()))
        }

        fn kill(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    impl Host for OsHost {
        fn current_exe(&self) -> std::io::Result<PathBuf> {
            std::env::current_exe()?.canonicalize()
        }

        fn var(&self, key: &str) -> Option<OsString> {
            std::env::var_os(key)
        }

        fn package_owner(&self, program: &str, flag: &str, path: &Path) -> bool {
            let mut command = Command::new(program);
            command
                .arg(flag)
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            hidden(&mut command);
            command.status().is_ok_and(|status| status.success())
        }

        fn extract(&self, archive: &Path, entry: &str, destination: &Path) -> Result<()> {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)?;
            let mut command = Command::new("tar");
            command
                .arg("-xOf")
                .arg(archive)
                .arg(entry)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            hidden(&mut command);
            let mut child = command
                .spawn()
                .context("Cannot run tar to unpack the update")?;
            let result = (|| -> Result<()> {
                let mut stdout = child
                    .stdout
                    .take()
                    .context("Missing archive stream")?
                    .take(LIMIT + 1);
                let mut file = file;
                let count = std::io::copy(&mut stdout, &mut file)?;
                ensure!(
                    count > 0 && count <= LIMIT,
                    "The update executable has an invalid size"
                );
                ensure!(
                    child.wait()?.success(),
                    "Cannot unpack the update executable"
                );
                file.sync_all()?;
                Ok(())
            })();
            if result.is_err() {
                let _ = child.kill();
                let _ = child.wait();
            }
            result
        }

        fn version_output(&self, executable: &Path) -> Result<String> {
            let mut command = Command::new(executable);
            command
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            hidden(&mut command);
            let mut child = command
                .spawn()
                .context("The downloaded app cannot run on this computer")?;
            let timing = self.timing();
            let start = Instant::now();
            loop {
                if let Some(status) = child.try_wait()? {
                    ensure!(
                        status.success(),
                        "The downloaded app failed its startup check"
                    );
                    let mut version = String::new();
                    child
                        .stdout
                        .take()
                        .context("Missing version output")?
                        .take(4096)
                        .read_to_string(&mut version)?;
                    return Ok(version.trim().to_owned());
                }
                if start.elapsed() >= timing.version_probe {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("The downloaded app did not answer its startup check");
                }
                std::thread::sleep(timing.poll);
            }
        }

        fn spawn(
            &self,
            executable: &Path,
            arguments: &[OsString],
            log: Option<File>,
        ) -> Result<Box<dyn Child>> {
            let mut command = Command::new(executable);
            command.args(arguments);
            if let Some(log) = log {
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(log);
            }
            hidden(&mut command);
            Ok(Box::new(Process(command.spawn()?)))
        }

        fn wait_for_parent(&self, parent: u32, ready: &Path) -> Result<()> {
            wait_for_parent(parent, ready, self.timing())
        }

        fn run_installer(&self, installer: &Path, arguments: &[String]) -> Result<bool> {
            let mut command = Command::new(installer);
            command.args(arguments);
            hidden(&mut command);
            Ok(command.status()?.success())
        }

        fn repoint_windows_shortcuts(
            &self,
            folders: &[PathBuf],
            old: &Path,
            new: &Path,
        ) -> Result<()> {
            ensure!(cfg!(windows), "Shortcuts are only repointed on Windows");
            // Shortcuts are COM objects; Windows PowerShell ships with every
            // supported Windows and reaches them without another crate.
            let mut folder_list = OsString::new();
            for folder in folders {
                folder_list.push(folder);
                folder_list.push("\n");
            }
            let mut command = Command::new("powershell.exe");
            command
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    crate::shortcuts::WINDOWS_SCRIPT,
                ])
                .env("FASTFRAME_OLD", old)
                .env("FASTFRAME_NEW", new)
                .env("FASTFRAME_FOLDERS", folder_list)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            hidden(&mut command);
            ensure!(
                command.status()?.success(),
                "Could not repoint the shortcuts"
            );
            Ok(())
        }

        fn plist(&self, bundle: &Path, key: &str) -> Result<String> {
            let output = Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", &format!("Print :{key}")])
                .arg(bundle.join("Contents/Info.plist"))
                .output()?;
            ensure!(output.status.success(), "The app bundle is missing {key}");
            Ok(String::from_utf8(output.stdout)?.trim().to_owned())
        }

        fn signing_team(&self, bundle: &Path) -> Result<Option<String>> {
            let verify = Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(bundle)
                .output()?;
            ensure!(
                verify.status.success(),
                "The app signature could not be verified"
            );
            let output = Command::new("/usr/bin/codesign")
                .args(["--display", "--verbose=4"])
                .arg(bundle)
                .output()?;
            ensure!(
                output.status.success(),
                "Cannot read the app signing identity"
            );
            Ok(crate::macos::team_identifier(&String::from_utf8(
                output.stderr,
            )?))
        }

        fn assess(&self, bundle: &Path) -> Result<bool> {
            Ok(Command::new("/usr/sbin/spctl")
                .args(["--assess", "--type", "execute"])
                .arg(bundle)
                .output()?
                .status
                .success())
        }

        fn attach(&self, image: &Path, mountpoint: &Path) -> Result<()> {
            let output = Command::new("/usr/bin/hdiutil")
                .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
                .arg(mountpoint)
                .arg(image)
                .output()?;
            ensure!(
                output.status.success(),
                "macOS could not open the downloaded disk image"
            );
            Ok(())
        }

        fn detach(&self, mountpoint: &Path) -> bool {
            Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(mountpoint)
                .output()
                .is_ok_and(|output| output.status.success())
        }

        fn copy_bundle(&self, source: &Path, destination: &Path) -> Result<()> {
            let copy = Command::new("/usr/bin/ditto")
                .arg(source)
                .arg(destination)
                .output()?;
            ensure!(copy.status.success(), "Could not copy the app bundle");
            Ok(())
        }
    }

    #[cfg(windows)]
    #[allow(
        unsafe_code,
        reason = "Win32 process handles have no safe wrapper in windows-sys"
    )]
    fn wait_for_parent(parent: u32, ready: &Path, timing: super::Timing) -> Result<()> {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        // SAFETY: OpenProcess takes plain values and returns a handle or null.
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent) };
        ensure!(!process.is_null(), "Cannot watch the running app");
        let result = fs::write(ready, b"ready");
        if result.is_err() {
            // SAFETY: `process` is a handle this function opened and owns.
            unsafe {
                CloseHandle(process);
            }
        }
        result?;
        let milliseconds = u32::try_from(timing.parent_exit.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: `process` is open; the wait does not take ownership.
        let outcome = unsafe { WaitForSingleObject(process, milliseconds) };
        // SAFETY: closed exactly once, after the wait.
        unsafe {
            CloseHandle(process);
        }
        ensure!(
            outcome == WAIT_OBJECT_0,
            "The app did not close within one minute"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn wait_for_parent(parent: u32, ready: &Path, timing: super::Timing) -> Result<()> {
        let process = PathBuf::from(format!("/proc/{parent}/stat"));
        let original = fs::read_to_string(&process).context("Cannot watch the running app")?;
        let identity =
            crate::host::process_identity(&original).context("Cannot identify the running app")?;
        fs::write(ready, b"ready")?;
        let start = Instant::now();
        loop {
            let current = match fs::read_to_string(&process) {
                Ok(current) => current,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(error).context("Cannot watch the running app"),
            };
            if crate::host::process_identity(&current) != Some(identity)
                || crate::host::is_zombie(&current)
            {
                break;
            }
            ensure!(
                start.elapsed() < timing.parent_exit,
                "The app did not close within one minute"
            );
            std::thread::sleep(timing.poll);
        }
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn wait_for_parent(parent: u32, ready: &Path, timing: super::Timing) -> Result<()> {
        fs::write(ready, b"ready")?;
        let start = Instant::now();
        while Command::new("/bin/kill")
            .args(["-0", &parent.to_string()])
            .stderr(Stdio::null())
            .status()?
            .success()
        {
            ensure!(
                start.elapsed() < timing.parent_exit,
                "The app did not close within one minute"
            );
            std::thread::sleep(timing.poll);
        }
        Ok(())
    }
}

/// The start time field of `/proc/<pid>/stat`, which tells a process from a
/// later one that reused its id. The name field may hold spaces and
/// parentheses, so fields are counted from the last `)`.
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "Linux reads /proc")
)]
pub(crate) fn process_identity(stat: &str) -> Option<&str> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)
}

#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "Linux reads /proc")
)]
pub(crate) fn is_zombie(stat: &str) -> bool {
    stat.rsplit_once(')')
        .is_some_and(|(_, fields)| fields.trim_start().starts_with('Z'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_identity_handles_spaces_and_parentheses_in_names() {
        let fields = "S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 98765 20";
        assert_eq!(
            process_identity(&format!("42 (app (test)) {fields}")),
            Some("98765")
        );
        assert_eq!(process_identity("42 (app) S"), None);
        assert!(!is_zombie(&format!("42 (app (Z)) {fields}")));
        assert!(is_zombie("42 (app) Z 1 2"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_has_an_identity() {
        let current =
            std::fs::read_to_string(format!("/proc/{}/stat", std::process::id())).unwrap();
        assert!(process_identity(&current).unwrap().parse::<u64>().is_ok());
    }
}
