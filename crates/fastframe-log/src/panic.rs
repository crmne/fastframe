//! The panic hook. Uses only the standard library, so it works whatever
//! logging facade the app uses.

use std::fmt::Display;
use std::io::Write;
use std::path::Path;

/// What a panic line says about the panic's message (its payload).
#[derive(Clone, Copy, Debug)]
pub enum PanicMessage {
    /// Leave the message out: the line ends in `(payload omitted)`. For apps
    /// whose data must never reach a log, such as a messenger's chats.
    Omit,
    /// Keep the message, passed through the app's redaction first (for
    /// example [`crate::redact::links`], or [`crate::redact::words`] with the
    /// app's private words) and collapsed to one line.
    Redacted(fn(&str) -> String),
}

/// Records every panic in `path` before the process dies of it, without its
/// message. Same as [`log_panics_with`] and [`PanicMessage::Omit`].
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
    log_panics_with(path, app, version, PanicMessage::Omit);
}

/// Records every panic in `path`, with its message as `message` says.
///
/// This replaces the default hook, which would print the raw payload to
/// stderr; with [`PanicMessage::Redacted`] only the redacted message is
/// printed or written.
pub fn log_panics_with(
    path: impl AsRef<Path>,
    app: &'static str,
    version: &'static str,
    message: PanicMessage,
) {
    let path = path.as_ref().to_path_buf();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let detail = match message {
            PanicMessage::Omit => None,
            PanicMessage::Redacted(redact) => Some(one_line(&redact(payload_text(info.payload())))),
        };
        let entry = panic_entry(
            jiff::Timestamp::now(),
            app,
            version,
            thread.name(),
            info.location(),
            detail.as_deref(),
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

/// The panic's message when it is text, as `panic!` and `expect` produce.
fn payload_text(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(non-text payload)")
}

/// Keeps a log entry on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// The panic log line. Takes only an already redacted message, never the
/// raw payload, so it cannot leak one.
fn panic_entry(
    time: impl Display,
    app: &str,
    version: &str,
    thread: Option<&str>,
    location: Option<&std::panic::Location<'_>>,
    redacted: Option<&str>,
) -> String {
    let location = location.map_or_else(|| "unknown location".to_owned(), ToString::to_string);
    let message = redacted.map_or_else(
        || "(payload omitted)".to_owned(),
        |text| format!(": {text}"),
    );
    let separator = if redacted.is_some() { "" } else { " " };
    format!(
        "{time} {app} {version} on thread {:?}, panic at {location}{separator}{message}\n",
        thread.unwrap_or("unnamed"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panic_line_names_the_app_thread_and_place_but_no_payload() {
        let location = std::panic::Location::caller();
        let entry = panic_entry(
            "2026-09-25T10:00:00Z",
            "zapfast",
            "0.16.3",
            Some("main"),
            Some(location),
            None,
        );
        assert_eq!(
            entry,
            format!(
                "2026-09-25T10:00:00Z zapfast 0.16.3 on thread \"main\", panic at {location} (payload omitted)\n"
            )
        );
        let unnamed = panic_entry("t", "spotifast", "0.10.1", None, None, None);
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

        // Same test, not a second one: the hook is process-wide, and two
        // tests swapping it in parallel would race.
        std::fs::create_dir_all(dir_redacted()).unwrap();
        let path = dir_redacted().join("panic.log");
        let previous = std::panic::take_hook();
        log_panics_with(
            &path,
            "spotifast",
            "0.10.2",
            PanicMessage::Redacted(crate::redact::links),
        );
        let result = std::thread::Builder::new()
            .name("player".into())
            .spawn(|| panic!("track failed at https://audio.example/secret?token=abc\nsecond line"))
            .unwrap()
            .join();
        std::panic::set_hook(previous);
        assert!(result.is_err());
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(dir_redacted()).unwrap();
        assert_eq!(written.lines().count(), 1, "{written}");
        assert!(written.contains("spotifast 0.10.2 on thread \"player\", panic at "));
        assert!(written.contains(": track failed at "), "{written}");
        assert!(written.contains("second line"), "{written}");
        assert!(!written.contains("token=abc"), "{written}");
        assert!(!written.contains("(payload omitted)"));
    }

    fn dir_redacted() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fastframe-log-redacted-{}", std::process::id()))
    }

    #[test]
    fn a_redacted_message_follows_the_place_on_the_same_line() {
        let entry = panic_entry("t", "spotifast", "0.10.2", Some("main"), None, Some("boom"));
        assert_eq!(
            entry,
            "t spotifast 0.10.2 on thread \"main\", panic at unknown location: boom\n"
        );
        assert_eq!(one_line(" a\n b\t c "), "a b c");
        assert_eq!(payload_text(&"text"), "text");
        assert_eq!(payload_text(&String::from("owned")), "owned");
        assert_eq!(payload_text(&7_u8), "(non-text payload)");
    }
}
