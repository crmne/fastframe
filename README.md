<p align="center">
  <img src="docs/assets/images/logo.svg" width="96" height="96" alt="fastframe logo">
</p>

<h1 align="center">fastframe</h1>
<p align="center"><strong>egui on rails.</strong> The desktop essentials around your app.</p>
<p align="center">
  <a href="https://fastframe.dev">Documentation</a> ·
  <a href="https://fastframe.dev/getting-started/">Get started</a> ·
  <a href="https://github.com/crmne/fastframe/releases">Releases</a> ·
  <a href="https://github.com/sponsors/crmne">Sponsor</a>
</p>

Build your interface with egui. Let fastframe handle the parts a desktop app
needs around it: native text, themes, tray menus, audio, media keys, sleep and
wake, notifications, and updates that can roll back.

Each capability is a small Rust crate. Take the ones you need, keep your own
layout and product decisions, and run on **Linux, macOS, and Windows**.
No telemetry or hosted service is required by the crates.

Extracted from [ZapFast](https://zapfast.rocks),
[Spotifast](https://spotifast.rocks), [Solco](https://getsolco.com),
[TonePush](https://docs.tonepush.rocks), and
[Chat with Work Desktop](https://github.com/crmne/chatwithwork-desktop).

## See it

Real egui windows from the [font specimen](crates/fastframe-fonts/examples/fonts.rs),
using synthetic text. Interface weights and installed script fallbacks work in
both themes. The light specimen uses a validated local font.

| Default interface font, dark | Local interface font, light |
| --- | --- |
| ![Dark native font specimen with multilingual text](docs/assets/images/fonts-dark.png) | ![Light native font specimen using a local font](docs/assets/images/fonts-light.png) |

Run it with `cargo run -p fastframe-fonts --example fonts`, or append
`-- /path/to/font.ttf` to try a local face. Installed fallback faces vary by platform.

## Start with fonts and text

Requires **Rust 1.98+** and **egui 0.36.2**. fastframe is not on crates.io yet;
pin every fastframe crate to the same release tag:

```toml
[dependencies]
egui = "0.36.2"
fastframe-fonts = { git = "https://github.com/crmne/fastframe", tag = "v0.14.0" }
fastframe-text = { git = "https://github.com/crmne/fastframe", tag = "v0.14.0" }
```

At startup, before the first frame:

```rust
let mut fonts = fastframe_fonts::FontSetup::default().definitions();
let rendering = fastframe_text::detect();
rendering.apply_to(&mut fonts);
ctx.set_fonts(fonts);
ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals));
```

Then add only what your app uses. Each crate README has its own setup example.
The [getting started guide](https://fastframe.dev/getting-started/) explains
integration, and the [migration guide](MIGRATION.md) shows what the existing
apps replaced.

## Crates

| Crate | What it does |
| --- | --- |
| [`fastframe-text`](crates/fastframe-text) | Desktop font rendering settings, text scaling, and pixel snapping. |
| [`fastframe-fonts`](crates/fastframe-fonts) | Inter, system or validated local interface fonts, with script and regional Han fallbacks. |
| [`fastframe-emoji`](crates/fastframe-emoji) | Platform colour emoji, drawn off the UI thread, with a bundled fallback. |
| [`fastframe-icons`](crates/fastframe-icons) | Embedded SVG icon sets, persistent image loading, and shared Lucide icons. |
| [`fastframe-theme`](crates/fastframe-theme) | Editable JSON palettes, system appearance, live Omarchy themes, and colour transitions. |
| [`fastframe-i18n`](crates/fastframe-i18n) | Bundled gettext catalogs, plural forms, language detection, and translation tooling. |
| [`fastframe-log`](crates/fastframe-log) | Per-run logs for bug reports, private-target rewriting, and safe panic logging. |
| [`fastframe-tray`](crates/fastframe-tray) | Native tray icons and menus on Linux, macOS, and Windows. |
| [`fastframe-dock`](crates/fastframe-dock) | macOS Dock icons and visibility, plus desktop unread badges. |
| [`fastframe-titlebar`](crates/fastframe-titlebar) | Draggable headers, native button layouts, double-click actions, and resize edges. |
| [`fastframe-shell`](crates/fastframe-shell) | Keep running without a window, reopen it, and restore its size, position, and zoom. |
| [`fastframe-audio`](crates/fastframe-audio) | Maintained audio input and output, preallocated output conversion, device changes, app and stream names in volume mixers, and rodio mixing. |
| [`fastframe-scroll`](crates/fastframe-scroll) | Platform wheel steps, touchpad momentum, and axis locking. |
| [`fastframe-autostart`](crates/fastframe-autostart) | Start at login through each platform, including Flatpak and hidden startup. |
| [`fastframe-instance`](crates/fastframe-instance) | One running copy per user, with a private channel for subsequent launches. |
| [`fastframe-now-playing`](crates/fastframe-now-playing) | Desktop media controls, artwork, media keys, and playback commands. |
| [`fastframe-power`](crates/fastframe-power) | Power and network state, sleep hooks and acknowledgements, and keep-awake guards. |
| [`fastframe-dirs`](crates/fastframe-dirs) | Platform config, state, and cache directories, including portable Windows installs. |
| [`fastframe-file`](crates/fastframe-file) | Atomic file replacement and background writing, with private permissions when needed. |
| [`fastframe-desktop`](crates/fastframe-desktop) | Desktop identity, portal settings, Flatpak entries, and macOS notification permission. |
| [`fastframe-notifications`](crates/fastframe-notifications) | Native notifications with click actions, replacement, withdrawal, and permissions. |
| [`fastframe-update`](crates/fastframe-update) | Signed updates from GitHub or HTTPS feeds, package detection, rollback, and prerelease channels. |

## egui and winit forks

fastframe builds against published egui releases. Our apps also use maintained
forks for fixes awaiting upstream releases. The
[getting started guide](docs/_guide/getting-started.md#egui-and-winit-forks)
has the matching revisions, the complete app-root patch block, and the status
of each fix. The current egui patch stack is based on **0.36.2**.

## How the project grows

Code comes from working apps. A piece moves here after it has proved useful
and is general enough to share. Small crates, good defaults, and few knobs
keep the API manageable. Branding, layout, and product behavior stay in the app.
Bugs in egui, winit, and other dependencies are fixed upstream.

The project is young and APIs can change. Read the
[release notes](https://github.com/crmne/fastframe/releases) before upgrading.

## Contributing

See [AGENTS.md](AGENTS.md) for the complete checks and contribution rules.
Behavior changes need focused tests, public API documentation, and support for
all three desktop platforms. Tests use injected data instead of live services.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
RUSTDOCFLAGS='-D warnings' cargo doc --locked --all-features --no-deps
```

Found a problem? [Open an issue](https://github.com/crmne/fastframe/issues).
For questions and ideas, use [Discussions](https://github.com/crmne/fastframe/discussions).

## License

MIT. See [LICENSE](LICENSE). Bundled fonts retain their own licenses.
