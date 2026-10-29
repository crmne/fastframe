# fastframe-scroll

Scrolling for egui apps that feels like the platform's.

egui scrolls 40 points per wheel notch, about a third of what other apps
scroll. On Linux, a touchpad's deltas reach egui one to one and stop dead
when the fingers lift, so scrolling there feels slow next to macOS, where
the system adds acceleration and momentum first. `Scrolling` makes up for
both:

- **A wheel notch scrolls 120 points** (`WHEEL_STEP`) on every platform,
  unless the app set its own `line_scroll_speed` first.
- **Linux touchpad gestures go 1.8 times further** (`TOUCHPAD_SCALE`) and
  **glide on after the fingers lift**, slowing down until they stop. A
  press or a wheel notch stops a glide. Fingers that rest on the pad before
  lifting do not fling the page.
- **A touchpad gesture holds the axis it started on**, so a vertical swipe
  does not drift sideways. The modifier that turns the wheel sideways
  (Shift by default) turns a gesture and its glide with it, and separate
  wheel notches change direction at once.

macOS touchpads already have the system's acceleration and momentum, and
Windows sends precision touchpads as wheel lines, so neither is scaled or
given a glide. Scroll direction (natural scrolling) is the system's setting,
which the deltas already follow.

It came out of Spotifast and ZapFast, which each had a copy. This one keeps
the fixes each had made on its own: resting fingers do not fling the page
(Spotifast), and gestures are timed on the frame's input clock, so a slow
frame or a redone pass is not taken for the fingers lifting (ZapFast).

## Usage

```rust
struct App {
    scrolling: fastframe_scroll::Scrolling,
}

impl App {
    fn ui(&mut self, ui: &mut egui::Ui) {
        // First thing in the frame, before any scroll area reads the input.
        self.scrolling.apply(ui.ctx());
        egui::ScrollArea::vertical().show(ui, |ui| {
            // ...
        });
    }
}
```

`apply` rewrites the frame's `smooth_scroll_delta`, so every `ScrollArea`
gets the change without knowing about it. Keep one `Scrolling` per window.

`stop()` ends the gesture in progress, its glide and its axis, for an app
that takes scrolling over (middle-click autoscroll, say). The wheel step is
checked every frame, so a window made again with a new egui context gets it
too.

`from_trackpad()` says whether the latest scroll input came from a touchpad,
for a view that pans with one and zooms with a wheel. `gliding()` says
whether a lifted gesture is still carrying the page, for an app that routes
a gesture to the pane it began over.

## Middle-click autoscroll

`autoscroll` is Chromium's middle-click autoscroll: a middle press in a
list, then the list follows the pointer, faster the further it moves from
the press (`0.04 * distance^2.2` points a second, after a 15-point dead zone
on each axis), with Chromium's cursors. Holding the button and releasing it
after moving stops; after a plain click it keeps going until the next click,
Escape, a wheel turn, or the window losing focus. That click or Escape only
stops it: the views never see it, so it neither opens what is under the
pointer nor closes what is behind the list.

The gesture belongs to the innermost list under the press that can scroll,
on the top layer there, so a menu keeps the list beneath it still, and
moving over another pane never hands it over. Text fields keep a middle
press for pasting, and anything that drags with any button (a slider, a
scroll bar, a panel edge) keeps it too.

`enabled(chosen_on_linux)` says whether it is on: always on Windows, on
Linux only by choice, because a middle click there usually pastes, and never
on macOS.

```rust
use fastframe_scroll::autoscroll;

struct App {
    scrolling: fastframe_scroll::Scrolling,
    autoscroll: autoscroll::Autoscroll,
    chosen: bool,
}

impl App {
    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let on = autoscroll::enabled(self.chosen);
        self.autoscroll.begin(&ctx, on);
        if self.autoscroll.active() {
            self.scrolling.stop();
            ctx.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
        } else {
            self.scrolling.apply(&ctx);
        }
        autoscroll::show(ui, egui::ScrollArea::vertical(), |ui| {
            // ...
        });
        if self.autoscroll.finish(&ctx, on) {
            self.scrolling.stop();
        }
    }
}
```

Lists show through `show` and `show_rows`, or `show_horizontal` for a
sideways strip. A widget that senses dragging but drags only with the
primary button, such as selectable text or a row that drags to reorder,
calls `primary_drag` so a middle press on it still starts autoscroll; egui's
text selection is cleared when it does. A list that scrolls itself rather
than through a `ScrollArea`, such as one that scrolls in whole rows, is
noted with `note`, and `Autoscroll::offset` says where to scroll it.
`Autoscroll::scrolls(id)` tells a list that sticks to its end, or follows
something on its own, to let go, as it would for a wheel turn.

A moving list repaints every frame. An app whose renderer is not paced by
the display asks for fewer with `Autoscroll::with_repaint_interval`.

It came out of Spotifast and ZapFast. Each step starts from the list's
stored offset, so a list that moves itself after drawing, as a chat does
when rows above the screen change height, keeps that move (ZapFast).

## How a gesture ends

Wayland says when fingers touch and lift, and the glide starts at the lift,
from the speed of the last 100 ms of movement. X11 never says, so there a
gesture that pauses for 150 ms has ended and glides. Pauses are measured on
egui's input time, which a slow frame and a redone pass do not advance.
