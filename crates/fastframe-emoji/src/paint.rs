//! Emoji inside egui text: each cluster is laid out as one transparent
//! placeholder glyph and painted over with its picture. From ZapFast's
//! `src/emoji.rs`.
//!
//! Selecting and copying work on the placeholders: the galley holds one
//! [`PLACEHOLDER`] per emoji, and the app keeps the `placements` that
//! [`append`] returns to turn each back into its cluster.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

use egui::text::LayoutJob;
use egui::{Color32, ColorImage, Pos2, Rect, Stroke, TextFormat, TextureHandle, TextureOptions};

use crate::segment::{Piece, pieces};
use crate::{Picture, available, get};

/// The glyph laid out in each emoji's place: a full square, drawn
/// transparent.
pub const PLACEHOLDER: char = '\u{2B1B}';

/// How tall each emoji's texture is, in pixels. Pictures are scaled to the
/// text on the GPU.
const TEXTURE_HEIGHT: u32 = 72;

/// The painted emoji's side, relative to the row height of the text beside
/// it.
const EMOJI_SIDE: f32 = 1.08;

/// Pictures uploaded to one egui context, and those on their way.
#[derive(Clone, Default)]
struct Cache(Arc<Mutex<Textures>>);

#[derive(Default)]
struct Textures {
    /// A texture per cluster, or `None` when no font draws it.
    ready: HashMap<String, Option<TextureHandle>>,
    /// Clusters queued to the worker.
    pending: HashSet<String>,
    /// Pictures the worker finished, to upload in the next frame.
    arrived: Vec<(String, Option<Picture>)>,
}

impl Cache {
    fn of(ctx: &egui::Context) -> Self {
        ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Cache>(egui::Id::unique("fastframe-emoji"))
                .clone()
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Textures> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A picture on its way from the worker.
struct Job {
    cluster: String,
    ctx: egui::Context,
    cache: Cache,
}

/// The worker that draws pictures, started on first use. One thread keeps
/// DirectWrite's per-thread factories to one set, and a burst of new emoji
/// (a picker opening) queues rather than competing for the processor.
fn worker() -> Option<&'static Mutex<mpsc::Sender<Job>>> {
    static WORKER: OnceLock<Option<Mutex<mpsc::Sender<Job>>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("emoji-pictures".into())
                .spawn(move || {
                    while let Ok(job) = receiver.recv() {
                        let picture = get().render(&job.cluster, TEXTURE_HEIGHT);
                        job.cache.lock().arrived.push((job.cluster, picture));
                        job.ctx.request_repaint();
                    }
                })
                .inspect_err(|error| log::warn!("cannot start the emoji worker: {error}"))
                .ok()?;
            Some(Mutex::new(sender))
        })
        .as_ref()
}

fn upload(ctx: &egui::Context, cluster: &str, picture: Option<Picture>) -> Option<TextureHandle> {
    let picture = picture?;
    let image = ColorImage::from_rgba_premultiplied(picture.size, &picture.rgba);
    Some(ctx.load_texture(format!("emoji-{cluster}"), image, TextureOptions::LINEAR))
}

/// The cluster's texture: `Some(Some)` ready, `Some(None)` no font draws
/// it, `None` on its way.
pub(crate) fn texture(ctx: &egui::Context, cluster: &str) -> Option<Option<TextureHandle>> {
    let cache = Cache::of(ctx);
    let mut textures = cache.lock();
    for (arrived, picture) in std::mem::take(&mut textures.arrived) {
        textures.pending.remove(&arrived);
        let handle = upload(ctx, &arrived, picture);
        textures.ready.insert(arrived, handle);
    }
    if let Some(known) = textures.ready.get(cluster) {
        return Some(known.clone());
    }
    let emoji = get();
    if emoji.synchronous {
        let handle = upload(ctx, cluster, emoji.render(cluster, TEXTURE_HEIGHT));
        textures.ready.insert(cluster.to_owned(), handle.clone());
        return Some(handle);
    }
    queue(ctx, &cache, &mut textures, cluster);
    None
}

fn queue(ctx: &egui::Context, cache: &Cache, textures: &mut Textures, cluster: &str) {
    if textures.ready.contains_key(cluster) || textures.pending.contains(cluster) {
        return;
    }
    let Some(worker) = worker() else {
        textures.ready.insert(cluster.to_owned(), None);
        return;
    };
    let job = Job {
        cluster: cluster.to_owned(),
        ctx: ctx.clone(),
        cache: cache.clone(),
    };
    let sent = worker
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .send(job)
        .is_ok();
    if sent {
        textures.pending.insert(cluster.to_owned());
    } else {
        textures.ready.insert(cluster.to_owned(), None);
    }
}

/// Queues pictures of these clusters to be drawn off the interface thread,
/// ahead of the frame that shows them: the first page of an emoji picker,
/// say. Clusters already drawn or queued are skipped.
pub fn prewarm<'a>(ctx: &egui::Context, clusters: impl IntoIterator<Item = &'a str>) {
    if !available() || get().synchronous {
        return;
    }
    let cache = Cache::of(ctx);
    let mut textures = cache.lock();
    for cluster in clusters {
        queue(ctx, &cache, &mut textures, cluster);
    }
}

/// The invisible placeholder's format for emoji beside text in `format`.
///
/// The placeholder is scaled to be exactly as wide as the picture painted
/// over it (the emoji font's cell, [`EMOJI_SIDE`] rows tall, or as wide as
/// it is tall when the cell is wider), so the emoji keeps the spaces on
/// either side, and its line height is pinned so the row is no taller than
/// plain text.
fn placeholder(ui: &egui::Ui, format: &TextFormat) -> TextFormat {
    let (row_height, width) = ui.fonts_mut(|fonts| {
        let shaped = fonts.layout_no_wrap(
            PLACEHOLDER.to_string(),
            format.font_id.clone(),
            Color32::TRANSPARENT,
        );
        (fonts.row_height(&format.font_id), shaped.size().x)
    });
    let mut hidden = format.clone();
    hidden.color = Color32::TRANSPARENT;
    hidden.underline = Stroke::NONE;
    hidden.strikethrough = Stroke::NONE;
    if width > 0.0 {
        hidden.font_id.size *= row_height * EMOJI_SIDE * get().aspect().min(1.0) / width;
    }
    hidden.line_height = Some(format.line_height.unwrap_or(row_height));
    hidden
}

/// Appends `text` to `job` in `format`, with a placeholder for each emoji,
/// and pushes each emoji's cluster onto `placements`. Returns how many
/// characters were appended.
///
/// Without a colour emoji font the text is appended as it is.
pub fn append(
    ui: &egui::Ui,
    job: &mut LayoutJob,
    placements: &mut Vec<String>,
    text: &str,
    format: &TextFormat,
) -> usize {
    let start = job.text.len();
    if !available() {
        job.append(text, 0.0, format.clone());
        return job.text[start..].chars().count();
    }
    let mut hidden = None;
    for piece in pieces(text) {
        match piece {
            Piece::Text(run) => job.append(run, 0.0, format.clone()),
            Piece::Emoji(cluster) => {
                let hidden = hidden.get_or_insert_with(|| placeholder(ui, format));
                job.append(&PLACEHOLDER.to_string(), 0.0, hidden.clone());
                placements.push(cluster.to_owned());
            }
        }
    }
    job.text[start..].chars().count()
}

/// Paints one emoji's picture into `rect`, centred: its cell a little taller
/// than the rectangle, or smaller to fit the rectangle's width.
///
/// A picture not drawn yet is queued to a worker thread and this frame
/// leaves the transparent placeholder; the worker asks for a repaint when it
/// is ready. A cluster no font draws is painted as text.
pub fn paint_cluster(ui: &egui::Ui, cluster: &str, rect: Rect) {
    let painter = ui.painter();
    match texture(ui.ctx(), cluster) {
        Some(Some(texture)) => {
            let image_rect = fit(rect, texture.size_vec2());
            painter.image(
                texture.id(),
                image_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        Some(None) => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                cluster,
                egui::FontId::proportional(rect.height() * 0.8),
                ui.visuals().text_color(),
            );
        }
        None => {}
    }
}

/// Where a picture of `size` is painted for the placeholder `rect`: its
/// height [`EMOJI_SIDE`] times the row's, unless that makes it wider than
/// the rectangle, centred a little below the row's middle.
pub(crate) fn fit(rect: Rect, size: egui::Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return Rect::from_center_size(rect.center(), egui::Vec2::ZERO);
    }
    let scale = (rect.width() / size.x).min(rect.height() * EMOJI_SIDE / size.y);
    Rect::from_center_size(
        rect.center() + egui::vec2(0.0, rect.height() * 0.02),
        size * scale,
    )
}

/// Lays text out for a text editor without changing character offsets: the
/// emoji keep their characters, drawn transparent, and come back as
/// `(first character, character count, cluster)` for [`paint_cluster`].
#[must_use]
pub fn editor_job(text: &str, format: &TextFormat) -> (LayoutJob, Vec<(usize, usize, String)>) {
    let mut job = LayoutJob::default();
    let mut clusters = Vec::new();
    if !available() {
        job.append(text, 0.0, format.clone());
        return (job, clusters);
    }
    let mut at = 0usize;
    for piece in pieces(text) {
        match piece {
            Piece::Text(run) => {
                job.append(run, 0.0, format.clone());
                at += run.chars().count();
            }
            Piece::Emoji(cluster) => {
                let mut hidden = format.clone();
                hidden.color = Color32::TRANSPARENT;
                job.append(cluster, 0.0, hidden);
                let length = cluster.chars().count();
                clusters.push((at, length, cluster.to_owned()));
                at += length;
            }
        }
    }
    (job, clusters)
}

/// Paints the pictures over a galley laid out with [`append`], placed at
/// `origin`.
pub fn paint(ui: &egui::Ui, galley: &egui::Galley, origin: Pos2, placements: &[String]) {
    if placements.is_empty() {
        return;
    }
    for (rect, cluster) in placeholder_rects(galley).zip(placements) {
        paint_cluster(ui, cluster, rect.translate(origin.to_vec2()));
    }
}

/// The placeholders' rectangles in logical order, relative to the galley.
/// Reordering right-to-left runs moves glyphs with their mesh, which keeps
/// each glyph's original vertex offset, so sorting by it restores the order
/// of `placements`.
pub fn placeholder_rects(galley: &egui::Galley) -> impl Iterator<Item = Rect> + '_ {
    galley.rows.iter().flat_map(|row| {
        let mut placeholders: Vec<_> = row
            .glyphs
            .iter()
            .filter(|glyph| glyph.chr == PLACEHOLDER)
            .collect();
        placeholders.sort_by_key(|glyph| glyph.first_vertex);
        // The placeholder is set larger than the text, so its own glyph box
        // is taller than the row. Paint over its advance at the row's height.
        placeholders.into_iter().map(move |glyph| {
            Rect::from_min_max(
                Pos2::new(glyph.pos.x, 0.0),
                Pos2::new(glyph.max_x(), row.size.y),
            )
            .translate(row.pos.to_vec2())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EmojiSetup;

    /// The process-wide setup every egui test here shares: the Noto
    /// subset, drawn inside the frame.
    fn install() {
        EmojiSetup::default()
            .system(false)
            .bundled(crate::bitmap::tests::NOTO_SUBSET)
            .synchronous(true)
            .install();
        assert!(available());
    }

    fn format() -> TextFormat {
        TextFormat::simple(egui::FontId::proportional(14.0), Color32::WHITE)
    }

    #[test]
    fn placeholders_line_up_with_placements() {
        install();
        let ctx = egui::Context::default();
        let mut job = LayoutJob::default();
        let mut placements = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let added = append(ui, &mut job, &mut placements, "a 😀 b 👍🏽", &format());
            assert_eq!(added, 7);
        });
        output.textures_delta.clear();
        assert_eq!(placements, vec!["😀".to_owned(), "👍🏽".to_owned()]);
        assert_eq!(job.text, format!("a {PLACEHOLDER} b {PLACEHOLDER}"));
    }

    #[test]
    fn an_editor_job_keeps_the_text_and_places_the_emoji() {
        install();
        let text = "hi 😊 and 👍🏽!";
        let (job, clusters) = editor_job(text, &format());
        assert_eq!(job.text, text, "the galley must mirror the buffer");
        assert_eq!(clusters.len(), 2);
        for (start, length, cluster) in &clusters {
            assert_eq!(
                text.chars().skip(*start).take(*length).collect::<String>(),
                *cluster
            );
        }
    }

    #[test]
    fn pictures_are_painted_over_the_placeholders() {
        install();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut job = LayoutJob::default();
            let mut placements = Vec::new();
            append(ui, &mut job, &mut placements, "x 😀 y", &format());
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let rects: Vec<Rect> = placeholder_rects(&galley).collect();
            assert_eq!(rects.len(), 1);
            let row_height = galley.rows[0].size.y;
            assert!((rects[0].height() - row_height).abs() < 0.01);
            // As wide as the picture painted over it.
            assert!((rects[0].width() - row_height * EMOJI_SIDE).abs() < 1.0);
            paint(ui, &galley, Pos2::ZERO, &placements);
        });
        let uploaded = output
            .textures_delta
            .set
            .values()
            .flatten()
            .any(|delta| delta.image.height() == TEXTURE_HEIGHT as usize);
        assert!(uploaded, "the picture was uploaded in the first frame");
        output.textures_delta.clear();
    }

    #[test]
    fn pictures_fill_their_placeholder() {
        let row = Rect::from_min_size(Pos2::ZERO, egui::vec2(20.0, 20.0));
        // Noto's cell, a little wider than tall, in a square placeholder.
        let wide = fit(
            row.with_max_x(row.height() * EMOJI_SIDE),
            egui::vec2(77.0, 72.0),
        );
        assert!((wide.width() - row.height() * EMOJI_SIDE).abs() < 0.01);
        assert!(wide.height() < wide.width());
        // Apple's, 160 by 210, in a placeholder as narrow as the cell.
        let aspect = 160.0 / 210.0;
        let narrow = row.with_max_x(row.height() * EMOJI_SIDE * aspect);
        let apple = fit(narrow, egui::vec2(160.0, 210.0));
        assert!((apple.height() - row.height() * EMOJI_SIDE).abs() < 0.01);
        assert!((apple.width() - narrow.width()).abs() < 0.01);
        assert!((apple.center().x - narrow.center().x).abs() < 0.01);
        // An editor's glyph box narrower than the cell shrinks the picture.
        let squeezed = fit(row.with_max_x(10.0), egui::vec2(160.0, 210.0));
        assert!((squeezed.width() - 10.0).abs() < 0.01);
        assert!(fit(row, egui::Vec2::ZERO).area() == 0.0);
    }

    #[test]
    fn a_cluster_no_font_draws_is_painted_as_text() {
        install();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(matches!(texture(ui.ctx(), "🦖"), Some(None)));
            paint_cluster(
                ui,
                "🦖",
                Rect::from_min_size(Pos2::ZERO, egui::vec2(20.0, 20.0)),
            );
        });
        output.textures_delta.clear();
    }
}
