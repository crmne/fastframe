# Moving apps onto fastframe

An index by app, then one section per crate: what each app deletes, and
what it calls instead. Each move is its own change in the app's repository,
made after the crate lands here. Paths are relative to the app's repository root; line counts are
estimates taken when the crate was extracted.

Depend on a pinned revision:

```toml
fastframe-<name> = { git = "https://github.com/crmne/fastframe", rev = "<commit>" }
```

Keep behaviour: each section lists where the crate differs from what an app
did, so the app either accepts the change on purpose or keeps its own code
for that part.

## Before you start

fastframe needs Rust 1.98 (`rust-version` in every crate). An app pinned to
an older toolchain in `rust-toolchain.toml` fails to resolve on macOS and
Windows with "rustc 1.97.1 is not supported by fastframe-*"; bump the app's
toolchain and `rust-version` to 1.98 first. Keep the app's `[patch.crates-io]`
block for the egui and winit forks: patches only apply at the app's root.

## By app

Which crates each app adopts, and which it leaves for later. Each crate's
section below has a subsection per app with the details.

| App | Adopts | Not now |
| --- | --- | --- |
| ZapFast | [text](#fastframe-text), [fonts](#fastframe-fonts), [icons](#fastframe-icons), [theme](#fastframe-theme), [i18n](#fastframe-i18n), [log](#fastframe-log), [tray](#fastframe-tray), [shell](#fastframe-shell), [macos](#fastframe-macos), [update](#fastframe-update) | |
| Spotifast | [text](#fastframe-text), [fonts](#fastframe-fonts), [icons](#fastframe-icons), [theme](#fastframe-theme), [i18n](#fastframe-i18n), [log](#fastframe-log), [tray](#fastframe-tray), [shell](#fastframe-shell), [macos](#fastframe-macos) (the double-click setting only), [update](#fastframe-update) | |
| RekordFlash | [text](#fastframe-text), [fonts](#fastframe-fonts), [icons](#fastframe-icons), [macos](#fastframe-macos) (done in b5985bb); [log](#fastframe-log) (only the facade-free redaction and panic line, with `default-features = false`) later | [theme](#fastframe-theme) and [i18n](#fastframe-i18n) until it adds custom themes or translations; [shell](#fastframe-shell) (no tray or background mode); its `tracing` logger stays |
| TonePush | [text](#fastframe-text), [fonts](#fastframe-fonts), [update](#fastframe-update) (with `portable_executable`) | [icons](#fastframe-icons) later (its own macros and layout); [log](#fastframe-log) (it uses `eprintln!`); [shell](#fastframe-shell); [theme](#fastframe-theme) and [i18n](#fastframe-i18n) until it adds custom themes or translations |
| Chat with Work Local Agent | [text](#fastframe-text) | [fonts](#fastframe-fonts) (it draws with the platform's UI font); [tray](#fastframe-tray) and [shell](#fastframe-shell) (one winit loop with `pump_app_events`, tray-icon 0.25); [log](#fastframe-log) until it wants a log file; [theme](#fastframe-theme) and [i18n](#fastframe-i18n) until it adds custom themes or translations |

Shared widgets have not moved; see [Not moved yet: widgets](#not-moved-yet-widgets).

## fastframe-text

Follows the desktop's font rendering. Every app replaces its hand-set
hinting and `color_transfer_function` with it:

```rust
let rendering = fastframe_text::detect(); // once per process, outside a tokio runtime
rendering.apply_to(&mut fonts); // before `ctx.set_fonts`
ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals));
```

- Linux reads the desktop portal, then fontconfig. With the usual slight
  hinting this is egui's default hinting with sub-pixel positions, and
  linear coverage in both themes, which matches GTK's weight.
- macOS is unhinted and Windows slightly hinted; both keep egui's coverage
  per theme, so dark text there is a little heavier than on Linux.
- Apps that shift a galley by hand (ZapFast's selectable bubbles, Spotifast's
  right-to-left runs) round both ends with `snap_to_pixels`.
- The `watch` feature follows portal changes live; call `apply_to` and
  `ctx.set_fonts` again from its callback.

Adopted by all five apps: ZapFast, Spotifast, RekordFlash, TonePush and Chat
with Work (58da999, which applies it to the system UI font it uses).

## fastframe-fonts

Bundled Inter at the app's weights, the monospace choice, companions such
as a bundled emoji face, and the system fallback faces with their baseline
and Arabic adjustments. Each app keeps its sizes and text styles, its own
extra faces (ZapFast's colour emoji overlay, Spotifast's skin font and
playlist face, TonePush's Plex Mono file), and when it installs fonts.

Everywhere, the helpers become one-liners or go away:

```rust
pub fn regular(size: f32) -> egui::FontId { fastframe_fonts::Weight::Regular.font_id(size) }
pub fn medium(size: f32) -> egui::FontId { fastframe_fonts::Weight::Medium.font_id(size) }
pub fn semibold(size: f32) -> egui::FontId { fastframe_fonts::Weight::SemiBold.font_id(size) }
pub fn bold(size: f32) -> egui::FontId { fastframe_fonts::Weight::Bold.font_id(size) }
```

The family names `inter-medium`, `inter-semibold` and `inter-bold` and the
regular key `inter` are the ones ZapFast and Spotifast already use.

### ZapFast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/system_fonts.rs` | 462 | `fastframe_fonts::system::fallbacks()` (used by `FontSetup`) |
| `src/theme.rs` `install_fonts` | 52 | `FontSetup::default().install(ctx)`, or `.definitions()` then `ctx.set_fonts` if the app adjusts them (for example with fastframe-text) |
| `src/theme.rs` `INTER_MEDIUM`, `INTER_SEMIBOLD`, `INTER_BOLD` | 3 | `Weight::*.family()` / `Weight::*.name()` |
| `src/theme.rs` test `inter_figures_are_tabular` | 20 | the crate's `figures_are_tabular` |
| `assets/fonts/InterVariable.ttf`, `assets/fonts/README.md` | 2 files | `fastframe_fonts::INTER` and the crate's `fonts/README.md`. Keep `Inter-LICENSE.txt` if packaging ships it from there. |
| `Cargo.toml` `memmap2` | 1 | the crate (keep `skrifa`: `emoji.rs` and the demo use it) |

About 540 lines. Demo and screenshot builds call `.system_fallbacks(false)`
if they want output independent of the machine's fonts (the current code
always adds them).

Behaviour that changes, all from Spotifast's newer copy: fallbacks are moved
onto Inter's baseline (`y_offset_factor`); macOS asks CoreText instead of
scanning `/System/Library/Fonts` (which never finds PingFang); on Linux the
directories fontconfig's configuration names are probed too (NixOS); a
Japanese face no longer serves Han on a Chinese desktop when a Chinese one
exists; Windows reads its display language for the Han cut; Yi and the ★
probe are added. The ♡ probe ZapFast called `symbols` is now `suits`, so its
font data key becomes `fallback-suits` when a different face than the one for
★ covers it.

### Spotifast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/system_fonts.rs` except `pledit_face`, `find_family`, `walk_fonts` and `rank_family` | 800 | `fastframe_fonts::system::fallbacks()`; the playlist face walks `fastframe_fonts::system::font_directories()` |
| `src/mac_fonts.rs` | 485 | the crate's macOS module (the same CoreText code) |
| `src/theme.rs` `install_fonts` | 76 | `FontSetup::default().companion("noto_emoji", Arc::new(FontData::from_static(include_bytes!("../assets/fonts/NotoEmoji.ttf")))).definitions()`, then `ctx.set_fonts` |
| `src/theme.rs` `fallback_baseline_y_offset` and its three tests | 47 + 70 | the crate's `baseline_offset` and `fallback_glyphs_are_painted_on_the_latin_baseline` |
| `src/theme.rs` `INTER_*` constants, `inter_figures_are_tabular` | 25 | as for ZapFast |
| `assets/fonts/InterVariable.ttf`, `assets/fonts/README.md` | 2 files | the crate |
| `tests/fixtures/yi/` | 3 files | the crate's copy, used by `a_font_that_draws_a_yi_name_is_chosen` |

Keep `memmap2`: the playlist face lookup that stays in Spotifast maps font
files with it. About 1,500 lines. `milkdrop/overlay.rs` and `ui/winamp/pixel_text.rs` read
`fastframe_fonts::system::fallbacks()` instead; `Fallback` gains `scale` and
`y_offset_factor`, which they may ignore. Code that looked up
`system_fonts::FALLBACK_SCRIPTS` for a probe character uses the fallback's
own face instead (the list is internal to the crate now).

Behaviour that changes, from ZapFast's copy: a small Arabic face is enlarged
up to 25% to match Inter's x-height, and Javanese, mathematical
alphanumerics, enclosed alphanumerics and ♡ get fallbacks too.

### RekordFlash

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/theme.rs` `install_fonts` | 38 | `FontSetup::default().weights(&[Weight::SemiBold]).monospace(Monospace::Inter).install(context)` |
| `src/theme.rs` `SEMIBOLD` | 1 | `Weight::SemiBold.family()` in `semibold(size)` |
| `assets/fonts/InterVariable.ttf`, `assets/fonts/README.md` | 2 files | the crate |

About 40 lines. RekordFlash has no system fallbacks today: the default adds
them, so track names in CJK, Arabic or Indic scripts stop drawing as boxes.
Pass `.system_fallbacks(false)` to keep today's behaviour. The font data keys
change from `rekordflash-inter` to `inter`.

### TonePush

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/theme.rs` `fonts` (the loop and family set-up) | 45 | `FontSetup::default().weights(&[Weight::SemiBold]).monospace(Monospace::Font { name: "plex-mono".into(), data: Arc::new(FontData::from_static(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"))) }).install(ctx)` |
| `assets/fonts/Inter-Regular.ttf`, `assets/fonts/Inter-SemiBold.ttf` | 2 files | `fastframe_fonts::INTER` at 400 and 600 |

About 45 lines. The `SEMIBOLD` family name changes from `semibold` to
`inter-semibold` (use `Weight::SemiBold.family()`). Inter's figures become
tabular in labels too, and system fallbacks are added unless turned off.

### Chat with Work

It draws with the platform's UI font and bundles none; nothing to move.

## fastframe-icons

The `icons!` macro, the icon enum it now generates, the bytes loader, and the
43 Lucide files ZapFast and Spotifast ship byte for byte alike. Each app keeps
its own SVG files, its icon widgets (`icon`, `paint_icon`, `icon_button`),
and its sizes and tints.

Every app does the same:

1. Replace the `macro_rules! icons`, the `pub enum Icon { .. }` list, the
   `const ICONS: &[(Icon, &str, &[u8])] = icons! { .. }` table, `impl Icon`
   (`uri`, `image`) and `register_icons` with one block:

   ```rust
   fastframe_icons::icons! {
       pub enum Icon {
           prefix: "zapfast-icon-",          // the app's current bytes:// prefix
           directory: "../assets/icons/",
           Archive => "archive",             // the old table's entries, unchanged
           Check => lucide "check",          // a shared icon: see step 3
           // ...
       }
   }
   ```

2. Call `fastframe_icons::install::<Icon>(ctx)` where `register_icons(ctx)`
   was called (after `egui_extras::install_image_loaders`).
3. For each of the 43 shared names (`fastframe_icons::lucide::ALL`), write
   `lucide "name"` and delete `assets/icons/<name>.svg`. Keep
   `assets/icons/LICENSE.txt`: the app still ships its own Lucide files.
4. Code that iterated `ICONS` uses `Icon::ALL` with `icon.uri()` and
   `icon.bytes()`.

### ZapFast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/theme.rs` `macro_rules! icons` | 9 | the crate's `icons!` |
| `src/theme.rs` `pub enum Icon` (variant list) | 78 | generated by `icons!` |
| `src/theme.rs` `impl Icon { uri, image }` | 14 | generated |
| `src/theme.rs` `struct IconBytes` and its `BytesLoader` impl, `register_icons` | 38 | `fastframe_icons::install::<Icon>(ctx)` (the same never-forget loader) |
| `src/theme.rs` tests `every_icon_has_a_file`, `icon_bytes_outlive_forgetting` | 40 | covered by the crate's loader tests |
| `assets/icons/*.svg`: the 43 shared names | 43 files | `lucide "name"` |

About 180 lines of Rust and 43 files. The `ICONS` table becomes the
`icons!` block (the same length).

### Spotifast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/theme.rs` `macro_rules! icons` | 9 | the crate's `icons!` |
| `src/theme.rs` `pub enum Icon` | 92 | generated |
| `src/theme.rs` `impl Icon { uri, image }` | 14 | generated |
| `src/theme.rs` `register_icons` (`ctx.include_bytes` per icon) | 5 | `fastframe_icons::install::<Icon>(ctx)` |
| `assets/icons/*.svg`: the 43 shared names | 43 files | `lucide "name"` |

About 120 lines and 43 files. Behaviour change: icons are served by the
never-forget loader instead of `ctx.include_bytes`, which fixes the red
placeholder if Spotifast ever turns on `reduce_texture_memory`.

### RekordFlash

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/theme.rs` `macro_rules! icons` | 9 | the crate's `icons!` with `prefix: "rekordflash-"` |
| `src/theme.rs` `enum Icon` | 57 | generated (declare it `pub(crate) enum Icon` or keep it private, as now) |
| `src/theme.rs` `impl Icon { uri }`, `register_icons` | 13 | generated, `fastframe_icons::install::<Icon>(context)` |

About 80 lines. Its SVGs are minified `lucide-static` files that differ
from the shared ones byte for byte, so it keeps all of them. Its draw-time
helpers keep their shape: `icon_image(icon, color, size)` becomes
`icon.image(color, size)` (the same `egui::Image`, tinted and sized), and
`icon(ui, ..)`, `icon_button` and `transport_button` keep calling
`icon.uri()`.

### TonePush

Its UI and category icons use their own macros and file layout. It can move
to `icons!` later; nothing is shared with it yet.

## fastframe-theme

Palette files, the background catalogue, the shared palettes, Omarchy
following and file watching. Each app keeps its `Palette` struct and its
dark and light defaults, `theme::apply` (the mapping onto `egui::Visuals`
and widget styling), the Settings picker and its wording, the
`reload-themes` command on its single-instance channel, and the packaging
files in `contrib/omarchy/`.

Every app does the same:

1. `impl fastframe_theme::Palette for Palette`: `base` returns
   `Palette::dark()` or `Palette::light()`; `set` is the old `match name`
   from `parse_palette`, returning `false` for unknown names; ZapFast's
   `derive` holds its "Spotifast palettes share the sixteen interface
   colours" block.
2. `theme::custom::CustomTheme` becomes `fastframe_theme::CustomTheme<Palette>`
   and `theme::custom::Catalog` becomes `fastframe_theme::Catalog<Palette>`.
   The serialized cache (`custom_theme_cache`, `system_theme_cache`) keeps
   its shape, so existing `settings.json` files read as before; point
   `deserialize_with` at `"fastframe_theme::read_cached_theme"`.
3. `theme::custom::label` becomes `fastframe_theme::display_name`.
4. `Catalog::start(directory, selected, &waker)` takes a
   `fastframe_theme::Waker`: build one once with
   `fastframe_theme::Waker::new({ let waker = self.waker.clone(); move || waker.wake() })`.
5. The status line: `catalog.status(selected)` returns `Status::Loading`,
   `Status::SelectedUnavailable` or `Status::Problem(..)`; the app maps each
   to its (translated) sentence. The sentences both apps use today:
   "Loading local themes…", "The selected theme is unavailable. Keeping the
   last usable appearance. See the log for details.", and per `Problem`:
   `Unreadable` "The themes folder could not be read. See the log for
   details.", `TooManyEntries` "The themes folder has more than 512 entries.
   Keep fewer files there to list the custom palettes.", `TooManyThemes`
   "Only 128 custom palettes can be listed. Keep fewer JSON files in the
   themes folder to see the rest.", `LoaderFailed` "Custom themes could not
   be loaded. Run <slug> reload-themes to try again.", `OmarchyUnreadable`
   "The Omarchy palette could not be loaded. Keeping the last usable
   appearance. See the log for details."
6. Add a test that the shipped hook has not drifted:
   `assert_eq!(include_str!("../contrib/omarchy/<slug>-theme"), fastframe_theme::omarchy::hook_script("<slug>"));`

### ZapFast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/theme/custom.rs` | 794 | the crate; keep only the `Palette` impl (about 45 lines, in `theme.rs`) |
| `src/theme/omarchy.rs` | 390 | `fastframe_theme::omarchy` (setup, rendering, install), used by the catalogue |
| `src/theme/presets.rs` | 95 | `fastframe_theme::presets` (`presets::themes::<Palette>()` in the demo and in `bubble_text_is_readable_in_every_palette`). Keep `spotifast_palettes_also_colour_the_conversation` as an app test of `derive`. |
| `src/theme/watch.rs` | 79 | the crate's watcher |
| `assets/themes/*.json` | 8 files | `fastframe_theme::presets::FILES` |
| `tests/fixtures/omarchy/` | 5 files | the crate's fixtures (they render the base template; keep ZapFast's own if it wants a test of its extended template through `fastframe_theme::omarchy::render_seed::<Palette>`) |
| `Cargo.toml` `notify` | 1 | the crate (Linux only) |

About 1,300 lines, less the 45-line `Palette` impl and a 25-line status
mapping. In `App::new`: `app.custom_themes.enable_desktop_themes(DesktopThemes { slug: "zapfast", omarchy_template: include_str!("../contrib/omarchy/zapfast.json.tpl"), presets: true })`.
Behaviour is unchanged.

### Spotifast

| Delete | Lines | Instead |
| --- | --- | --- |
| `src/theme/custom.rs` | 684 | the crate; keep the `Palette` impl (about 30 lines) and `Problem::text` becomes the status mapping above |
| `src/theme/omarchy.rs` | 328 | `fastframe_theme::omarchy`, except the `fastpotify` hook upgrade and the legacy-profile check (see below) |
| `tests/fixtures/omarchy/` | 5 files | the crate's fixtures (identical) |

About 950 lines. `entrypoint.rs` calls
`app.custom_themes.enable_desktop_themes(DesktopThemes { slug: "spotifast", omarchy_template: include_str!("../contrib/omarchy/spotifast.json.tpl"), presets: false })`
(`presets: true` adds the eight shared palettes to its picker, which it does
not show today). Keep in the app, run before that call on Linux:

- the upgrade of an installed hook that still says `/fastpotify/themes`
  (the block at the top of `Setup::install`, about 20 lines), and
- skipping `enable_desktop_themes` when the themes directory is the legacy
  `fastpotify` one during an updater trial.

`tests/omarchy.rs` (which runs the shipped hook with bash) can stay, or give
way to the `hook_script` equality test.

Behaviour that changes, from ZapFast's newer copy: Omarchy is followed
whenever it is set up, not only when a package installed the assets, and
its palette is read from Omarchy's rendering (or rendered from its colours)
rather than from `themes/omarchy.json` alone; the themes folder and the
current Omarchy theme are watched, so palettes reload without the hook or a
restart.

### RekordFlash, TonePush, Chat with Work

RekordFlash and TonePush draw one fixed dark palette, and Chat with Work
follows the platform's light or dark setting; none reads palette files.
They can adopt the crate by implementing `Palette` when they add custom
themes or Omarchy support.

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
| `src/entrypoint.rs`: the `env_logger::Builder` block, the log-file `match`, `logger.init()`, and the `Starting Spotifast ...` line | 20 | `fastframe_log::Logging::new("spotifast", env!("CARGO_PKG_VERSION")).filter(default_filter).file(dirs.log_file()).panic_log(dirs.panic_log()).panic_message(PanicMessage::Redacted(fastframe_log::redact::links)).init()` |
| `src/entrypoint.rs`: `struct Tee` and its `Write` impl | 15 | inside the crate |
| `src/entrypoint.rs`: `fn log_panics` | 25 | `.panic_log(..)` with `.panic_message(..)` above |
| `Cargo.toml` `env_logger` | 1 | comes with the crate |

About 60 lines. `http.rs` keeps `error.without_url()` (reqwest does it
better on its own errors); `redact::links` is for errors that are already
strings.

Differences, all deliberate:

- **The panic log keeps the payload, with links removed**
  (`PanicMessage::Redacted(redact::links)`), on one line after the location.
  Spotifast wrote `{info}` with the raw payload; a payload can quote the data
  being handled, such as a URL with a token. The default hook is no longer
  chained, so the raw payload is not printed to stderr either. (ZapFast keeps
  `PanicMessage::Omit`, the default.)
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
(`pump_app_events`) and uses tray-icon 0.25. Its menu also needs what the
crate doesn't have yet: disabled status lines, a tooltip that changes, a
dimmed icon while paused, and an item that relabels itself. It can adopt the
crate once the crate grows those and moves to tray-icon 0.25, and if the app
moves to the run-native loop.

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

## fastframe-update

Self-update from GitHub releases. Each app keeps its `UpdateConfig` (names,
legacy names, macOS identity, publisher key), its HTTP client settings, and
its update interface. The move below targets ZapFast 0.16.x and Spotifast
0.10.x. Take the `reqwest` feature:

```toml
fastframe-update = { git = "https://github.com/crmne/fastframe", rev = "<commit>", features = ["reqwest"] }
```

Both apps already build reqwest 0.12 with `blocking`, so the `reqwest`
feature adds nothing to their trees; ring, sha2 0.11, url, serde and anyhow
are already there too.

The first release built on the crate is installed by the previous release's
own updater, which is unchanged. What that old helper needs from the new
binary stays the same: `--version` prints `<slug> <version>`, the new app
accepts `--update-receipt <job>` and `--update-error <message>` (now through
`intercept`), and it writes `started` next to the receipt. The crate's
`compat` tests cover the receipts of both apps' current releases.

### ZapFast

#### Delete

| File | Lines |
| --- | --- |
| `src/updates.rs` | 123 |
| `src/updates/install.rs` | 798 |
| `src/updates/macos.rs` | 440 |
| `src/updates/signing.rs` | 85 |
| `src/updates/transfer.rs` | 498 |
| **Total** | **1,944** |

The 19 updater tests in those files move into the crate. Keep
`assets/update-public-key.hex`, `packaging/zapfast-portable.txt`,
`packaging/windows/zapfast-installer.txt` and `packaging/UPDATE_SIGNING.md`
(point its verification paragraph at this crate).

#### Add

A new `src/updates.rs` of about 35 lines:

```rust
pub use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Installation, Kind, Prepared, Release, Source, Unsupported,
    Updater,
};
use fastframe_update::{MacConfig, ReqwestTransport, UpdateConfig};

pub const CONFIG: UpdateConfig = UpdateConfig {
    // Cask and bundle names from before the rename. This also accepts
    // fastsapp-* marker files and `fastsapp <version>` answers, which no
    // release produces.
    legacy_names: &["fastsapp"],
    macos: MacConfig {
        bundle_ids: &["me.paolino.fastsapp"],
        executable_names: &[],
        legacy_bundle_names: &["FastsApp.app"],
    },
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    ..UpdateConfig::new("crmne/zapfast", "ZapFast", "zapfast", env!("CARGO_PKG_VERSION"))
};

/// An updater on the proxy-aware reqwest client.
pub fn updater() -> anyhow::Result<Updater> {
    let mut builder = reqwest::blocking::Client::builder();
    if let Some(proxy) = crate::proxy::reqwest_proxy() {
        builder = builder.proxy(proxy);
    }
    Ok(Updater::new(CONFIG, ReqwestTransport::new(builder)?))
}

#[test]
fn update_config_is_valid() {
    CONFIG.validate().unwrap();
}
```

#### Replace

| Where | Today | With |
| --- | --- | --- |
| `main.rs` start | `if arguments.len() == 3 && arguments[1] == "--apply-update" { run_helper(..) }` | `let launch = fastframe_update::intercept(&updates::CONFIG);` |
| `main.rs` `Cli` | hidden `update_receipt` and `update_error` arguments; `Cli::parse()` | remove both; `Cli::parse_from(&launch.arguments)` |
| `main.rs` | `cli.update_error` toast | `launch.error` |
| `main.rs` | `update_receipt: Option<PathBuf>`, `update_receipt.is_none()` before starting hidden | `Option<fastframe_update::Receipt>`, `launch.receipt.is_none()` |
| `main.rs` `ui` | `install::acknowledge(&receipt)` | `receipt.acknowledge()` |
| `worker.rs` `InspectUpdate` | `install::detect()` | `updates::updater()?.installation()` (map the error with `to_string()`) |
| `worker.rs` `DownloadUpdate` | `updates::download(&release, &source, progress)` | `updater.with_source(source).download(&release, progress)` |
| `worker.rs` `InstallUpdate` | `install::handoff(&prepared, arguments)` | `updater.handoff(*prepared, arguments)` |
| `worker.rs` `CheckForUpdates` | `updates::newer_release()` over `proxy::agent()` | `updater.check()` |
| `app.rs`, `demo.rs` tests | `Prepared { installation, directory, payload, sha256, version }` | `Prepared::sample(installation, "99.0.0")` |
| `Source::GitHub` | enum variant | `Source::github()`; `source.is_github()` |

`Command::InstallUpdate` already moves the `Prepared` out of
`DownloadState::Ready` with `mem::replace`, which suits the non-`Clone`
`Prepared`.

Net: about 1,944 lines deleted and 60 added or changed, so roughly 1,880
fewer lines.

#### What changes for users

- The helper's standard error goes to `helper.log` in the staging folder (as
  Spotifast does), and `handoff` names that file when the helper dies.
- A Nix install says "Update this installation with Nix." instead of "with
  Nix or Homebrew".
- A handoff into a staging folder that already holds a marker is refused
  (none can today, since each download gets a new folder).
- `result.txt` is written before the rolled-back app restarts, not after.

### Spotifast

#### Delete

| File | Lines |
| --- | --- |
| `src/updates.rs` | 122 |
| `src/updates/install.rs` | 791 |
| `src/updates/macos.rs` | 503 |
| `src/updates/transfer.rs` | 587 |
| **Total** | **2,003** |

The 21 updater tests move into the crate. Keep the marker files in
`packaging/` (`spotifast-` and `fastpotify-portable.txt`,
`spotifast-` and `fastpotify-installer.txt`) and the CI step that runs
`--apply-update` on a missing job inside a real bundle: `intercept` prints
the error to standard error and exits with 1, as today.

`examples/updater-inspect.rs` (11 lines) becomes a call to
`Updater::installation_at(&path)`.

#### Add

A new `src/updates.rs` of about 40 lines:

```rust
pub use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Installation, Kind, Prepared, Release, Source, Unsupported,
    Updater,
};
use fastframe_update::{MacConfig, ReqwestTransport, UpdateConfig};

pub const CONFIG: UpdateConfig = UpdateConfig {
    legacy_names: &["fastpotify"],
    legacy_windows_installs: &["Programs/Fastpotify/fastpotify.exe"],
    macos: MacConfig {
        bundle_ids: &["rocks.spotifast.Spotifast", "me.paolino.fastpotify"],
        // 0.9.1 kept "fastpotify" for older clients' validation; later
        // releases may rename it to "Spotifast" (#538).
        executable_names: &["fastpotify", "Spotifast"],
        legacy_bundle_names: &["Fastpotify.app"],
    },
    // Checksums only until releases are signed; see below.
    publisher_key: None,
    ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", env!("CARGO_PKG_VERSION"))
};

pub fn updater(proxy: &crate::settings::ProxyConfig) -> anyhow::Result<Updater> {
    let builder = crate::http::blocking_builder(proxy).map_err(anyhow::Error::msg)?;
    Ok(Updater::new(CONFIG, ReqwestTransport::new(builder)?))
}
```

#### Replace

| Where | Today | With |
| --- | --- | --- |
| `entrypoint.rs` start | `--apply-update` block with `eprintln!` and `exit` | `let launch = fastframe_update::intercept(&updates::CONFIG);` (keep it before the MilkDrop child check) |
| `entrypoint.rs` `Cli` | hidden `update_receipt`/`update_error`; `Cli::command().name(name).get_matches()` | remove both; `.get_matches_from(&launch.arguments)`. The `fastpotify`/`spotifast` name choice from `argv[0]` stays: old updaters check `fastpotify <version>`. |
| `entrypoint.rs` | `cli.update_receipt.is_some()` for migration and `AppDirs::for_launch` | `launch.receipt.is_some()` |
| `entrypoint.rs` demo feed | `updates::Source::local(feed)` | `fastframe_update::Source::local(feed)` |
| `backend.rs` `check_for_updates` | async `newer_release_from(&http, &source)` | `tokio::task::spawn_blocking(move \|\| updater.with_source(source).check())` |
| `backend.rs` `InspectUpdate` | `install::detect()` | `updater.installation()` |
| `backend.rs` `DownloadUpdate` | `updates::download(&release, &source, &proxy, progress)` | `updates::updater(&proxy)?.with_source(source).download(&release, progress)` |
| `backend.rs` `InstallUpdate` | `install::handoff(&prepared, arguments)` | `updater.handoff(*prepared, arguments)` |
| `app.rs` `InstallUpdate` | `prepared.clone()` from `&self.update_download` | move it out with `std::mem::replace(&mut self.update_download, DownloadState::Installing)`; `Prepared` is not `Clone` |
| `app.rs` | `matches!(self.update_source, Source::GitHub)` | `self.update_source.is_github()` |
| `app.rs` receipt | `install::acknowledge(&receipt)` | `receipt.acknowledge()` |
| `demo.rs`, `app.rs` tests | `Prepared { .. }` literals | `Prepared::sample(installation, version)` |

Net: about 2,003 lines deleted and 70 added or changed, so roughly 1,930
fewer lines.

#### What changes for users

- **The macOS helper runs from a copy of the whole bundle** (ZapFast's fix),
  not from the installed bundle. The installed bundle can then be moved and
  restored without touching the helper's own code.
- `portable_entry`'s special case for the 0.8.0 and 0.9.0 archive layout is
  gone. Those versions are older than 0.10, so no app built on the crate can
  be offered them.
- The pre-marker Windows location now also checks the setup program's
  current default, `Programs/Spotifast/spotifast.exe`, as well as
  `Programs/Fastpotify/fastpotify.exe`.
- A Nix install says "Update this installation with Nix."

#### Turning on publisher signatures

Spotifast releases are not signed yet, so `publisher_key` stays `None` and
the update is verified against `checksums.txt` alone. To sign:

1. Add native-packages' `sign-release` step and a `release-signing`
   environment with the private key to the release workflow, as ZapFast has,
   and publish a release that carries `checksums.txt.sig`.
2. Only then ship a version with `publisher_key: Some(include_str!(..))`.
   From that version on, a release without a valid signature is refused, so
   every later release must be signed.

Older installs keep updating on checksums until they reach a version that
embeds the key.

### TonePush

TonePush has no updater of its own to delete: 0.6.1 only asks GitHub for
the newest tag and links to it. It adopts the crate over its own ureq
client, implementing `Transport` with redirects turned off, and without the
`reqwest` feature.

Its archives hold two programs, the `tonepush` command-line tool and the
`tonepush-gui` editor, beside one marker, so it sets `portable_executable`:

```rust
pub const CONFIG: UpdateConfig = UpdateConfig {
    portable_executable: Some("tonepush-gui"),
    macos: MacConfig {
        bundle_ids: &["rocks.tonepush.editor"],
        executable_names: &[],
        legacy_bundle_names: &[],
    },
    publisher_key: Some(include_str!("../../../assets/update-public-key.hex")),
    ..UpdateConfig::new("crmne/tonepush", "TonePush", "tonepush", env!("CARGO_PKG_VERSION"))
};
```

The editor answers `--version` with `tonepush <version>`, the slug, and the
release workflow puts `tonepush-portable.txt` (`tonepush-portable-v1`) into
the Linux and Windows archives. Only a running `tonepush-gui` beside that
marker updates itself; the `tonepush` tool next to it never matches. Inside
the macOS bundle the editor's executable is `tonepush`, so the bundle needs
no executable names. ZapFast and Spotifast leave `portable_executable` at
`None` and nothing changes for them.

### Deleted lines, both apps

About 3,950 lines of updater code and tests leave the two apps, replaced by
about 75 lines of configuration and 130 of changed call sites. The crate is
about 6,000 lines as formatted: about 2,900 of code and documentation and
3,100 of tests and test fakes.

## Not moved yet: widgets

Widgets stay in the apps for now. These exist in two or more apps and are
the candidates for a later `fastframe-widgets`, once the look crates above
have settled:

| Widget | Apps (where) |
| --- | --- |
| Switch (toggle) | ZapFast (`ui/widgets.rs` `switch`), Spotifast (`ui/widgets.rs`), RekordFlash (`theme.rs`), Chat with Work (`ui/widgets.rs`) |
| Settings row (label, detail, control) | ZapFast (`setting_row`), Spotifast (`setting_row`, `setting_row_sized`), RekordFlash (`setting_row`, `settings_section`, `setting_divider`), Chat with Work (`setting_row`, `section`, `group`) |
| Icon button and icon painting | ZapFast (`theme.rs` `icon`, `paint_icon`, `icon_button`), Spotifast (same names), RekordFlash (`icon`, `icon_image`, `icon_button`, `toolbar_icon_button`), TonePush (`icon_button`, `tinted_icon_button`), Chat with Work (`paint_icon`) |
| Primary, secondary and danger buttons | RekordFlash (`primary_button`, `secondary_button`, `danger_button`), Chat with Work (`primary_button`, `destructive_button`); ZapFast and Spotifast use `pill_button`, `soft_button`, `circle_button` for the same roles |
| Dialog title and footer | RekordFlash (`dialog_title`, `dialog_note`, `dialog_footer`), Chat with Work (`dialog_buttons`), ZapFast (`dialog_scroll_height`) |
| Segmented control, chips | RekordFlash (`segmented`, `deck_segment`), ZapFast (`chip`, `filter_chip`), Spotifast (`chips`), TonePush (`category_chip`) |
| Menu item, separator, frame | ZapFast (`menu_item`, `menu_item_enabled`, `menu_separator`, `menu_frame`, `submenu`), Spotifast (the same, plus `menu_submenu`) |
| Search field | ZapFast, Spotifast (`search_field`) |
| Empty state, loading and error rows | ZapFast (`empty_state`), Spotifast (`empty_state`, `loading_row`, `error_row`) |
| Spinner | ZapFast (`spinner`, `paint_spinner`), Spotifast (`spinner`, `circle_spinner`), RekordFlash (`spinner`, paced for an event-driven loop), TonePush (`spinner`) |
| Status dot, badge | RekordFlash (`status_dot`), Chat with Work (`status_dot`, `badge`), TonePush (`status_dot`), ZapFast (`badge`, `unread_dot`) |
| Focus ring | ZapFast (`focus_outline`, `reveal_focus`), Spotifast (`focus_ring`) |
| Credits line, logo | ZapFast, Spotifast (`credit`, `logo`) |
| Navigation row | RekordFlash (`navigation_row`, `navigation_highlight`), Spotifast (sidebar rows), ZapFast (chat rows) |
| Drag ghost | Spotifast, TonePush (`drag_ghost`) |
| Vertical gradient | ZapFast, Spotifast (`paint_vertical_gradient`) |

Their sizes and colours differ by app, so a shared crate would take the
palette through a small trait (or the `BASE_COLORS` names) rather than fix
any app's look.
