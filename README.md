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
| [`fastframe-audio`](crates/fastframe-audio) | Audio output with the app's own renderer: a stream that gets no callbacks while paused, follows the default device, reopens after failures, and keeps a clock of what has played. |
| [`fastframe-scroll`](crates/fastframe-scroll) | Scrolling that feels like the platform's: a 120-point wheel step, and Linux touchpad gestures that keep their speed, glide after the lift, and hold one axis. |
| [`fastframe-instance`](crates/fastframe-instance) | One running copy per user: a crash-safe lock, and a private channel a second launch hands its request over (show the window, open a link, any line the app understands). |
| [`fastframe-now-playing`](crates/fastframe-now-playing) | The desktop's media controls: what is playing, with artwork, in MPRIS, the Windows media overlay and the macOS Now Playing panel, and the media keys and buttons back. |
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

Every app on fastframe should use the revisions the apps use, so it gets the
same fixes. Copy this into the app's root `Cargo.toml`:

```toml
[patch.crates-io]
ecolor = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
eframe = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
egui = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
egui-wgpu = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
egui-winit = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
egui_extras = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
egui_glow = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
emath = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
epaint = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
epaint_default_fonts = { git = "https://github.com/crmne/egui", rev = "ba6790fe7cf46e58e8d27ce1524cbfdee745e938" }
winit = { git = "https://github.com/crmne/winit", rev = "ed7caa9023f10b397f5b6ec8284a840cbd8a6f65" }
```

Patch every egui crate the app uses from the same revision, so they share one
`emath` and `epaint` (add `egui_kittest` if the app's tests use it), and move
egui and winit together: the egui revision is built against that winit, and
moving one alone does not build. An app that compensated for the zoom shrink
itself (a `window_builder` that scales the restored size) drops that code when
it moves to egui `ba6790fe` or later, or the size is scaled twice. Cargo warns
about a patch for a crate the app does not use (`egui_extras`, say); leave
that line out. With egui `ba6790fe` or later (emilk/egui#8621), a window or
menu is left out of the accessibility tree on the frame that sizes it, so a
test that reads the tree right after opening one runs one more frame first.

What the forks carry, and where each patch stands upstream. Each patch goes
upstream as a pull request and stays in the fork until an egui or winit
release includes it; then the fork drops it.

| Fix | Fork | Upstream |
| --- | --- | --- |
| Right-to-left text shaped in its direction | egui `6147de7b`, `8e592348` | [emilk/egui#8577](https://github.com/emilk/egui/pull/8577), open |
| No busy loop while waiting for a redraw | egui `f14640be` | [emilk/egui#8398](https://github.com/emilk/egui/pull/8398), merged, not yet released |
| No freeze when a Wayland window is hidden: frames paced by the compositor's callbacks | egui `41ff9ddf` | [emilk/egui#8631](https://github.com/emilk/egui/pull/8631), open |
| The same, eframe's side of winit's `Occluded` | egui `0b431145` | [emilk/egui#8660](https://github.com/emilk/egui/pull/8660), a draft that waits for winit#4710 |
| The xdg-shell `suspended` state reported as `Occluded` | winit `a51e41b2` | [rust-windowing/winit#4709](https://github.com/rust-windowing/winit/pull/4709) and [#4710](https://github.com/rust-windowing/winit/pull/4710) (0.30), open |
| Paste and file drops together on Wayland: one data device for both (crmne/spotifast#614) | winit `1a8306ad`, `fb8b24c3`; egui `2ab31332` | [rust-windowing/winit#4729](https://github.com/rust-windowing/winit/pull/4729) (0.30), open; on winit's main branch, the clipboard pull request [#4658](https://github.com/rust-windowing/winit/pull/4658) uses the same data device. [emilk/egui#8657](https://github.com/emilk/egui/pull/8657), a draft that waits for winit#4729 |
| Each emoji one emoji wide (families, skin tones, keycaps, flags) | egui `73e8b6c3` | not proposed: egui's main branch already draws each of these one emoji wide through the system's colour emoji fonts, and the patch only changes the monochrome fonts an app opts into |
| No runaway resizing on Windows when a window moves between monitors of different scale | winit `f4fed12c` | on winit's main branch (`39c4009c`); the fork carries it for 0.30 |
| macOS Quit through close requests, behind a feature | winit `a7b78b27` | [rust-windowing/winit#4692](https://github.com/rust-windowing/winit/pull/4692), open |
| A focus request for a widget not drawn yet no longer crashes accessibility on Windows and macOS | egui `4c143c1d` | [emilk/egui#8621](https://github.com/emilk/egui/pull/8621), merged, not yet released |
| Windows starts on OpenGL drivers older than 3.3: a compatibility-profile fallback | egui `57eb5714` | [emilk/egui#8655](https://github.com/emilk/egui/pull/8655), open |
| A window keeps its size across restarts at any interface zoom | egui `56e7ac3c` | [emilk/egui#8656](https://github.com/emilk/egui/pull/8656), open |
| A closed window leaves the screen on Touch Bar Macs | winit `ed7caa90` | [rust-windowing/winit#4728](https://github.com/rust-windowing/winit/pull/4728), a draft until confirmed on a Touch Bar Mac |

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
