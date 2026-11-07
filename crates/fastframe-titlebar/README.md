# fastframe-titlebar

The title bar an egui app draws itself.

On macOS the apps run their content under a hidden title bar and keep the
native traffic lights: only AppKit's own buttons offer the zoom button's
tiling menu, Option-click and the inactive look. On Windows and Linux the user
can turn the platform's frame off, and the app then draws the buttons too.
This crate holds what such a header needs on all three.

| | macOS | Windows | Linux |
| --- | --- | --- | --- |
| `drag` | drag from the press itself; double-click as System Settings says | drag; double-click maximizes or restores | same as Windows |
| `footprint` | the traffic lights' room | the three buttons' room | the desktop's buttons' room, at either end |
| `controls` | nothing (native traffic lights) | square caption buttons, red under Close | GNOME's round buttons in its button layout, with the icon theme's glyphs |
| `resize` | nothing (native edges) | edges and corners | edges and corners |

On Windows and Linux everything waits for the user's choice (`set_enabled`)
and does nothing in full screen. On macOS `drag` and `footprint` always work
outside full screen.

## Usage

```rust
// At startup, before the first window:
fastframe_titlebar::init(); // GNOME's button layout and icons, through the portal
fastframe_titlebar::set_enabled(settings.custom_titlebar);
let viewport = egui::ViewportBuilder::default()
    // macOS: content under a hidden title bar, traffic lights kept.
    .with_fullsize_content_view(cfg!(target_os = "macos"))
    .with_titlebar_shown(!cfg!(target_os = "macos"))
    .with_title_shown(!cfg!(target_os = "macos"))
    // Windows and Linux: no frame when the user chose the app's own.
    .with_decorations(!fastframe_titlebar::enabled());

// Once per context, after egui's SVG loaders:
fastframe_titlebar::install(&ctx);

// In logic, every frame (macOS; nothing elsewhere):
fastframe_titlebar::align_traffic_lights(frame, ctx, HEADER_HEIGHT);

// In the header:
let room = fastframe_titlebar::footprint(ui.ctx(), HEADER_HEIGHT);
let header = ui.max_rect();
fastframe_titlebar::drag(ui, header); // before the header's own widgets
ui.add_space(room.left);
let reserve = room.reserve(header.width(), MIN_CONTENT);
// ... the header's widgets, leaving reserve.inline_width free at the right,
// or moved down by reserve.push_down when the header is too narrow.
fastframe_titlebar::controls(
    ui,
    &fastframe_titlebar::Style {
        icon: palette.secondary,
        button: palette.surface_hover,
        header_height: HEADER_HEIGHT,
    },
    &fastframe_titlebar::Labels {
        minimize: &gettext("Minimize"),
        maximize: &gettext("Maximize"),
        restore: &gettext("Restore"),
        close: &gettext("Close"),
    },
);
fastframe_titlebar::resize(ui); // after the window's content
```

`drag` registers its area before the widgets inside it, so they keep their
clicks; call it for every strip that should move the window (the top of each
panel, say). `controls` draws in egui's foreground layer, over every panel.

## The details

- **macOS drag.** AppKit starts a window drag only from the mouse-down
  that began it, so `drag` starts it on the press, not once the pointer
  moves. egui handles the press after AppKit has moved on (with a Force Touch
  trackpad, often to a pressure event), so a local event monitor keeps each
  mouse-down, and the press uses it once: for its click count, and as the
  event the drag starts from. `align_traffic_lights` names the root
  viewport's window, so a press in one window never acts on another; a child
  viewport's header reads the current event, as before.
  The second press of a double-click is not a drag: the crate performs the
  system's setting (zoom or minimize, after the frame), and Fill comes from
  AppKit's own drag.
- **Windows buttons** are 36 points square, centred in the header's height,
  5 points from the right edge, with Windows' caption feedback: a light wash
  over Minimize and Maximize, red (232, 17, 35) under Close, dimmer while
  pressed.
- **Linux buttons** follow GNOME's `button-layout` (`appmenu:close` by
  default on GNOME; minimize, maximize and close elsewhere), 28 points round,
  14 points from the edges. The glyphs are the icon theme's
  `window-*-symbolic` icons, filled white so egui can tint them, with Lucide
  outlines when the theme has none.
- **Resizing** starts within 5 points of an edge, or 10 points along it from
  a corner, and not while the window is maximized.
- **Traffic lights.** `align_traffic_lights(frame, ctx, header_height)`
  centres them on the header's centre line in their usual columns. AppKit
  puts them back for its own 28-point bar during layout passes, so call it
  every frame (it changes nothing when they are in place). The height is in
  egui points and scales with zoom; their room, `TRAFFIC_LIGHTS_WIDTH` (84
  AppKit points), does not.
- **The double-click setting** is System Settings, Desktop & Dock,
  "Double-click a window's title bar to", read from `AppleActionOnDoubleClick`
  (Zoom is stored as `Maximize`), falling back to the older
  `AppleMiniaturizeOnDoubleClick`. A value the crate does not know does
  nothing.
- **Room for the header.** `Footprint::reserve(width, min_content)` puts a
  header beside the right-hand buttons while both fit, and under the caption
  row otherwise. Which panel moves down is the app's choice.

It came from Spotifast's custom title bar (Windows and Linux) and its macOS
header drag, which ZapFast and Solco each wrote a part of, and replaces
`fastframe-macos`, whose traffic-light alignment came from ZapFast and Solco.

## Unsafe code

The workspace forbids unsafe code. This crate lowers that to `deny` and
allows it only in `src/macos/appkit.rs`, each block with a safety note.

## Not here: application menus

ZapFast builds its macOS menu bar with muda and keeps it alive across window
recreation; Spotifast builds its own with AppKit directly (plus a Touch Bar
guard); Solco has none. They share too little to extract a menu model yet.
