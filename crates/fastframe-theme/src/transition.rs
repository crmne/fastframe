//! A change of colours revealed from the middle of the window outwards.
//!
//! egui draws each frame from scratch, so the old colours are gone as soon
//! as new ones are applied. The transition keeps a picture of them instead:
//! [`Transition::begin`] asks the window for a screenshot while it still
//! shows the old colours, [`Transition::holding`] tells the app to keep them
//! until the picture arrives, and [`Transition::paint`] then lays the
//! picture over the new colours with an opening cut out of its middle that
//! widens until the old colours are gone. The opening is Omarchy's slanted
//! band by default ([`Reveal`]).
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

/// How long the old colours are held for a screenshot that may never come
/// (a hidden window, or a renderer without screenshots), in seconds.
const WAIT: f64 = 0.25;
/// How far Omarchy's band leans: its middle moves this share of the
/// window's height to the right from the bottom edge to the top.
const SLANT: f32 = -0.18;
/// The band's anti-aliased edge, in points.
const EDGE: f32 = 1.0;
/// How wide the soft edge of the circle is, in points.
const FEATHER: f32 = 48.0;
/// How many sides the circle has.
const SEGMENTS: usize = 96;

/// The shape the new colours open out in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reveal {
    /// Omarchy's own theme change: a band leaning slightly to the right,
    /// opening from the middle towards both sides over 0.42 seconds.
    #[default]
    Band,
    /// A circle growing from the middle past the corners, with a soft
    /// edge, over 0.6 seconds.
    Circle,
}

impl Reveal {
    /// How long the reveal takes, in seconds.
    fn duration(self) -> f64 {
        match self {
            Self::Band => 0.42,
            Self::Circle => 0.6,
        }
    }

    /// How far along the opening is at `t`, from 0 to 1 of the time.
    fn progress(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            // Qt's InOutCubic, as Omarchy's animation uses.
            Self::Band => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Self::Circle => 1.0 - (1.0 - t).powi(3),
        }
    }

    fn mesh(self, screen: Rect, texture: egui::TextureId, progress: f32) -> Mesh {
        match self {
            Self::Band => band(screen, texture, progress),
            Self::Circle => circle(screen, texture, progress),
        }
    }
}

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
/// per window, for the life of the app. `default()` reveals as Omarchy
/// does; [`Transition::new`] picks another [`Reveal`].
#[derive(Default)]
pub struct Transition {
    state: State,
    reveal: Reveal,
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
            .field("reveal", &self.reveal)
            .finish()
    }
}

impl Transition {
    /// A transition that reveals new colours in `reveal`'s shape.
    pub fn new(reveal: Reveal) -> Self {
        Self {
            state: State::Idle,
            reveal,
        }
    }

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
        let duration = self.reveal.duration();
        if elapsed >= duration {
            self.state = State::Idle;
            return;
        }
        let screen = ctx.viewport_rect();
        let progress = self.reveal.progress((elapsed / duration) as f32);
        let painter = ctx.layer_painter(LayerId::new(
            Order::Debug,
            Id::new("fastframe-theme-transition"),
        ));
        painter.add(self.reveal.mesh(screen, picture.id(), progress));
        ctx.request_repaint();
    }
}

/// Adds a vertex of the old picture at `position`, which the picture covers
/// as `screen` does, with `alpha` of it showing.
fn vertex(mesh: &mut Mesh, screen: Rect, position: Pos2, alpha: f32) {
    let size = screen.size().max(Vec2::splat(1.0));
    mesh.vertices.push(egui::epaint::Vertex {
        pos: position,
        uv: Pos2::new(
            (position.x - screen.min.x) / size.x,
            (position.y - screen.min.y) / size.y,
        ),
        color: Color32::WHITE.gamma_multiply(alpha),
    });
}

/// Adds the quadrilateral through the last four vertices.
fn quad(mesh: &mut Mesh) {
    let first = mesh.vertices.len() as u32 - 4;
    mesh.add_triangle(first, first + 1, first + 2);
    mesh.add_triangle(first, first + 2, first + 3);
}

/// Omarchy's reveal: the old picture on both sides of a band that leans by
/// [`SLANT`] and has opened `progress` (0 to 1) of the way past both edges
/// of `screen`, as its background shell draws it.
fn band(screen: Rect, texture: egui::TextureId, progress: f32) -> Mesh {
    let (width, height) = (screen.width(), screen.height());
    let top = screen.min.x + width / 2.0 - SLANT * height / 2.0;
    let bottom = screen.min.x + width / 2.0 + SLANT * height / 2.0;
    let reach = width / 2.0 + SLANT.abs() * height / 2.0 + 4.0;
    let spread = reach * progress;
    let (y0, y1) = (screen.min.y, screen.max.y);
    let far = reach + EDGE;
    let mut mesh = Mesh::with_texture(texture);
    for side in [-1.0f32, 1.0] {
        // From the band's edge outwards: the anti-aliased edge, then solid
        // picture past the side of the window.
        let edge_top = top + side * spread;
        let edge_bottom = bottom + side * spread;
        for (from, to, alpha) in [(0.0, EDGE, (0.0, 1.0)), (EDGE, far + spread, (1.0, 1.0))] {
            vertex(
                &mut mesh,
                screen,
                Pos2::new(edge_top + side * from, y0),
                alpha.0,
            );
            vertex(
                &mut mesh,
                screen,
                Pos2::new(edge_top + side * to, y0),
                alpha.1,
            );
            vertex(
                &mut mesh,
                screen,
                Pos2::new(edge_bottom + side * to, y1),
                alpha.1,
            );
            vertex(
                &mut mesh,
                screen,
                Pos2::new(edge_bottom + side * from, y1),
                alpha.0,
            );
            quad(&mut mesh);
        }
    }
    mesh
}

/// The old picture covering `screen`, with a circle cleared from its middle
/// that has grown `progress` (0 to 1) of the way past the farthest corner.
/// The circle's edge fades over [`FEATHER`] points.
fn circle(screen: Rect, texture: egui::TextureId, progress: f32) -> Mesh {
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
    fn at_the_start_the_circle_leaves_the_old_picture_whole() {
        let mesh = circle(screen(), egui::TextureId::default(), 0.0);
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
    fn at_the_end_the_circle_has_cleared_the_screen() {
        let mesh = circle(screen(), egui::TextureId::default(), 1.0);
        let corner = screen().center().distance(screen().min);
        for vertex in &mesh.vertices {
            if vertex.color.a() > 0 {
                assert!(vertex.pos.distance(screen().center()) > corner);
            }
        }
    }

    #[test]
    fn the_picture_lines_up_with_the_screen() {
        let mesh = circle(screen(), egui::TextureId::default(), 0.5);
        for vertex in &mesh.vertices {
            let expected = Pos2::new(vertex.pos.x / 800.0, vertex.pos.y / 600.0);
            assert!((vertex.uv - expected).length() < 1e-4);
        }
        assert_eq!(mesh.indices.len(), SEGMENTS * 2 * 2 * 3);
    }

    #[test]
    fn each_reveal_runs_from_closed_to_open() {
        for reveal in [Reveal::Band, Reveal::Circle] {
            assert_eq!(reveal.progress(0.0), 0.0);
            assert!((reveal.progress(1.0) - 1.0).abs() < 1e-6);
            assert!((reveal.progress(0.5) - 0.5).abs() < 0.4);
        }
        // Omarchy's band eases in and out; the circle starts fast.
        assert!((Reveal::Band.progress(0.5) - 0.5).abs() < 1e-6);
        assert!(Reveal::Band.progress(0.2) < 0.05);
        assert!(Reveal::Circle.progress(0.25) > 0.5);
        assert_eq!(Transition::default().reveal, Reveal::Band);
    }

    /// Where the band's opening is at height `y`: its two edges, the only
    /// vertices at that height where the old picture is fully transparent.
    fn opening(mesh: &Mesh, y: f32) -> (f32, f32) {
        let edges: Vec<f32> = mesh
            .vertices
            .iter()
            .filter(|vertex| (vertex.pos.y - y).abs() < 0.01 && vertex.color.a() == 0)
            .map(|vertex| vertex.pos.x)
            .collect();
        assert_eq!(edges.len(), 2, "{edges:?}");
        (edges[0].min(edges[1]), edges[0].max(edges[1]))
    }

    #[test]
    fn omarchys_band_opens_from_the_middle_leaning_right() {
        let closed = band(screen(), egui::TextureId::default(), 0.0);
        let (left, right) = opening(&closed, 0.0);
        assert!((right - left).abs() < 1e-3, "closed at the start");
        // The band's middle sits right of centre at the top, left at the
        // bottom.
        let (top_left, _) = opening(&closed, 0.0);
        let (bottom_left, _) = opening(&closed, 600.0);
        assert!(top_left > 400.0 && bottom_left < 400.0);

        // Halfway down, the opening lies midway between its top and bottom.
        let half = band(screen(), egui::TextureId::default(), 0.5);
        let ((top_left, top_right), (bottom_left, bottom_right)) =
            (opening(&half, 0.0), opening(&half, 600.0));
        let (left, right) = (
            (top_left + bottom_left) / 2.0,
            (top_right + bottom_right) / 2.0,
        );
        assert!(left < 400.0 && right > 400.0, "the middle is open");
        assert!(left > 0.0 && right < 800.0, "the sides are not yet");

        let open = band(screen(), egui::TextureId::default(), 1.0);
        for y in [0.0, 600.0] {
            let (left, right) = opening(&open, y);
            assert!(left <= 0.0 && right >= 800.0, "open past both sides at {y}");
        }
        for vertex in &open.vertices {
            let expected = Pos2::new(vertex.pos.x / 800.0, vertex.pos.y / 600.0);
            assert!((vertex.uv - expected).length() < 1e-4);
        }
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

        let output = frame(&ctx, 0.05 + Reveal::Band.duration() + 0.01, vec![], |ui| {
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
