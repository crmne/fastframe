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

Not moving the logger now. RekordFlash logs through `tracing-subscriber`
(an `EnvFilter` in `main.rs`, stderr only) and records panics as structured
session evidence (`src/diagnostics/sessions.rs`), which chains the previous
hook and already never captures the payload. It can take the facade-free
parts with `default-features = false`: `redact::links`/`words` for error
strings, and, if it wants a human-readable panic line next to its session
records, `log_panics` installed before `Session::install_panic_hook` (so the
session hook chains it). Its logger stays as it is; the crate has no
`tracing` subscriber until a second app logs through `tracing`.

TonePush uses `eprintln!`. Chat with Work logs to stderr only and reads
`CWW_APP_LOG`, not `RUST_LOG`; it can adopt the crate once it wants a log
file, which would change its variable.

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
| `src/main.rs`: `zapfast::tray::idle(..)` | 1 | `.idle(fastframe_tray::idle)` on the shell (see fastframe-shell) |
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
| `src/entrypoint.rs`: `spotifast::tray::idle(..)` | 1 | `.idle(fastframe_tray::idle)` |
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

## fastframe-shell

The keep-running loop around `eframe::run_native`, the `Waker`, start
hidden, and off-screen window recovery. Each app keeps its `eframe::App`
wrapper (demo shots, tours, thumb bar, menus), its native options, and its
decisions about when to hide, show, reopen and quit. Single instance stays in
each app (see the crate README for why).

### ZapFast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/backend.rs`: `struct Waker` and its impl | 26 | `pub use fastframe_shell::Waker;` (same methods, including `wake_after`) |
| `src/main.rs`: `slot`, the `start_hidden` computation's `hides_to_tray` part, the outer `loop`, the headless inner `loop`, the final `shutdown` | 100 | `impl fastframe_shell::Resident for App` (below), then `Shell::new(app, &waker).start_hidden(cli.start_hidden && !demo && update_receipt.is_none()).idle(fastframe_tray::idle).run(\|lease\| eframe::run_native("ZapFast", native_options(..), Box::new(move \|cc\| { let mut app = lease.take(&cc.egui_ctx); app.attach(&cc.egui_ctx); ... Ok(Box::new(Shell { app, .. })) })))` |
| `src/main.rs`: `creator_waker.attach(..)`, the `creator_slot` take, `waker.detach()` | 10 | `lease.take(&cc.egui_ctx)` and the shell |
| `src/main.rs` `Shell`: `app: Option<App>`, `slot`, `impl Drop for Shell` | 8 | `app: fastframe_shell::Held<App>`; `self.app.as_mut()` becomes `&mut self.app` |
| `src/main.rs`: `recovered_window_position`, `recover_offscreen_window` | 80 | `fastframe_shell::window::recover_offscreen(ctx, frame)` behind the existing `window_recovery_checked` flag (drop the `cfg`: the crate does nothing on macOS) |
| `src/main.rs`: `mod window_tests` | 70 | the same cases are in the crate |
| `src/app.rs`: `hide_intent`/`quit_requested`/`wants_show` reads in `main` | 0 | the `Resident` impl |

The `Resident` impl (about 25 lines in `app.rs`):

```rust
impl fastframe_shell::Resident for App {
    fn closed(&self) -> Closed {
        if !self.quit_requested && self.hide_intent { Closed::Hide } else { Closed::Quit }
    }
    fn window_gone(&mut self) { App::window_gone(self) }
    fn headless_frame(&mut self, ctx: &egui::Context) -> Headless {
        self.background_frame(ctx);
        if self.quit_requested { Headless::Quit }
        else if self.wants_show { Headless::Show }
        else { Headless::Wait }
    }
    fn start_hidden(&mut self) -> bool {
        if !self.hides_to_tray() { return false; }
        App::start_hidden(self); // sets hide_intent, releases the backend
        true
    }
    fn shutdown(&mut self) { App::shutdown(self) }
}
```

About 270 lines out, 25 in. Keep `a_hidden_start_starts_the_backend_without_a_frame`
in `app.rs`: the crate tests that the shell calls `start_hidden` before any
window, the app test that it releases the backend.

No behaviour changes. Moving the hide-to-tray check into `start_hidden`
keeps the rule "without a tray there is no way back, so show the window".

### Spotifast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/backend.rs`: `struct Waker` and its impl | 24 | `pub use fastframe_shell::Waker;` |
| `src/entrypoint.rs`: `slot`, the outer `loop`'s bookkeeping (`creator_slot`, `waker.detach()`, the `switch`/`hide` reads, the headless inner `loop`, the final `shutdown`) | 70 | `impl Resident for App` with `closed()` returning `Closed::Reopen` for `switch_intent`, then `Shell::new(app, &waker).idle(fastframe_tray::idle).run(\|lease\| { let mini = ...; let options = ...; eframe::run_native("Spotifast", options, Box::new(move \|cc\| { ...; let mut app = lease.take(&cc.egui_ctx); ... })) })` |
| `src/entrypoint.rs` `Shell`: `app: Option<App>`, `slot`, the slot line in `Drop` | 4 | `app: Held<App>`; `Drop` keeps only `thumbbar.detach()` (the app returns after it, when the field drops) |

About 95 lines out, 25 in. The per-window option building (`MiniWindow`,
`native_options`, `profile_options`, the thumb bar) moves into the `run`
closure unchanged, since the closure runs once per window.

Off-screen recovery: Spotifast checks its own saved session positions before
applying them (`window::can_restore`, the Windows work-area test of the
title bar), which stays. It can add `recover_offscreen` on the main window's
first frame as a net for positions eframe restored itself; that is new
behaviour on X11 and Windows, so it is optional.

No other behaviour changes.

### RekordFlash, TonePush, Chat with Work

Not moving: RekordFlash and TonePush have no tray or background mode. Chat
with Work keeps its window alive with `pump_app_events` on one winit loop, a
different design.

## fastframe-macos

Traffic lights in the app's own title bar, the inset that clears them, and
the title-bar double-click setting. Each app keeps its header heights, its
double-click handling (which viewport command or AppKit selector it sends),
and its application menus.

### ZapFast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/macos.rs`: `update_window` | 55 | `fastframe_macos::align_traffic_lights(frame, ctx, if linked { 60.0 } else { 28.0 / ctx.zoom_factor() })` in `Shell::logic` (the `cfg` can go) |
| `src/theme.rs` `traffic_light_inset`: the `84.0 / ctx.zoom_factor()` arm | 3 | `fastframe_macos::traffic_light_inset(ctx)` when not previewing; keep the `macos_chrome(ctx)` check for the demo's macOS preview on other platforms, using `fastframe_macos::TRAFFIC_LIGHTS_WIDTH / ctx.zoom_factor()` there |
| `Cargo.toml`: `NSButton`, `NSControl`, `NSView`, `NSWindow` features of `objc2-app-kit` | 0 | still needed by the rest of `macos.rs`; leave them |

About 55 lines. The menu code in `macos.rs` stays. No behaviour change.

### RekordFlash

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/ui/macos_chrome.rs` | 112 | `fastframe_macos` |
| `src/ui.rs`: `macos_chrome::update(frame, ui.ctx(), TITLE_BAR_HEIGHT)` | 1 | `fastframe_macos::align_traffic_lights(frame, ui.ctx(), TITLE_BAR_HEIGHT)` |
| `src/ui.rs` `title_bar_leading_inset`: the macOS arm | 5 | `fastframe_macos::traffic_light_inset(context)` (zero in full screen; keep the 12 fallback) |
| `src/ui.rs` `title_bar_drag`: `macos_chrome::double_click_action()` and `DoubleClick` | 0 | `fastframe_macos::double_click_action()`, with `DoubleClick::Zoom \| DoubleClick::Fill` on the zoom arm |
| `Cargo.toml`: the macOS `objc2-app-kit` 0.2 and `raw-window-handle` entries used only by `macos_chrome.rs` | 2 | come with the crate (objc2-app-kit 0.3, as ZapFast and Spotifast use) |

About 115 lines. Differences: a double-click setting the crate does not know
(a future macOS value) now does nothing, where RekordFlash zoomed.

### Spotifast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/window.rs`: `MacosDoubleClickAction`, `macos_double_click_action` and their test | 30 | `fastframe_macos::double_click_action()` inside `macos_titlebar_should_drag`: `Minimize` to `performMiniaturize:`, `Zoom` to `performZoom:`, `Fill` and `Nothing` ignored as now |
| `src/window.rs`: the `NSUserDefaults` read | 3 | inside the crate |

About 35 lines. Difference: Spotifast now also honours the older
`AppleMiniaturizeOnDoubleClick` switch when the newer key is unset.
Spotifast keeps AppKit's own traffic-light positions (its top bar is laid
out around them), so `align_traffic_lights` is not needed there.

## fastframe-i18n

The PO compiler, the lookup functions, language tag parsing, and the
template update script. Each app keeps its `Locale` enum, its tags, native
names, language picker, `settings.json` format, and its own phrase helpers
(Spotifast's `song_count` and friends).

### ZapFast

| Delete | Lines | Instead |
| --- | --- | --- |
| `build_support/catalogs.rs` | 45 | `fastframe_i18n::build::compile` (used by the next row) |
| `build.rs`: the `catalogs` module, the two `rerun-if-changed` lines, and the loop over `assets/i18n` that writes `catalogs.rs` | 30 | `fastframe_i18n::build::compile_catalogs("assets/i18n");` at the top of `main` |
| `Cargo.toml` `[build-dependencies]` `include-po`, `polib` | 4 | `fastframe-i18n = { ..., features = ["build"] }` as a build-dependency |
| `Cargo.toml` `tr` | 1 | nothing: generated catalogs implement `fastframe_i18n::Translator` |
| `src/i18n.rs`: `Locale::translator`, `gettext`, `pgettext`, `ngettext` | 45 | `impl fastframe_i18n::Locale for Locale { fn catalog(self) ... }` (the old `translator` body) and `pub use fastframe_i18n::{gettext, ngettext, pgettext};` |
| `src/i18n.rs`: the subtag split in `Locale::from_system` | 6 | `fastframe_i18n::LanguageTag::parse`; match on `tag.language` |
| `src/i18n.rs`: `sys_locale::get_locale()` in `detect` | 4 | `fastframe_i18n::detect(from_tag)` (keep the `cfg!(test)` guard in the app). This walks every preferred language, not only the first: a desktop listing Norwegian then German now gets German. |
| `Cargo.toml` `sys-locale` | 1 | comes with the crate |
| `.github/scripts/update-translations.sh` | 32 | a wrapper calling the crate's `scripts/update-translations.sh --package ZapFast --domain zapfast --bugs '<translation issue URL>' --keyword translated:2 --fuzzy-matching "$@"` (see the crate README for finding the script). Drop `--fuzzy-matching` to adopt Spotifast's choice. |

About 130 lines. Call sites (`crate::i18n::gettext(app.locale, ..)`) stay as
they are because `crate::i18n` re-exports the functions. The tests that
check real catalogs (`german_catalog_translates_the_pilot` and the plural
tests) stay in the app.

### Spotifast

| Delete | Lines | Instead |
| --- | --- | --- |
| `build_support/catalogs.rs` | 45 | `fastframe_i18n::build::compile` |
| `build.rs`: the `catalogs` module, its `rerun-if-changed` lines, the loop over `assets/i18n`, and the `tests/fixtures/translation.po` compile with `test_catalog.rs` | 45 | `fastframe_i18n::build::compile_catalogs("assets/i18n");` |
| `tests/localization.rs`: `compiled_po_omits_unfinished_messages_and_uses_locale_plural_rules`, and `tests/fixtures/translation.po` | 30 + 32 | covered by `fastframe-i18n` (`build` tests and the `tests/i18n-app` fixture). Keep `catalogs_cover_the_template_and_preserve_named_placeholders` (it needs `polib` as a dev-dependency still). |
| `Cargo.toml` `[build-dependencies]` `polib`, `include-po`; `tr` | 5 | the crate, as above |
| `src/i18n.rs`: `Locale::translator`, `gettext`, `pgettext`, `ngettext` | 50 | `impl fastframe_i18n::Locale for Locale` and `pub use fastframe_i18n::{gettext, ngettext, pgettext};` |
| `src/i18n.rs`: the tag splitting at the top of `from_language_tag`, and `from_preferred` | 20 | `LanguageTag::parse(tag)` then the existing `match` on `language`, `script`, `region`; `fastframe_i18n::first_supported(tags, ..)` |
| `src/i18n.rs`: the `OnceLock` and `sys_locale::get_locales()` in `from_system` | 8 | `fastframe_i18n::detect(..)` (cached per process in the crate) |
| `.github/scripts/update-translations.sh` | 33 | a wrapper: `--package Spotifast --domain spotifast --bugs '<translation issue URL>'` (no fuzzy matching is the default) |

About 200 lines. `Locale::from_tag` keeps using `clap::ValueEnum`.

### RekordFlash, TonePush, Chat with Work

No translations yet. Start with the crate when they add one.
