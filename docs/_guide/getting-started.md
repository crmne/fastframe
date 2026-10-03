---
title: Getting Started
description: Add fastframe crates to an egui app, and keep the egui and winit forks working.
nav_order: 2
---

## Requirements

- Rust 1.98 or later (the `rust-version` of every crate). An app pinned to an
  older toolchain in `rust-toolchain.toml` fails to resolve; bump it first.
- egui and eframe 0.36.

## Add a crate

fastframe is not on crates.io. Depend on a release tag from GitHub:

```toml
[dependencies]
fastframe-text = { git = "https://github.com/crmne/fastframe", tag = "v0.2.3" }
fastframe-fonts = { git = "https://github.com/crmne/fastframe", tag = "v0.2.3" }
```

Use the same tag for every fastframe crate, and move it deliberately: the
[release notes](https://github.com/crmne/fastframe/releases) name every change
an app has to make to upgrade.

## A first window

Fonts and text rendering are where most apps start. At startup, before the
first frame:

```rust
use fastframe_fonts::{FontSetup, Weight};

fn setup(ctx: &egui::Context) {
    // Inter at 400, 500, 600 and 700, and installed fonts for other scripts.
    let mut fonts = FontSetup::default().definitions();

    // Hint and antialias like the desktop does.
    let rendering = fastframe_text::detect();
    rendering.apply_to(&mut fonts);
    ctx.set_fonts(fonts);

    // After the app has set its own visuals, for both themes.
    ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals));
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(Weight::SemiBold.font_id(16.0)));
}
```

From there, each [crate page](/fastframe-text/) shows how to wire it in. The
[migration guide](/moving-apps/) lists, app by app, what each crate replaces
in [ZapFast](https://zapfast.rocks), [Spotifast](https://spotifast.rocks),
[Solco](https://getsolco.com), [TonePush](https://docs.tonepush.rocks) and
[Chat with Work](https://chatwithwork.com), which makes
it a good set of worked examples.

## egui and winit forks

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
| The same, eframe's side of winit's `Occluded` | egui `0b431145` | needs a pull request |
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
