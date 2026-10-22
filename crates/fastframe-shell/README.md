# fastframe-shell

Keep an egui app running without a window, recreate the window on demand,
open it where it was, and bring back windows restored off-screen.

eframe ties the event loop to the window. ZapFast and Spotifast keep working
with the window closed (the connection, playback, the tray, notifications),
so they run `eframe::run_native` in a loop: closing the window with "keep
running" on destroys it, the app's state waits in a slot, and a headless loop
keeps calling the app until the tray, a notification or another launch asks
for the window again. This crate is that loop.

## Usage

The app implements `Resident`:

```rust
impl fastframe_shell::Resident for App {
    fn closed(&self) -> Closed {
        if !self.quit_requested && self.hide_intent { Closed::Hide } else { Closed::Quit }
    }
    fn window_gone(&mut self) { /* reset hide_intent, wants_show, per-window state */ }
    fn headless_frame(&mut self, ctx: &egui::Context) -> Headless {
        self.background_frame(ctx);
        if self.quit_requested { Headless::Quit }
        else if self.wants_show { Headless::Show }
        else { Headless::Wait }
    }
    fn start_hidden(&mut self) -> bool {
        if !self.hides_to_tray() { return false; }
        self.hide_intent = true;
        // Release whatever waits for a first frame (the backend's startup).
        true
    }
    fn shutdown(&mut self) { /* stop threads, save */ }
}
```

`main` hands the app to a `Shell` and makes each window with `run_native`:

```rust
let waker = fastframe_shell::Waker::default();
let app = App::new(&waker, ...);   // threads and the tray hold `waker`
fastframe_shell::Shell::new(app, &waker)
    .start_hidden(cli.start_hidden)
    .idle(fastframe_tray::idle)
    .run(|lease| {
        eframe::run_native("ZapFast", native_options(), Box::new(move |cc| {
            let mut app = lease.take(&cc.egui_ctx); // attaches the waker
            app.attach(&cc.egui_ctx);
            Ok(Box::new(Window { app, recovery_checked: false }))
        }))
    })
```

The window's `eframe::App` keeps the `Held<App>` (it derefs to the app).
When eframe drops it with the window, the app goes back to the shell, which
asks `closed()` what to do next: quit, run headless, or open the next window
at once (`Closed::Reopen`, for switching to a different kind of window).
Before `run_native`, `lease.peek(|app| ..)` reads the app to choose that
window's options, such as which kind of window it is.

- `Waker` repaints whichever window exists, from any thread. While none
  does, it ends the shell's sleep between headless ticks instead.
- A hidden start (`start_hidden(true)`) calls `Resident::start_hidden`, which
  can refuse (no tray, so no way back). Anything that waits for a first frame
  must be released there: ZapFast's backend waited for one, so a hidden start
  at login connected nothing until the window was shown (ZapFast 5da9d38).
- Headless ticks run every 150 ms (`HEADLESS_TICK`) for `Headless::Wait`, or
  after the app's own `Headless::WaitFor(duration)` when nothing is due
  sooner, and at once when the `Waker` wakes. Without `.idle` the shell
  sleeps on the waker in between. On Windows that sleep also answers the
  main thread's window messages: winit keeps a visible event-target window
  there between windows, and Windows calls a thread that leaves it
  unanswered for five seconds hung ("not responding", `Process.Responding`
  false) for as long as the app sits in the tray. On macOS pass `fastframe_tray::idle`,
  which runs AppKit's loop so the menu-bar item keeps answering; it runs in
  ticks, so there a wake is seen at the next one.
- Each headless tick is one egui pass on the headless context, so what a
  tick asks of it (repaints, viewport commands, textures) is let go when the
  tick ends. Without passes, egui kept every repaint request for as long as
  the app sat in the tray (Spotifast #954).
- If `run_native` fails, `run` returns the error and the app is dropped
  without `shutdown`, as the apps' `?` did.

## Hiding to the tray: decide in `logic`

eframe runs only `App::logic` for a minimized or occluded window; it skips
`ui`. Read the close request and set what `closed()` reads in `logic`:

```rust
fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
    if ctx.input(|input| input.viewport().close_requested())
        && !self.app.quit_requested
        && self.app.hides_to_tray()
    {
        self.app.hide_intent = true;
    }
}
```

Decided in `ui`, closing a minimized window (from the taskbar, say) reaches
the shell with no wish to hide, and the app quits instead of going to the
tray (Spotifast #725).

## Window size and place

```rust
// Before the window exists: the default size, then what was remembered.
let geometry = lease.peek(|app| app.settings.geometry);
let viewport = egui::ViewportBuilder::default().with_inner_size([1180.0, 780.0]);
let viewport = geometry.apply(viewport);

// Every frame, in `logic`:
if self.app.settings.geometry.remember(ctx) {
    self.app.mark_settings_dirty();
}
```

`window::Geometry` is the inner size, the position and whether the window
was maximized, as plain fields the app stores in its own settings format.
Applying it to the builder opens the window in place, maximized when it was,
instead of resizing and moving it after it appears. `remember` returns
whether anything changed by more than half a pixel, so the app saves only
then.

- Sizes and positions are logical pixels (points at zoom 1), so a window
  saved while the app is zoomed opens at the same size on screen. The
  builder is scaled by the new context's zoom, which is 1 unless eframe
  restores egui's saved memory (whose options carry the zoom): keep the zoom
  in the app's settings, give it to each window with `window::keep_zoom`
  (below), and leave `persist_egui_memory` off.
- A maximized window keeps the normal size and position behind it, to
  restore to. A minimized or full-screen window is not remembered.
- Wayland never reveals a position: the size is remembered and the last
  known position kept (or none, so the compositor places the window).
- egui reads macOS's maximized state only when the window is created
  (emilk/egui#3494), so there a window zoomed to fill the screen is
  remembered at that size and the flag is left alone.
- Values from a corrupt file (not finite, sizes above 16384, positions
  beyond 32768) are ignored. On macOS, a position whose title bar would be
  on no connected screen is dropped before the window is made there.

### The zoom

```rust
// Every frame, in `logic`, for each window that follows the saved zoom:
if let Some(zoom) = window::keep_zoom(ctx, self.app.settings.zoom) {
    self.app.settings.zoom = zoom;
    self.app.mark_settings_dirty();
}
```

Every new window has a new egui context, which starts at zoom 1.
`keep_zoom` gives it the saved zoom on its first call. After that it returns
the zoom the user picked with Ctrl and plus or minus, for the app to save. A
corrupt value (not a number, or outside egui's 0.2 to 5) is applied as the
nearest usable zoom and returned to be saved. A window with a scale of its
own, such as Spotifast's mini player, does not call it.

## Off-screen windows

```rust
fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
    if !std::mem::replace(&mut self.recovery_checked, true) {
        fastframe_shell::window::recover_offscreen(ctx, frame);
    }
}
```

On a window's first frame, this moves a window that no connected monitor
shows (displays rearranged, or a secondary one late after a restart) to the
middle of the primary monitor (ZapFast #171). Any overlap with any monitor
counts as visible. On macOS, which creates a borderless window wherever it
is told, the window's title bar must be on a screen's visible area (clear of
the menu bar and the Dock), checked against AppKit's screens because winit's
macOS monitor positions mix scale factors (Spotifast #719). Nothing happens
on Wayland (no global positions) or for a minimized window.
`window::recovered_position` is the pure decision, for apps that check a
saved position themselves.

## Bringing the window forward

```rust
} else if self.wayland && fastframe_shell::window::raise_on_compositor() {
    // The compositor focused the window where it is.
} else if self.wayland {
    // Close the window and let the shell open a new one.
}
```

Wayland drops a client's request to focus its own window, so apps close the
window and open a new one, which lands on the current workspace instead of
taking the user to the window. Under niri (`NIRI_SOCKET` set),
`raise_on_compositor` asks `niri msg` to focus this process's window, which
also switches to its workspace (ZapFast #474). It returns `false` elsewhere,
or when `niri` is missing or fails, so the app keeps its fallback. Hyprland
is not asked: `hyprctl dispatch` changed to Lua syntax in 0.56, so one
request does not work across versions.

## Saves on the UI thread

`Shell::run` marks its thread, which runs every window and headless tick, as
the UI thread for `fastframe-file`. A `fastframe_file::write`,
`write_private` or `write_with` there then panics in a debug build and logs
an error in a release one, because it waits for the disk while the window
waits for it. Hand saves to a `fastframe_file::Writer`, which writes on a
thread of its own.

Move `fastframe-file` and `fastframe-shell` to the same tag. Two copies of
`fastframe-file` in one build keep two separate marks, and the check then
misses the app's saves.

## Stalls

A `StallWatch` times each update of the window, logic and drawing together,
and logs a warning when one takes 250 ms or more, long enough to see the
window freeze:

```rust
struct Window { app: Held<App>, stalls: fastframe_shell::StallWatch }

impl eframe::App for Window {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.stalls.logic(|| self.app.logic(ctx, frame));
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.stalls.ui(|| self.app.ui(ui));
    }
}
```

```text
WARN the window stalled for 1200 ms in one update (and 2 more times since the last warning)
```

The first update, which builds fonts and textures, is never reported. A pass
that only runs its logic (eframe skips `ui` for a minimized window) is an
update of its own. A burst of stalls makes one warning every ten seconds,
counting the ones it did not report. It came from Spotifast, where it showed
the saves that froze the window.

## Not here: single instance

A second launch should surface the running copy, but the apps do this three
different ways, sharing 3 to 13% of their code:

- ZapFast: a lock file and a Unix socket, with a token-guarded loopback port
  on Windows; verbs `show`, `ping`, `reload-themes`; a wire identity kept
  compatible with copies still running under the old name.
- Spotifast: a D-Bus well-known name and MPRIS `Raise` on Linux; an
  exclusive loopback port on macOS and Windows that also carries the Stream
  Deck plugin's verbs (play a URI, transfer playback, snapshots); Apple
  Events for links on macOS.
- Chat with Work Desktop: a Unix socket or a Windows named pipe.

They differ in transport, in what they carry, and in their compatibility
promises to older installed versions, so unifying them would change the
behaviour of at least two apps. It stays in the apps until two of them
converge on one design. The shell does not need it: the app reports "show"
requests through `headless_frame` like any other.

Chat with Work Desktop is a Tauri app, so it is not a user of this crate.

## Checking the native tray lifecycle

`cargo run -p fastframe-shell --example tray_lifecycle` opens a synthetic
window with a tray menu. Close and reopen it through the tray repeatedly,
then choose **Recreate window** to replace the window immediately. After each
transition, verify that both mouse buttons still reach the native menu where
supported and that **Show or hide** and **Quit fixture** work. On macOS also
check the Dock after minimizing and closing. Repeat with `-- --hidden` to
start without a window. This exercises real `eframe::run_native` teardown;
hiding an `NSWindow` alone does not cover it. No account or user files are used.

Winit 0.30.10 through 0.30.13 closes every AppKit window when its event loop
exits, including the window behind a macOS status item. The icon can remain
visible but stop accepting clicks. Apps need the fix in
[rust-windowing/winit#4754](https://github.com/rust-windowing/winit/pull/4754)
(backported in [crmne/winit#2](https://github.com/crmne/winit/pull/2)).
Fastframe cannot fix that ownership error by keeping the tray object alive;
its native window must survive the event loop too. Keep the application's
winit patch when running this example to verify the same dependency versions.

Starting hidden also needs
[rust-windowing/winit#4770](https://github.com/rust-windowing/winit/pull/4770):
the headless AppKit loop can finish launching before winit installs its
delegate. Without that fix, the menu works but **Show or hide** never creates
the first window. After opening and closing that first window, also verify
that clicking the Dock icon opens another window.
