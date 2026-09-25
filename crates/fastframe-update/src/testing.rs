//! Fakes for the network and the operating system.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use ring::signature::{Ed25519KeyPair, KeyPair};

use crate::host::{Child, Host, Timing};
use crate::transport::{Request, Response, Transport};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A test-only key, never trusted by a real configuration.
pub(crate) fn fixture_key() -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(&[42; 32]).unwrap()
}

pub(crate) fn fixture_key_hex() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| crate::hex(fixture_key().public_key().as_ref()))
}

enum Route {
    Serve(Vec<u8>),
    Redirect(String),
}

/// Answers from a table; anything else is a 404.
#[derive(Default)]
pub(crate) struct FakeTransport {
    routes: HashMap<String, Route>,
    log: Mutex<Vec<(String, String, String)>>,
}

impl FakeTransport {
    pub(crate) fn serve(mut self, url: &str, body: &[u8]) -> Self {
        self.routes
            .insert(url.to_owned(), Route::Serve(body.to_vec()));
        self
    }

    pub(crate) fn redirect(mut self, url: &str, location: &str) -> Self {
        self.routes
            .insert(url.to_owned(), Route::Redirect(location.to_owned()));
        self
    }

    pub(crate) fn requested(&self) -> Vec<String> {
        lock(&self.log)
            .iter()
            .map(|entry| entry.0.clone())
            .collect()
    }

    pub(crate) fn accepts(&self) -> Vec<String> {
        lock(&self.log)
            .iter()
            .map(|entry| entry.1.clone())
            .collect()
    }

    pub(crate) fn user_agents(&self) -> Vec<String> {
        lock(&self.log)
            .iter()
            .map(|entry| entry.2.clone())
            .collect()
    }
}

impl Transport for FakeTransport {
    fn get(&self, request: &Request<'_>) -> Result<Response> {
        lock(&self.log).push((
            request.url.to_owned(),
            request.accept.to_owned(),
            request.user_agent.to_owned(),
        ));
        Ok(match self.routes.get(request.url) {
            Some(Route::Serve(body)) => Response::ok(std::io::Cursor::new(body.clone())),
            Some(Route::Redirect(location)) => Response::redirect(location.clone()),
            None => Response {
                status: 404,
                location: None,
                body: Box::new(std::io::empty()),
            },
        })
    }
}

/// What a spawned process does.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Behaviour {
    /// A helper: writes `ready` next to its job and keeps running.
    WriteReady,
    /// A relaunched app: acknowledges its receipt and keeps running.
    Acknowledge,
    /// Exits at once, successfully or not.
    Exit(bool),
    /// Runs and never does anything.
    Hang,
}

struct FakeChild(Behaviour);

impl Child for FakeChild {
    fn try_wait(&mut self) -> Result<Option<bool>> {
        Ok(match self.0 {
            Behaviour::Exit(success) => Some(success),
            _ => None,
        })
    }

    fn kill(&mut self) {}
}

#[derive(Clone, Debug)]
pub(crate) struct Spawned {
    pub(crate) executable: PathBuf,
    pub(crate) arguments: Vec<OsString>,
    pub(crate) logged: bool,
}

#[derive(Clone)]
struct BundleInfo {
    id: String,
    executable: String,
    version: String,
}

type Volume = Box<dyn Fn(&Path) + Send + Sync>;

/// The operating system, as the tests want it.
pub(crate) struct FakeHost {
    current_exe: Option<PathBuf>,
    vars: HashMap<String, OsString>,
    owners: Vec<String>,
    queries: Mutex<Vec<String>>,
    entries: HashMap<String, Vec<u8>>,
    version: Option<String>,
    probes: Mutex<Vec<PathBuf>>,
    behaviour: Behaviour,
    spawned: Mutex<Vec<Spawned>>,
    waited: Mutex<Vec<u32>>,
    installer_fails: bool,
    installers: Mutex<Vec<(PathBuf, Vec<String>)>>,
    bundles: HashMap<PathBuf, BundleInfo>,
    any_bundle: Option<BundleInfo>,
    teams: HashMap<PathBuf, String>,
    default_team: Option<String>,
    images: HashMap<PathBuf, Volume>,
    detach_fails: bool,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self {
            current_exe: None,
            vars: HashMap::new(),
            owners: Vec::new(),
            queries: Mutex::default(),
            entries: HashMap::new(),
            version: None,
            probes: Mutex::default(),
            behaviour: Behaviour::Hang,
            spawned: Mutex::default(),
            waited: Mutex::default(),
            installer_fails: false,
            installers: Mutex::default(),
            bundles: HashMap::new(),
            any_bundle: None,
            teams: HashMap::new(),
            default_team: None,
            images: HashMap::new(),
            detach_fails: false,
        }
    }
}

impl FakeHost {
    pub(crate) fn with_current_exe(mut self, path: &Path) -> Self {
        self.current_exe = Some(path.canonicalize().unwrap_or_else(|_| path.to_owned()));
        self
    }

    pub(crate) fn with_var(mut self, key: &str, value: impl Into<OsString>) -> Self {
        self.vars.insert(key.to_owned(), value.into());
        self
    }

    pub(crate) fn owned_by(mut self, program: &str) -> Self {
        self.owners.push(program.to_owned());
        self
    }

    pub(crate) fn package_queries(&self) -> Vec<String> {
        lock(&self.queries).clone()
    }

    pub(crate) fn with_archive_entry(mut self, entry: &str, contents: &[u8]) -> Self {
        self.entries.insert(entry.to_owned(), contents.to_vec());
        self
    }

    pub(crate) fn with_version_output(mut self, output: &str) -> Self {
        self.version = Some(output.to_owned());
        self
    }

    pub(crate) fn version_probes(&self) -> Vec<PathBuf> {
        lock(&self.probes).clone()
    }

    pub(crate) fn on_spawn(mut self, behaviour: Behaviour) -> Self {
        self.behaviour = behaviour;
        self
    }

    pub(crate) fn spawned(&self) -> Vec<Spawned> {
        lock(&self.spawned).clone()
    }

    pub(crate) fn waited_for(&self) -> Vec<u32> {
        lock(&self.waited).clone()
    }

    pub(crate) fn failing_installer(mut self) -> Self {
        self.installer_fails = true;
        self
    }

    pub(crate) fn installers(&self) -> Vec<(PathBuf, Vec<String>)> {
        lock(&self.installers).clone()
    }

    pub(crate) fn with_bundle(
        mut self,
        bundle: &Path,
        id: &str,
        executable: &str,
        version: &str,
    ) -> Self {
        self.bundles.insert(
            bundle.to_owned(),
            BundleInfo {
                id: id.into(),
                executable: executable.into(),
                version: version.into(),
            },
        );
        self
    }

    /// Info.plist answers for every bundle not set up one by one.
    pub(crate) fn with_any_bundle(mut self, id: &str, executable: &str, version: &str) -> Self {
        self.any_bundle = Some(BundleInfo {
            id: id.into(),
            executable: executable.into(),
            version: version.into(),
        });
        self
    }

    pub(crate) fn with_team(mut self, bundle: &Path, team: &str) -> Self {
        self.teams.insert(bundle.to_owned(), team.to_owned());
        self
    }

    pub(crate) fn with_default_team(mut self, team: &str) -> Self {
        self.default_team = Some(team.to_owned());
        self
    }

    /// What mounting `image` shows: `fill` writes the volume's contents.
    pub(crate) fn with_image(
        mut self,
        image: &Path,
        fill: impl Fn(&Path) + Send + Sync + 'static,
    ) -> Self {
        self.images.insert(image.to_owned(), Box::new(fill));
        self
    }

    pub(crate) fn failing_detach(mut self) -> Self {
        self.detach_fails = true;
        self
    }
}

/// Writes `name` next to the path that follows `flag` in `arguments`, or
/// next to the last argument.
fn marker_beside(arguments: &[OsString], flag: Option<&str>, name: &str, contents: &[u8]) {
    let path = match flag {
        Some(flag) => arguments
            .iter()
            .position(|argument| argument == flag)
            .and_then(|index| arguments.get(index + 1)),
        None => arguments.last(),
    };
    if let Some(parent) = path.and_then(|path| Path::new(path).parent()) {
        fs::write(parent.join(name), contents).unwrap();
    }
}

impl Host for FakeHost {
    fn timing(&self) -> Timing {
        Timing {
            helper_ready: Duration::from_millis(200),
            parent_exit: Duration::from_millis(200),
            app_start: Duration::from_millis(200),
            version_probe: Duration::from_millis(200),
            poll: Duration::from_millis(5),
        }
    }

    fn current_exe(&self) -> std::io::Result<PathBuf> {
        self.current_exe
            .clone()
            .ok_or_else(|| std::io::Error::other("no current executable in this test"))
    }

    fn var(&self, key: &str) -> Option<OsString> {
        self.vars.get(key).cloned()
    }

    fn package_owner(&self, program: &str, flag: &str, path: &Path) -> bool {
        lock(&self.queries).push(format!("{program} {flag} {}", path.display()));
        self.owners.iter().any(|owner| owner == program)
    }

    fn extract(&self, _archive: &Path, entry: &str, destination: &Path) -> Result<()> {
        let contents = self
            .entries
            .get(entry)
            .ok_or_else(|| anyhow!("Cannot unpack the update executable"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        std::io::Write::write_all(&mut file, contents)?;
        Ok(())
    }

    fn version_output(&self, executable: &Path) -> Result<String> {
        lock(&self.probes).push(executable.to_owned());
        self.version
            .clone()
            .ok_or_else(|| anyhow!("The downloaded app failed its startup check"))
    }

    fn spawn(
        &self,
        executable: &Path,
        arguments: &[OsString],
        log: Option<File>,
    ) -> Result<Box<dyn Child>> {
        lock(&self.spawned).push(Spawned {
            executable: executable.to_owned(),
            arguments: arguments.to_vec(),
            logged: log.is_some(),
        });
        match self.behaviour {
            Behaviour::WriteReady => marker_beside(arguments, None, "ready", b"ready"),
            Behaviour::Acknowledge => {
                marker_beside(arguments, Some("--update-receipt"), "started", b"started")
            }
            Behaviour::Exit(_) | Behaviour::Hang => {}
        }
        Ok(Box::new(FakeChild(self.behaviour)))
    }

    fn wait_for_parent(&self, parent: u32, ready: &Path) -> Result<()> {
        lock(&self.waited).push(parent);
        fs::write(ready, b"ready")?;
        Ok(())
    }

    fn run_installer(&self, installer: &Path, arguments: &[String]) -> Result<bool> {
        lock(&self.installers).push((installer.to_owned(), arguments.to_vec()));
        Ok(!self.installer_fails)
    }

    fn plist(&self, bundle: &Path, key: &str) -> Result<String> {
        let info = self
            .bundles
            .get(bundle)
            .or(self.any_bundle.as_ref())
            .ok_or_else(|| anyhow!("The app bundle is missing {key}"))?;
        Ok(match key {
            "CFBundleIdentifier" => info.id.clone(),
            "CFBundleExecutable" => info.executable.clone(),
            "CFBundlePackageType" => "APPL".into(),
            "CFBundleShortVersionString" => info.version.clone(),
            _ => bail!("The app bundle is missing {key}"),
        })
    }

    fn signing_team(&self, bundle: &Path) -> Result<Option<String>> {
        Ok(self
            .teams
            .get(bundle)
            .cloned()
            .or(self.default_team.clone()))
    }

    fn assess(&self, _bundle: &Path) -> Result<bool> {
        Ok(true)
    }

    fn attach(&self, image: &Path, mountpoint: &Path) -> Result<()> {
        // An image set up by file name alone matches in any folder, for a
        // download whose staging folder is random.
        let by_name = || {
            image
                .file_name()
                .and_then(|name| self.images.get(Path::new(name)))
        };
        if let Some(fill) = self.images.get(image).or_else(by_name) {
            fill(mountpoint);
        }
        Ok(())
    }

    fn detach(&self, mountpoint: &Path) -> bool {
        if self.detach_fails {
            return false;
        }
        let _ = fs::remove_dir_all(mountpoint);
        fs::create_dir(mountpoint).is_ok()
    }

    fn copy_bundle(&self, source: &Path, destination: &Path) -> Result<()> {
        copy_tree(source, destination)
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// A minimal `.app` folder: an executable and an Info.plist stand-in.
pub(crate) fn bundle(app: &Path, executable: &str, code: &[u8], metadata: &[u8]) {
    fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    fs::write(app.join("Contents/MacOS").join(executable), code).unwrap();
    fs::write(app.join("Contents/Info.plist"), metadata).unwrap();
}
