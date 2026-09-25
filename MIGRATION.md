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

## fastframe-tray

The tray item on all three platforms, with the menu supplied by the app.
Each app keeps its menu labels (and translations), its icon drawing
(`util::app_icon_rgba`, `util::tray_template_rgba`), and the mapping from
tray events to its own actions.

### ZapFast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/tray.rs` | 120 | `fastframe_tray` |
| `src/tray_native.rs` | 400 | `fastframe_tray` |
| `src/lib.rs`: the two `cfg`'d `pub mod tray` declarations | 5 | nothing |
| `src/app.rs`: `TrayService::spawn(move \|\| waker.wake())` | 1 | `fastframe_tray::Tray::spawn(Config { id: "zapfast", title: "ZapFast".into(), icon: util::app_icon_rgba, template_icon: Some(util::tray_template_rgba), menu: vec![MenuItem::action("show", "Show or hide ZapFast"), MenuItem::Separator, MenuItem::action("quit", "Quit")] }, move \|\| waker.wake())` |
| `src/app.rs` `handle_tray`: the `TrayCommand` match | 10 | `for event in tray.events()`: `Event::Show` to `ShowWindow`; `Event::Toggle` and `Event::Menu("show")` to show or hide; `Event::Menu("quit")` to `Quit` |
| `src/app.rs`: `tray.hidden()` in `window_gone` | 3 | nothing (it did nothing) |
| `src/main.rs`: `zapfast::tray::idle(..)` | 1 | `fastframe_tray::idle(..)` |
| `src/macos.rs` `attach`: the menu handler | 0 | call `if fastframe_tray::claim_menu_event(&event.id.0) { return; }` first in the `MenuEvent` handler |
| `src/macos.rs` `action`: the `"show"` arm shared with the tray | 7 | nothing: the tray's ids are its own (`fastframe-tray:show`) and reach `tray.events()`. Keep `"quit"` for the app menu's Quit. |
| `Cargo.toml` `ksni`, `tray-icon` | 2 | come with the crate. `tray-icon` stays if `macos.rs` keeps building the menu bar with muda (it does). |

About 540 lines. The `dock_reopen_requests_a_window_only_when_none_is_visible`
test moves with the code (the crate's macOS test covers the new behaviour).

Differences, all taken from Spotifast on purpose:

- **Windows**: a left click on the icon now always shows the window
  (`Event::Show`) and no longer opens the menu; the menu opens on right
  click (Spotifast #310, 5eb054e).
- **macOS**: a Dock click now brings back a minimized window too, and wakes
  the app so the request is read (Spotifast ec75951). ZapFast only asked
  when AppKit reported no visible window, which a minimized window is not.
- **Flatpak**: the item registers the unique bus name (Spotifast 1c03c21).
- On Windows, dropping the tray now ends its thread and removes the icon
  (before, it lived until the process ended).

### Spotifast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/tray.rs` | 281 | `fastframe_tray` |
| `src/tray_native.rs` | 507 | `fastframe_tray` |
| `src/lib.rs`: the `pub mod tray` declarations | 5 | nothing |
| `src/app.rs`: `TrayService::spawn(..)` | 1 | `Tray::spawn(Config { id: "spotifast", title: "Spotifast".into(), icon: util::app_icon_rgba, template_icon: Some(util::tray_template_rgba), menu: vec![action("show", ..), Separator, action("play-pause", "Play"), action("next", "Next"), action("previous", "Previous"), Separator, action("quit", "Quit")] }, ..)` |
| `src/app.rs`: the `TrayCommand` match | 12 | match `Event::Toggle`/`Menu("show")`, `Show`, `Menu("play-pause")`, `Menu("next")`, `Menu("previous")`, `Menu("quit")` |
| `src/app.rs`: `tray.set_playing(playing)` | 1 | `tray.set_label("play-pause", if playing { "Pause" } else { "Play" })` (the crate does not skip repeats; keep the app's `playing` comparison, or call only on change) |
| `src/app.rs`: `tray.hidden()` | 3 | nothing |
| `src/entrypoint.rs`: `spotifast::tray::idle(..)` | 1 | `fastframe_tray::idle(..)` |
| `Cargo.toml` `ksni`, `tray-icon` | 2 | come with the crate |

About 800 lines, of which 140 are the Flatpak private-bus test in
`tray.rs`. fastframe tests do not start `dbus-daemon`, so that test is not
in the crate; to keep it, rewrite it in Spotifast's `tests/` against
`fastframe_tray::Tray::spawn` and `events()`, without the "previous
registration method must fail" half (which needs ksni's own builder).

Differences:

- **macOS: the headless pump now runs `-[NSApplication run]` in slices**
  (ZapFast 655509b). Spotifast's `nextEventMatchingMask:`/`sendEvent:` loop
  can let an Objective-C exception unwind into Rust and abort (ZapFast
  #199). Media keys and the menu bar keep working, since AppKit's own loop
  serves them.
- The labels are the app's; Spotifast's untranslated `"Play"`, `"Next"`...
  can now go through its gettext.

### Chat with Work

Not moving now: its tray shares one winit loop with the window
(`pump_app_events`) and uses tray-icon 0.25. It can adopt the crate if it
moves to the run-native loop, after the workspace moves to tray-icon 0.25.
