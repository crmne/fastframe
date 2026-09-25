# fastframe-shell

Keep an egui app running without a window, recreate the window on demand,
and bring back windows restored off-screen.

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

- `Waker` repaints whichever window exists, from any thread, and does
  nothing while none does.
- A hidden start (`start_hidden(true)`) calls `Resident::start_hidden`, which
  can refuse (no tray, so no way back). Anything that waits for a first frame
  must be released there: ZapFast's backend waited for one, so a hidden start
  at login connected nothing until the window was shown (ZapFast 5da9d38).
- Headless ticks run every 150 ms (`HEADLESS_TICK`), idling through the
  function given to `.idle`. On macOS pass `fastframe_tray::idle`, which runs
  AppKit's loop so the menu-bar item keeps answering.
- If `run_native` fails, `run` returns the error and the app is dropped
  without `shutdown`, as the apps' `?` did.

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
counts as visible. Nothing happens on Wayland (no global positions), on
macOS (which keeps windows on screen itself), or for a minimized window.
`window::recovered_position` is the pure decision, for apps that check a
saved position themselves.

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
- Chat with Work: a Unix socket or a Windows named pipe.

They differ in transport, in what they carry, and in their compatibility
promises to older installed versions, so unifying them would change the
behaviour of at least two apps. It stays in the apps until two of them
converge on one design. The shell does not need it: the app reports "show"
requests through `headless_frame` like any other.

Chat with Work's window loop is also different (`pump_app_events` on one
winit loop with its tray), so it is not a user of this crate yet.
