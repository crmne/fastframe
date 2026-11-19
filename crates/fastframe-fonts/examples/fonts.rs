//! A small native font specimen, also used for the documentation screenshots.
//! Run with an optional local font path. FASTFRAME_CAPTURE writes a PPM and
//! exits; FASTFRAME_LIGHT selects the light palette.

use std::io::Write as _;

use eframe::egui;
use fastframe_fonts::{FontSetup, LocalFont, Weight};

struct Specimen {
    frames: usize,
    capture: Option<std::path::PathBuf>,
    shot: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>,
    face: String,
}

impl eframe::App for Specimen {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        self.frames += 1;
        let accent = egui::Color32::from_rgb(226, 95, 45);
        egui::Frame::new().inner_margin(38.0).show(ui, |ui| {
            ui.label(
                egui::RichText::new("fastframe / FONT SPECIMEN")
                    .font(Weight::SemiBold.font_id(15.0))
                    .color(accent),
            );
            ui.add_space(18.0);
            ui.label(
                egui::RichText::new("Text that belongs on your desktop.")
                    .font(Weight::Bold.font_id(32.0)),
            );
            ui.add_space(10.0);
            ui.label(egui::RichText::new(&self.face).size(17.0));
            ui.add_space(24.0);
            ui.separator();
            ui.add_space(20.0);
            for (weight, text) in [
                (
                    Weight::Regular,
                    "Regular  /  A quiet place to read and create.",
                ),
                (
                    Weight::Medium,
                    "Medium  /  Clear labels, familiar controls.",
                ),
                (Weight::SemiBold, "Semibold  /  Find what matters."),
                (Weight::Bold, "Bold  /  Make something your own."),
            ] {
                ui.label(egui::RichText::new(text).font(weight.font_id(21.0)));
                ui.add_space(10.0);
            }
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new("Script fallbacks, on a shared baseline")
                    .font(Weight::SemiBold.font_id(17.0))
                    .color(accent),
            );
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("你好，世界  /  历史 · 单向 · 谢谢  /  日本語 · 音楽 · 駅")
                    .size(23.0),
            );
            ui.add_space(12.0);
            ui.label(egui::RichText::new("안녕하세요  /  Ελληνικά  /  Українська").size(23.0));
            ui.add_space(24.0);
            ui.label(
                egui::RichText::new(
                    "Native egui  ·  Linux, macOS, Windows  ·  Synthetic sample text",
                )
                .size(13.0)
                .weak(),
            );
        });
        if let Some(path) = &self.capture {
            if self.frames == 4 {
                let shot = std::sync::Arc::clone(&self.shot);
                let repaint = ctx.clone();
                ctx.request_screenshot(move |image| {
                    *shot.lock().expect("screenshot slot") = Some(image);
                    repaint.request_repaint();
                });
            }
            let image = self.shot.lock().expect("screenshot slot").take();
            if let Some(image) = image {
                let mut file = std::fs::File::create(path).expect("create screenshot");
                write!(file, "P6\n{} {}\n255\n", image.width(), image.height())
                    .expect("write screenshot header");
                for pixel in &image.pixels {
                    file.write_all(&pixel.to_array()[..3])
                        .expect("write screenshot pixels");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result {
    let local = std::env::args_os().nth(1);
    let mut fonts = FontSetup::default();
    let face = if let Some(path) = local {
        fonts = fonts.local_font(LocalFont::load(&path, 0).expect("a usable local font"));
        "A validated local font, with the same weight roles and script fallbacks".to_owned()
    } else {
        "The default interface face, with installed fonts for other scripts".to_owned()
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 620.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "fastframe font specimen",
        options,
        Box::new(move |cc| {
            cc.egui_ctx
                .set_visuals(if std::env::var_os("FASTFRAME_LIGHT").is_some() {
                    egui::Visuals::light()
                } else {
                    egui::Visuals::dark()
                });
            fonts.install(&cc.egui_ctx);
            Ok(Box::new(Specimen {
                frames: 0,
                capture: std::env::var_os("FASTFRAME_CAPTURE").map(Into::into),
                shot: Default::default(),
                face,
            }))
        }),
    )
}
