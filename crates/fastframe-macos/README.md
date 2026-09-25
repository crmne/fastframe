# fastframe-macos

macOS window chrome for egui apps that draw their own header under a hidden
title bar (`with_fullsize_content_view(true)`, `with_titlebar_shown(false)`).

- `align_traffic_lights(frame, ctx, header_height)` centres the close,
  minimize and zoom buttons on the header's centre line, in their usual
  columns. AppKit puts them back for its own 28-point bar during layout
  passes, so call it every frame (it changes nothing when they are in place).
  The height is in egui points and scales with zoom.
- `traffic_light_inset(ctx)` is the room to leave at the header's left: 84
  AppKit points (not zoomed), zero in full screen.
- `double_click_action()` reads System Settings, Desktop & Dock,
  "Double-click a window's title bar to": `Zoom`, `Fill`, `Minimize` or
  `Nothing`. The app performs it.

Off macOS everything compiles and does nothing (`double_click_action` answers
`Zoom`, the inset is zero).

## Usage

```rust
const HEADER_HEIGHT: f32 = 48.0;

fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
    fastframe_macos::align_traffic_lights(frame, ctx, HEADER_HEIGHT);
}

fn header(ui: &mut egui::Ui) {
    ui.add_space(fastframe_macos::traffic_light_inset(ui.ctx()));
    // ...
    if response.double_clicked() {
        match fastframe_macos::double_click_action() {
            DoubleClick::Zoom | DoubleClick::Fill => { /* toggle Maximized */ }
            DoubleClick::Minimize => { /* Minimized(true) */ }
            DoubleClick::Nothing => {}
        }
    }
}
```

`Fill` (and `Maximize`, its older name) is separate because AppKit performs
it itself when the double-click starts a native window drag
(`ViewportCommand::StartDrag` on mouse down, as Spotifast does). An app that
starts dragging only after movement treats it like `Zoom`. A value the crate
does not know does nothing.

## Not here: application menus

ZapFast builds its macOS menu bar with muda and keeps it alive across window
recreation; Spotifast builds its own with AppKit directly (plus a Touch Bar
guard); RekordFlash has none. They share too little to extract a menu model
yet.

## Unsafe code

The workspace forbids unsafe code. This crate lowers that to `deny` and
allows it only in `appkit.rs`, each block with a safety note.
