//! Colour emoji in every egui text, with no layout code in the app: an egui
//! plugin that finds each emoji in the frame's text shapes, hides its
//! monochrome glyph and paints the picture in its place. See DESIGN.md,
//! "Emoji in every egui text: the plugin".

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egui::epaint::{ClippedShape, Galley, Shape, TextShape};
use egui::text::{ByteIndex, LayoutJob};
use egui::{Color32, FullOutput, Pos2, Rect, Stroke, TextureId, Vec2};

use crate::paint::{fit, texture};
use crate::segment::{Piece, flag_letters, pieces};

/// Draws every emoji in every egui text in colour: labels, buttons, menus,
/// tooltips, text fields and text an app paints itself.
///
/// ```no_run
/// # let ctx = egui::Context::default();
/// ctx.add_plugin(fastframe_emoji::EmojiPlugin::default());
/// ```
///
/// Choose the fonts with [`crate::EmojiSetup`] and run [`crate::warm_up`]
/// on a thread of its own, as for [`crate::append`]. Text laid out with
/// [`crate::append`] or [`crate::editor_job`] is left to the app, which
/// paints those emoji itself.
#[derive(Default)]
pub struct EmojiPlugin {
    /// What each galley needed last frame, by its identity.
    seen: HashMap<Key, Seen>,
}

/// A galley's address and the colour a text shape forced on it.
type Key = (usize, Option<Color32>);

struct Seen {
    /// Held so the address in the key cannot be reused by another galley.
    _galley: Arc<Galley>,
    coloured: Option<Coloured>,
}

/// A galley with its emoji glyphs hidden and the pictures to paint.
#[derive(Clone)]
struct Coloured {
    galley: Arc<Galley>,
    /// Pictures and where they go, relative to the galley.
    pictures: Vec<(TextureId, Rect)>,
    /// Flags no font draws, spelled out, and where each goes, relative to
    /// the galley.
    spelled: Vec<(Pos2, Arc<Galley>)>,
    /// Whether the text shape's `override_text_color` was baked into the
    /// glyphs and must be dropped.
    baked: bool,
}

/// A cluster's picture: ready (its texture and size), `Some(None)` when no
/// font draws it, `None` while it is being drawn.
type Lookup<'a> = dyn FnMut(&str) -> Option<Option<(TextureId, Vec2)>> + 'a;

/// Lays text out with the context's fonts.
type Layout<'a> = dyn FnMut(LayoutJob) -> Arc<Galley> + 'a;

impl egui::Plugin for EmojiPlugin {
    fn debug_name(&self) -> &'static str {
        "fastframe-emoji"
    }

    fn output_hook(&mut self, ctx: &egui::Context, output: &mut FullOutput) {
        if !crate::available() {
            return;
        }
        let mut lookup = |cluster: &str| {
            texture(ctx, cluster).map(|ready| ready.map(|handle| (handle.id(), handle.size_vec2())))
        };
        let mut spelled = false;
        let mut layout = |job: LayoutJob| {
            spelled = true;
            ctx.fonts_mut(|fonts| fonts.layout_job(job))
        };
        self.colour(&mut output.shapes, &mut lookup, &mut layout);
        if spelled {
            // A spelled flag's letters may be new to the font texture, which
            // egui uploads at the end of the next frame.
            ctx.request_repaint();
        }
        // egui took this frame's texture uploads before the hook ran;
        // pictures uploaded above belong to this frame.
        let uploaded = ctx.tex_manager().write().take_delta();
        output.textures_delta.append(uploaded);
    }
}

impl EmojiPlugin {
    /// Colours the emoji in `shapes`, keeping what this frame used for the
    /// next one.
    fn colour(
        &mut self,
        shapes: &mut Vec<ClippedShape>,
        lookup: &mut Lookup<'_>,
        layout: &mut Layout<'_>,
    ) {
        let mut previous = std::mem::take(&mut self.seen);
        let mut inserts: Vec<(usize, ClippedShape)> = Vec::new();
        for (index, clipped) in shapes.iter_mut().enumerate() {
            for picture in self.visit(&mut clipped.shape, &mut previous, lookup, layout) {
                inserts.push((
                    index,
                    ClippedShape {
                        clip_rect: clipped.clip_rect,
                        shape: picture,
                    },
                ));
            }
        }
        splice_after(shapes, inserts);
    }

    /// Colours the emoji in one shape and returns the pictures to paint
    /// right after it. A `Shape::Vec` gets its pictures inside it.
    fn visit(
        &mut self,
        shape: &mut Shape,
        previous: &mut HashMap<Key, Seen>,
        lookup: &mut Lookup<'_>,
        layout: &mut Layout<'_>,
    ) -> Vec<Shape> {
        match shape {
            Shape::Text(text) => self.text(text, previous, lookup, layout),
            Shape::Vec(shapes) => {
                let mut inserts = Vec::new();
                for (index, inner) in shapes.iter_mut().enumerate() {
                    for picture in self.visit(inner, previous, lookup, layout) {
                        inserts.push((index, picture));
                    }
                }
                splice_after(shapes, inserts);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn text(
        &mut self,
        text: &mut TextShape,
        previous: &mut HashMap<Key, Seen>,
        lookup: &mut Lookup<'_>,
        layout: &mut Layout<'_>,
    ) -> Vec<Shape> {
        if text.angle != 0.0 || !may_hold_emoji(&text.galley.job.text) {
            return Vec::new();
        }
        let key = (Arc::as_ptr(&text.galley) as usize, text.override_text_color);
        let coloured = if let Some(seen) = previous.remove(&key) {
            let coloured = seen.coloured.clone();
            self.seen.insert(key, seen);
            coloured
        } else {
            let (coloured, settled) =
                colour_galley(&text.galley, text.override_text_color, lookup, layout);
            if settled {
                self.seen.insert(
                    key,
                    Seen {
                        _galley: text.galley.clone(),
                        coloured: coloured.clone(),
                    },
                );
            }
            coloured
        };
        let Some(coloured) = coloured else {
            return Vec::new();
        };
        text.galley = coloured.galley;
        if coloured.baked {
            text.override_text_color = None;
        }
        let tint = Color32::WHITE.gamma_multiply(text.opacity_factor);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let mut shapes: Vec<Shape> = coloured
            .pictures
            .iter()
            .map(|(id, rect)| Shape::image(*id, rect.translate(text.pos.to_vec2()), uv, tint))
            .collect();
        shapes.extend(coloured.spelled.iter().map(|(at, letters)| {
            let mut spelled = TextShape::new(
                text.pos + at.to_vec2(),
                letters.clone(),
                text.fallback_color,
            );
            spelled.opacity_factor = text.opacity_factor;
            Shape::Text(spelled)
        }));
        shapes
    }
}

/// Inserts each shape right after the one at its index, keeping order.
fn splice_after<T>(shapes: &mut Vec<T>, inserts: Vec<(usize, T)>) {
    if inserts.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(shapes.len() + inserts.len());
    let mut inserts = inserts.into_iter().peekable();
    for (index, shape) in std::mem::take(shapes).into_iter().enumerate() {
        out.push(shape);
        while let Some((_, picture)) = inserts.next_if(|(at, _)| *at == index) {
            out.push(picture);
        }
    }
    *shapes = out;
}

/// Whether `text` has a character from U+00A9 up, which every emoji has
/// (keycaps through U+20E3). Plain ASCII is rejected by its bytes alone.
fn may_hold_emoji(text: &str) -> bool {
    !text.is_ascii() && text.chars().any(|c| u32::from(c) >= 0xA9)
}

/// The galley with its emoji hidden and their pictures (for a flag no font
/// draws, its letters), or `None` when it has nothing to colour; and whether
/// that answer can be kept (no picture still on its way).
fn colour_galley(
    galley: &Arc<Galley>,
    override_color: Option<Color32>,
    lookup: &mut Lookup<'_>,
    layout: &mut Layout<'_>,
) -> (Option<Coloured>, bool) {
    let text = &galley.job.text;
    let clusters: Vec<(Range<usize>, &str)> = {
        let mut at = 0;
        let mut found = Vec::new();
        for piece in pieces(text) {
            let (Piece::Text(run) | Piece::Emoji(run)) = piece;
            if matches!(piece, Piece::Emoji(_)) {
                found.push((at..at + run.len(), run));
            }
            at += run.len();
        }
        found
    };
    if clusters.is_empty() {
        return (None, true);
    }
    let Some(glyphs) = pair_glyphs(galley) else {
        return (None, true);
    };

    let mut settled = true;
    let mut pictures = Vec::new();
    let mut spellings = Vec::new();
    // (row, first vertex) of each glyph quad to hide.
    let mut hide: Vec<(usize, usize)> = Vec::new();
    for (range, cluster) in clusters {
        let mine: Vec<&(usize, usize, usize)> = glyphs
            .iter()
            .filter(|(_, _, byte)| range.contains(byte))
            .collect();
        let Some(&&(row, first, _)) = mine.first() else {
            continue; // elided away
        };
        let first_glyph = &galley.rows[row].row.glyphs[first];
        if !cluster.starts_with(first_glyph.chr) {
            return (None, true);
        }
        let visible: Vec<(usize, usize)> = mine
            .iter()
            .map(|&&(row, glyph, _)| (row, glyph))
            .filter(|&(row, glyph)| !galley.rows[row].row.glyphs[glyph].uv_rect.is_nothing())
            .collect();
        if visible.is_empty()
            || visible
                .iter()
                .all(|&(row, glyph)| hidden(galley, row, glyph))
        {
            // Nothing drawn, or the app made it transparent to paint it
            // itself (fastframe's placeholder, `editor_job`).
            continue;
        }
        let Some(rect) = cluster_rect(galley, &mine) else {
            continue;
        };
        let drawn = match lookup(cluster) {
            Some(Some((id, size))) => {
                pictures.push((id, fit(rect, size)));
                true
            }
            Some(None) => {
                let baseline = galley.rows[row].pos.y + first_glyph.pos.y;
                if let Some(spelling) =
                    spell(galley, range.start, cluster, rect, baseline, override_color)
                {
                    spellings.push(spelling);
                    true
                } else {
                    false
                }
            }
            None => {
                settled = false;
                false
            }
        };
        if drawn {
            for &(row, glyph) in &visible {
                let first_vertex = galley.rows[row].row.glyphs[glyph].first_vertex as usize;
                hide.push((row, first_vertex));
            }
        }
    }
    if pictures.is_empty() && spellings.is_empty() {
        return (None, settled);
    }
    let spelled = spellings
        .into_iter()
        .filter_map(|spelling| spelling.lay_out(layout))
        .collect();

    let mut copy = (**galley).clone();
    let baked = override_color.is_some();
    for (index, placed) in copy.rows.iter_mut().enumerate() {
        let hides_here = hide.iter().any(|&(row, _)| row == index);
        if !hides_here && !baked {
            continue;
        }
        let row = Arc::make_mut(&mut placed.row);
        if let Some(color) = override_color {
            let range = row.visuals.glyph_vertex_range.clone();
            for vertex in &mut row.visuals.mesh.vertices[range] {
                vertex.color = color;
            }
        }
        for &(_, first) in hide.iter().filter(|&&(row, _)| row == index) {
            for vertex in row.visuals.mesh.vertices.iter_mut().skip(first).take(4) {
                vertex.color = Color32::TRANSPARENT;
            }
        }
    }
    (
        Some(Coloured {
            galley: Arc::new(copy),
            pictures,
            spelled,
            baked,
        }),
        settled,
    )
}

/// A flag to spell out: its letters' job, the room its emoji takes and the
/// baseline it sits on, relative to the galley.
struct Spelling {
    job: LayoutJob,
    room: Rect,
    baseline: f32,
}

/// A flag no font draws, as its two letters (🇳🇱 as "NL") in the text's own
/// font and colour, as browsers on Windows show it. `None` for any other
/// cluster.
fn spell(
    galley: &Galley,
    byte: usize,
    cluster: &str,
    room: Rect,
    baseline: f32,
    override_color: Option<Color32>,
) -> Option<Spelling> {
    let letters = flag_letters(cluster)?;
    let section = galley
        .job
        .sections
        .iter()
        .find(|section| section.byte_range.contains(&ByteIndex(byte)))?;
    let mut format = section.format.clone();
    // The text already draws its background and lines under the emoji.
    format.background = Color32::TRANSPARENT;
    format.underline = Stroke::NONE;
    format.strikethrough = Stroke::NONE;
    if let Some(color) = override_color {
        format.color = color;
    }
    Some(Spelling {
        job: LayoutJob::single_section(letters, format),
        room,
        baseline,
    })
}

impl Spelling {
    /// The letters laid out, shrunk to fit the emoji's room, and where they
    /// go: centred on the room, on the text's baseline.
    fn lay_out(self, layout: &mut Layout<'_>) -> Option<(Pos2, Arc<Galley>)> {
        let mut letters = layout(self.job.clone());
        let width = letters.size().x;
        if width > self.room.width() {
            let mut job = self.job;
            for section in &mut job.sections {
                section.format.font_id.size *= self.room.width() / width;
            }
            letters = layout(job);
        }
        let placed = letters.rows.first()?;
        let own_baseline = placed.pos.y + placed.row.glyphs.first()?.pos.y;
        let at = Pos2::new(
            self.room.center().x - letters.size().x / 2.0,
            self.baseline - own_baseline,
        );
        Some((at, letters))
    }
}

/// Every glyph with the byte offset of the character it draws, as
/// `(row, glyph, byte)`, in logical order. `None` when the glyphs do not
/// pair off with the text, so the galley is left as egui drew it.
fn pair_glyphs(galley: &Galley) -> Option<Vec<(usize, usize, usize)>> {
    let mut order: Vec<(usize, usize)> = Vec::new();
    for (row, placed) in galley.rows.iter().enumerate() {
        let mut glyphs: Vec<usize> = (0..placed.row.glyphs.len()).collect();
        // A right-to-left row reordered for display keeps each glyph's
        // vertices, so the first vertex gives the logical order back.
        glyphs.sort_by_key(|&glyph| placed.row.glyphs[glyph].first_vertex);
        order.extend(glyphs.into_iter().map(|glyph| (row, glyph)));
    }
    if galley.elided {
        // The overflow character, added at the end, draws no text.
        order.pop();
    }
    let characters: Vec<usize> = galley
        .job
        .text
        .char_indices()
        .filter(|&(_, c)| c != '\n')
        .map(|(byte, _)| byte)
        .collect();
    if order.len() > characters.len() {
        return None;
    }
    Some(
        order
            .into_iter()
            .zip(characters)
            .map(|((row, glyph), byte)| (row, glyph, byte))
            .collect(),
    )
}

/// Whether a glyph's quad is fully transparent.
fn hidden(galley: &Galley, row: usize, glyph: usize) -> bool {
    let row = &galley.rows[row].row;
    let first = row.glyphs[glyph].first_vertex as usize;
    row.visuals
        .mesh
        .vertices
        .iter()
        .skip(first)
        .take(4)
        .all(|vertex| vertex.color.a() == 0)
}

/// The rectangle a cluster's glyphs take on its first row, a row tall,
/// relative to the galley.
fn cluster_rect(galley: &Galley, glyphs: &[&(usize, usize, usize)]) -> Option<Rect> {
    let row = glyphs.first()?.0;
    let placed = &galley.rows[row];
    glyphs
        .iter()
        .filter(|(at, _, _)| *at == row)
        .map(|&&(_, glyph, _)| {
            let glyph = &placed.row.glyphs[glyph];
            Rect::from_min_max(
                Pos2::new(glyph.pos.x, 0.0),
                Pos2::new(glyph.pos.x + glyph.advance_width, placed.row.size.y),
            )
        })
        .reduce(|a, b| a.union(b))
        .map(|rect| rect.translate(placed.pos.to_vec2()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EmojiSetup;
    use egui::epaint::TextureId;

    /// The Noto subset, drawn inside the frame, as the paint tests use.
    fn install() {
        EmojiSetup::default()
            .system(false)
            .bundled(crate::bitmap::tests::NOTO_SUBSET)
            .synchronous(true)
            .install();
        assert!(crate::available());
    }

    fn context() -> egui::Context {
        install();
        let ctx = egui::Context::default();
        ctx.add_plugin(EmojiPlugin::default());
        ctx
    }

    /// Every text shape and every image in the frame's output, in order.
    fn flatten(shapes: &[ClippedShape]) -> Vec<(Rect, Shape)> {
        fn walk(clip: Rect, shape: &Shape, out: &mut Vec<(Rect, Shape)>) {
            if let Shape::Vec(inner) = shape {
                for shape in inner {
                    walk(clip, shape, out);
                }
            } else {
                out.push((clip, shape.clone()));
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(clipped.clip_rect, &clipped.shape, &mut out);
        }
        out
    }

    fn texts(shapes: &[(Rect, Shape)]) -> Vec<TextShape> {
        shapes
            .iter()
            .filter_map(|(_, shape)| match shape {
                Shape::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Images painted with a texture other than the font atlas.
    fn pictures(shapes: &[(Rect, Shape)]) -> Vec<(Rect, Rect)> {
        shapes
            .iter()
            .filter_map(|(clip, shape)| match shape {
                Shape::Mesh(mesh) if mesh.texture_id != TextureId::default() => {
                    Some((*clip, mesh.calc_bounds()))
                }
                _ => None,
            })
            .collect()
    }

    /// The absolute rectangle of each visible glyph and whether it is
    /// transparent.
    fn glyphs(text: &TextShape) -> Vec<(char, Rect, bool)> {
        let mut out = Vec::new();
        for placed in &text.galley.rows {
            for glyph in &placed.row.glyphs {
                if glyph.uv_rect.is_nothing() {
                    continue;
                }
                let first = glyph.first_vertex as usize;
                let transparent = placed.row.visuals.mesh.vertices[first..first + 4]
                    .iter()
                    .all(|vertex| vertex.color.a() == 0);
                let rect = Rect::from_min_max(
                    Pos2::new(glyph.pos.x, 0.0),
                    Pos2::new(glyph.pos.x + glyph.advance_width, placed.row.size.y),
                )
                .translate(placed.pos.to_vec2() + text.pos.to_vec2());
                out.push((glyph.chr, rect, transparent));
            }
        }
        out
    }

    /// The layout for texts with no flag to spell.
    fn nothing_to_spell(_: egui::text::LayoutJob) -> Arc<Galley> {
        panic!("no flag to spell")
    }

    fn frame(ctx: &egui::Context, ui: impl FnMut(&mut egui::Ui)) -> Vec<(Rect, Shape)> {
        let mut output = ctx.run_ui(egui::RawInput::default(), ui);
        output.textures_delta.clear();
        flatten(&output.shapes)
    }

    /// Runs empty frames until egui asks for no other.
    fn settle(ctx: &egui::Context) {
        for _ in 0..10 {
            if !ctx.has_requested_repaint() {
                return;
            }
            let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
        }
        panic!("egui keeps asking for frames");
    }

    #[test]
    fn a_labels_emoji_is_a_picture_over_its_hidden_glyph() {
        let ctx = context();
        let shapes = frame(&ctx, |ui| {
            ui.label("a 😀 b");
        });
        let text = texts(&shapes)
            .into_iter()
            .find(|text| text.galley.job.text == "a 😀 b")
            .expect("the label's text");
        let glyphs = glyphs(&text);
        let hidden: Vec<char> = glyphs.iter().filter(|g| g.2).map(|g| g.0).collect();
        assert_eq!(hidden, vec!['😀']);
        let emoji = glyphs.iter().find(|g| g.0 == '😀').expect("emoji glyph").1;
        let pictures = pictures(&shapes);
        assert_eq!(pictures.len(), 1);
        let picture = pictures[0].1;
        assert!(
            (picture.center().x - emoji.center().x).abs() < 0.5,
            "{picture:?} over {emoji:?}"
        );
        assert!(
            (picture.height() - emoji.height() * 1.08).abs() < 0.5
                || picture.width() <= emoji.width() + 0.5
        );
    }

    #[test]
    fn the_picture_is_uploaded_in_the_frame_that_shows_it() {
        let ctx = context();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("x 👍🏽");
        });
        let uploaded = output
            .textures_delta
            .set
            .values()
            .flatten()
            .any(|delta| delta.image.height() == 72);
        output.textures_delta.clear();
        assert!(uploaded);
    }

    #[test]
    fn text_without_emoji_is_left_as_egui_drew_it() {
        install();
        let ctx = egui::Context::default();
        let mut plugin = EmojiPlugin::default();
        let mut galleys = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for text in ["plain", "Sigur Rós · Hoppípolla"] {
                galleys.push(ui.painter().layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::proportional(14.0),
                    Color32::WHITE,
                ));
            }
        });
        output.textures_delta.clear();
        let mut shapes: Vec<ClippedShape> = galleys
            .iter()
            .map(|galley| ClippedShape {
                clip_rect: Rect::EVERYTHING,
                shape: Shape::galley(Pos2::ZERO, galley.clone(), Color32::WHITE),
            })
            .collect();
        let mut lookup =
            |_: &str| -> Option<Option<(TextureId, Vec2)>> { panic!("no emoji to look up") };
        plugin.colour(&mut shapes, &mut lookup, &mut nothing_to_spell);
        assert_eq!(shapes.len(), 2, "nothing painted");
        for (shape, galley) in shapes.iter().zip(&galleys) {
            let Shape::Text(text) = &shape.shape else {
                panic!("still text");
            };
            assert!(Arc::ptr_eq(&text.galley, galley), "not cloned");
        }
        // ASCII is rejected by its bytes; accented text is remembered as
        // having nothing to colour, so it is not segmented again.
        assert_eq!(plugin.seen.len(), 1);
        assert!(plugin.seen.values().all(|seen| seen.coloured.is_none()));
    }

    #[test]
    fn a_flag_no_font_draws_is_spelled_in_the_texts_font_and_colour() {
        // The Noto subset has no Italian flag.
        let ctx = context();
        let shapes = frame(&ctx, |ui| {
            ui.label(egui::RichText::new("🇮🇹 Italia").color(Color32::RED));
        });
        assert!(
            pictures(&shapes).is_empty(),
            "no picture of its first letter"
        );
        let texts = texts(&shapes);
        let label = texts
            .iter()
            .find(|text| text.galley.job.text == "🇮🇹 Italia")
            .expect("the label's text");
        let glyphs = glyphs(label);
        let flag: Vec<&(char, Rect, bool)> = glyphs
            .iter()
            .filter(|glyph| ('\u{1F1E6}'..='\u{1F1FF}').contains(&glyph.0))
            .collect();
        assert!(!flag.is_empty());
        assert!(
            flag.iter().all(|glyph| glyph.2),
            "the flag's glyphs are hidden"
        );
        let room = flag
            .iter()
            .map(|glyph| glyph.1)
            .reduce(|a, b| a.union(b))
            .expect("the flag's room");

        let letters = texts
            .iter()
            .find(|text| text.galley.job.text == "IT")
            .expect("the flag spelled out");
        let format = &letters.galley.job.sections[0].format;
        let label_format = &label.galley.job.sections[0].format;
        assert_eq!(format.font_id, label_format.font_id);
        assert_eq!(format.color, Color32::RED);
        assert_eq!(letters.fallback_color, label.fallback_color);
        let left = letters.pos.x;
        let right = left + letters.galley.size().x;
        assert!(left >= room.left() - 0.5 && right <= room.right() + 0.5);
        // On the label's baseline.
        let baseline = |text: &TextShape, chr: char| {
            let placed = &text.galley.rows[0];
            let glyph = placed
                .row
                .glyphs
                .iter()
                .find(|g| g.chr == chr)
                .expect("glyph");
            text.pos.y + placed.pos.y + glyph.pos.y
        };
        assert!((baseline(letters, 'I') - baseline(label, 'I')).abs() < 0.01);
    }

    #[test]
    fn a_spelled_flag_asks_for_one_more_frame_once() {
        install();
        let ctx = egui::Context::default();
        let mut galley = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            galley = Some(ui.painter().layout_no_wrap(
                "🇮🇹".to_owned(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            ));
        });
        output.textures_delta.clear();
        let galley = galley.expect("laid out");
        let mut plugin = EmojiPlugin::default();
        let hook = |plugin: &mut EmojiPlugin| {
            let mut output = FullOutput::default();
            output.shapes.push(ClippedShape {
                clip_rect: Rect::EVERYTHING,
                shape: Shape::galley(Pos2::ZERO, galley.clone(), Color32::WHITE),
            });
            egui::Plugin::output_hook(plugin, &ctx, &mut output);
            output.textures_delta.clear();
        };
        settle(&ctx);
        hook(&mut plugin);
        assert!(
            ctx.has_requested_repaint(),
            "the letters' glyphs upload next frame"
        );

        // Spelled once, the flag asks for no more frames.
        settle(&ctx);
        hook(&mut plugin);
        assert!(!ctx.has_requested_repaint());
    }

    #[test]
    fn a_spelled_flag_shrinks_to_the_emojis_room() {
        let ctx = context();
        let mut galley = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            galley = Some(ui.painter().layout_no_wrap(
                "🇲🇼".to_owned(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            ));
        });
        output.textures_delta.clear();
        let galley = galley.expect("laid out");
        let room = Rect::from_min_size(Pos2::new(3.0, 0.0), egui::vec2(12.0, 20.0));
        let spelling = spell(&galley, 0, "🇲🇼", room, 15.0, None).expect("a flag");
        let mut layout = |job: egui::text::LayoutJob| ctx.fonts_mut(|fonts| fonts.layout_job(job));
        let (at, letters) = spelling.lay_out(&mut layout).expect("laid out");
        assert_eq!(letters.job.text, "MW");
        assert!(letters.job.sections[0].format.font_id.size < 14.0);
        assert!(letters.size().x <= room.width() + 0.01);
        assert!((at.x + letters.size().x / 2.0 - room.center().x).abs() < 0.01);
        assert!(spell(&galley, 0, "😀", room, 15.0, None).is_none());
    }

    #[test]
    fn a_clipped_labels_picture_keeps_the_clip() {
        let ctx = context();
        let clip = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(30.0, 40.0));
        let shapes = frame(&ctx, |ui| {
            ui.scope_builder(egui::UiBuilder::new(), |ui| {
                ui.set_clip_rect(clip);
                ui.label("😀 a long label that runs past the clip");
            });
        });
        let pictures = pictures(&shapes);
        assert_eq!(pictures.len(), 1);
        assert_eq!(pictures[0].0, clip);
    }

    #[test]
    fn a_picture_on_its_way_keeps_the_glyph() {
        install();
        let ctx = egui::Context::default();
        let mut galley = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            galley = Some(ui.painter().layout_no_wrap(
                "wait 😀".to_owned(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            ));
        });
        output.textures_delta.clear();
        let galley = galley.expect("laid out");
        let mut plugin = EmojiPlugin::default();
        let mut shapes = vec![ClippedShape {
            clip_rect: Rect::EVERYTHING,
            shape: Shape::galley(Pos2::ZERO, galley.clone(), Color32::WHITE),
        }];
        let mut lookup = |_: &str| None;
        plugin.colour(&mut shapes, &mut lookup, &mut nothing_to_spell);
        assert_eq!(shapes.len(), 1, "no picture yet");
        let Shape::Text(text) = &shapes[0].shape else {
            panic!("still text");
        };
        assert!(Arc::ptr_eq(&text.galley, &galley), "the glyph stays drawn");
        assert!(plugin.seen.is_empty(), "asked again next frame");

        // Next frame the picture is there.
        let mut ready = |_: &str| Some(Some((TextureId::User(7), egui::vec2(77.0, 72.0))));
        plugin.colour(&mut shapes, &mut ready, &mut nothing_to_spell);
        assert_eq!(shapes.len(), 2);
    }

    #[test]
    fn an_emoji_with_no_picture_keeps_its_glyph() {
        install();
        let ctx = egui::Context::default();
        let mut galley = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            galley = Some(ui.painter().layout_no_wrap(
                "wait 😀".to_owned(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            ));
        });
        output.textures_delta.clear();
        let galley = galley.expect("laid out");
        let mut plugin = EmojiPlugin::default();
        let mut shapes = vec![ClippedShape {
            clip_rect: Rect::EVERYTHING,
            shape: Shape::galley(Pos2::ZERO, galley.clone(), Color32::WHITE),
        }];
        let mut none = |_: &str| Some(None);
        plugin.colour(&mut shapes, &mut none, &mut nothing_to_spell);
        assert_eq!(shapes.len(), 1, "nothing painted over it");
        let Shape::Text(text) = &shapes[0].shape else {
            panic!("still text");
        };
        assert!(Arc::ptr_eq(&text.galley, &galley), "the glyph stays drawn");
        assert_eq!(plugin.seen.len(), 1, "not asked again next frame");
    }

    #[test]
    fn placeholders_the_app_paints_are_skipped_but_a_typed_square_is_not() {
        let ctx = context();
        let format = egui::TextFormat::simple(egui::FontId::proportional(14.0), Color32::WHITE);
        let mut placeholder = None;
        let mut typed = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut job = egui::text::LayoutJob::default();
            let mut placements = Vec::new();
            crate::append(ui, &mut job, &mut placements, "hi 😀", &format);
            placeholder = Some(ui.fonts_mut(|fonts| fonts.layout_job(job)));
            typed = Some(ui.painter().layout_no_wrap(
                "\u{2B1B}\u{FE0F} square".to_owned(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            ));
        });
        output.textures_delta.clear();
        let mut plugin = EmojiPlugin::default();
        let mut shapes: Vec<ClippedShape> = [placeholder, typed]
            .into_iter()
            .map(|galley| ClippedShape {
                clip_rect: Rect::EVERYTHING,
                shape: Shape::galley(Pos2::ZERO, galley.expect("laid out"), Color32::WHITE),
            })
            .collect();
        let mut asked = Vec::new();
        let mut lookup = |cluster: &str| {
            asked.push(cluster.to_owned());
            Some(Some((TextureId::User(1), egui::vec2(72.0, 72.0))))
        };
        plugin.colour(&mut shapes, &mut lookup, &mut nothing_to_spell);
        assert_eq!(asked, vec!["\u{2B1B}\u{FE0F}".to_owned()]);
        assert_eq!(shapes.len(), 3, "one picture, after the typed square");
        assert!(matches!(shapes[2].shape, Shape::Mesh(_)));
    }

    #[test]
    fn scrolled_text_carries_its_picture_along() {
        let ctx = context();
        let shapes = frame(&ctx, |ui| {
            egui::ScrollArea::horizontal()
                .max_width(60.0)
                .horizontal_scroll_offset(40.0)
                .show(ui, |ui| {
                    ui.add(egui::Label::new("some words then 😀 at the end").extend());
                });
        });
        let text = texts(&shapes)
            .into_iter()
            .find(|text| text.galley.job.text.contains('😀'))
            .expect("the scrolled label");
        let emoji = glyphs(&text)
            .into_iter()
            .find(|g| g.0 == '😀')
            .expect("emoji glyph")
            .1;
        let pictures = pictures(&shapes);
        assert_eq!(pictures.len(), 1);
        assert!((pictures[0].1.center().x - emoji.center().x).abs() < 0.5);
        let text_clip = shapes
            .iter()
            .find_map(|(clip, shape)| match shape {
                Shape::Text(t) if t.galley.job.text.contains('😀') => Some(*clip),
                _ => None,
            })
            .expect("the label's clip");
        assert_eq!(pictures[0].0, text_clip, "the scroll area's clip");
        assert!(text_clip.width() < 70.0, "{text_clip:?}");
    }

    #[test]
    fn a_scrolled_text_edit_carries_its_picture_along() {
        let ctx = context();
        let mut buffer = "type a long line of text and then 😀".to_owned();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = frame(&ctx, |ui| {
                let output = egui::TextEdit::singleline(&mut buffer)
                    .desired_width(80.0)
                    .show(ui);
                if !output.response.has_focus() {
                    output.response.request_focus();
                    let mut state = output.state;
                    let end = egui::text::CCursor::new(buffer.chars().count());
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(end)));
                    state.store(ui.ctx(), output.response.id);
                }
            });
        }
        let text = texts(&shapes)
            .into_iter()
            .find(|text| text.galley.job.text.contains('😀'))
            .expect("the field's text");
        let emoji = glyphs(&text)
            .into_iter()
            .find(|g| g.0 == '😀')
            .expect("emoji glyph");
        assert!(emoji.2, "hidden");
        let pictures = pictures(&shapes);
        assert_eq!(pictures.len(), 1);
        assert!((pictures[0].1.center().x - emoji.1.center().x).abs() < 0.5);
        assert!(
            text.pos.x < pictures[0].0.min.x,
            "scrolled left of its clip"
        );
    }

    #[test]
    fn an_elided_label_colours_only_what_it_shows() {
        let ctx = context();
        let shapes = frame(&ctx, |ui| {
            ui.set_max_width(60.0);
            ui.add(egui::Label::new("😀 a label cut short before this 👍🏽").truncate());
        });
        assert_eq!(pictures(&shapes).len(), 1);
    }

    #[test]
    fn an_override_colour_is_kept_on_the_text() {
        install();
        let ctx = egui::Context::default();
        let mut galley = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            galley = Some(ui.painter().layout_no_wrap(
                "red 😀".to_owned(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            ));
        });
        output.textures_delta.clear();
        let mut plugin = EmojiPlugin::default();
        let mut text = TextShape::new(Pos2::ZERO, galley.expect("laid out"), Color32::WHITE);
        text.override_text_color = Some(Color32::RED);
        let mut shapes = vec![ClippedShape {
            clip_rect: Rect::EVERYTHING,
            shape: Shape::Text(text),
        }];
        let mut lookup = |_: &str| Some(Some((TextureId::User(1), egui::vec2(72.0, 72.0))));
        plugin.colour(&mut shapes, &mut lookup, &mut nothing_to_spell);
        let Shape::Text(text) = &shapes[0].shape else {
            panic!("still text");
        };
        assert_eq!(text.override_text_color, None);
        let glyphs = glyphs(text);
        let r = glyphs.iter().find(|g| g.0 == 'r').expect("r");
        assert!(!r.2);
        let row = &text.galley.rows[0].row;
        let first = row.glyphs[0].first_vertex as usize;
        assert_eq!(row.visuals.mesh.vertices[first].color, Color32::RED);
        assert!(glyphs.iter().find(|g| g.0 == '😀').expect("emoji").2);
    }

    #[test]
    fn a_tooltip_layers_picture_stays_above_what_it_covers() {
        let ctx = context();
        let draw = |ui: &mut egui::Ui| {
            ui.label("under 👍🏽");
            egui::Area::new(egui::Id::new("tip"))
                .order(egui::Order::Tooltip)
                .fixed_pos(Pos2::new(0.0, 0.0))
                .show(ui.ctx(), |ui| {
                    ui.label("over 😀");
                });
        };
        // A new area is measured in its first frame and drawn from the next.
        let _ = frame(&ctx, draw);
        let shapes = frame(&ctx, draw);
        let position = |text: &str| {
            shapes
                .iter()
                .position(|(_, shape)| matches!(shape, Shape::Text(t) if t.galley.job.text == text))
                .expect("drawn")
        };
        let (under, over) = (position("under 👍🏽"), position("over 😀"));
        assert!(under < over, "the tooltip layer is painted later");
        // Each picture follows its own text: the tooltip's comes after the
        // tooltip's text, so nothing below paints over it.
        let is_picture = |shape: &Shape| matches!(shape, Shape::Mesh(mesh) if mesh.texture_id != TextureId::default());
        assert!(is_picture(&shapes[under + 1].1));
        assert!(is_picture(&shapes[over + 1].1));
    }

    #[test]
    fn a_steady_frame_reuses_last_frames_work() {
        let ctx = context();
        let mut first = None;
        let mut second = None;
        for slot in [&mut first, &mut second] {
            let shapes = frame(&ctx, |ui| {
                ui.label("steady 😀");
            });
            *slot = texts(&shapes)
                .into_iter()
                .find(|text| text.galley.job.text == "steady 😀")
                .map(|text| text.galley);
        }
        let (first, second) = (first.expect("first"), second.expect("second"));
        assert!(Arc::ptr_eq(&first, &second), "the same hidden galley");
    }

    /// The label for `text` and its picture, both relative to the label.
    fn label_and_picture(ctx: &egui::Context, text: &str) -> (Arc<Galley>, Rect) {
        let shapes = frame(ctx, |ui| {
            ui.label(text);
        });
        let label = texts(&shapes)
            .into_iter()
            .find(|shape| shape.galley.job.text == text)
            .expect("the label");
        let pictures = pictures(&shapes);
        assert_eq!(pictures.len(), 1, "{text}");
        (
            label.galley.clone(),
            pictures[0].1.translate(-label.pos.to_vec2()),
        )
    }

    #[test]
    fn a_family_and_a_keycap_are_laid_out_and_painted_as_one_emoji() {
        let ctx = context();
        let (single, single_picture) = label_and_picture(&ctx, "a 😀 z");
        let (family, _) = label_and_picture(&ctx, "a 👨‍👩‍👧 z");
        if (family.size().x - single.size().x).abs() > 0.01 {
            // Stock egui lays a sequence out as the glyphs its font draws;
            // giving each one emoji's width takes the apps' egui fork
            // (crmne/egui, "Lay out each emoji cluster as one emoji's
            // width"). Nothing more to check here.
            return;
        }
        let cursor =
            |galley: &Galley, at: usize| galley.pos_from_cursor(egui::text::CCursor::new(at)).min.x;
        for (text, length) in [("a 👨‍👩‍👧 z", 5), ("a #️⃣ z", 3), ("a 👍🏽 z", 2)]
        {
            let (galley, picture) = label_and_picture(&ctx, text);
            assert!(
                (galley.size().x - single.size().x).abs() < 0.01,
                "{text}: the label is as wide as with a single emoji"
            );
            // Before the emoji, after it, and after the space that follows.
            for (here, there) in [(2, 2), (2 + length, 3), (3 + length, 4)] {
                assert!(
                    (cursor(&galley, here) - cursor(&single, there)).abs() < 0.01,
                    "{text}: the cursor at {here}"
                );
            }
            assert!(
                (picture.size() - single_picture.size()).length() < 0.01,
                "{text}: {picture:?}, a single emoji {single_picture:?}"
            );
            assert!(
                (picture.center() - single_picture.center()).length() < 0.01,
                "{text}: painted where a single emoji is"
            );
        }
    }
}
