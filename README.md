# fastframe

**egui on rails.**

fastframe is the shared foundation of a family of native desktop apps built
with Rust and [egui](https://github.com/emilk/egui): ZapFast, Spotifast,
RekordFlash, TonePush, and the Chat with Work local agent. Each piece here was
written for one of those apps first and moved here once another app needed it.

## Principles

- **Extract from working apps, never design up front.** Code arrives here
  after it has shipped in an app, not before.
- **Two apps, then it moves.** A piece moves in only once at least two apps
  have it.
- **Small crates, one workspace.** Each piece is its own `fastframe-*` crate,
  so an app takes only what it uses.
- **Good defaults, few knobs.** A crate does the right thing with no
  configuration. Apps keep their own decisions (branding, layout, product
  choices); fastframe does not make them.
- **No telemetry, no hosted services.** Nothing here phones home or depends
  on a server run by us.
- **Fix upstream.** Bugs in egui, winit and friends are fixed in those
  projects, not carried here as patches forever.

## Crates

| Crate | What it does |
| --- | --- |
| [`fastframe-text`](crates/fastframe-text) | Follows the desktop's font rendering settings (hinting, antialiasing, sub-pixel positioning, text weight) in egui, and snaps hand-placed text to whole pixels. |
| [`fastframe-fonts`](crates/fastframe-fonts) | Bundled Inter (tabular figures) at the app's weights, and installed fonts for every script Inter lacks, aligned to its baseline. |
| [`fastframe-icons`](crates/fastframe-icons) | Embedded SVG icon sets for egui: the `icons!` macro, a bytes loader that survives `reduce_texture_memory`, and the Lucide icons the apps share. |
| [`fastframe-theme`](crates/fastframe-theme) | JSON colour palettes, a catalogue loaded off the interface thread, the eight shared palettes, following the Omarchy desktop's theme with filesystem notifications, and revealing a change of colours from the middle of the window outwards. |
| [`fastframe-i18n`](crates/fastframe-i18n) | Bundled gettext catalogs: a build-time PO compiler for `build.rs`, `gettext`/`pgettext`/`ngettext` lookups, system language detection, and the template update script. |
| [`fastframe-log`](crates/fastframe-log) | Logs to stderr and a per-run file for bug reports, rewrites private targets, and records panics without their payload. |
| [`fastframe-tray`](crates/fastframe-tray) | A tray item with the app's own menu: StatusNotifier on Linux, the notification area on Windows, the menu bar on macOS. |
| [`fastframe-macos`](crates/fastframe-macos) | Puts the macOS traffic lights on the centre line of an app's own title bar, and reads the system's title-bar double-click setting. |
| [`fastframe-shell`](crates/fastframe-shell) | Keeps an app running without a window around `eframe::run_native`, starts hidden, and brings back windows restored off-screen. |
| [`fastframe-update`](crates/fastframe-update) | Self-update from GitHub releases: detects package-managed installs, verifies signed checksums, installs through a helper with rollback, keeps the handoff format older app versions use, moves renamed installations onto the new name with their launchers, and offers an opt-in pre-release channel. |

## Status

Early, extracted piece by piece. APIs will change while the first crates
settle; nothing is on crates.io yet.

## Using it

Depend on a git revision for now:

```toml
[dependencies]
fastframe-text = { git = "https://github.com/crmne/fastframe", rev = "<commit>" }
```

Pin a revision rather than a branch, and update it deliberately.

### egui and winit forks

The apps build against forks of egui and winit that carry fixes waiting for
upstream releases. A library cannot pin those: Cargo applies `[patch]` only in
the root workspace of the app being built, so each app keeps its own
`[patch.crates-io]` section. fastframe crates depend on the published egui
versions, and they must compile against both the release and the apps' fork.

A future `forks.toml`, with a generator that writes each app's patch section
and a CI check that they agree, will keep those pins in one place. It does not
exist yet.

## Development

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
RUSTDOCFLAGS='-D warnings' cargo doc --locked --all-features --no-deps
```

[MIGRATION.md](MIGRATION.md) lists what each app replaces with each crate.

See [AGENTS.md](AGENTS.md) for the rules contributors and coding agents follow.

## License

MIT. See [LICENSE](LICENSE).
