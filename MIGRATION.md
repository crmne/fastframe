# Moving apps onto fastframe

One section per crate: what each app deletes, and what it calls instead.
Each move is its own change in the app's repository, made after the crate
lands here. Paths are relative to the app's repository root; line counts are
estimates taken when the crate was extracted.

Depend on a pinned revision:

```toml
fastframe-<name> = { git = "https://github.com/crmne/fastframe", rev = "<commit>" }
```

Keep behaviour: each section lists where the crate differs from what an app
did, so the app either accepts the change on purpose or keeps its own code
for that part.

## fastframe-log

The logger (stderr plus a per-run file), the panic hook, and redaction
helpers. Each app keeps its default filter strings (and their tests), its
paths (`log_file`, `panic_log`), and its own redactors.

### ZapFast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/main.rs`: the `env_logger::Builder` block, the `File::create`/`Tee` target, and the `logger.format(..)` closure | 33 | `fastframe_log::Logging::new("zapfast", env!("CARGO_PKG_VERSION")).filter(default_log_filter(cli.verbose))`, then `.file(dirs.log_file())` unless `demo`, `.panic_log(dirs.panic_log())`, `.redact(redact_protocol)`, `.init()` |
| `src/main.rs`: `struct Tee` and its `Write` impl | 15 | inside the crate |
| `src/main.rs`: `fn log_panics` | 25 | `.panic_log(..)` above (same line format, payload still omitted) |
| `src/backend/worker/stickers.rs`: `fn redacted` | 14 | `fastframe_log::redact::words(&error, \|w\| redact::is_link(w) \|\| w.contains("/v/") \|\| w.contains("oh="))`; keep its test against the crate call |
| `Cargo.toml` `env_logger` | 1 | comes with the crate (`log` stays) |

Keep `src/diagnostics.rs`; add a three-line adapter for `.redact`:

```rust
fn redact_protocol(record: &log::Record<'_>, message: &str) -> Option<Cow<'static, str>> {
    (is_protocol_target(record.target())
        || is_protocol_target(record.module_path().unwrap_or_default()))
    .then(|| protocol_summary(message).into())
}
```

About 90 lines. Keep `default_log_filter` and its two tests.

Differences: `init` also logs `Starting zapfast <version> on <os> (<arch>)`
at info, which the file keeps at the default filter. A log file that cannot
be created is reported through the logger (on stderr) instead of
`eprintln!`.

### Spotifast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/entrypoint.rs`: the `env_logger::Builder` block, the log-file `match`, `logger.init()`, and the `Starting Spotifast ...` line | 20 | `fastframe_log::Logging::new("spotifast", env!("CARGO_PKG_VERSION")).filter(default_filter).file(dirs.log_file()).panic_log(dirs.panic_log()).init()` |
| `src/entrypoint.rs`: `struct Tee` and its `Write` impl | 15 | inside the crate |
| `src/entrypoint.rs`: `fn log_panics` | 25 | `.panic_log(..)` |
| `Cargo.toml` `env_logger` | 1 | comes with the crate |

About 60 lines. `http.rs` keeps `error.without_url()` (reqwest does it
better on its own errors); `redact::links` is for errors that are already
strings.

Differences, all deliberate:

- **The panic log no longer records the payload**, and the default hook is no
  longer chained, so the payload is not printed to stderr either. Spotifast
  wrote `{info}` (payload included) to `panic.log`. A payload can quote the
  data being handled; the location is kept.
- Log lines use `[time LEVEL target] message` with the level unpadded
  (env_logger's default pads it and colours it on a terminal).
- The start line says `spotifast` (the name passed to `Logging::new`, which
  the panic line also uses) where it said `Spotifast`.

### RekordFlash, TonePush, Chat with Work

Not moving now. RekordFlash logs through `tracing-subscriber`. TonePush uses
`eprintln!`. Chat with Work logs to stderr only and reads `CWW_APP_LOG`, not
`RUST_LOG`; it can adopt the crate once it wants a log file, which would
change its variable.
