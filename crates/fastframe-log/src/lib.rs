//! Log to stderr and a file for bug reports, keep private data out, and record
//! panics without their payload.
//!
//! A desktop app launched from a menu has no terminal, so its stderr goes
//! nowhere; the log file is what users attach to bug reports. It therefore
//! must never hold message contents, phone numbers, keys, tokens, or pairing
//! payloads. This crate sets up the logger the way ZapFast and Spotifast do:
//!
//! - every line goes to stderr and, when asked, to a file created fresh for
//!   this run;
//! - `RUST_LOG` overrides the app's default filter;
//! - an optional [`Redactor`] rewrites the lines of chosen targets (a
//!   protocol library that quotes what it received, say) before they are
//!   written;
//! - a panic hook appends one line per panic to a panic log, with the time,
//!   the app's version, the thread and the source location, and never the
//!   panic's payload, which can quote whatever data the code was handling.
//!
//! ```no_run
//! fastframe_log::Logging::new("zapfast", env!("CARGO_PKG_VERSION"))
//!     .filter("warn,zapfast=info")
//!     .file("/home/me/.local/state/zapfast/zapfast.log")
//!     .panic_log("/home/me/.local/state/zapfast/panic.log")
//!     .init()
//!     .expect("the only logger");
//! log::info!("ready");
//! ```
//!
//! The app's name and version are passed in: this crate's own version is not
//! the app's.
//!
//! For the words that must not reach a line in the first place, see
//! [`redact`].

use std::borrow::Cow;
use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};

pub mod redact;

/// Rewrites a log line before it is written.
///
/// It receives the record (for its target, module path and level) and the
/// formatted message, and returns the text to write instead, or `None` to
/// keep the message. Use it for dependencies whose messages can carry
/// private data: summarise them into a fixed category.
pub type Redactor = fn(record: &log::Record<'_>, message: &str) -> Option<Cow<'static, str>>;

/// Logger settings for one app. See the [crate] documentation.
#[derive(Clone, Debug)]
#[must_use = "nothing is logged until `init` is called"]
pub struct Logging {
    app: &'static str,
    version: &'static str,
    filter: String,
    file: Option<PathBuf>,
    panic_log: Option<PathBuf>,
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
            redact: None,
        }
    }

    /// The filter used when `RUST_LOG` is unset, in `env_logger` syntax
    /// (`warn,zapfast=info`).
    pub fn filter(mut self, default: impl Into<String>) -> Self {
        self.filter = default.into();
        self
    }

    /// Also writes every line to `path`, replacing what an earlier run left
    /// there. If the file cannot be created, logging continues on stderr and
    /// says why.
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }

    /// Appends a line to `path` for every panic. See [`log_panics`].
    pub fn panic_log(mut self, path: impl Into<PathBuf>) -> Self {
        self.panic_log = Some(path.into());
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
            log_panics(path, self.app, self.version);
        }
        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(max_level);
        if let Some((path, error)) = file_error {
            log::warn!("not keeping a log file at {}: {error}", path.display());
        }
        log::info!(
            "Starting {} {} on {} ({})",
            self.app,
            self.version,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        Ok(())
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
            match std::fs::File::create(path) {
                Ok(file) => {
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

/// Records every panic in `path` before the process dies of it.
///
/// Release builds usually abort on panic and, on Windows, have no console, so
/// a crash would otherwise leave nothing behind for a bug report. Each panic
/// appends one line: the time, `app`, `version`, the thread's name and the
/// source location, followed by `(payload omitted)`. The payload is left out
/// on purpose, from the file and from stderr: an `expect` or a formatted
/// panic message can quote the data being handled.
///
/// This replaces the default hook, which would print the payload to stderr.
pub fn log_panics(path: impl AsRef<Path>, app: &'static str, version: &'static str) {
    let path = path.as_ref().to_path_buf();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let entry = panic_entry(
            jiff::Timestamp::now(),
            app,
            version,
            thread.name(),
            info.location(),
        );
        report_panic(&entry);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path);
        if let Ok(mut file) = file {
            let _ = file.write_all(entry.as_bytes());
        }
    }));
}

/// Prints the panic line where the default hook would have printed the
/// panic: stderr is the only place left once a panic is under way.
#[allow(
    clippy::print_stderr,
    reason = "the hook replaces the default one, which reports on stderr"
)]
fn report_panic(entry: &str) {
    eprint!("{entry}");
}

/// The panic log line. Takes no payload, so it cannot leak one.
fn panic_entry(
    time: impl Display,
    app: &str,
    version: &str,
    thread: Option<&str>,
    location: Option<&std::panic::Location<'_>>,
) -> String {
    let location = location.map_or_else(|| "unknown location".to_owned(), ToString::to_string);
    format!(
        "{time} {app} {version} on thread {:?}, panic at {location} (payload omitted)\n",
        thread.unwrap_or("unnamed"),
    )
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
    fn an_unwritable_log_file_is_reported_not_fatal() {
        let path = std::env::temp_dir()
            .join(format!("fastframe-log-missing-{}", std::process::id()))
            .join("no-such-dir")
            .join("app.log");
        let (_, error) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        assert_eq!(error.map(|(failed, _)| failed), Some(path));
    }

    #[test]
    fn the_panic_line_names_the_app_thread_and_place_but_no_payload() {
        let location = std::panic::Location::caller();
        let entry = panic_entry(
            "2026-09-25T10:00:00Z",
            "zapfast",
            "0.16.3",
            Some("main"),
            Some(location),
        );
        assert_eq!(
            entry,
            format!(
                "2026-09-25T10:00:00Z zapfast 0.16.3 on thread \"main\", panic at {location} (payload omitted)\n"
            )
        );
        let unnamed = panic_entry("t", "spotifast", "0.10.1", None, None);
        assert_eq!(
            unnamed,
            "t spotifast 0.10.1 on thread \"unnamed\", panic at unknown location (payload omitted)\n"
        );
    }

    /// The real hook, end to end: the file gets the line, never the payload.
    #[test]
    fn a_panic_is_recorded_without_its_payload() {
        let dir = std::env::temp_dir().join(format!("fastframe-log-panic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("panic.log");
        let previous = std::panic::take_hook();
        log_panics(&path, "zapfast", "0.16.3");
        let result = std::thread::Builder::new()
            .name("worker".into())
            .spawn(|| panic!("secret fixture payload 123456789"))
            .unwrap()
            .join();
        std::panic::set_hook(previous);
        assert!(result.is_err());
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(written.contains("zapfast 0.16.3 on thread \"worker\", panic at "));
        assert!(written.ends_with("(payload omitted)\n"));
        assert!(!written.contains("secret fixture payload"));
        assert!(!written.contains("123456789"));
    }
}
