//! Middle-click autoscroll, as Chromium does it: a middle press in a list,
//! then the list follows the pointer at a speed that grows with its distance
//! from the press.
//!
//! Views show their lists through [`show`] and [`show_rows`], or
//! [`show_horizontal`] for a sideways strip, which note where each one is,
//! and note a list the app scrolls itself with [`note`].
//! [`Autoscroll::finish`] resolves a press after the views have drawn and
//! moves only the list it started in: moving over another pane never hands
//! the gesture over. Speed, dead zone, cursors and the hold-or-click choice
//! follow Chromium. The click or Escape that stops a gesture does nothing
//! else, so it neither opens what is under the pointer nor closes what is
//! behind the list.

use std::time::Duration;

use egui::{
    Context, Id, LayerId, Pos2, Rect, Response, ScrollArea, Ui, Vec2, Vec2b, ViewportId,
    scroll_area::ScrollAreaOutput,
};

const FRAME_ID: &str = "fastframe-scroll-autoscroll";
/// Points the pointer may move on each axis before that axis scrolls.
const DEAD_ZONE: f32 = 15.0;
/// Chromium's speed is `distance ^ 2.2 * 0.000008` per fling unit, and its
/// fixed-velocity fling advances 5000 units a second.
const EXPONENT: f32 = 2.2;
const MULTIPLIER: f32 = 0.000_008 * 5000.0;
/// The longest step one frame may take, so a stalled frame cannot jump.
const MAX_STEP: f64 = 0.1;

/// Whether middle-click autoscroll is on: always on Windows, on Linux only
/// when the user chose it, because a middle click there usually pastes the
/// primary selection, and never on macOS.
#[must_use]
pub fn enabled(chosen_on_linux: bool) -> bool {
    enabled_for(cfg!(windows), cfg!(target_os = "linux"), chosen_on_linux)
}

const fn enabled_for(windows: bool, linux: bool, chosen_on_linux: bool) -> bool {
    windows || (linux && chosen_on_linux)
}

#[derive(Clone, Copy, Default)]
struct Input {
    viewport: ViewportId,
    focused: bool,
    pointer: Option<Pos2>,
    middle: bool,
    released: bool,
    cancel: bool,
    time: f64,
}

impl Input {
    fn read(ctx: &Context) -> Self {
        let viewport = ctx.viewport_id();
        ctx.input(|input| Self {
            viewport,
            focused: input.focused,
            pointer: input.pointer.hover_pos(),
            middle: input.pointer.button_pressed(egui::PointerButton::Middle),
            released: input.pointer.any_released(),
            cancel: input.pointer.any_pressed()
                || input.key_pressed(egui::Key::Escape)
                || input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::MouseWheel { .. })),
            time: input.time,
        })
    }
}

#[derive(Clone, Copy)]
struct Area {
    id: Id,
    layer: LayerId,
    rect: Rect,
    /// How far the list scrolls on each axis: zero on an axis it does not.
    max: Vec2,
    input: Input,
    offset: Vec2,
    /// A `ScrollArea`'s state, which the gesture moves. A noted list has
    /// none: the app reads [`Autoscroll::offset`] and moves it.
    state: Option<egui::scroll_area::State>,
}

#[derive(Clone, Default)]
struct Observations {
    enabled: bool,
    root: Input,
    areas: Vec<Area>,
    /// Widgets that drag only with the primary button, pressed this frame.
    primary_drags: Vec<Id>,
}

impl Observations {
    fn released(&self) -> bool {
        self.root.released || self.areas.iter().any(|area| area.input.released)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// The middle button is down and the pointer is still in the dead zone.
    Pressed,
    /// The pointer left the dead zone with the button down: releasing stops.
    Held,
    /// The button came up inside the dead zone: the next click stops.
    Toggled,
}

#[derive(Clone, Copy, Debug)]
struct Active {
    id: Id,
    viewport: ViewportId,
    anchor: Pos2,
    /// The previous frame's time. egui's `stable_dt` is only a frame-rate
    /// prediction when nothing asked for an immediate repaint, so the
    /// elapsed time is measured instead.
    time: f64,
    /// Kept while the pointer is outside the window, as in Chromium.
    velocity: Vec2,
    /// Where the gesture has moved the list, fractions included, for a
    /// noted list that scrolls in whole rows.
    offset: Vec2,
    mode: Mode,
}

/// Middle-click autoscroll for one app, across its windows.
///
/// Call [`begin`](Self::begin) first in each frame, before
/// [`Scrolling::apply`](crate::Scrolling::apply), and
/// [`finish`](Self::finish) once the views have drawn. While a gesture is
/// under way the wheel and the touchpad must leave the lists alone:
///
/// ```no_run
/// # struct App {
/// #     scrolling: fastframe_scroll::Scrolling,
/// #     autoscroll: fastframe_scroll::autoscroll::Autoscroll,
/// #     chosen: bool,
/// # }
/// # impl App {
/// fn ui(&mut self, ui: &mut egui::Ui) {
///     let ctx = ui.ctx().clone();
///     let on = fastframe_scroll::autoscroll::enabled(self.chosen);
///     self.autoscroll.begin(&ctx, on);
///     if self.autoscroll.active() {
///         self.scrolling.stop();
///         ctx.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
///     } else {
///         self.scrolling.apply(&ctx);
///     }
///     fastframe_scroll::autoscroll::show(ui, egui::ScrollArea::vertical(), |ui| {
///         // ...
///     });
///     if self.autoscroll.finish(&ctx, on) {
///         self.scrolling.stop();
///     }
/// }
/// # }
/// ```
#[derive(Debug, Default)]
pub struct Autoscroll {
    active: Option<Active>,
    cancelled: bool,
    /// The longest wait between frames while a list moves; `None` repaints
    /// every frame.
    repaint: Option<Duration>,
}

/// Keeps the press or Escape that stops a gesture from the views, in an
/// [`egui::Plugin`] input hook: otherwise it would also open what is under
/// the pointer or close what is behind the list.
#[derive(Default)]
struct Gate {
    /// The viewport whose gesture the next press or Escape stops.
    armed: Option<ViewportId>,
    /// Whether a press or Escape stopped the gesture since the last frame.
    stopped: bool,
    /// Buttons and keys pressed to stop it, whose repeats and releases are
    /// kept from the views as well.
    held: Vec<(ViewportId, Held)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Held {
    Button(egui::PointerButton),
    Escape,
}

impl egui::Plugin for Gate {
    fn debug_name(&self) -> &'static str {
        "fastframe_scroll::autoscroll"
    }

    fn input_hook(&mut self, _ctx: &Context, input: &mut egui::RawInput) {
        let viewport = input.viewport_id;
        input.events.retain(|event| {
            let (held, pressed, repeat) = match *event {
                egui::Event::PointerButton {
                    button, pressed, ..
                } => (Held::Button(button), pressed, false),
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed,
                    repeat,
                    ..
                } => (Held::Escape, pressed, repeat),
                _ => return true,
            };
            if let Some(index) = self.held.iter().position(|&kept| kept == (viewport, held)) {
                // A fresh press means the release never came, as when the
                // window lost focus first: it is not the stopping press's.
                if !pressed || !repeat {
                    self.held.swap_remove(index);
                }
                if !pressed || repeat {
                    return false;
                }
            }
            if pressed && self.armed == Some(viewport) {
                self.armed = None;
                self.stopped = true;
                self.held.push((viewport, held));
                return false;
            }
            true
        });
    }
}

fn gate<R>(ctx: &Context, change: impl FnOnce(&mut Gate) -> R) -> R {
    change(&mut ctx.plugin_or_default::<Gate>().lock())
}

impl Autoscroll {
    /// Repaints at most every `interval` while a list moves, instead of
    /// every frame, for an app whose renderer is not paced by the display.
    /// egui takes one predicted frame off the wait, so ask for two frames'
    /// worth.
    #[must_use]
    pub fn with_repaint_interval(mut self, interval: Duration) -> Self {
        self.repaint = Some(interval);
        self
    }

    /// Whether a gesture is under way.
    #[must_use]
    pub fn active(&self) -> bool {
        self.active.is_some()
    }

    /// Whether the gesture under way scrolls the list with this id, the
    /// `ScrollAreaOutput::id` of a list shown through [`show`] or
    /// [`show_rows`], or the id given to [`note`]. A list that sticks to its
    /// end, or follows something on its own, lets go while this holds, as it
    /// would for a wheel turn.
    #[must_use]
    pub fn scrolls(&self, id: Id) -> bool {
        self.active.is_some_and(|active| active.id == id)
    }

    /// Where the gesture under way has moved the list noted under this id
    /// with [`note`]. The app scrolls the list there; a list shown through
    /// [`show`] or [`show_rows`] moves on its own.
    #[must_use]
    pub fn offset(&self, id: Id) -> Option<Vec2> {
        self.active
            .filter(|active| active.id == id)
            .map(|active| active.offset)
    }

    /// Stops the gesture if its window lost focus, for frames the app runs
    /// without drawing, such as a hidden window's: focus changes still
    /// arrive then. Returns whether it stopped one.
    pub fn cancel_if_unfocused(&mut self, ctx: &Context) -> bool {
        let input = Input::read(ctx);
        if self
            .active
            .is_some_and(|active| active.viewport == input.viewport && !input.focused)
        {
            self.active = None;
            gate(ctx, |gate| gate.armed = None);
            true
        } else {
            false
        }
    }

    /// Starts the frame: a click, Escape, a wheel turn, losing focus, or
    /// `enabled` turning false stops the gesture. Call it before
    /// [`Scrolling::apply`](crate::Scrolling::apply) and before any list is
    /// shown.
    pub fn begin(&mut self, ctx: &Context, enabled: bool) {
        let root = Input::read(ctx);
        let stopped = gate(ctx, |gate| std::mem::take(&mut gate.stopped));
        self.cancelled = self.active.is_some_and(|active| {
            !enabled
                || stopped
                || root.cancel
                || (active.viewport == root.viewport && !root.focused)
        });
        if self.cancelled {
            self.active = None;
        }
        ctx.data_mut(|data| {
            data.insert_temp(
                Id::new(FRAME_ID),
                Observations {
                    enabled,
                    root,
                    ..Default::default()
                },
            );
        });
    }

    /// Ends the frame, after the views have drawn: a middle press in a list
    /// starts a gesture, and a gesture under way moves its list. Returns
    /// whether the gesture owns this frame's scrolling, its last frame
    /// included, so the app can stop a touchpad glide that would otherwise
    /// resume behind it.
    pub fn finish(&mut self, ctx: &Context, enabled: bool) -> bool {
        let owned = self.step(ctx, enabled);
        let armed = self.active.map(|active| active.viewport);
        gate(ctx, |gate| gate.armed = armed);
        owned
    }

    fn step(&mut self, ctx: &Context, enabled: bool) -> bool {
        let Some(frame) = ctx.data_mut(|data| data.remove_temp::<Observations>(Id::new(FRAME_ID)))
        else {
            self.active = None;
            return false;
        };
        let owned = self.active() || self.cancelled;
        if !enabled || self.cancelled {
            self.active = None;
            return owned;
        }
        if let Some(mut active) = self.active {
            let owner = frame
                .areas
                .iter()
                .find(|area| area.id == active.id && area.input.viewport == active.viewport);
            let Some(owner) = owner.filter(|area| {
                area.input.focused && area.rect.is_positive() && area.max != Vec2::ZERO
            }) else {
                self.active = None;
                return true;
            };
            if let Some(pointer) = owner.input.pointer {
                active.velocity = scroll_velocity(pointer - active.anchor);
            }
            if active.mode == Mode::Pressed && active.velocity != Vec2::ZERO {
                active.mode = Mode::Held;
            }
            if frame.released() {
                match active.mode {
                    Mode::Pressed => active.mode = Mode::Toggled,
                    Mode::Held => {
                        self.active = None;
                        return true;
                    }
                    Mode::Toggled => {}
                }
            }
            if frame.root.cancel || frame.areas.iter().any(|area| area.input.cancel) {
                self.active = None;
                return true;
            }
            let dt = (owner.input.time - active.time).clamp(0.0, MAX_STEP) as f32;
            active.time = owner.input.time;
            let mut velocity = active.velocity;
            if owner.max.x <= 0.0 {
                velocity.x = 0.0;
            }
            if owner.max.y <= 0.0 {
                velocity.y = 0.0;
            }
            let moved = if let Some(drawn) = owner.state {
                // A list may move its own offset after drawing, as a chat
                // does to keep the rows on screen still while rows above
                // them change height, so each step starts from the stored
                // offset.
                let mut state = egui::scroll_area::State::load(ctx, owner.id).unwrap_or(drawn);
                let offset = (state.offset + velocity * dt).clamp(Vec2::ZERO, owner.max);
                let moved = offset != state.offset;
                if moved {
                    state.offset = offset;
                    state.store(ctx, owner.id);
                }
                active.offset = offset;
                moved
            } else {
                let offset = (active.offset + velocity * dt).clamp(Vec2::ZERO, owner.max);
                let moved = offset != active.offset;
                active.offset = offset;
                moved
            };
            if moved {
                match self.repaint {
                    Some(interval) => ctx.request_repaint_after(interval),
                    None => ctx.request_repaint(),
                }
            }
            if owner.input.viewport == ctx.viewport_id() {
                ctx.set_cursor_icon(cursor(velocity, owner.max));
            }
            self.active = Some(active);
            return true;
        }
        let Some(position) = frame.root.pointer.filter(|_| frame.root.middle) else {
            return false;
        };
        // Only the top layer under the press may own it, so a menu or dialog
        // keeps the list beneath it still.
        let layer = ctx
            .layer_id_at(position)
            .unwrap_or_else(LayerId::background);
        if claimed(ctx, &frame, layer, position) {
            return false;
        }
        // Nested lists finish first. An inner list owns the press if it can
        // scroll; otherwise the list around it may.
        let Some(owner) = frame.areas.iter().find(|area| {
            area.input.viewport == frame.root.viewport
                && area.layer == layer
                && area.input.focused
                && area.max != Vec2::ZERO
                && area.rect.contains(position)
        }) else {
            return false;
        };
        self.active = Some(Active {
            id: owner.id,
            viewport: frame.root.viewport,
            anchor: position,
            time: owner.input.time,
            velocity: Vec2::ZERO,
            offset: owner.offset,
            mode: if frame.released() {
                Mode::Toggled
            } else {
                Mode::Pressed
            },
        });
        // The press is the gesture's now: the widget under it must not start
        // a drag with the middle button, and egui's text selection, which
        // follows any held button until it is released, must not select what
        // scrolls past.
        ctx.stop_dragging();
        if let Some(selection) = ctx.plugin_opt::<egui::text_selection::LabelSelectionState>() {
            selection.lock().clear_selection();
        }
        ctx.set_cursor_icon(cursor(Vec2::ZERO, owner.max));
        true
    }
}

/// Whether a widget under the press keeps it: a text field, for middle-click
/// paste, or anything that drags with any button, such as a slider, a seek
/// bar, a scroll bar or a panel edge. Widgets marked with [`primary_drag`]
/// drag only with the primary button.
fn claimed(ctx: &Context, frame: &Observations, layer: LayerId, position: Pos2) -> bool {
    let hits = ctx.interaction_snapshot(|snapshot| {
        snapshot
            .contains_pointer
            .iter()
            .copied()
            .collect::<Vec<_>>()
    });
    hits.into_iter()
        .filter_map(|id| ctx.read_response(id))
        .any(|hit| {
            hit.enabled()
                && hit.layer_id == layer
                && hit.interact_rect.contains(position)
                && (egui::text_edit::TextEditState::load(ctx, hit.id).is_some()
                    || (hit.sense.senses_drag()
                        && !frame.primary_drags.contains(&hit.id)
                        // A touch screen lets the area itself drag to scroll.
                        && !frame.areas.iter().any(|area| hit.id == area.id.with("area"))))
        })
}

/// Chromium's autoscroll velocity, in points per second, for the pointer's
/// offset from the anchor. Each axis has its own dead zone.
fn scroll_velocity(distance: Vec2) -> Vec2 {
    let axis = |distance: f32| {
        if distance.abs() <= DEAD_ZONE {
            0.0
        } else {
            distance.signum() * distance.abs().powf(EXPONENT) * MULTIPLIER
        }
    };
    Vec2::new(axis(distance.x), axis(distance.y))
}

/// Chromium's cursors on Linux: the four-way arrows until the list moves,
/// then the direction it is moving in, along the axes it can scroll.
fn cursor(velocity: Vec2, max: Vec2) -> egui::CursorIcon {
    use egui::CursorIcon::{
        AllScroll, ResizeEast, ResizeNorth, ResizeNorthEast, ResizeNorthWest, ResizeSouth,
        ResizeSouthEast, ResizeSouthWest, ResizeWest,
    };
    let sign = |velocity: f32, scrolls: bool| {
        if scrolls && velocity != 0.0 {
            velocity.signum() as i8
        } else {
            0
        }
    };
    match (sign(velocity.x, max.x > 0.0), sign(velocity.y, max.y > 0.0)) {
        (1, -1) => ResizeNorthEast,
        (-1, -1) => ResizeNorthWest,
        (0, -1) => ResizeNorth,
        (1, 1) => ResizeSouthEast,
        (-1, 1) => ResizeSouthWest,
        (0, 1) => ResizeSouth,
        (1, 0) => ResizeEast,
        (-1, 0) => ResizeWest,
        _ => AllScroll,
    }
}

/// Lets a middle press on a widget that drags only with the primary button,
/// such as selectable text or a row that drags to reorder, start
/// autoscroll. Such a widget senses dragging; without this, the press would
/// belong to it like a slider's.
pub fn primary_drag(ui: &Ui, response: &Response) {
    if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Middle)) {
        observing(ui, |frame| frame.primary_drags.push(response.id));
    }
}

/// Shows a vertical list as [`ScrollArea::show`] does and notes it for
/// autoscroll.
pub fn show<R>(
    ui: &mut Ui,
    area: ScrollArea,
    contents: impl FnOnce(&mut Ui) -> R,
) -> ScrollAreaOutput<R> {
    observe(ui, Vec2b::new(false, true), |ui| area.show(ui, contents))
}

/// Shows a vertical list as [`ScrollArea::show_rows`] does and notes it for
/// autoscroll.
pub fn show_rows<R>(
    ui: &mut Ui,
    area: ScrollArea,
    row_height: f32,
    rows: usize,
    contents: impl FnOnce(&mut Ui, std::ops::Range<usize>) -> R,
) -> ScrollAreaOutput<R> {
    observe(ui, Vec2b::new(false, true), |ui| {
        area.show_rows(ui, row_height, rows, contents)
    })
}

/// Shows a horizontal strip as [`ScrollArea::show`] does and notes it for
/// autoscroll, which then moves it sideways.
pub fn show_horizontal<R>(
    ui: &mut Ui,
    area: ScrollArea,
    contents: impl FnOnce(&mut Ui) -> R,
) -> ScrollAreaOutput<R> {
    observe(ui, Vec2b::new(true, false), |ui| area.show(ui, contents))
}

/// Keeps the area's id, sizing, animation and drawing, and only adds an
/// observation for [`Autoscroll::finish`].
fn observe<R>(
    ui: &mut Ui,
    axes: Vec2b,
    show: impl FnOnce(&mut Ui) -> ScrollAreaOutput<R>,
) -> ScrollAreaOutput<R> {
    let layer = ui.layer_id();
    let clip = ui.clip_rect();
    let output = show(ui);
    if observing(ui, |_| ()).is_some() {
        // The area does not say which axes it scrolls, and its inner rect is
        // the room it had, so content can overflow an axis it does not
        // scroll: the caller names the axes.
        let max = (output.content_size - output.inner_rect.size()).max(Vec2::ZERO) * axes.to_vec2();
        let area = Area {
            id: output.id,
            layer,
            rect: output.inner_rect.intersect(clip),
            max,
            input: Input::read(ui.ctx()),
            offset: output.state.offset,
            state: Some(output.state),
        };
        observing(ui, |frame| frame.areas.push(area));
    }
    output
}

/// Notes a list the app scrolls itself rather than through a
/// [`ScrollArea`], such as one that scrolls in whole rows: `id` names it,
/// `rect` is where it shows, `offset` is how far it is scrolled, and `max`
/// how far it can scroll on each axis, zero on an axis it does not. While a
/// gesture moves it, [`Autoscroll::offset`] says where to.
pub fn note(ui: &Ui, id: Id, rect: Rect, offset: Vec2, max: Vec2) {
    if observing(ui, |_| ()).is_some() {
        let area = Area {
            id,
            layer: ui.layer_id(),
            rect: rect.intersect(ui.clip_rect()),
            max: max.max(Vec2::ZERO),
            input: Input::read(ui.ctx()),
            offset,
            state: None,
        };
        observing(ui, |frame| frame.areas.push(area));
    }
}

/// Changes this frame's observations while autoscroll is on, between
/// [`Autoscroll::begin`] and [`Autoscroll::finish`].
fn observing<R>(ui: &Ui, change: impl FnOnce(&mut Observations) -> R) -> Option<R> {
    ui.ctx().data_mut(|data| {
        let frame = data.get_temp_mut_or_default::<Observations>(Id::new(FRAME_ID));
        frame.enabled.then(|| change(frame))
    })
}

#[cfg(test)]
mod tests;
