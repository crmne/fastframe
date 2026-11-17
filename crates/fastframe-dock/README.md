# fastframe-dock

The application's Dock icon, whether macOS shows it at all, and the unread
badge, independent of its windows.
Extracted from ZapFast's shipped desktop badges.

```no_run
// At startup on the main thread, use your embedded high-resolution PNG or ICNS.
// fastframe_dock::set_icon(include_bytes!("../assets/icon.png"))?;
let mut badge = fastframe_dock::Badge::new("my-app");
badge.set(12);
badge.set(0); // Clear the badge.
```

`set_icon` fixes the generic executable Dock image for unbundled macOS runs,
including `cargo run --release`. It leaves a packaged `.app`'s icon alone and
does nothing on Windows or Linux, whose existing executable/window icons apply.
It does not rename the process or invent bundle identity for notifications.

## Hiding the Dock icon

```no_run
let show_in_dock = false; // The app's own setting.
// At startup, before `eframe::run_native`, when the setting hides it:
if !show_in_dock {
    fastframe_dock::set_dock_icon_visible(false);
}
// And whenever the setting changes, or a window is made:
fastframe_dock::set_dock_icon_visible(show_in_dock);
```

`set_dock_icon_visible(false)` turns a macOS app into a menu-bar app while it
runs (AppKit's `Accessory` activation policy), and `true` makes it an
ordinary app again (`Regular`). `Info.plist`'s `LSUIElement` does the same
but cannot change while the app runs. Call it on the main thread. It returns
whether the policy changed, and does nothing on Windows and Linux.

- Without a Dock icon the app is also out of Cmd-Tab and shows no menu bar
  of its own. Its windows still take the focus when clicked or raised, and
  the main menu's key equivalents (Cmd-Q, Cmd-C, Cmd-comma) still reach it.
  `Badge` labels go nowhere until the icon is back.
- Keep a status item (`fastframe-tray`) while the icon is hidden: with no
  Dock icon and no window, it is the only way back. The Dock reopen handler
  still runs when the app is opened again from Finder or Launchpad.
- An app that was active is activated again after the switch, so its
  window stays in front and keeps the focus, and returning to the Dock gives
  the menu bar back to it.
- To start without the icon, call it before the event loop. A packaged
  `.app` keeps that; the Dock may show the icon for a moment as the app
  launches. For an unbundled run (`cargo run`), winit sets `Regular` once
  AppKit finishes launching, so call it again when the first window exists.

## Badges

Keep one `Badge` with the application, across window closure and recreation.
Call `set` on the UI thread. Counts are drawn natively on macOS; Linux emits
the Unity Launcher API on a background connection for Plasma and compatible
launchers. Pass the desktop entry without `.desktop`, using
`fastframe_desktop::desktop_entry` when Flatpak identity applies. Unsupported
Linux launchers simply do not show a badge. Zero clears it.

Windows uses a per-window `Taskbar` in addition to the application `Badge`:
call `Taskbar::show(&window, count, &localized_description)` when the count is
known, and schedule another repaint at the returned retry time if present.
The description is app-owned accessible text, such as "3 unread messages".
Create a fresh `Taskbar` for every recreated window. It watches Explorer's
taskbar recreation, retries failures with bounded backoff, and displays `99+`
for larger counts. No taskbar button exists while every window is closed;
keep the application count and apply it when a window returns.

The `Taskbar` interface is a no-op on other platforms. Linux counts cap at
9999; macOS displays the full number. This crate never requests notification
permission or chooses what an app should count. macOS shows the Dock badge of
an app registered with Notification Center only while badges are allowed:
fastframe-notifications asks for them with alerts and sounds.
