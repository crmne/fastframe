//! A change of colours revealed from the middle of the window outwards.
//!
//! egui draws each frame from scratch, so the old colours are gone as soon
//! as new ones are applied. The transition keeps a picture of them instead:
//! [`Transition::begin`] asks the window for a screenshot while it still
//! shows the old colours, [`Transition::holding`] tells the app to keep them
//! until the picture arrives, and [`Transition::paint`] then lays the
//! picture over the new colours with a soft-edged circle cut out of its
//! middle that grows until the old colours are gone.
//!
//! ```no_run
//! # let ctx = egui::Context::default();
//! # let mut transition = fastframe_theme::Transition::default();
//! # let (applied, wanted) = (1, 2);
//! // Each frame, before drawing:
//! if applied != wanted {
//!     transition.begin(&ctx);
//!     if !transition.holding(&ctx) {
//!         // apply the new palette
//!     }
//! }
//! // ... draw the interface, then last:
//! transition.paint(&ctx);
//! ```

use std::sync::Arc;

use egui::{Color32, Context, Id, LayerId, Mesh, Order, Pos2, Rect, TextureHandle, Vec2};

/// How long the reveal takes, in seconds.
pub(crate) const DURATION: f64 = 0.6;
/// How long the old colours are held for a screenshot that may never come
/// (a hidden window, or a renderer without screenshots), in seconds.
const WAIT: f64 = 0.25;
/// How wide the soft edge of the circle is, in points.
const FEATHER: f32 = 48.0;
/// How many sides the circle has.
const SEGMENTS: usize = 96;

/// Marks the screenshots this transition asked for.
#[derive(Debug)]
struct Marker;

#[derive(Default)]
enum State {
    #[default]
    Idle,
    /// A screenshot was asked for at this time.
    Capturing { since: f64 },
    /// The old colours are being revealed away since `start`.
    Revealing { picture: TextureHandle, start: f64 },
}

/// A reveal of new colours from the middle of the window outwards. Keep one
/// per window, for the life of the app.
#[derive(Default)]
pub struct Transition {
    state: State,
}

impl std::fmt::Debug for Transition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self.state {
            State::Idle => "idle",
            State::Capturing { .. } => "capturing",
            State::Revealing { .. } => "revealing",
        };
        formatter
            .debug_struct("Transition")
            .field("state", &state)
            .finish()
    }
}

impl Transition {
    /// Asks for a picture of the window as this frame draws it, with the old
    /// colours. Call it when the colours are about to change; while a
    /// picture is already on its way it does nothing.
    pub fn begin(&mut self, ctx: &Context) {
        if matches!(self.state, State::Capturing { .. }) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            Marker,
        )));
        self.state = State::Capturing {
            since: ctx.input(|input| input.time),
        };
        ctx.request_repaint();
    }

    /// Whether the app should keep drawing the old colours this frame: the
    /// picture was asked for and has not arrived yet. Once it arrives, or
    /// after a short wait without it, this returns `false` and the app
    /// applies its new colours.
    pub fn holding(&mut self, ctx: &Context) -> bool {
        let State::Capturing { since } = self.state else {
            return false;
        };
        let picture = ctx.input(|input| {
            input.raw.events.iter().find_map(|event| match event {
                egui::Event::Screenshot {
                    user_data, image, ..
                } if user_data
                    .data
                    .as_ref()
                    .is_some_and(|data| data.downcast_ref::<Marker>().is_some()) =>
                {
                    Some(Arc::clone(image))
                }
                _ => None,
            })
        });
        let now = ctx.input(|input| input.time);
        if let Some(image) = picture {
            let picture = ctx.load_texture(
                "fastframe-theme-transition",
                Arc::unwrap_or_clone(image),
                egui::TextureOptions::LINEAR,
            );
            self.state = State::Revealing {
                picture,
                start: now,
            };
            ctx.request_repaint();
            return false;
        }
        if now - since > WAIT {
            self.state = State::Idle;
            return false;
        }
        ctx.request_repaint();
        true
    }

    /// Whether the old colours are still showing, as held or as the
    /// picture being revealed away.
    pub fn active(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Lays the old picture over everything drawn this frame, with its
    /// middle cleared further each frame, and asks for the next frame until
    /// it is gone. Call it last, after the interface is drawn.
    pub fn paint(&mut self, ctx: &Context) {
        let State::Revealing { picture, start } = &self.state else {
            return;
        };
        let elapsed = ctx.input(|input| input.time) - start;
        if elapsed >= DURATION {
            self.state = State::Idle;
            return;
        }
        let screen = ctx.viewport_rect();
        let progress = ease_out((elapsed / DURATION) as f32);
        let painter = ctx.layer_painter(LayerId::new(
            Order::Debug,
            Id::new("fastframe-theme-transition"),
        ));
        painter.add(reveal(screen, picture.id(), progress));
        ctx.request_repaint();
    }
}

/// Starts fast and settles, as a reveal should.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// The old picture covering `screen`, with a circle cleared from its middle
/// that has grown `progress` (0 to 1) of the way past the farthest corner.
/// The circle's edge fades over [`FEATHER`] points.
fn reveal(screen: Rect, texture: egui::TextureId, progress: f32) -> Mesh {
    let centre = screen.center();
    let corner = centre.distance(screen.min);
    let hole = progress * (corner + FEATHER);
    let radii = [hole, hole + FEATHER, corner + FEATHER * 2.0];
    let alphas = [0.0, 1.0, 1.0];
    let size = screen.size().max(Vec2::splat(1.0));
    let mut mesh = Mesh::with_texture(texture);
    for (radius, alpha) in radii.into_iter().zip(alphas) {
        let tint = Color32::WHITE.gamma_multiply(alpha);
        for segment in 0..SEGMENTS {
            let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let position = centre + Vec2::angled(angle) * radius;
            let uv = Pos2::new(
                (position.x - screen.min.x) / size.x,
                (position.y - screen.min.y) / size.y,
            );
            mesh.vertices.push(egui::epaint::Vertex {
                pos: position,
                uv,
                color: tint,
            });
        }
    }
    let ring = SEGMENTS as u32;
    for band in 0..2u32 {
        for segment in 0..ring {
            let next = (segment + 1) % ring;
            let (inner, inner_next) = (band * ring + segment, band * ring + next);
            let (outer, outer_next) = (inner + ring, inner_next + ring);
            mesh.add_triangle(inner, outer, outer_next);
            mesh.add_triangle(inner, outer_next, inner_next);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))
    }

    /// Where a point of the screen shows the old picture, and how strongly:
    /// the tint of the nearest vertex at that distance from the middle.
    fn cover(mesh: &Mesh, distance: f32) -> f32 {
        let centre = screen().center();
        mesh.vertices
            .iter()
            .filter(|vertex| vertex.pos.distance(centre) <= distance + 0.01)
            .map(|vertex| f32::from(vertex.color.a()) / 255.0)
            .fold(0.0, f32::max)
    }

    #[test]
    fn at_the_start_the_old_picture_covers_everything() {
        let mesh = reveal(screen(), egui::TextureId::default(), 0.0);
        let corner = screen().center().distance(screen().min);
        assert!(mesh.vertices.iter().all(|vertex| vertex.color.a() == 0
            || vertex.pos.distance(screen().center()) >= FEATHER - 0.01));
        // The outer ring reaches past every corner, fully opaque.
        assert!(mesh.vertices.iter().any(
            |vertex| vertex.pos.distance(screen().center()) > corner && vertex.color.a() == 255
        ));
        assert!(cover(&mesh, FEATHER) > 0.99);
    }

    #[test]
    fn at_the_end_nothing_of_the_old_picture_is_on_screen() {
        let mesh = reveal(screen(), egui::TextureId::default(), 1.0);
        let corner = screen().center().distance(screen().min);
        for vertex in &mesh.vertices {
            if vertex.color.a() > 0 {
                assert!(vertex.pos.distance(screen().center()) > corner);
            }
        }
    }

    #[test]
    fn the_picture_lines_up_with_the_screen() {
        let mesh = reveal(screen(), egui::TextureId::default(), 0.5);
        for vertex in &mesh.vertices {
            let expected = Pos2::new(vertex.pos.x / 800.0, vertex.pos.y / 600.0);
            assert!((vertex.uv - expected).length() < 1e-4);
        }
        assert_eq!(mesh.indices.len(), SEGMENTS * 2 * 2 * 3);
    }

    #[test]
    fn the_reveal_starts_fast_and_settles() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.25) > 0.5);
    }

    /// One frame at `time` with `events`, running `draw`.
    fn frame(
        ctx: &Context,
        time: f64,
        events: Vec<egui::Event>,
        draw: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                screen_rect: Some(screen()),
                events,
                ..Default::default()
            },
            draw,
        );
        output.textures_delta.clear();
        output
    }

    #[test]
    fn a_picture_is_held_for_then_revealed_away() {
        let ctx = Context::default();
        let mut transition = Transition::default();
        let output = frame(&ctx, 0.0, vec![], |ui| {
            transition.begin(ui.ctx());
            assert!(transition.holding(ui.ctx()), "hold the old colours");
        });
        let user_data = output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .find_map(|command| match command {
                egui::ViewportCommand::Screenshot(user_data) => Some(user_data.clone()),
                _ => None,
            })
            .expect("a screenshot is asked for");
        assert!(transition.active());

        let screenshot = egui::Event::Screenshot {
            viewport_id: egui::ViewportId::ROOT,
            user_data,
            image: Arc::new(egui::ColorImage::filled([4, 4], Color32::RED)),
        };
        let mut held = None;
        let output = frame(&ctx, 0.05, vec![screenshot], |ui| {
            held = Some(transition.holding(ui.ctx()));
            transition.paint(ui.ctx());
        });
        assert_eq!(held, Some(false), "the new colours go on");
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| matches!(shape.shape, egui::Shape::Mesh(_))),
            "the old picture is drawn over them"
        );
        assert!(transition.active());

        let output = frame(&ctx, 0.05 + DURATION + 0.01, vec![], |ui| {
            transition.paint(ui.ctx());
        });
        assert!(!transition.active(), "gone after the reveal");
        assert!(output.shapes.is_empty());
    }

    #[test]
    fn without_a_picture_the_new_colours_go_on_after_a_short_wait() {
        let ctx = Context::default();
        let mut transition = Transition::default();
        let mut held = Vec::new();
        for time in [0.0, 0.1, WAIT + 0.01] {
            frame(&ctx, time, vec![], |ui| {
                transition.begin(ui.ctx());
                held.push(transition.holding(ui.ctx()));
            });
        }
        assert_eq!(held, [true, true, false]);
        assert!(!transition.active());
    }
}
