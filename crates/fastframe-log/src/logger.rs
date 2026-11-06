//! The `log` logger: stderr plus a per-run file, with redaction.

use std::borrow::Cow;
use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::panic::{make_folder, make_private, open_private};
use crate::{PanicMessage, log_panics_with};

/// Rewrites a log line before it is written.
///
/// It receives the record (for its target, module path and level) and the
/// formatted message, and returns the text to write instead, or `None` to
/// keep the message. Use it for dependencies whose messages can carry
/// private data: summarise them into a fixed category.
pub type Redactor = fn(record: &log::Record<'_>, message: &str) -> Option<Cow<'static, str>>;

/// Logger settings for one app. See the [crate] documentation.
///
/// ```no_run
/// fastframe_log::Logging::new("zapfast", env!("CARGO_PKG_VERSION"))
///     .filter("warn,zapfast=info")
///     .file("/home/me/.local/state/zapfast/zapfast.log")
///     .panic_log("/home/me/.local/state/zapfast/panic.log")
///     .init()
///     .expect("the only logger");
/// log::info!("ready");
/// ```
#[derive(Clone, Debug)]
#[must_use = "nothing is logged until `init` is called"]
pub struct Logging {
    app: &'static str,
    version: &'static str,
    filter: String,
    file: Option<PathBuf>,
    panic_log: Option<PathBuf>,
    panic_message: PanicMessage,
    redact: Option<Redactor>,
}

impl Logging {
    /// Settings for `app` (the short name in log lines, such as `zapfast`)
    /// at `version` (the app's `env!("CARGO_PKG_VERSION")`).
    ///
    /// By default only warnings and errors are kept, only on stderr, and
    /// panics are not recorded.
    pub fn new(app: &'static str, version: &'static str) -> Self {
        Self {
            app,
            version,
            filter: "warn".to_owned(),
            file: None,
            panic_log: None,
            panic_message: PanicMessage::Omit,
            redact: None,
        }
    }

    /// The filter used when `RUST_LOG` is unset, in `env_logger` syntax
    /// (`warn,zapfast=info`).
    pub fn filter(mut self, default: impl Into<String>) -> Self {
        self.filter = default.into();
        self
    }

    /// Also writes every line to `path`, starting it afresh each run. The
    /// earlier run's file is kept beside it as [`previous_log`] (`app.log`
    /// becomes `app.previous.log`), so a run that hung and was killed still
    /// has its log after the restart. The folder it is in is made when it is
    /// missing, as on a first run. If the file cannot be created, logging
    /// continues on stderr and says why.
    ///
    /// An app that lets several copies run at once keeps each copy's log: a
    /// file a running copy still writes is never replaced. When the previous
    /// log is one, the earlier run's file goes to the first of
    /// `app.previous.2.log`, `app.previous.3.log` and so on that no running
    /// copy writes.
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }

    /// Appends a line to `path` for every panic, without its message unless
    /// [`Logging::panic_message`] says otherwise. See [`crate::log_panics`].
    pub fn panic_log(mut self, path: impl Into<PathBuf>) -> Self {
        self.panic_log = Some(path.into());
        self
    }

    /// Whether panic lines keep the panic's message, redacted. The default,
    /// [`PanicMessage::Omit`], leaves it out.
    pub fn panic_message(mut self, message: PanicMessage) -> Self {
        self.panic_message = message;
        self
    }

    /// Rewrites lines before they are written. See [`Redactor`].
    pub fn redact(mut self, redactor: Redactor) -> Self {
        self.redact = Some(redactor);
        self
    }

    /// Installs the logger and, if asked, the panic hook, then logs the app's
    /// version and platform.
    ///
    /// # Errors
    ///
    /// When another logger is already installed. The panic hook is installed
    /// either way.
    pub fn init(self) -> Result<(), log::SetLoggerError> {
        let rust_log = std::env::var("RUST_LOG").ok();
        let (logger, file_error) = self.build(rust_log.as_deref());
        let max_level = logger.filter();
        if let Some(path) = &self.panic_log {
            log_panics_with(path, self.app, self.version, self.panic_message);
        }
        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(max_level);
        if let Some((path, error)) = file_error {
            log::warn!("not keeping a log file at {}: {error}", path.display());
        }
        // Under the app's own target, so the app's filter (`warn,zapfast=info`)
        // keeps it; this crate's target is filtered out by default.
        log::info!(target: self.app, "{}", self.start_line());
        Ok(())
    }

    /// The line `init` logs once the logger is up.
    fn start_line(&self) -> String {
        format!(
            "Starting {} {} on {} ({})",
            self.app,
            self.version,
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    }

    /// The logger, reading `rust_log` in place of the environment, and the
    /// reason the log file could not be created.
    fn build(
        &self,
        rust_log: Option<&str>,
    ) -> (env_logger::Logger, Option<(PathBuf, std::io::Error)>) {
        let mut builder = env_logger::Builder::new();
        builder.parse_filters(rust_log.unwrap_or(&self.filter));
        let mut file_error = None;
        if let Some(path) = &self.file {
            keep_earlier_log(path);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            match make_folder(path).and_then(|()| open_private(path, &mut options)) {
                Ok(file) => {
                    hold(&file);
                    builder.target(env_logger::Target::Pipe(Box::new(Tee {
                        stderr: std::io::stderr(),
                        file,
                    })));
                }
                Err(error) => file_error = Some((path.clone(), error)),
            }
        }
        let redact = self.redact;
        builder.format(move |buffer, record| {
            let line = line(buffer.timestamp(), record, redact);
            buffer.write_all(line.as_bytes())
        });
        (builder.build(), file_error)
    }
}

/// Where [`Logging::file`] keeps the earlier run's log: `app.log` becomes
/// `app.previous.log`, and a name without an extension gains `.previous`.
///
/// While a copy of the app that is still running writes this file, the
/// earlier run's log is kept as `app.previous.2.log` (or the next number
/// that no running copy writes) instead.
pub fn previous_log(path: &Path) -> PathBuf {
    kept_log(path, 1)
}

/// The `number`th name for an earlier run's log: [`previous_log`] for the
/// first, then `app.previous.2.log`, `app.previous.3.log` and so on.
fn kept_log(path: &Path, number: usize) -> PathBuf {
    let mut name = path.file_stem().unwrap_or_default().to_os_string();
    name.push(".previous");
    if number > 1 {
        name.push(format!(".{number}"));
    }
    if let Some(extension) = path.extension() {
        name.push(".");
        name.push(extension);
    }
    path.with_file_name(name)
}

/// Moves the earlier run's log at `path` aside, to the first kept name
/// ([`kept_log`]) that no running copy of the app writes, so a copy started
/// while others run never replaces their logs.
///
/// The earlier run's log is worth more than a failed rename: the new file is
/// made either way.
fn keep_earlier_log(path: &Path) {
    let Some(kept) = (1..)
        .map(|number| kept_log(path, number))
        .find(|kept| !in_use(kept))
    else {
        return;
    };
    if std::fs::rename(path, &kept).is_ok() {
        // A version before logs were private left it readable by others.
        make_private(&kept);
    }
}

/// Marks the log this run writes as in use, for [`in_use`] in copies of the
/// app started while this one runs. On Unix it is an advisory lock, which
/// readers of the log never notice, and which moves with the file when a
/// later copy renames it. On Windows the open file is mark enough: a lock
/// there would keep everyone else from reading the log.
fn hold(file: &std::fs::File) {
    // Without the lock the log is still written, as before there was one.
    #[cfg(unix)]
    let _ = file.try_lock();
    #[cfg(not(unix))]
    let _ = file;
}

/// Whether a running copy of the app writes the log at `path`. A missing
/// file, a link, or one whose state cannot be told is not in use, and is
/// replaced as before.
fn in_use(path: &Path) -> bool {
    // A link may lead to a pipe, which would block the open below.
    std::fs::symlink_metadata(path).is_ok_and(|found| found.is_file()) && held(path)
}

/// Whether another open file holds the log at `path`: on Unix, the lock that
/// [`hold`] takes.
#[cfg(unix)]
fn held(path: &Path) -> bool {
    std::fs::File::open(path).is_ok_and(|file| {
        matches!(
            file.try_lock_shared(),
            Err(std::fs::TryLockError::WouldBlock)
        )
    })
}

/// Whether another open file holds the log at `path`: on Windows, whether it
/// cannot be opened for this process alone. A running copy writing it is the
/// usual reason; anything else holding it open only leaves it in place too.
#[cfg(windows)]
fn held(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    // `ERROR_SHARING_VIOLATION`.
    const SHARING_VIOLATION: i32 = 32;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .is_err_and(|error| error.raw_os_error() == Some(SHARING_VIOLATION))
}

/// Whether another open file holds the log at `path`: never known here, so
/// the earlier run's log replaces the previous one as before.
#[cfg(not(any(unix, windows)))]
fn held(_path: &Path) -> bool {
    false
}

/// One log line: `[time LEVEL target] message`, with the message rewritten
/// by `redact` when it asks to.
fn line(timestamp: impl Display, record: &log::Record<'_>, redact: Option<Redactor>) -> String {
    let message = record.args().to_string();
    let message = redact
        .and_then(|redact| redact(record, &message))
        .unwrap_or(Cow::Borrowed(&message));
    format!(
        "[{timestamp} {} {}] {message}\n",
        record.level(),
        record.target()
    )
}

/// Writes to stderr and to the run's log file.
///
/// stderr is best effort: a desktop launch may have closed it.
struct Tee<E, F> {
    stderr: E,
    file: F,
}

impl<E: Write, F: Write> Write for Tee<E, F> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.stderr.write_all(buf);
        self.file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = self.stderr.flush();
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_line(
        target: &str,
        message: std::fmt::Arguments<'_>,
        redact: Option<Redactor>,
    ) -> String {
        line(
            "2026-09-25T10:00:00Z",
            &log::Record::builder()
                .level(log::Level::Warn)
                .target(target)
                .args(message)
                .build(),
            redact,
        )
    }

    /// Apps filter to `warn,<app>=info`: the start line is logged under the
    /// app's name so that filter keeps it in the file.
    #[test]
    fn the_start_line_is_kept_by_the_apps_default_filter() {
        use log::Log as _;
        let logging = Logging::new("zapfast", "0.16.3").filter("warn,zapfast=info");
        let (logger, _) = logging.build(None);
        let start = log::Metadata::builder()
            .level(log::Level::Info)
            .target(logging.app)
            .build();
        assert!(logger.enabled(&start));
        assert!(
            logging
                .start_line()
                .starts_with("Starting zapfast 0.16.3 on ")
        );
    }

    #[test]
    fn a_line_carries_time_level_target_and_message() {
        assert_eq!(
            record_line("zapfast::backend", format_args!("reconnecting"), None),
            "[2026-09-25T10:00:00Z WARN zapfast::backend] reconnecting\n"
        );
    }

    fn summarise(record: &log::Record<'_>, _message: &str) -> Option<Cow<'static, str>> {
        record
            .target()
            .starts_with("protocol")
            .then_some(Cow::Borrowed(
                "protocol diagnostic (private details omitted)",
            ))
    }

    #[test]
    fn the_redactor_replaces_only_the_lines_it_claims() {
        let secret = "123456789@s.whatsapp.net sent fixture text";
        let redacted = record_line("protocol::recv", format_args!("{secret}"), Some(summarise));
        assert!(!redacted.contains(secret));
        assert!(redacted.ends_with("] protocol diagnostic (private details omitted)\n"));
        assert!(
            record_line("zapfast::ui", format_args!("kept"), Some(summarise)).ends_with("] kept\n")
        );
    }

    #[test]
    fn the_tee_writes_both_and_survives_a_closed_stderr() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let mut both = Tee {
            stderr: Vec::new(),
            file: Vec::new(),
        };
        both.write_all(b"one\n").unwrap();
        both.flush().unwrap();
        assert_eq!(both.stderr, b"one\n");
        assert_eq!(both.file, b"one\n");

        let mut closed = Tee {
            stderr: Closed,
            file: Vec::new(),
        };
        closed.write_all(b"two\n").unwrap();
        closed.flush().unwrap();
        assert_eq!(closed.file, b"two\n");
    }

    fn matches(logger: &env_logger::Logger, level: log::Level, target: &str) -> bool {
        use log::Log;
        logger.enabled(&log::Metadata::builder().level(level).target(target).build())
    }

    #[test]
    fn rust_log_overrides_the_default_filter() {
        let logging = Logging::new("zapfast", "1.0.0").filter("warn,zapfast=info");
        let (default, _) = logging.build(None);
        assert!(matches(&default, log::Level::Info, "zapfast::app"));
        assert!(!matches(&default, log::Level::Info, "other"));
        let (overridden, _) = logging.build(Some("debug"));
        assert!(matches(&overridden, log::Level::Debug, "other"));
    }

    #[test]
    fn the_default_filter_keeps_only_warnings() {
        let (logger, _) = Logging::new("zapfast", "1.0.0").build(None);
        assert!(matches(&logger, log::Level::Warn, "zapfast"));
        assert!(!matches(&logger, log::Level::Info, "zapfast"));
    }

    #[test]
    fn the_log_file_starts_empty_each_run() {
        let dir = std::env::temp_dir().join(format!("fastframe-log-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        std::fs::write(&path, "last run\n").unwrap();
        let (_, error) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        assert!(error.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_earlier_runs_log_is_kept_beside_the_new_one() {
        let dir = std::env::temp_dir().join(format!("fastframe-log-kept-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        std::fs::write(&path, "the run that hung\n").unwrap();
        std::fs::write(dir.join("app.previous.log"), "an older run\n").unwrap();
        let (_, error) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        assert!(error.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        assert_eq!(
            std::fs::read_to_string(dir.join("app.previous.log")).unwrap(),
            "the run that hung\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_previous_log_is_named_after_the_log() {
        assert_eq!(
            previous_log(Path::new("/state/zapfast.log")),
            Path::new("/state/zapfast.previous.log")
        );
        assert_eq!(
            previous_log(Path::new("/state/log")),
            Path::new("/state/log.previous")
        );
        assert_eq!(
            kept_log(Path::new("/state/zapfast.log"), 2),
            Path::new("/state/zapfast.previous.2.log")
        );
        assert_eq!(
            kept_log(Path::new("/state/log"), 3),
            Path::new("/state/log.previous.3")
        );
    }

    /// Writes `message` through `logger`, as a running copy of the app would.
    fn log_through(logger: &env_logger::Logger, message: &str) {
        use log::Log as _;
        logger.log(
            &log::Record::builder()
                .level(log::Level::Warn)
                .target("zapfast")
                .args(format_args!("{message}"))
                .build(),
        );
        logger.flush();
    }

    #[test]
    fn a_log_is_in_use_while_its_copy_runs() {
        let dir = std::env::temp_dir().join(format!("fastframe-log-use-{}", std::process::id()));
        let path = dir.join("app.log");
        let missing = in_use(&path);
        let (running, _) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        let while_running = in_use(&path);
        drop(running);
        let after = in_use(&path);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!((missing, while_running, after), (false, true, false));
    }

    /// Several copies of an app running at once: the third start keeps the
    /// second copy's log under a new name instead of replacing the previous
    /// log, which the first copy still writes.
    #[test]
    fn copies_running_at_once_each_keep_their_log() {
        let dir = std::env::temp_dir().join(format!("fastframe-log-copies-{}", std::process::id()));
        let path = dir.join("app.log");
        let logging = Logging::new("zapfast", "1.0.0").file(&path);
        let (first, _) = logging.build(None);
        let (second, _) = logging.build(None);
        let (third, _) = logging.build(None);
        log_through(&first, "from the first copy");
        log_through(&second, "from the second copy");
        log_through(&third, "from the third copy");
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        let logs = [
            read("app.log"),
            read("app.previous.log"),
            read("app.previous.2.log"),
        ];
        drop((first, second, third));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(logs[0].ends_with("] from the third copy\n"), "{logs:?}");
        assert!(logs[1].ends_with("] from the first copy\n"), "{logs:?}");
        assert!(logs[2].ends_with("] from the second copy\n"), "{logs:?}");
    }

    /// Kept logs no running copy writes are replaced as before: once the
    /// first copy quits, the previous log takes the next earlier run again,
    /// and a numbered log left by a copy that quit is reused, not added to.
    #[test]
    fn a_kept_log_no_copy_writes_is_replaced() {
        let dir = std::env::temp_dir().join(format!("fastframe-log-quit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        std::fs::write(dir.join("app.previous.2.log"), "a copy that quit\n").unwrap();
        let logging = Logging::new("zapfast", "1.0.0").file(&path);
        let (first, _) = logging.build(None);
        let (second, _) = logging.build(None);
        let (third, _) = logging.build(None);
        log_through(&second, "from the second copy");
        log_through(&third, "from the third copy");
        drop(first);
        let (fourth, _) = logging.build(None);
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        let logs = [read("app.previous.log"), read("app.previous.2.log")];
        let third_kept = dir.join("app.previous.3.log").exists();
        drop((second, third, fourth));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(logs[0].ends_with("] from the third copy\n"), "{logs:?}");
        assert!(logs[1].ends_with("] from the second copy\n"), "{logs:?}");
        assert!(!third_kept, "no new name while a free one is left");
    }

    #[test]
    fn the_log_files_folder_is_made_on_a_first_run() {
        let dir = std::env::temp_dir().join(format!("fastframe-log-fresh-{}", std::process::id()));
        let path = dir.join("state").join("app.log");
        let (_, error) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        assert!(error.is_none(), "{error:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        #[cfg(unix)]
        let modes = (
            crate::tests::mode(&path),
            crate::tests::mode(&dir.join("state")),
        );
        std::fs::remove_dir_all(dir).unwrap();
        #[cfg(unix)]
        assert_eq!(modes, (0o600, 0o700), "the log and its folder are private");
    }

    /// A log an older version left readable by others is the user's alone
    /// from the next run.
    #[cfg(unix)]
    #[test]
    fn a_log_others_could_read_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fastframe-log-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        std::fs::write(&path, "last run\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (_, error) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        let previous = previous_log(&path);
        let kept = std::fs::read_to_string(&previous).unwrap();
        let modes = (crate::tests::mode(&path), crate::tests::mode(&previous));
        std::fs::remove_dir_all(dir).unwrap();
        assert!(error.is_none(), "{error:?}");
        assert_eq!(kept, "last run\n", "the earlier run's log is kept");
        assert_eq!(modes, (0o600, 0o600), "the new log and the kept one");
    }

    /// A link at the log's path may lead to a file the user shares with
    /// others, or to a device: its target keeps its permissions.
    #[cfg(unix)]
    #[test]
    fn a_linked_log_keeps_its_targets_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fastframe-log-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shared = dir.join("shared.log");
        std::fs::write(&shared, "last run\n").unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path = dir.join("app.log");
        std::os::unix::fs::symlink(&shared, &path).unwrap();
        let (_, error) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        let modes = (crate::tests::mode(&shared), crate::tests::mode(&path));
        std::fs::remove_dir_all(dir).unwrap();
        assert!(error.is_none(), "{error:?}");
        assert_eq!(modes, (0o644, 0o600), "the shared file and the new log");
    }

    #[test]
    fn an_unwritable_log_file_is_reported_not_fatal() {
        // A file where the folder should be: nothing can be made inside it.
        let dir =
            std::env::temp_dir().join(format!("fastframe-log-unwritable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("not-a-folder");
        std::fs::write(&blocker, "").unwrap();
        let path = blocker.join("app.log");
        let (_, error) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(error.map(|(failed, _)| failed), Some(path));
    }
}
