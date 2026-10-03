//! Scrolling for egui apps that feels like the platform's.
//!
//! egui scrolls 40 points per wheel notch, about a third of what other apps
//! scroll, and on Linux a touchpad's deltas reach it one to one, with no
//! momentum: scrolling there feels slow next to macOS, where the system adds
//! acceleration and momentum before egui sees a delta. [`Scrolling`] makes up
//! for both, once per frame, before the app lays anything out:
//!
//! - a wheel notch scrolls [`WHEEL_STEP`] points, unless the app set its own
//!   step;
//! - on Linux, touchpad gestures go [`TOUCHPAD_SCALE`] times further and
//!   glide on after the fingers lift, slowing down, as on macOS;
//! - a touchpad gesture holds the axis it started on, so a vertical swipe
//!   does not drift sideways, and a modifier that turns the wheel sideways
//!   (Shift by default) turns the gesture and its glide with it.
//!
//! ```no_run
//! # struct App { scrolling: fastframe_scroll::Scrolling }
//! # impl App {
//! fn ui(&mut self, ui: &mut egui::Ui) {
//!     // First thing in the frame, before any scroll area reads the input.
//!     self.scrolling.apply(ui.ctx());
//!     egui::ScrollArea::vertical().show(ui, |ui| {
//!         // ...
//!     });
//! }
//! # }
//! ```

use egui::{Context, Event, MouseWheelUnit, TouchPhase, Vec2};

/// Points a wheel notch scrolls, about what browsers and native lists scroll
/// per notch. egui's own default is 40.
pub const WHEEL_STEP: f32 = 120.0;

/// How much further a Linux touchpad gesture scrolls than its deltas say, to
/// match a macOS touchpad.
pub const TOUCHPAD_SCALE: f32 = 1.8;

/// A gesture that pauses this long, in seconds, has ended where the platform
/// never says when the fingers lift (X11). Measured on the frame's input
/// clock, so a frame that is slow to draw is not taken for a pause.
const GESTURE_GAP: f64 = 0.15;
/// The glide's exponential decay time, in seconds.
const GLIDE_DECAY: f32 = 0.35;
/// The release speed below which a lift starts no glide, points per second.
const GLIDE_START: f32 = 120.0;
/// The speed at which a glide stops, points per second.
const GLIDE_STOP: f32 = 40.0;
/// How long fingers may rest on the pad before lifting and still glide, in
/// seconds: the span the release speed is measured over.
const GLIDE_REST: f64 = 0.1;
/// A first movement this much wider than it is tall starts a sideways
/// gesture.
const SIDEWAYS: f32 = 1.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// Scrolling state for one window: the gesture in progress and its glide.
///
/// Call [`apply`](Self::apply) once per frame, before anything that scrolls.
/// It rewrites the frame's `InputState::smooth_scroll_delta`, so every
/// `ScrollArea` picks the change up without knowing about it.
#[derive(Debug)]
pub struct Scrolling {
    /// Whether Linux touchpad gestures get the scale and the glide.
    touchpad_help: bool,
    /// Whether the wheel step was looked at yet.
    wheel_checked: bool,
    /// Whether the latest scroll input came in points, from a touchpad.
    from_trackpad: bool,
    /// Recent positions of the gesture, for its speed at the lift.
    history: egui::util::History<Vec2>,
    /// Where the gesture has scrolled to so far, for the history.
    travelled: Vec2,
    /// The speed still carrying the page after the fingers lifted.
    glide: Option<Vec2>,
    /// Input time of the gesture's latest movement.
    last_movement: Option<f64>,
    /// The platform says when fingers touch and lift (Wayland does, X11 does
    /// not), so a pause with fingers resting is not taken for a lift.
    lift_announced: bool,
    /// The axis the gesture holds, and the input time it last moved on it.
    lock: Option<(Axis, f64)>,
}

impl Default for Scrolling {
    fn default() -> Self {
        Self::with_touchpad_help(cfg!(target_os = "linux"))
    }
}

impl Scrolling {
    fn with_touchpad_help(touchpad_help: bool) -> Self {
        Self {
            touchpad_help,
            wheel_checked: false,
            from_trackpad: false,
            history: egui::util::History::new(2..16, 0.1),
            travelled: Vec2::ZERO,
            glide: None,
            last_movement: None,
            lift_announced: false,
            lock: None,
        }
    }

    /// Whether the latest scroll input came from a touchpad (deltas in
    /// points) rather than a wheel (deltas in lines), for an app that pans
    /// with one and zooms with the other.
    #[must_use]
    pub fn from_trackpad(&self) -> bool {
        self.from_trackpad
    }

    /// Whether a lifted touchpad gesture is still gliding.
    #[must_use]
    pub fn gliding(&self) -> bool {
        self.glide.is_some()
    }

    /// Applies this frame's scrolling: the wheel step, the touchpad scale and
    /// glide, and the axis lock. Call it once per frame, before anything
    /// scrolls.
    pub fn apply(&mut self, ctx: &Context) {
        if !self.wheel_checked {
            self.wheel_checked = true;
            // The app's own step wins: only egui's default is replaced.
            ctx.options_mut(|options| {
                let default = egui::InputOptions::default().line_scroll_speed;
                if options.input_options.line_scroll_speed == default {
                    options.input_options.line_scroll_speed = WHEEL_STEP;
                }
            });
        }

        let options = ctx.options(|options| options.input_options);
        let wheel = ctx.input(|input| read_wheel(&input.events, &options));
        let (now, first_pass) = (ctx.input(|input| input.time), ctx.current_pass_index() == 0);
        let moved = wheel.delta != Vec2::ZERO;
        self.lift_announced |= wheel.announced;
        if moved {
            self.from_trackpad = wheel.in_points;
        }

        let helped = self.touchpad_help && self.from_trackpad;
        if helped {
            ctx.input_mut(|input| input.smooth_scroll_delta *= TOUCHPAD_SCALE);
        }
        if helped && moved {
            self.glide = None;
            self.travelled += wheel.delta * TOUCHPAD_SCALE;
            self.history.add(now, self.travelled);
            self.last_movement = Some(now);
            // Where nothing says when the fingers lift, the quiet-gap check
            // below needs a frame to run in.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(GESTURE_GAP / 2.0));
        } else if moved || ctx.input(|input| input.pointer.any_down()) {
            // A wheel notch or a press stops a glide.
            self.glide = None;
            self.history.clear();
            self.last_movement = None;
        }

        // A redone pass (after `Context::request_discard`) gets no input
        // events: that is no pause in the gesture, and the frame's glide step
        // was already taken. Where the platform announces the lift, resting
        // fingers are not taken for one either.
        let quiet = first_pass
            && !self.lift_announced
            && self.last_movement.is_some_and(|at| now - at > GESTURE_GAP);
        if wheel.lifted || quiet {
            self.release(now, wheel.lifted);
        }
        if let Some(speed) = self.glide {
            if !moved && first_pass {
                let dt = ctx.input(|input| input.stable_dt).clamp(0.001, 0.05);
                ctx.input_mut(|input| input.smooth_scroll_delta += speed * dt);
                let slower = speed * (-dt / GLIDE_DECAY).exp();
                self.glide = (slower.length() > GLIDE_STOP).then_some(slower);
            }
            ctx.request_repaint();
        }

        self.lock_axis(ctx, &wheel, now);
    }

    /// The fingers lifted (or, without announcements, paused): only the
    /// movement just before carries on as a glide.
    fn release(&mut self, now: f64, announced: bool) {
        let rested = announced
            && self
                .history
                .iter()
                .last()
                .is_some_and(|(time, _)| now - time > GLIDE_REST);
        let mut speed = if rested {
            Vec2::ZERO
        } else {
            self.history.velocity().unwrap_or(Vec2::ZERO)
        };
        match self.lock.map(|(axis, _)| axis) {
            Some(Axis::Horizontal) => speed.y = 0.0,
            Some(Axis::Vertical) => speed.x = 0.0,
            None => {}
        }
        self.glide = (speed.length() > GLIDE_START).then_some(speed);
        self.history.clear();
        self.travelled = Vec2::ZERO;
        self.last_movement = None;
    }

    /// Holds a touchpad gesture on the axis it started on. Separate wheel
    /// notches may change direction at once, including when the modifier
    /// that turns them sideways is pressed or released.
    fn lock_axis(&mut self, ctx: &Context, wheel: &Wheel, now: f64) {
        let moved = wheel.delta != Vec2::ZERO;
        let held = wheel.forced.or_else(|| {
            self.lock
                .filter(|(_, at)| now - at < GESTURE_GAP && (!moved || wheel.in_points))
                .map(|(axis, _)| axis)
        });
        let axis = match held {
            Some(axis) => axis,
            None if moved && wheel.delta.x.abs() > wheel.delta.y.abs() * SIDEWAYS => {
                Axis::Horizontal
            }
            None if moved => Axis::Vertical,
            None => {
                self.lock = None;
                return;
            }
        };
        if moved {
            self.lock = Some((axis, now));
        }
        ctx.input_mut(|input| match axis {
            Axis::Horizontal => input.smooth_scroll_delta.y = 0.0,
            Axis::Vertical => input.smooth_scroll_delta.x = 0.0,
        });
    }
}

/// What a frame's wheel events add up to.
#[derive(Debug, Default)]
struct Wheel {
    /// The movement, turned the way egui turns it for the modifiers.
    delta: Vec2,
    /// Whether any delta came in points, as a touchpad's do.
    in_points: bool,
    /// Whether the fingers lifted.
    lifted: bool,
    /// Whether the platform said when fingers touched or lifted.
    announced: bool,
    /// The axis a modifier forces, as egui applies it to the smooth delta.
    forced: Option<Axis>,
}

fn read_wheel(events: &[Event], options: &egui::InputOptions) -> Wheel {
    let mut wheel = Wheel::default();
    for event in events {
        if let Event::MouseWheel {
            unit,
            delta,
            phase,
            modifiers,
        } = event
        {
            let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
            let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
            wheel.forced = match (horizontal, vertical) {
                (true, false) => Some(Axis::Horizontal),
                (false, true) => Some(Axis::Vertical),
                _ => None,
            };
            wheel.delta += match wheel.forced {
                Some(Axis::Horizontal) => Vec2::new(delta.x + delta.y, 0.0),
                Some(Axis::Vertical) => Vec2::new(0.0, delta.x + delta.y),
                None => *delta,
            };
            wheel.in_points |= *unit == MouseWheelUnit::Point;
            wheel.lifted |= matches!(phase, TouchPhase::End | TouchPhase::Cancel);
            wheel.announced |= *phase != TouchPhase::Move;
        }
    }
    wheel
}

#[cfg(test)]
mod tests;
