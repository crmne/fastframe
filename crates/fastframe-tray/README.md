# fastframe-tray

A tray item with the app's own menu: a StatusNotifierItem on Linux (ksni), a
notification-area icon on Windows and a menu-bar item on macOS (tray-icon).

The app supplies its id, its name, its icon and the menu entries (already
translated). Clicks arrive as events on a channel, and a wake function is
called for each one so the app reads them promptly, with or without a window.

## Usage

```rust
use fastframe_tray::{Config, Event, IconColour, MenuItem, Tray};

let tray = Tray::spawn(
    Config {
        id: "spotifast",
        title: "Spotifast".into(),
        icon: util::app_icon_rgba,                 // fn(usize) -> Vec<u8>, RGBA
        template_icon: Some(util::tray_template_rgba), // macOS menu bar
        themed_icon: true,     // Linux: also name the installed app icon
        menu: vec![
            MenuItem::action("show", "Show or hide Spotifast"),
            MenuItem::Separator,
            MenuItem::action("play-pause", "Play"),
            // Hidden until the app turns it on:
            MenuItem::action("lock", "Lock Spotifast").visible(false),
            MenuItem::Separator,
            MenuItem::action("quit", "Quit"),
        ],
    },
    move || waker.wake(),
);
// None: the tray could not be made at all. On Linux a panel may come later
// (an app started at login beats it), so ask before keeping the app running:
let keep_running_on_close = tray.as_ref().is_some_and(Tray::is_shown);

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

// Labels can change, entries can be greyed out or come and go, keeping
// their place, and the icon and tooltip can change:
tray.set_label("play-pause", if playing { "Pause" } else { "Play" });
tray.set_enabled("play-pause", connected);
tray.set_visible("lock", password_set);
tray.set_icon(if online { util::tray_rgba } else { util::tray_dimmed_rgba }, None);
tray.set_tooltip("Spotifast\nPlaying: Song, by Band"); // Linux: title, then detail
// A white or black rendition, so the item matches the panel:
tray.set_icon_colour(IconColour::White);

// When a window is made (the macOS item is created by the first call):
tray.attach();
// Or, for an app that starts with no window (at login), make the macOS item
// now without bringing the app forward:
tray.create_item();

// While no window exists (runs AppKit's loop on macOS, sleeps elsewhere):
fastframe_tray::idle(std::time::Duration::from_millis(150));
```

Labels, greying, visibility, the icon and the tooltip change while the app
runs, on every platform, without making the item again.

## White and black icons

`set_icon_colour` picks the icon's rendition, for a person to match it to
their panel:

- `IconColour::Original` (the default) is the icon as configured: `icon`, and
  on macOS `template_icon` when there is one, which the menu bar draws black
  or white to match itself.
- `IconColour::White` and `IconColour::Black` paint the shape of
  `template_icon` (or of `icon`, without one) white or black, keeping its
  transparency, so the app supplies no extra artwork. They are ordinary
  images on every platform, macOS included, where the menu bar does not
  recolour them. On Linux they are sent without the installed icon's name
  (`themed_icon`), since a host that has the name would draw the colour icon.

The choice survives `set_icon`, and changes the item in place. A disabled entry (`MenuItem::enabled(false)`) is
greyed out, as for a status line. On Linux
a hidden entry stays in the StatusNotifier menu with its `visible` flag off;
tray-icon's menus have no hidden entries, so on Windows and macOS it is taken
out and put back after the shown entries before it.

## Platforms

- **Linux**: ksni on its own thread. The item registers even before a
  StatusNotifier host is up, and appears once one is; `is_shown` says
  whether one shows it now (false on a desktop without a tray, such as
  stock GNOME). Inside Flatpak (`/.flatpak-info` or `FLATPAK_ID`) the item
  registers its unique bus name, since the sandbox does not let it own one.
  With `themed_icon`, the item also names the installed app icon, for hosts
  that draw only named icons; turn it off when the tray icon is a glyph or
  changes, or those hosts draw the app icon instead. Labels escape `_`,
  which DBusMenu would read as a shortcut marker. A left click is
  `Event::Toggle`.
- **Windows**: tray-icon on its own thread with a message loop. A left click
  is `Event::Show`: the two releases of a double-click arrive separately,
  possibly on both sides of window creation, and a toggle would hide the
  window again. The menu opens on right click only.
- **macOS**: status items only exist on the main thread while AppKit's loop
  runs, so the first `attach` makes the item, and each `attach` brings the
  app forward; `create_item` makes it without activating the app, for a
  start with no window. The icon is drawn at 36 pixels, the menu bar's
  18 points at 2x. Either click opens the menu. The Dock reopen handler is
  installed again when the first window attaches after a hidden start, once
  winit's application delegate exists. A click on the Dock icon is
  `Event::Show`, even for a minimized
  window (AppKit calls that visible). While headless, `idle` runs
  `-[NSApplication run]` in slices, which catches Objective-C exceptions
  raised while handling an event; a hand-written event loop let them unwind
  into Rust and abort the process (ZapFast #199).
- On Windows, while headless, `idle` answers the main thread's window
  messages as it waits, so Windows does not call the app hung (winit keeps a
  window on that thread between windows).
- Anywhere else, `spawn` returns `None` and `idle` sleeps.

## The Dock alone (macOS)

A macOS app that keeps running with its window closed may want no menu-bar
item: the Dock icon already shows that it runs. `Dock` hears clicks on the
Dock icon without making an item:

```rust
let mut dock = fastframe_tray::Dock::spawn(move || waker.wake());
dock.attach(); // when a window has been made
for event in dock.events() {
    if event == fastframe_tray::Event::Show { /* open or raise the window */ }
}
// While no window exists:
fastframe_tray::idle(std::time::Duration::from_millis(150));
```

A Dock click is `Event::Show`, as with a tray. `attach` installs the reopen
handler on winit's application delegate, which exists once the first window
has been made, and later calls change nothing. Elsewhere a `Dock` reports
nothing and its calls do nothing, so the app can hold one on every platform
and bring the window back through a `Tray` there. Use one or the other on
macOS: both install the same handler, and the last to attach hears the
clicks.

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
