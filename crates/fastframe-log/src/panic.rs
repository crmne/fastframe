//! The panic hook. Uses only the standard library, so it works whatever
//! logging facade the app uses.

use std::fmt::Display;
use std::io::Write;
use std::path::Path;

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
