# fastframe-log

Log to stderr and a file for bug reports, keep private data out, and record
panics without their payload.

A desktop app launched from a menu has no terminal: the log file is what
people attach to bug reports, so it must never hold message contents, phone
numbers, keys, tokens, or pairing payloads. This is the setup ZapFast and
Spotifast share.

## Usage

```rust
fastframe_log::Logging::new("zapfast", env!("CARGO_PKG_VERSION"))
    // Used when RUST_LOG is unset.
    .filter(if verbose { "info,zapfast=debug" } else { "warn,zapfast=info" })
    // Created fresh each run. Leave it out for demo runs (stderr only).
    .file(dirs.log_file())
    // One line per panic, appended.
    .panic_log(dirs.panic_log())
    // Optional: rewrite the lines of chatty or private targets.
    .redact(|record, _message| {
        record
            .target()
            .starts_with("whatsapp_rust")
            .then_some("protocol diagnostic (private details omitted)".into())
    })
    .init()?;
```

`init` logs `Starting zapfast 0.16.3 on linux (x86_64)` once the logger is
up, and warns (on stderr) if the log file could not be created. The app's
name and version are passed in; this crate's version is not the app's.

Lines look like `[2026-09-25T10:00:00Z WARN zapfast::backend] message`.

## Panics

`log_panics` (called by `init` when `.panic_log` is set) replaces the default
panic hook. Each panic appends one line to the panic log and prints it to
stderr:

```text
2026-09-25T10:00:00Z zapfast 0.16.3 on thread "main", panic at src/app.rs:10:5 (payload omitted)
```

The payload is never written anywhere: an `expect` or a formatted panic
message can quote whatever the code was handling. The source location is
enough to find the panic.

## Redaction

For single messages, before they are logged:

```rust
use fastframe_log::redact;

// Error strings from HTTP clients quote URLs with their tokens.
log::warn!("sticker download failed: {}", redact::links(&error.to_string()));

// Add the shapes an app knows are private.
let cdn = |word: &str| redact::is_link(word) || word.contains("oh=");
log::warn!("{}", redact::words(&error, cdn));
```

For a whole dependency whose messages can carry private data, pass a
`Redactor` to `.redact` and summarise its lines into fixed categories.

Redaction is a net, not the rule: the rule is not to format private values
into log lines at levels that ship.
