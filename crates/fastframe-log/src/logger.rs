//! The `log` logger: stderr plus a per-run file, with redaction.

use std::borrow::Cow;
use std::fmt::Display;
use std::io::Write;
use std::path::PathBuf;

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

    /// Also writes every line to `path`, replacing what an earlier run left
    /// there. If the file cannot be created, logging continues on stderr and
    /// says why.
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
    fn an_unwritable_log_file_is_reported_not_fatal() {
        let path = std::env::temp_dir()
            .join(format!("fastframe-log-missing-{}", std::process::id()))
            .join("no-such-dir")
            .join("app.log");
        let (_, error) = Logging::new("zapfast", "1.0.0").file(&path).build(None);
        assert_eq!(error.map(|(failed, _)| failed), Some(path));
    }
}
