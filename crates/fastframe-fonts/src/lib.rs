//! The apps' interface font, and system fonts for every other script.
//!
//! [`INTER`] is Inter 4.001 as a variable font, with its tabular figures
//! frozen into the character map so numbers that change keep their width.
//! [`FontSetup`] registers it at the weights an app uses, adds the system
//! fonts that draw scripts Inter lacks ([`system`]), and returns egui's
//! [`FontDefinitions`]:
//!
//! ```
//! let ctx = egui::Context::default();
//! let fonts = fastframe_fonts::FontSetup::default().definitions();
//! // fastframe_text::detect().apply_to(&mut fonts), if the app follows the
//! // desktop's hinting, then:
//! ctx.set_fonts(fonts);
//!
//! let title = fastframe_fonts::Weight::SemiBold.font_id(16.0);
//! # let _ = title;
//! ```
//!
//! The default is Inter at 400 (egui's proportional family) and 500, 600
//! and 700 (named families), with system fallbacks. Apps choose the weights,
//! the monospace face, and any face of their own that should come right after
//! Inter (Spotifast's monochrome emoji, for one).

use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::{FontData, FontDefinitions, FontFamily, FontId};

pub mod system;

/// Inter 4.001, the variable font with `wght` and `opsz` axes, with the
/// `tnum` feature frozen into its character map (see `fonts/README.md`).
///
/// SIL Open Font License 1.1: apps ship `fonts/Inter-LICENSE.txt` with it.
pub const INTER: &[u8] = include_bytes!("../fonts/InterVariable.ttf");

/// The font data key the regular weight is registered under.
pub const INTER_REGULAR: &str = "inter";

/// A weight of Inter the interface draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Weight {
    /// 400: body text. egui's [`FontFamily::Proportional`].
    Regular,
    /// 500: the family `inter-medium`.
    Medium,
    /// 600: the family `inter-semibold`.
    SemiBold,
    /// 700: the family `inter-bold`.
    Bold,
}

impl Weight {
    /// Every weight, lightest first.
    pub const ALL: [Self; 4] = [Self::Regular, Self::Medium, Self::SemiBold, Self::Bold];

    /// The `wght` axis value.
    #[must_use]
    pub const fn value(self) -> f32 {
        match self {
            Self::Regular => 400.0,
            Self::Medium => 500.0,
            Self::SemiBold => 600.0,
            Self::Bold => 700.0,
        }
    }

    /// The font data key and, except for regular, the family name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Regular => INTER_REGULAR,
            Self::Medium => "inter-medium",
            Self::SemiBold => "inter-semibold",
            Self::Bold => "inter-bold",
        }
    }

    /// The egui family that draws this weight. Only registered weights
    /// exist: asking egui for a family [`FontSetup`] did not add panics.
    #[must_use]
    pub fn family(self) -> FontFamily {
        match self {
            Self::Regular => FontFamily::Proportional,
            other => FontFamily::Name(other.name().into()),
        }
    }

    /// A font at `size` points in this weight.
    #[must_use]
    pub fn font_id(self, size: f32) -> FontId {
        FontId::new(size, self.family())
    }
}

/// What egui's [`FontFamily::Monospace`] draws with.
#[derive(Clone, Debug)]
pub enum Monospace {
    /// egui's own monospace font (Hack, with the `default_fonts` feature),
    /// as ZapFast and Spotifast use.
    EguiDefault,
    /// Inter at regular weight. Its figures are tabular, so readings keep
    /// their width without a second face (RekordFlash).
    Inter,
    /// A face of the app's own, registered under `name`, ahead of egui's.
    Font {
        /// The font data key.
        name: String,
        /// The face.
        data: Arc<FontData>,
    },
}

/// Which fonts an app registers with egui. See the [crate] documentation.
#[derive(Clone, Debug)]
pub struct FontSetup {
    weights: Vec<Weight>,
    monospace: Monospace,
    companions: Vec<(String, Arc<FontData>)>,
    system_fallbacks: bool,
}

impl Default for FontSetup {
    /// Inter at every [`Weight`], egui's monospace, and system fallbacks.
    fn default() -> Self {
        Self {
            weights: Weight::ALL.to_vec(),
            monospace: Monospace::EguiDefault,
            companions: Vec::new(),
            system_fallbacks: true,
        }
    }
}

impl FontSetup {
    /// Registers only these weights. Regular is always registered, since
    /// it is egui's proportional family.
    #[must_use]
    pub fn weights(mut self, weights: &[Weight]) -> Self {
        self.weights = weights.to_vec();
        self
    }

    /// Chooses the monospace face.
    #[must_use]
    pub fn monospace(mut self, monospace: Monospace) -> Self {
        self.monospace = monospace;
        self
    }

    /// Adds a face right after Inter in every family, ahead of egui's own
    /// fallbacks and the system ones, such as a bundled emoji font. Added in
    /// call order.
    #[must_use]
    pub fn companion(mut self, name: impl Into<String>, data: Arc<FontData>) -> Self {
        self.companions.push((name.into(), data));
        self
    }

    /// Whether to add installed fonts for scripts Inter does not draw
    /// ([`system::fallbacks`]). On by default; demos and screenshot tests
    /// turn it off so the machine's fonts do not change the result.
    #[must_use]
    pub fn system_fallbacks(mut self, enabled: bool) -> Self {
        self.system_fallbacks = enabled;
        self
    }

    /// The font definitions, starting from egui's defaults.
    ///
    /// Each family lists, in order: its Inter weight, the companions,
    /// egui's own fonts, and the system fallbacks. The first call with
    /// system fallbacks on scans the installed fonts (see
    /// [`system::fallbacks`]); later calls reuse the result.
    #[must_use]
    pub fn definitions(&self) -> FontDefinitions {
        let fallbacks: &[system::Fallback] = if self.system_fallbacks {
            system::fallbacks()
        } else {
            &[]
        };
        self.definitions_with(fallbacks)
    }

    /// [`Self::definitions`] with the system fallbacks given, for tests.
    fn definitions_with(&self, fallbacks: &'static [system::Fallback]) -> FontDefinitions {
        let mut fonts = FontDefinitions::default();
        let weighted = |weight: Weight| {
            let mut data = FontData::from_static(INTER);
            data.tweak.coords = VariationCoords::new([(b"wght", weight.value())]);
            Arc::new(data)
        };
        fonts
            .font_data
            .insert(INTER_REGULAR.to_owned(), weighted(Weight::Regular));
        for (name, data) in &self.companions {
            fonts.font_data.insert(name.clone(), data.clone());
        }

        let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
        proportional.insert(0, INTER_REGULAR.to_owned());
        for (position, (name, _)) in self.companions.iter().enumerate() {
            proportional.insert(1 + position, name.clone());
        }

        let monospace_primary = match &self.monospace {
            Monospace::EguiDefault => None,
            Monospace::Inter => Some(INTER_REGULAR.to_owned()),
            Monospace::Font { name, data } => {
                fonts.font_data.insert(name.clone(), data.clone());
                Some(name.clone())
            }
        };
        let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
        // Companions sit right behind the primary face, which for egui's
        // default monospace is its first entry.
        let primary = if let Some(name) = monospace_primary {
            monospace.insert(0, name);
            1
        } else {
            usize::from(!monospace.is_empty())
        };
        for (position, (name, _)) in self.companions.iter().enumerate() {
            monospace.insert(primary + position, name.clone());
        }

        // Each named weight falls back like the regular one.
        let behind: Vec<String> = fonts.families[&FontFamily::Proportional]
            .iter()
            .skip(1)
            .cloned()
            .collect();
        for weight in &self.weights {
            if *weight == Weight::Regular {
                continue;
            }
            fonts
                .font_data
                .insert(weight.name().to_owned(), weighted(*weight));
            let mut family = vec![weight.name().to_owned()];
            family.extend(behind.iter().cloned());
            fonts.families.insert(weight.family(), family);
        }

        // Installed fonts come last, so they never replace Inter's Latin or
        // the companions' glyphs.
        for fallback in fallbacks {
            let mut data = FontData::from_static(&fallback.bytes);
            data.index = fallback.index;
            data.tweak.scale = fallback.scale;
            data.tweak.y_offset_factor = fallback.y_offset_factor;
            fonts
                .font_data
                .insert(fallback.name.clone(), Arc::new(data));
            for family in fonts.families.values_mut() {
                family.push(fallback.name.clone());
            }
        }
        fonts
    }

    /// Builds the definitions and hands them to egui.
    pub fn install(&self, ctx: &egui::Context) {
        ctx.set_fonts(self.definitions());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn family(fonts: &FontDefinitions, family: &FontFamily) -> Vec<String> {
        fonts.families[family].clone()
    }

    #[test]
    fn inter_leads_every_family_at_its_weight() {
        let fonts = FontSetup::default().system_fallbacks(false).definitions();
        assert_eq!(family(&fonts, &FontFamily::Proportional)[0], "inter");
        for weight in [Weight::Medium, Weight::SemiBold, Weight::Bold] {
            let names = family(&fonts, &weight.family());
            assert_eq!(names[0], weight.name());
            assert_eq!(
                names[1..],
                family(&fonts, &FontFamily::Proportional)[1..],
                "{weight:?} falls back like regular"
            );
            let coords = &fonts.font_data[weight.name()].tweak.coords;
            assert_eq!(coords, &VariationCoords::new([(b"wght", weight.value())]));
        }
        assert_ne!(
            family(&fonts, &FontFamily::Monospace)
                .first()
                .map(String::as_str),
            Some("inter"),
            "egui's monospace stays by default"
        );
    }

    #[test]
    fn only_chosen_weights_are_registered() {
        let fonts = FontSetup::default()
            .weights(&[Weight::SemiBold])
            .system_fallbacks(false)
            .definitions();
        assert!(fonts.families.contains_key(&Weight::SemiBold.family()));
        assert!(!fonts.families.contains_key(&Weight::Bold.family()));
        assert!(!fonts.font_data.contains_key(Weight::Medium.name()));
        assert!(fonts.font_data.contains_key(INTER_REGULAR));
    }

    #[test]
    fn companions_follow_inter_everywhere() {
        let emoji = Arc::new(FontData::from_static(INTER));
        let fonts = FontSetup::default()
            .companion("emoji", emoji)
            .system_fallbacks(false)
            .definitions();
        assert_eq!(
            family(&fonts, &FontFamily::Proportional)[..2],
            ["inter", "emoji"]
        );
        assert_eq!(
            family(&fonts, &Weight::Bold.family())[..2],
            ["inter-bold", "emoji"]
        );
        let monospace = family(&fonts, &FontFamily::Monospace);
        assert_eq!(monospace.iter().position(|name| name == "emoji"), Some(1));
    }

    #[test]
    fn the_monospace_face_is_the_apps_choice() {
        let fonts = FontSetup::default()
            .monospace(Monospace::Inter)
            .system_fallbacks(false)
            .definitions();
        assert_eq!(family(&fonts, &FontFamily::Monospace)[0], "inter");
        let plex = Arc::new(FontData::from_static(INTER));
        let fonts = FontSetup::default()
            .monospace(Monospace::Font {
                name: "plex-mono".into(),
                data: plex,
            })
            .companion("emoji", Arc::new(FontData::from_static(INTER)))
            .system_fallbacks(false)
            .definitions();
        assert_eq!(
            family(&fonts, &FontFamily::Monospace)[..2],
            ["plex-mono", "emoji"]
        );
        assert!(fonts.font_data.contains_key("plex-mono"));
    }

    #[test]
    fn system_fallbacks_come_last_with_their_adjustments() {
        let fallbacks: &'static [system::Fallback] = Box::leak(Box::new([system::Fallback {
            name: "fallback-arabic".into(),
            bytes: INTER.to_vec(),
            index: 0,
            scale: 1.1,
            y_offset_factor: 0.05,
        }]));
        let fonts = FontSetup::default().definitions_with(fallbacks);
        for names in fonts.families.values() {
            assert_eq!(names.last().map(String::as_str), Some("fallback-arabic"));
        }
        let tweak = &fonts.font_data["fallback-arabic"].tweak;
        assert!((tweak.scale - 1.1).abs() < f32::EPSILON);
        assert!((tweak.y_offset_factor - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn figures_are_tabular() {
        let ctx = egui::Context::default();
        FontSetup::default().system_fallbacks(false).install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let width = |text: &str| {
                ui.painter()
                    .layout_no_wrap(
                        text.to_owned(),
                        Weight::Regular.font_id(13.0),
                        egui::Color32::WHITE,
                    )
                    .rect
                    .width()
            };
            // With proportional figures "1:11" is far narrower than "8:88",
            // and timers jitter as they count.
            assert!((width("1:11") - width("8:88")).abs() < 0.01);
        });
        output.textures_delta.clear();
    }

    /// Compares each painted glyph, raster offset included, with the same
    /// glyph drawn by its untweaked face: `glyph.pos` alone misses the
    /// `FontTweak`, which moves the glyph's texture.
    #[test]
    fn fallback_glyphs_are_painted_on_the_latin_baseline() {
        let yi = system::tests_support::tall_yi_fallback();
        assert!(yi[0].y_offset_factor.abs() > 0.1, "a real shift is tested");
        let character = '\u{a248}';
        for pixels_per_point in [1.0, 1.5, 2.0] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(pixels_per_point);
            let mut fonts = FontSetup::default().definitions_with(yi);
            let mut raw = (*fonts.font_data[&yi[0].name]).clone();
            raw.tweak.y_offset_factor = 0.0;
            fonts.font_data.insert("raw".into(), Arc::new(raw));
            fonts
                .families
                .insert(FontFamily::Name("raw".into()), vec!["raw".into()]);
            ctx.set_fonts(fonts);
            ctx.run_ui(egui::RawInput::default(), |_| {})
                .textures_delta
                .clear();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                for weight in Weight::ALL {
                    for size in [14.0, 28.0] {
                        let layout = |text: String, family: FontFamily| {
                            ui.painter().layout_no_wrap(
                                text,
                                FontId::new(size, family),
                                egui::Color32::WHITE,
                            )
                        };
                        let raw = layout(character.to_string(), FontFamily::Name("raw".into()));
                        let galley = layout(format!("A{character}A"), weight.family());
                        let row = &galley.rows[0];
                        let glyph = row.glyphs.iter().find(|g| g.chr == character).unwrap();
                        let raw_row = &raw.rows[0];
                        let raw_glyph = &raw_row.glyphs[0];
                        let top = row.visuals.mesh.vertices[glyph.first_vertex as usize].pos.y;
                        let raw_top = raw_row.visuals.mesh.vertices
                            [raw_glyph.first_vertex as usize]
                            .pos
                            .y;
                        let baseline = top - raw_top + raw_glyph.pos.y;
                        let latin = row.glyphs[0].pos.y;
                        // Glyphs and their offsets snap to pixels separately.
                        let error = (baseline - latin).abs() * pixels_per_point;
                        assert!(
                            error <= 1.01,
                            "{weight:?} {size} pt at {pixels_per_point}x: {error} px apart"
                        );
                    }
                }
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn weights_name_their_families() {
        assert_eq!(Weight::Regular.family(), FontFamily::Proportional);
        assert_eq!(
            Weight::Bold.font_id(12.0),
            FontId::new(12.0, FontFamily::Name("inter-bold".into()))
        );
    }
}
