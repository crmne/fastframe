# fastframe-text

Follow the desktop's font rendering settings in egui.

egui's defaults match no desktop exactly. Linux desktops ask for slight
hinting with sub-pixel positions and draw coverage linearly (cairo,
FreeType); egui's dark theme thickens glyphs and nothing tells it what the
desktop wants. A hard-coded "full grid fit on whole pixels" looks too sharp,
with uneven kerning. This crate reads what the desktop asks for and applies
it to every font egui draws.

## Usage

```rust
let rendering = fastframe_text::detect();

let mut fonts = egui::FontDefinitions::default();
// ... insert the app's own fonts ...
rendering.apply_to(&mut fonts);
ctx.set_fonts(fonts);

// After the app has set its own visuals (replacing them resets this),
// for both themes:
ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals));
```

`detect()` blocks briefly on Linux (a D-Bus call, then `fc-match` if the
portal has no answer), so call it once at startup, outside any tokio runtime.

With the `watch` feature, follow changes while the app runs:

```rust
let current = fastframe_text::detect();
let ctx = ctx.clone();
fastframe_text::watch::watch(current, move |rendering| {
    // Hand `rendering` to the interface thread, rebuild the fonts with
    // `rendering.apply_to(..)`, call `ctx.set_fonts`, reapply the visuals,
    // and repaint.
    ctx.request_repaint();
})?;
```

### Whole pixels

Text placed by hand should start on a whole physical pixel, or egui resamples
the galley and it looks soft. At 133% a point is 1.33 pixels, so whole points
are often not whole pixels:

```rust
let x = fastframe_text::snap_to_pixels(rect.center().x - galley.size().x / 2.0, ctx.pixels_per_point());
```

For `Pos2`, `Vec2` and `Rect`, `egui::emath::GuiRounding::round_to_pixels`
does the same.

### The desktop's text size

GNOME's Large Text and Omarchy's `omarchy display text size` set the
desktop's text size (`text-scaling-factor`, 1 is the normal size).
`text_scale()` reads it, for the app to apply as its interface zoom, so
panels sized for their labels grow with them:

```rust
let scale = fastframe_text::text_scale();
ctx.set_zoom_factor(scale);
```

With the `watch` feature, follow changes while the app runs:

```rust
let ctx = ctx.clone();
fastframe_text::watch::watch_text_scale(scale, move |scale| {
    ctx.set_zoom_factor(scale);
})?;
```

The crate never sets the zoom itself. An app that keeps a zoom the user
picked (fastframe-shell's `window::keep_zoom`) multiplies the two, and
divides the text size out before saving, so the setting stays the user's
own zoom:

```rust
// Every frame, in `logic`:
let scale = self.text_scale;
if let Some(zoom) = window::keep_zoom(ctx, self.settings.zoom * scale) {
    self.settings.zoom = zoom / scale;
    self.mark_settings_dirty();
}
// After `keep_zoom`, when the watch sent a new size:
if let Ok(scale) = self.text_scale_changes.try_recv() {
    self.text_scale = scale;
    ctx.set_zoom_factor(self.settings.zoom * scale);
}
```

The new size goes after `keep_zoom`: egui takes a zoom at the start of the
next pass, and `keep_zoom` would otherwise save the old one.

It is followed only under Wayland. Under X11, GNOME's settings daemon
already multiplies the `Xft/DPI` it publishes by the factor and winit takes
its scale from that, so zooming as well would apply the text size twice
(1.25 became about 1.56). There `text_scale()` returns 1 and the watch
returns `WatchError::Unsupported`. A session is Wayland when
`WAYLAND_DISPLAY` or `WAYLAND_SOCKET` is set, which is when winit opens its
windows through Wayland; an app that forces X11 itself (winit's `with_x11`)
should not follow it. Values outside GNOME's range for the key (0.5 to
3) are ignored.

It came from [RAWmakase](https://github.com/pch/rawmakase), which zooms
its interface to the desktop's text size.

## The model

```rust
pub struct TextRendering {
    pub hinting: Hinting, // None | Slight | Medium | Full
    pub antialias: bool,
    pub subpixel_positioning: bool,
    pub coverage: Coverage, // Linear | ThemeDefault
}
```

`TextRendering::default()` is what Linux desktops render: slight hinting,
antialiased, sub-pixel positions, linear coverage.

## Mapping to egui

Per font (`FontTweak`, through `apply_to`):

| Setting | `hinting` | `hinting_target` | `subpixel_binning` |
| --- | --- | --- | --- |
| `Hinting::None` | `Some(false)` | egui default (unused) | |
| `Hinting::Slight` | `Some(true)` | egui default: `Smooth { light: false, symmetric_rendering: true, preserve_linear_metrics: true }` | |
| `Hinting::Medium` | `Some(true)` | `Smooth { light: false, symmetric_rendering: true, preserve_linear_metrics: false }` | |
| `Hinting::Full` | `Some(true)` | same as Medium | |
| `antialias: false` | | `Mono` (overrides the above) | |
| `subpixel_positioning` | | | `Some(value)` |

Per theme (`Visuals::text_options`, through `apply_to_visuals`):

| Setting | Field | Value |
| --- | --- | --- |
| `Coverage::Linear` | `color_transfer_function` | `Off` in light and dark |
| `Coverage::ThemeDefault` | `color_transfer_function` | egui's own: `Off` in light, `TwoCoverageMinusCoverageSq` in dark |
| `hinting` | `font_hinting` | `false` for `Hinting::None`, else `true` |
| `subpixel_positioning` | `subpixel_binning` | the value |

`apply_to` touches only the three hinting fields, so each font's scale,
offsets and variation coordinates stay as the app set them.

### Why this mapping

Measured on Hyprland at 133% and 160%, pixel for pixel against pango and
cairo with `hintslight`, grayscale antialiasing and sub-pixel positions:

- **Slight is egui's default target.** With linear metrics preserved, the
  autohinter only snaps vertically, which is what slight hinting is. For
  fonts without TrueType instructions (Inter has no `fpgm`, `prep` or `cvt`),
  `light: true` renders identically, so the crate keeps egui's tested default.
- **Sub-pixel positions stay on.** Placing glyphs on whole pixels, with a full
  grid fit, measured two to four times the spacing error: the uneven kerning
  that made that setting look wrong. Medium and Full still grid-fit
  horizontally (`preserve_linear_metrics: false`) for those who ask for it:
  sharper stems, less even spacing.
- **Coverage is linear on Linux.** egui's dark theme maps coverage through
  `2c - c²`, which drew text 11 to 18% heavier than GTK. Linear coverage
  (`Off`, egui's light-theme value) matched GTK's ink weight within 2%. macOS
  keeps egui's per-theme choice, since CoreText renders heavier than FreeType;
  Windows keeps it too until someone measures DirectWrite.

The sub-pixel (LCD) order a desktop may request is ignored: egui renders
grayscale coverage only, so `rgba` antialiasing counts as grayscale.

## Where settings come from

| Platform | Source |
| --- | --- |
| Linux | The desktop portal (`org.freedesktop.portal.Settings`, namespace `org.gnome.desktop.interface`, keys `font-hinting` and `font-antialiasing`, and `text-scaling-factor` for the text size under Wayland), then fontconfig through `fc-match -f '%{hintstyle}\|%{hinting}\|%{antialias}' sans-serif`, then the default. |
| macOS | No hinting with sub-pixel positions (CoreText does not hint), egui's per-theme coverage. The text size is 1. |
| Windows | Slight hinting with sub-pixel positions, the closest match to DirectWrite's natural rendering (vertical fitting only, fractional advances), egui's per-theme coverage. The registry is not read yet, and the text size is 1. |

Each Linux source answers only if it holds a known value; a source that is
missing (no session bus, no portal, no `fc-match`) is skipped. Portal calls
time out after a second, and `fc-match` is stopped when it has not answered
in one. Coverage is not a desktop setting: it comes from the platform. The
portal does not describe sub-pixel positioning, so it stays on, as in GTK 4.
Only the portal is watched for changes; edits to fontconfig files are not.

fontconfig is read through `fc-match` rather than a binding because no
fontconfig crate is common to the apps' dependency trees and linking the C
library would add a build dependency.

## egui version

The crate depends on `egui` 0.36.1 or later from crates.io, without default
features. The per-font hinting target and sub-pixel binning (emilk/egui#8262)
and the font colour transfer function are in the stock 0.36.1 release, so no fork is required; apps that patch egui to `crmne/egui` apps-0.36
work too.
