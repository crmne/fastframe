# fastframe-power

The device's power, session and network state for an egui app: whether the
user asked to save energy, the battery, the display, the session lock, the
network's reach and cost, and sleep, wake and the end of the session. It
also keeps audio playing behind another app's window out of App Nap on
macOS.

A thin reader per platform writes what the system reports into shared
`Conditions` and queues `Event`s, then calls the app's wake function. The
app reads both in its logic pass, which runs with or without a window, and
keeps its own policy: how much to draw and poll, what to hold back on a
metered connection, what to save before sleeping.

It came out of Spotifast, which follows all of this to draw and poll less
when nobody can see it, and to save its state before the machine sleeps.

## Usage

```rust
use fastframe_power::{Activity, Event, Power};

let mut power = Power::spawn(move || waker.wake());
let mut playing_activity: Option<Activity> = None;

// In the logic pass, window or not:
let conditions = power.conditions();
let nobody_sees = !window_shown || conditions.display_off || conditions.locked;
let budget = if nobody_sees {
    Budget::Background // draw nothing, poll slowly
} else if conditions.saver || !focused {
    Budget::Saver // animations at half rate
} else {
    Budget::Normal
};
power.set_background(nobody_sees); // EcoQoS on Windows

let hold_downloads = !conditions.online || conditions.metered;

for event in power.events() {
    match event {
        Event::Suspending(ack) | Event::EndingSession(ack) => {
            save_state();
            ack.done();
        }
        Event::Resumed => reconnect(),
    }
}

// macOS: keep playback at full speed behind other windows, and let a
// paused app nap again.
if player.is_playing() {
    playing_activity.get_or_insert_with(|| Activity::begin("Playing audio"));
} else {
    playing_activity = None;
}
```

`Budget` above is the app's own: the crate reports the conditions and
leaves the decision to the app.

## What each platform reports

| | Linux | Windows | macOS |
| --- | --- | --- | --- |
| `saver` | the power-saver profile (desktop portal, or power-profiles-daemon) | Energy Saver or battery saver | Low Power Mode |
| `on_battery` | | yes (a UPS counts as battery) | |
| `display_off` | | every display off (dimmed is on) | |
| `locked` | | locked, or switched away from or disconnected | |
| `online` | | the network connectivity hint | SCNetworkReachability |
| `metered` | | a fixed or variable cost, roaming, or over the data limit | |
| `Suspending` | logind, with a delay lock | the suspend callback | |
| `Resumed` | logind | the resume callback | |
| `EndingSession` | | `WM_ENDSESSION` | |

What a platform does not report keeps its default, which is the
unconstrained case: online, not metered, no saver, on mains power, display
on, unlocked. An app that holds something back on a condition never holds
it back because a platform says nothing.

## Sleep, wake and the end of the session

`Suspending` and `EndingSession` carry an `Ack`. Save, then call
`ack.done()`: the reader holds the system back until then, for at most
1.5 seconds before sleep (Windows gives its suspend callback about two) and
3 seconds before signing out or shutting down (Windows lists a window as
blocking after about five). Call `events()` in every logic pass, including
the headless ones, so the answer comes in time.

The logic pass may still come too late as the display goes off. For work
that must not wait for it, such as pausing playback, `on_suspending` has
the reader run a hook itself before it queues `Suspending`. Keep it short
and safe on another thread: a hook that blocks holds back the sleep and the
events after it. The reader also notes the wall-clock time, and
`take_suspended_at()` returns it once, so on waking the app can tell how
long it slept, within the 1.5 second wait, even on Windows, where `Instant`
counts the suspend. The wall clock can be set back meanwhile, so allow for
`duration_since` failing. Both work where a reader reports the sleep, on
Linux and Windows.

`Resumed` says the machine woke up. A connection open across the sleep can
still look established while nothing arrives, so reconnect it then rather
than waiting for a keepalive to time out. This replaces guessing a sleep
from the wall clock running ahead of `Instant` between two ticks (as
ZapFast's link watch does): that guess never fires on Windows, where
`Instant` counts the suspend, and `Resumed` does. Keep a check for a link
that has gone silent, since a network change can wedge a connection
without any sleep. macOS does not report `Resumed` yet, so an app that
needs it there keeps its clock check on macOS for now.

## Efficiency in the background

`set_background(true)` puts the process under EcoQoS on Windows while
nobody can see the app, so its remaining work runs on efficient cores at
efficient clocks; `set_background(false)` hands the choice back to the
system as soon as a window is shown. If something else set the process's
throttling (the Task Manager's efficiency mode, say), the crate leaves it
alone for the rest of the run. An audio thread keeps its place through
MMCSS (`fastframe-audio`). On Linux and macOS this does nothing.

## App Nap

macOS slows down an app whose windows are covered or hidden while another
app is in front. Hold an `Activity` while the user is waiting on work that
has to keep pace, above all audio playing or loading, and drop it on pause,
stop, an output error or a disconnection. It allows idle system sleep and
the display to turn off: it does not keep the Mac awake. Elsewhere it does
nothing.

## Tests and demo runs

`Power::fixed(conditions)` starts no reader, so the device's own state
never leaks into a test or a demo. `set_conditions` and `send` change it as
a reader would:

```rust
let power = Power::fixed(Conditions::default());
power.set_conditions(Conditions { metered: true, ..Conditions::default() });
let ack = Ack::default();
power.send(Event::Suspending(ack.clone()));
// The app's logic pass answers it:
for event in power.events() {
    if let Event::Suspending(answer) = event {
        answer.done();
    }
}
assert!(ack.is_done());
```

`send` runs the `on_suspending` hook and notes the time for `Suspending`,
as a reader does, so a test can check what the app does on both.

`cargo run -p fastframe-power --example watch` prints the conditions and
events as the running system reports them.

## Platform notes

- **Linux:** each reader runs on a thread of its own over D-Bus. Inside
  Flatpak the power profile comes through the portal; the delay before
  sleep needs `--system-talk-name=org.freedesktop.login1`, and without it
  the app still hears about the wake. The lock is listed under the
  program's name in `systemd-inhibit --list`.
- **Windows:** power settings, sleep and the network arrive through system
  callbacks; the session lock and the end of the session through a hidden
  top-level window on a thread of its own. The network hint needs Windows
  10 2004 or later and is looked up at run time, so older systems still
  start.
- **macOS:** Low Power Mode is read by `conditions()` (one message to
  NSProcessInfo) rather than followed, so a change shows on the next logic
  pass. Reachability is delivered on the main dispatch queue, which the
  app's event loop runs, and watches the internet as a whole rather than
  one server.
