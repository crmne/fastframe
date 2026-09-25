# fastframe-tray

A tray item with the app's own menu: a StatusNotifierItem on Linux (ksni), a
notification-area icon on Windows and a menu-bar item on macOS (tray-icon).

The app supplies its id, its name, its icon and the menu entries (already
translated). Clicks arrive as events on a channel, and a wake function is
called for each one so the app reads them promptly, with or without a window.

## Usage

```rust
use fastframe_tray::{Config, Event, MenuItem, Tray};

let tray = Tray::spawn(
    Config {
        id: "spotifast",
        title: "Spotifast".into(),
        icon: util::app_icon_rgba,                 // fn(usize) -> Vec<u8>, RGBA
        template_icon: Some(util::tray_template_rgba), // macOS menu bar
        menu: vec![
            MenuItem::action("show", "Show or hide Spotifast"),
            MenuItem::Separator,
            MenuItem::action("play-pause", "Play"),
            MenuItem::Separator,
            MenuItem::action("quit", "Quit"),
        ],
    },
    move || waker.wake(),
);
// None: no tray on this desktop, so closing the window should quit.

// Every frame and every headless tick:
for event in tray.events() {
    match event {
        Event::Toggle | Event::Menu("show") => { /* show or hide */ }
        Event::Show => { /* show */ }
        Event::Menu("play-pause") => { /* ... */ }
        Event::Menu("quit") => { /* quit */ }
        Event::Menu(_) => {}
    }
}

// Labels can change:
tray.set_label("play-pause", if playing { "Pause" } else { "Play" });

// When a window is made (the macOS item is created by the first call):
tray.attach();

// While no window exists (runs AppKit's loop on macOS, sleeps elsewhere):
fastframe_tray::idle(std::time::Duration::from_millis(150));
```

## Platforms

- **Linux**: ksni on its own thread. `spawn` returns `None` without a
  StatusNotifier host. Inside Flatpak (`/.flatpak-info` or `FLATPAK_ID`) the
  item registers its unique bus name, since the sandbox does not let it own
  one. A left click is `Event::Toggle`.
- **Windows**: tray-icon on its own thread with a message loop. A left click
  is `Event::Show`: the two releases of a double-click arrive separately,
  possibly on both sides of window creation, and a toggle would hide the
  window again. The menu opens on right click only.
- **macOS**: status items only exist on the main thread while AppKit's loop
  runs, so the first `attach` makes the item, and each `attach` brings the
  app forward. A left click is `Event::Toggle`; the menu opens on right
  click. A click on the Dock icon is `Event::Show`, even for a minimized
  window (AppKit calls that visible). While headless, `idle` runs
  `-[NSApplication run]` in slices, which catches Objective-C exceptions
  raised while handling an event; a hand-written event loop let them unwind
  into Rust and abort the process (ZapFast #199).
- Anywhere else, `spawn` returns `None` and `idle` sleeps.

## Sharing muda's menu handler

tray-icon's menus (muda) report clicks through one process-wide handler,
which the tray installs when it makes its item (Windows: `Tray::spawn`;
macOS: the first `Tray::attach`). muda keeps the first handler it is given
and silently ignores later ones. An app that builds its own muda menus, such
as a macOS menu bar, must install its handler before the tray makes its
item, or its menu does nothing (ZapFast #215), and hand each event to the
tray first:

```rust
MenuEvent::set_event_handler(Some(|event: MenuEvent| {
    if fastframe_tray::claim_menu_event(&event.id.0) {
        return;
    }
    // The app's own ids.
}));
```

The tray's muda ids carry a `fastframe-tray:` prefix, so they never collide
with the app's.

## Unsafe code

The workspace forbids unsafe code. This crate lowers that to `deny` and
allows it only in `windows.rs` (the Win32 message loop) and `macos.rs` (the
Objective-C runtime), each block with a safety note.
