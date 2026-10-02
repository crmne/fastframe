# fastframe

**egui on rails.** Documentation: [fastframe.dev](https://fastframe.dev).

fastframe is the shared foundation of a family of native desktop apps built
with Rust and [egui](https://github.com/emilk/egui):
[ZapFast](https://zapfast.rocks), [Spotifast](https://spotifast.rocks),
[Solco](https://getsolco.com), [TonePush](https://docs.tonepush.rocks), and
the [Chat with Work](https://chatwithwork.com) local agent. Each piece here was
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
| [`fastframe-fonts`](crates/fastframe-fonts) | Bundled Inter (tabular figures) or the platform's own interface face at the app's weights, and installed fonts for every script it lacks, aligned to its baseline. |
| [`fastframe-emoji`](crates/fastframe-emoji) | Colour emoji in the platform's own style (Apple, Segoe UI, the desktop's) with a bundled fallback, in every egui text through an egui plugin or inline as selectable placeholders, drawn off the interface thread. |
| [`fastframe-icons`](crates/fastframe-icons) | Embedded SVG icon sets for egui: the `icons!` macro, a bytes loader that survives `reduce_texture_memory`, and the Lucide icons the apps share. |
| [`fastframe-theme`](crates/fastframe-theme) | JSON colour palettes, a catalogue loaded off the interface thread, the eight shared palettes, following the Omarchy desktop's theme with filesystem notifications, and revealing a change of colours from the middle of the window outwards. |
| [`fastframe-i18n`](crates/fastframe-i18n) | Bundled gettext catalogs: a build-time PO compiler for `build.rs`, `gettext`/`pgettext`/`ngettext` lookups, system language detection, and the template update script. |
| [`fastframe-log`](crates/fastframe-log) | Logs to stderr and a per-run file for bug reports, rewrites private targets, and records panics without their payload. |
| [`fastframe-tray`](crates/fastframe-tray) | A tray item with the app's own menu: StatusNotifier on Linux, the notification area on Windows, the menu bar on macOS. |
| [`fastframe-macos`](crates/fastframe-macos) | Puts the macOS traffic lights on the centre line of an app's own title bar, and reads the system's title-bar double-click setting. |
| [`fastframe-shell`](crates/fastframe-shell) | Keeps an app running without a window around `eframe::run_native`, starts hidden, and brings back windows restored off-screen. |
| [`fastframe-update`](crates/fastframe-update) | Self-update from GitHub releases or the app's own HTTPS feed: detects package-managed installs, verifies signed checksums, installs through a helper with rollback, keeps the handoff format older app versions use, moves renamed installations onto the new name with their launchers, and offers an opt-in pre-release channel. |

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

Every app on fastframe should use the revisions the apps use, so it gets the
same fixes. Copy this into the app's root `Cargo.toml`:

```toml
[patch.crates-io]
ecolor = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
eframe = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
egui = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
egui-wgpu = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
egui-winit = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
egui_extras = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
egui_glow = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
emath = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
epaint = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
epaint_default_fonts = { git = "https://github.com/crmne/egui", rev = "61a38149a4010ff9b8257dc6a03b98818b98fd02" }
winit = { git = "https://github.com/crmne/winit", rev = "fb8b24c3ec3f2c499daa92aeb4e783a8efa2e973" }
```

Patch every egui crate the app uses from the same revision, so they share one
`emath` and `epaint` (add `egui_kittest` if the app's tests use it), and move
egui and winit together: the egui revision is built against that winit, and
moving one alone does not build. Cargo warns about a patch for a crate the
app does not use (`egui_extras`, say); leave that line out.

What the forks fix:

- **Paste and file drops together on Wayland.** winit offers the clipboard and
  reports dropped files through one data device, and egui-winit uses it:
  Hyprland sends the selection and drags only to a client's first data device,
  so a second one for the clipboard broke paste (crmne/spotifast#614).
- **No freeze when a Wayland window is hidden.** eframe paces frames by the
  compositor's frame callbacks and keeps running when they stop, and winit
  reports the xdg-shell `suspended` state as `Occluded` (emilk/egui#8631,
  rust-windowing/winit#4709, rust-windowing/winit#4710).
- **No busy loop while waiting for a redraw** (emilk/egui#8398).
- **Right-to-left text shaped in its direction** (emilk/egui#8577).
- **Each emoji one emoji wide.** Families, skin tones, keycaps and flags are laid
  out as one emoji, so `fastframe-emoji`'s pictures, the cursor and widget sizes
  agree.
- **No runaway resizing when a window is dragged between monitors of different
  scale** on Windows.
- **macOS Quit through close requests**, behind a winit feature an app opts into.

On stock egui and winit, fastframe still builds and works, without these
fixes.

A future `forks.toml`, with a generator that writes each app's patch section
and a CI check that they agree, will keep those pins in one place. It does not
exist yet; until then, this section has the current revisions, and a fastframe
release that needs newer ones says so in its notes.

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
