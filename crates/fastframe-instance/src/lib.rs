//! One running copy of an app per user.
//!
//! The running copy holds an exclusive lock on a file in a private per-user
//! directory, its [`Slot`]. The operating system releases the lock when the
//! process ends, even after a crash, so nothing left behind blocks a later
//! launch. A second launch finds the lock taken, hands its request (show the
//! window, open a link, any line the app understands) to the running copy over
//! a private channel, and exits.
//!
//! On Unix the channel is a socket in the slot's directory, which only the
//! user can open. On Windows it is a loopback port, which any local process
//! can reach, so every request must first present a random token that the
//! running copy writes to the slot's directory.
//!
//! ```no_run
//! use fastframe_instance::{Claim, Slot};
//!
//! let slot = Slot::new("rocks.example.App");
//! let guard = match slot.claim("show", |request| match request {
//!     // Queue it for the app and wake its window; the reply goes back to
//!     // the launch that asked.
//!     "show" | "ping" => Some("ok".to_owned()),
//!     // Not a request this app knows: no reply, and the launch is told so.
//!     _ => None,
//! }) {
//!     Claim::First(guard) => guard,
//!     Claim::Running(_reply) => return, // the running copy took it
//!     Claim::Declined => return,        // the running copy declined it
//!     Claim::Unanswered => return,      // running, but it did not answer
//! };
//! // Keep `guard` until the process exits.
//! # drop(guard);
//! ```

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(not(unix))]
mod tcp;

const LOCK_FILE: &str = "instance.lock";
#[cfg(unix)]
const SOCKET_FILE: &str = "instance.sock";
#[cfg(any(not(unix), test))]
const KEY_FILE: &str = "instance.key";

/// The longest request line accepted, token included. Long enough for a
/// percent-encoded search link.
const REQUEST_LIMIT: usize = 16 * 1024;
/// The time a client gets to send its whole request. Requests are served one
/// at a time, so this bounds how long a stray connection holds up the rest.
const REQUEST_TIME: Duration = Duration::from_secs(1);
/// The time a client waits for the reply, which may queue behind a stray one.
const REPLY_TIME: Duration = Duration::from_secs(5);
/// How long a second launch waits for the running copy to answer, which it
/// may not yet while it is still starting, before [`Claim::Unanswered`].
pub const ANSWER_WAIT: Duration = Duration::from_secs(3);

/// Where one running copy lives: a private directory for the lock and the
/// channel, and the name every request and reply starts with.
///
/// [`Slot::new`] gives one slot per user. A demo or a test that should run
/// beside the real app takes a [`scoped`](Slot::scoped) slot of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    dir: PathBuf,
    prefix: String,
    startup_wait: Duration,
}

/// What [`Slot::claim`] found.
#[derive(Debug)]
pub enum Claim {
    /// This process is the running copy. Keep the guard until it exits.
    First(Guard),
    /// Another copy is running and took the request; this is its reply.
    Running(String),
    /// Another copy is running and declined the request: its handler
    /// returned `None`.
    Declined,
    /// Another copy holds the slot but did not answer within
    /// [`ANSWER_WAIT`].
    Unanswered,
}

/// Marks this process as the running copy until it exits. The channel is
/// served on a thread of its own for the rest of the process.
#[derive(Debug)]
pub struct Guard {
    _lock: Option<std::fs::File>,
}

impl Slot {
    /// The one slot per user for `app_id` (a reverse-DNS name such as
    /// `rocks.example.App`): the per-user runtime directory on Linux
    /// (`$XDG_RUNTIME_DIR`, or the app's own inside a Flatpak sandbox), the
    /// user's private temporary directory on macOS (`$TMPDIR`, short enough
    /// for a socket path), and the user's local data on Windows.
    #[must_use]
    pub fn new(app_id: &str) -> Self {
        Self::at(default_dir(app_id), app_id)
    }

    /// A slot in a directory the app chose, for an app that already keeps
    /// its runtime files somewhere. `name` starts every request and reply,
    /// so a copy of the app answers only its own kind.
    #[must_use]
    pub fn at(dir: impl Into<PathBuf>, name: &str) -> Self {
        Self {
            dir: dir.into(),
            prefix: format!("{name}:"),
            startup_wait: ANSWER_WAIT,
        }
    }

    /// A separate slot beside this one, for a copy that should run alongside
    /// it, such as a demo. Characters other than ASCII letters, digits, `-`
    /// and `_` become `_`.
    #[must_use]
    pub fn scoped(mut self, scope: &str) -> Self {
        self.dir = self.dir.join(format!("scope-{}", file_name_safe(scope)));
        self
    }

    /// The slot's directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Becomes the running copy, or hands `request` to the one already
    /// running.
    ///
    /// The running copy calls `handle` on a thread of its own for each
    /// request a later launch sends, in order, and sends back what it
    /// returns. `None` declines the request, and the launch is told so
    /// ([`Claim::Declined`]). Keep
    /// `handle` quick (queue the request and wake the app), since requests
    /// wait their turn.
    ///
    /// A slot whose lock cannot be taken at all (a read-only directory, say)
    /// runs unguarded rather than not at all, with a warning in the log.
    pub fn claim(
        &self,
        request: &str,
        handle: impl FnMut(&str) -> Option<String> + Send + 'static,
    ) -> Claim {
        let lock = match lock(&self.dir) {
            Ok(Some(lock)) => lock,
            Ok(None) => return self.hand_over(request),
            Err(error) => {
                log::warn!("cannot take the instance lock; running unguarded: {error}");
                return Claim::First(Guard { _lock: None });
            }
        };
        // Holding the lock without listening would leave every later launch
        // waiting for an answer that never comes, so a slot that cannot
        // listen lets the lock go and runs unguarded.
        if let Err(error) = listen(&self.dir, self.prefix.clone(), handle) {
            log::warn!("cannot listen for other launches; running unguarded: {error}");
            return Claim::First(Guard { _lock: None });
        }
        Claim::First(Guard { _lock: Some(lock) })
    }

    /// Sends `request` to the running copy and returns its reply, without
    /// becoming the running copy when there is none: for a command-line
    /// action that only makes sense while the app runs. An error of kind
    /// `NotFound` or `ConnectionRefused` means no copy is running, and
    /// `PermissionDenied` that the running copy declined the request.
    pub fn send(&self, request: &str) -> std::io::Result<String> {
        if request.contains('\n') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a request is one line",
            ));
        }
        send(&self.dir, &self.prefix, request)
    }

    /// Sends `request` to the lock holder, waiting while it may still be
    /// starting.
    fn hand_over(&self, request: &str) -> Claim {
        let deadline = Instant::now() + self.startup_wait;
        loop {
            match self.send(request) {
                Ok(reply) => return Claim::Running(reply),
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Claim::Declined;
                }
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                    log::warn!("cannot hand the request over: {error}");
                    return Claim::Unanswered;
                }
                Err(error) if Instant::now() >= deadline => {
                    log::warn!("the running instance did not answer: {error}");
                    return Claim::Unanswered;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }
}

/// The per-user directory for `app_id`'s default slot.
fn default_dir(app_id: &str) -> PathBuf {
    let project = directories::ProjectDirs::from("", "", app_id);
    // Flatpak gives each sandbox a private runtime directory and shares only
    // this one between instances of the app.
    #[cfg(target_os = "linux")]
    if let (Some(runtime), Some(id)) = (
        project
            .as_ref()
            .and_then(|project| project.runtime_dir())
            .and_then(Path::parent),
        std::env::var_os("FLATPAK_ID"),
    ) {
        return runtime.join("app").join(id);
    }
    // A socket path has room for 104 bytes on macOS, which a directory in
    // Application Support under a long user name can use up. The user's
    // temporary directory is private to them, and short.
    #[cfg(target_os = "macos")]
    {
        let _ = project;
        std::env::temp_dir().join(file_name_safe(app_id))
    }
    #[cfg(not(target_os = "macos"))]
    match &project {
        Some(project) => match project.runtime_dir() {
            Some(runtime) => runtime.to_path_buf(),
            None => project.data_local_dir().join("instance"),
        },
        // No home directory at all: a directory of the user's own is gone,
        // so the slot is at least per app.
        None => std::env::temp_dir().join(format!("{}-instance", file_name_safe(app_id))),
    }
}

fn file_name_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_start_matches('.')
        .to_owned()
}

/// Takes the slot's lock, or returns `None` when another process holds it.
fn lock(dir: &Path) -> std::io::Result<Option<std::fs::File>> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        builder.mode(0o700);
        options.mode(0o600);
        builder.create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    builder.create(dir)?;
    let file = options.open(dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

/// Sends one request over the socket and returns the reply.
#[cfg(unix)]
fn send(dir: &Path, prefix: &str, request: &str) -> std::io::Result<String> {
    let stream = std::os::unix::net::UnixStream::connect(dir.join(SOCKET_FILE))?;
    exchange(stream, None, prefix, request)
}

/// Sends one request over loopback, with the token, and returns the reply.
#[cfg(not(unix))]
fn send(dir: &Path, prefix: &str, request: &str) -> std::io::Result<String> {
    tcp::send(dir, prefix, request)
}

/// Listens on a socket only the user can open. Only the lock holder gets
/// here, so a socket that already exists was left by a copy that ended.
#[cfg(unix)]
fn listen(
    dir: &Path,
    prefix: String,
    handle: impl FnMut(&str) -> Option<String> + Send + 'static,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(SOCKET_FILE);
    match std::fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    spawn(move || serve(listener.incoming(), None, &prefix, handle))
}

#[cfg(not(unix))]
fn listen(
    dir: &Path,
    prefix: String,
    handle: impl FnMut(&str) -> Option<String> + Send + 'static,
) -> std::io::Result<()> {
    tcp::listen(dir, prefix, handle)
}

fn spawn(serve: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("fastframe-instance".to_owned())
        .spawn(serve)
        .map(drop)
}

/// A random secret, as 64 hex digits.
#[cfg(any(not(unix), test))]
fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Compares without stopping at the first difference, so the time an answer
/// takes reveals nothing about the token.
fn token_matches(expected: &str, presented: &[u8]) -> bool {
    let expected = expected.as_bytes();
    presented.len() == expected.len()
        && presented
            .iter()
            .zip(expected)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Writes the port and token, readable only by the user. Windows keeps the
/// slot in the user's profile, which other users cannot read.
#[cfg(any(not(unix), test))]
fn write_key(dir: &Path, port: u16, token: &str) -> std::io::Result<()> {
    let partial = dir.join(format!("{KEY_FILE}.partial"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&partial)?
        .write_all(format!("{port}\n{token}\n").as_bytes())?;
    // Clients never read a half-written file.
    std::fs::rename(partial, dir.join(KEY_FILE))
}

#[cfg(any(not(unix), test))]
fn read_key(dir: &Path) -> std::io::Result<(u16, String)> {
    let text = std::fs::read_to_string(dir.join(KEY_FILE))?;
    let mut lines = text.lines();
    let port = lines.next().and_then(|port| port.parse().ok());
    match (port, lines.next()) {
        (Some(port), Some(token)) => Ok((port, token.to_owned())),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the instance key file is damaged",
        )),
    }
}

/// Either end of the channel.
trait Connection: Read + Write {
    fn set_timeouts(&self, timeout: Option<Duration>) -> std::io::Result<()>;
}

impl Connection for std::net::TcpStream {
    fn set_timeouts(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(timeout)?;
        self.set_write_timeout(timeout)
    }
}

#[cfg(unix)]
impl Connection for std::os::unix::net::UnixStream {
    fn set_timeouts(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(timeout)?;
        self.set_write_timeout(timeout)
    }
}

/// Sends the token line, when there is one, and the request, then reads the
/// one-line reply, which must carry the prefix.
fn exchange(
    mut stream: impl Connection,
    token: Option<&str>,
    prefix: &str,
    request: &str,
) -> std::io::Result<String> {
    stream.set_timeouts(Some(REPLY_TIME))?;
    let token = token.map(|token| format!("{token}\n")).unwrap_or_default();
    stream.write_all(format!("{token}{prefix}{request}\n").as_bytes())?;
    // The reply is one line, then the connection closes.
    let mut reply = String::new();
    stream
        .take(REQUEST_LIMIT as u64)
        .read_to_string(&mut reply)?;
    let line = reply.lines().next().unwrap_or_default();
    if line == declined(prefix) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the running copy declined the request",
        ));
    }
    match line.strip_prefix(prefix) {
        Some(reply) => Ok(reply.to_owned()),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the channel is not the app's",
        )),
    }
}

/// The line that declines a request: the app's name and `!declined`, which
/// no reply can be, since every reply starts with the name and `:`. Copies
/// from before it never send it, and their silence reads as before.
fn declined(prefix: &str) -> String {
    format!("{}!declined", prefix.strip_suffix(':').unwrap_or(prefix))
}

/// Handles one request and reply per connection until the listener closes.
fn serve<C: Connection>(
    incoming: impl Iterator<Item = std::io::Result<C>>,
    token: Option<&str>,
    prefix: &str,
    mut handle: impl FnMut(&str) -> Option<String>,
) {
    for mut stream in incoming.flatten() {
        // Clients without the token or the prefix get nothing.
        let Some(request) = receive(&mut stream, token, prefix) else {
            continue;
        };
        let line = match handle(&request) {
            Some(reply) => format!("{prefix}{}", reply.replace('\n', " ")),
            None => declined(prefix),
        };
        let _ = stream.write_all(format!("{line}\n").as_bytes());
    }
}

/// Reads one request, refusing it unless its first line is the token when
/// the transport needs one, and it carries the prefix.
fn receive(stream: &mut impl Connection, token: Option<&str>, prefix: &str) -> Option<String> {
    let lines = 1 + usize::from(token.is_some());
    let request = read_lines(stream, lines)?;
    let mut request = request.split(|&byte| byte == b'\n');
    if let Some(token) = token
        && !token_matches(token, request.next()?)
    {
        return None;
    }
    let line = std::str::from_utf8(request.next()?).ok()?;
    line.trim_end_matches('\r')
        .strip_prefix(prefix)
        .map(str::to_owned)
}

/// Reads until `lines` newlines within the size and time limits. Refuses
/// read errors, oversized input, and clients that stall.
fn read_lines(stream: &mut impl Connection, lines: usize) -> Option<Vec<u8>> {
    let deadline = Instant::now() + REQUEST_TIME;
    let mut buffer = vec![0u8; REQUEST_LIMIT];
    let mut filled = 0;
    loop {
        if filled == buffer.len() {
            return None;
        }
        let left = deadline.checked_duration_since(Instant::now())?;
        stream
            .set_timeouts(Some(left.max(Duration::from_millis(1))))
            .ok()?;
        match stream.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => {
                filled += read;
                let newlines = buffer[..filled].iter().filter(|&&b| b == b'\n').count();
                if newlines >= lines {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    buffer.truncate(filled);
    Some(buffer)
}

#[cfg(test)]
mod tests;
