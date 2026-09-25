# fastframe-fonts

The apps' interface font, and installed fonts for every other script, for
egui.

- **Inter**, bundled: Inter 4.001 as a variable font with its tabular
  figures frozen into the character map, so a timer or a counter keeps its
  width as it changes (see [`fonts/README.md`](fonts/README.md) for how it
  was made). The same file ZapFast, Spotifast and RekordFlash ship.
- **Weights**: 400 is egui's proportional family; 500, 600 and 700 are
  named families (`inter-medium`, `inter-semibold`, `inter-bold`), each
  falling back like the regular one.
- **System fallbacks**: one installed face per script Inter lacks (CJK,
  Arabic, Hebrew, Indic scripts, Thai, Yi, symbols and more), chosen the way
  the desktop would, aligned to Inter's baseline, and Arabic enlarged to
  read as large as Latin text.

## Usage

```rust
use fastframe_fonts::{FontSetup, Weight};

// The default: Inter at 400/500/600/700, egui's monospace, system fallbacks.
let mut fonts = FontSetup::default().definitions();
// Follow the desktop's hinting (fastframe-text), then hand them to egui:
fastframe_text::detect().apply_to(&mut fonts);
ctx.set_fonts(fonts);

let title = Weight::SemiBold.font_id(16.0);
let body = Weight::Regular.font_id(14.0); // FontFamily::Proportional
```

Apps choose:

```rust
FontSetup::default()
    .weights(&[Weight::SemiBold])                  // regular is always there
    .monospace(Monospace::Inter)                   // or EguiDefault, or Font { name, data }
    .companion("noto_emoji", Arc::new(emoji))      // right after Inter, in every family
    .system_fallbacks(!demo)                       // off for reproducible screenshots
    .install(ctx);                                 // or .definitions() to adjust first
```

`definitions()` starts from `FontDefinitions::default()`, so egui's own
fonts (with its `default_fonts` feature) stay behind Inter and any
companions, and the system fallbacks come last.

## System fallbacks

`fastframe_fonts::system::fallbacks()` finds the faces once per process and
reuses them for every window.

| Platform | How the face is chosen |
| --- | --- |
| macOS | CoreText names the face it draws each script with, in the language the user reads, and its cascade behind it. Faces epaint cannot draw (Apple's `hvgl` outlines in `PingFangUI.ttc`) are skipped for the next in the cascade. |
| Linux | Every font under fontconfig's configured directories (NixOS lists its store paths only there), the XDG data directories, Flatpak's `/run/host/fonts`, `~/.fonts` and `$XDG_DATA_HOME/fonts` is probed through a memory map. The best regular sans face per script wins: a face named for the script beats incidental coverage, sans beats serif, mono and display cuts, weight near 400 beats far, and for Han the locale's regional cut (and a face declaring that region's code page) wins. |
| Windows | The same probe over `%SystemRoot%\Fonts` and the per-user font directory; the display language picks the Han cut. |

Each chosen face gets `y_offset_factor` so its baseline sits on Inter's
(epaint centres a fallback's line box on the primary's, which shifts faces
with other vertical metrics), and an Arabic face drawn small beside Inter is
scaled up by at most 25%.

## Why a crate of its own

`fastframe-text` follows the desktop's rendering settings and works with any
font; this crate decides which fonts. Keeping them apart keeps the 880 KB
font, the font parser and the platform font code out of an app that only
wants the rendering settings, and keeps `fastframe-text` free of unsafe code
(memory-mapping fonts and asking CoreText and Win32 need it here).

## What stays in the app

Font sizes and text styles, the colour emoji rendering ZapFast draws over
text, Spotifast's skin bitmap font and its Winamp playlist face, and any
face an app bundles besides Inter (TonePush's IBM Plex Mono goes in through
`Monospace::Font`).

## Licences

The code is MIT. Inter is under the SIL Open Font License 1.1; apps ship
[`fonts/Inter-LICENSE.txt`](fonts/Inter-LICENSE.txt) with their other
licences, as they already do.
