//! Follow the desktop's font rendering settings in egui.
//!
//! This crate reads what the desktop asks for (hinting strength,
//! antialiasing, sub-pixel positioning) into a small [`TextRendering`] value,
//! adds how glyph coverage becomes ink ([`Coverage`]), and applies both to
//! egui:
//!
//! ```no_run
//! let rendering = fastframe_text::detect();
//! let mut fonts = egui::FontDefinitions::default();
//! rendering.apply_to(&mut fonts);
//! let ctx = egui::Context::default();
//! ctx.set_fonts(fonts);
//! // After the app has set its own visuals, for both themes:
//! ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals));
//! ```
//!
//! Where the settings come from:
//!
//! - Linux: the desktop portal (`org.gnome.desktop.interface` `font-hinting`
//!   and `font-antialiasing`), then fontconfig (`fc-match`), then
//!   [`TextRendering::default`]. See [`portal`] and [`fontconfig`].
//! - macOS: no hinting with sub-pixel positions, as CoreText renders.
//! - Windows: slight hinting with sub-pixel positions, the closest match to
//!   DirectWrite's natural rendering. The registry is not read yet.
//!
//! egui renders grayscale coverage only, so the sub-pixel (LCD) order a
//! desktop may ask for (`rgba`) is ignored: an `rgba` antialiasing setting is
//! treated as grayscale.
//!
//! Text drawn at a manual offset should also land on whole physical pixels;
//! see [`snap_to_pixels`].

use egui::Visuals;
use egui::epaint::FontColorTransferFunction;
use egui::epaint::text::{FontDefinitions, FontTweak, HintingTarget, SmoothHinting};
use std::sync::Arc;

pub mod fontconfig;
pub mod portal;
#[cfg(feature = "watch")]
pub mod watch;

/// How strongly glyph outlines are fitted to the pixel grid.
///
/// The four steps match fontconfig's `hintstyle` and GNOME's `font-hinting`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Hinting {
    /// No grid fitting: the designer's outlines, as macOS draws them.
    None,
    /// Fit stems vertically only, keeping the font's advances. Most Linux
    /// desktops' default (fontconfig `hintslight`).
    #[default]
    Slight,
    /// Fit in both directions and let horizontal metrics snap: sharper
    /// stems, less even spacing. Same as [`Self::Full`] in egui.
    Medium,
    /// Fit in both directions and let horizontal metrics snap: the sharpest
    /// stems, with visibly uneven spacing at small sizes.
    Full,
}

/// How the rasterizer's glyph coverage becomes text alpha, which decides how
/// heavy text looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Coverage {
    /// Coverage is used as is in both themes, as cairo and FreeType draw
    /// text on Linux (`FontColorTransferFunction::Off`). Measured against
    /// GTK at 133% and 160%, this matches its ink weight within 2%, where
    /// egui's dark theme draws 11 to 18% heavier.
    Linear,
    /// egui's own choice per theme: linear in light mode, thickened
    /// (`2c - c²`) in dark mode. Kept on macOS, where CoreText renders
    /// heavier than FreeType, and on Windows until measured.
    ThemeDefault,
}

/// The desktop's font rendering settings, independent of the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextRendering {
    /// How strongly outlines are fitted to the pixel grid.
    pub hinting: Hinting,
    /// Whether text is antialiased. egui always draws antialiased coverage;
    /// `false` selects the strongest hinting target, meant for 1-bit text.
    pub antialias: bool,
    /// Whether glyphs may start at fractional pixel positions (even spacing)
    /// rather than whole pixels. Whole-pixel positions measured two to four
    /// times the spacing error of fractional ones.
    pub subpixel_positioning: bool,
    /// How coverage becomes ink. Set by the platform, not read from the
    /// desktop's settings.
    pub coverage: Coverage,
}

impl Default for TextRendering {
    /// Slight hinting, antialiased, sub-pixel positions, linear coverage:
    /// what current Linux desktops (fontconfig `hintslight`, GTK 4, cairo)
    /// render.
    fn default() -> Self {
        Self {
            hinting: Hinting::Slight,
            antialias: true,
            subpixel_positioning: true,
            coverage: Coverage::Linear,
        }
    }
}

impl TextRendering {
    /// The platform's own rendering, used when the desktop says nothing.
    ///
    /// macOS: CoreText does not hint, so [`Hinting::None`], and keeps egui's
    /// per-theme coverage. Windows: DirectWrite's natural mode fits
    /// vertically only with fractional advances, which is
    /// [`Hinting::Slight`], with egui's per-theme coverage. Elsewhere
    /// [`Self::default`].
    #[must_use]
    pub fn platform_default() -> Self {
        if cfg!(target_os = "macos") {
            Self {
                hinting: Hinting::None,
                coverage: Coverage::ThemeDefault,
                ..Self::default()
            }
        } else if cfg!(target_os = "windows") {
            Self {
                coverage: Coverage::ThemeDefault,
                ..Self::default()
            }
        } else {
            Self::default()
        }
    }

    /// The hinting target this rendering asks for.
    ///
    /// Slight uses egui's default target (`Smooth { light: false,
    /// preserve_linear_metrics: true }`): with linear metrics kept, the
    /// autohinter only snaps vertically, which is what slight hinting means,
    /// and for fonts without TrueType instructions (such as Inter) it renders
    /// identically to `light: true`.
    #[must_use]
    pub fn hinting_target(&self) -> HintingTarget {
        if !self.antialias {
            return HintingTarget::Mono;
        }
        match self.hinting {
            Hinting::None | Hinting::Slight => HintingTarget::Smooth(SmoothHinting::default()),
            Hinting::Medium | Hinting::Full => HintingTarget::Smooth(SmoothHinting {
                light: false,
                symmetric_rendering: true,
                preserve_linear_metrics: false,
            }),
        }
    }

    /// egui's transfer function for this rendering's coverage in a theme.
    #[must_use]
    pub fn color_transfer_function(&self, dark_mode: bool) -> FontColorTransferFunction {
        match self.coverage {
            Coverage::Linear => FontColorTransferFunction::Off,
            Coverage::ThemeDefault if dark_mode => FontColorTransferFunction::DARK_MODE_DEFAULT,
            Coverage::ThemeDefault => FontColorTransferFunction::LIGHT_MODE_DEFAULT,
        }
    }

    /// Sets the hinting fields of one font's tweak, leaving its scale,
    /// offsets and variation coordinates alone.
    pub fn tweak(&self, tweak: &mut FontTweak) {
        tweak.hinting = Some(self.hinting != Hinting::None);
        tweak.hinting_target = self.hinting_target();
        tweak.subpixel_binning = Some(self.subpixel_positioning);
    }

    /// Applies [`Self::tweak`] to every font in `fonts`.
    ///
    /// Call it after inserting the app's own fonts and before
    /// `ctx.set_fonts`. A font whose data is shared with another
    /// `FontDefinitions` is cloned first (`Arc::make_mut`).
    pub fn apply_to(&self, fonts: &mut FontDefinitions) {
        for data in fonts.font_data.values_mut() {
            self.tweak(&mut Arc::make_mut(data).tweak);
        }
    }

    /// Sets a theme's text options: the coverage transfer function, and the
    /// global hinting and sub-pixel binning that apply to any font whose
    /// tweak leaves them unset.
    ///
    /// Apply it to both themes after the app has set its own visuals, since
    /// replacing the visuals resets these:
    /// `ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals))`.
    pub fn apply_to_visuals(&self, visuals: &mut Visuals) {
        let options = &mut visuals.text_options;
        options.font_hinting = self.hinting != Hinting::None;
        options.subpixel_binning = self.subpixel_positioning;
        options.color_transfer_function = self.color_transfer_function(visuals.dark_mode);
    }
}

/// Rounds a length or coordinate in points to the nearest whole physical
/// pixel.
///
/// Use it for manual galley offsets (`painter.galley(pos, ..)` at a
/// computed position, centring, indents): egui rasterizes glyphs for a
/// position on the pixel grid, and a galley placed a fraction of a pixel off
/// is resampled and looks soft. At 133% a point is 1.33 pixels, so values
/// that are whole points are often not whole pixels. For `Pos2`, `Vec2` and
/// `Rect`, `egui::emath::GuiRounding::round_to_pixels` does the same.
///
/// A `pixels_per_point` that is not positive and finite returns `value`
/// unchanged.
#[must_use]
pub fn snap_to_pixels(value: f32, pixels_per_point: f32) -> f32 {
    if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        (value * pixels_per_point).round() / pixels_per_point
    } else {
        value
    }
}

/// A source of settings: returns `None` when it has nothing to say.
pub type Reader<'a> = &'a dyn Fn() -> Option<TextRendering>;

/// The first answer from `readers`, in order, else `fallback`.
///
/// Readers after the first answer are never called, so a slow source (a
/// process, a bus call) only runs when the faster ones had nothing.
#[must_use]
pub fn resolve(readers: &[Reader<'_>], fallback: TextRendering) -> TextRendering {
    readers.iter().find_map(|read| read()).unwrap_or(fallback)
}

/// Reads the desktop's font rendering settings.
///
/// On Linux this asks the desktop portal, then fontconfig for `sans-serif`,
/// and blocks for at most about a second when the portal does not answer.
/// Elsewhere it returns [`TextRendering::platform_default`] without any I/O.
///
/// Do not call it from inside a tokio runtime: the D-Bus client blocks on
/// its own.
#[must_use]
pub fn detect() -> TextRendering {
    #[cfg(target_os = "linux")]
    {
        resolve(
            &[&portal::read, &|| fontconfig::read("sans-serif")],
            TextRendering::platform_default(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        TextRendering::platform_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn rendering(hinting: Hinting, antialias: bool, subpixel_positioning: bool) -> TextRendering {
        TextRendering {
            hinting,
            antialias,
            subpixel_positioning,
            ..TextRendering::default()
        }
    }

    const EGUI_DEFAULT: HintingTarget = HintingTarget::Smooth(SmoothHinting {
        light: false,
        symmetric_rendering: true,
        preserve_linear_metrics: true,
    });

    const GRID_FIT: HintingTarget = HintingTarget::Smooth(SmoothHinting {
        light: false,
        symmetric_rendering: true,
        preserve_linear_metrics: false,
    });

    #[test]
    fn default_is_slight_antialiased_subpixel_and_linear() {
        let default = TextRendering::default();
        assert_eq!(default.hinting, Hinting::Slight);
        assert!(default.antialias);
        assert!(default.subpixel_positioning);
        assert_eq!(default.coverage, Coverage::Linear);
    }

    #[test]
    fn egui_default_target_is_what_slight_relies_on() {
        assert_eq!(HintingTarget::default(), EGUI_DEFAULT);
    }

    #[test]
    fn every_combination_maps_to_the_expected_tweak() {
        let hintings = [
            (Hinting::None, false, EGUI_DEFAULT),
            (Hinting::Slight, true, EGUI_DEFAULT),
            (Hinting::Medium, true, GRID_FIT),
            (Hinting::Full, true, GRID_FIT),
        ];
        for (hinting, hinted, antialiased_target) in hintings {
            for antialias in [true, false] {
                for subpixel in [true, false] {
                    let mut tweak = FontTweak::default();
                    rendering(hinting, antialias, subpixel).tweak(&mut tweak);
                    let target = if antialias {
                        antialiased_target
                    } else {
                        HintingTarget::Mono
                    };
                    assert_eq!(tweak.hinting, Some(hinted), "{hinting:?}");
                    assert_eq!(tweak.hinting_target, target, "{hinting:?} {antialias}");
                    assert_eq!(tweak.subpixel_binning, Some(subpixel), "{hinting:?}");
                }
            }
        }
    }

    #[test]
    fn slight_keeps_subpixel_positions() {
        let mut tweak = FontTweak::default();
        TextRendering::default().tweak(&mut tweak);
        assert_eq!(tweak.hinting, Some(true));
        assert_eq!(tweak.hinting_target, EGUI_DEFAULT);
        assert_eq!(tweak.subpixel_binning, Some(true));
    }

    #[test]
    fn tweak_keeps_the_fonts_own_adjustments() {
        let mut tweak = FontTweak {
            scale: 0.8,
            y_offset_factor: -0.1,
            y_offset: 1.0,
            ..FontTweak::default()
        };
        TextRendering::default().tweak(&mut tweak);
        assert_eq!(tweak.scale, 0.8);
        assert_eq!(tweak.y_offset_factor, -0.1);
        assert_eq!(tweak.y_offset, 1.0);
    }

    #[test]
    fn apply_to_reaches_every_font() {
        let mut fonts = FontDefinitions::default();
        assert!(!fonts.font_data.is_empty());
        rendering(Hinting::Full, true, false).apply_to(&mut fonts);
        for data in fonts.font_data.values() {
            assert_eq!(data.tweak.hinting_target, GRID_FIT);
            assert_eq!(data.tweak.subpixel_binning, Some(false));
        }
    }

    #[test]
    fn linear_coverage_is_off_in_both_themes() {
        let linear = TextRendering::default();
        for mut visuals in [Visuals::dark(), Visuals::light()] {
            linear.apply_to_visuals(&mut visuals);
            assert_eq!(
                visuals.text_options.color_transfer_function,
                FontColorTransferFunction::Off
            );
        }
    }

    #[test]
    fn theme_default_coverage_restores_eguis_choice_per_theme() {
        let theme = TextRendering {
            coverage: Coverage::ThemeDefault,
            ..TextRendering::default()
        };
        for fresh in [Visuals::dark(), Visuals::light()] {
            let mut visuals = fresh.clone();
            TextRendering::default().apply_to_visuals(&mut visuals);
            theme.apply_to_visuals(&mut visuals);
            assert_eq!(
                visuals.text_options.color_transfer_function,
                fresh.text_options.color_transfer_function
            );
        }
    }

    #[test]
    fn visuals_follow_hinting_and_positioning() {
        let mut visuals = Visuals::dark();
        rendering(Hinting::None, true, false).apply_to_visuals(&mut visuals);
        assert!(!visuals.text_options.font_hinting);
        assert!(!visuals.text_options.subpixel_binning);
        rendering(Hinting::Slight, true, true).apply_to_visuals(&mut visuals);
        assert!(visuals.text_options.font_hinting);
        assert!(visuals.text_options.subpixel_binning);
    }

    #[test]
    fn snapping_lands_on_whole_physical_pixels() {
        for ppp in [1.0, 1.25, 4.0 / 3.0, 1.6, 2.0] {
            for value in [0.0, 0.3, 1.0, 7.5, 12.37, -3.2] {
                let snapped = snap_to_pixels(value, ppp);
                let pixels = snapped * ppp;
                assert!((pixels - pixels.round()).abs() < 1e-3, "{value} at {ppp}");
                assert!(
                    (snapped - value).abs() <= 0.5 / ppp + 1e-6,
                    "{value} at {ppp}"
                );
            }
        }
        // One point at 133% is 1.33 pixels: it snaps to one pixel.
        assert!((snap_to_pixels(1.0, 4.0 / 3.0) - 0.75).abs() < 1e-6);
        assert_eq!(snap_to_pixels(2.5, 0.0), 2.5);
        assert_eq!(snap_to_pixels(2.5, f32::NAN), 2.5);
    }

    #[test]
    fn first_answer_wins_and_later_readers_are_not_called() {
        let later_calls = Cell::new(0);
        let portal = || Some(rendering(Hinting::Full, true, true));
        let fontconfig = || {
            later_calls.set(later_calls.get() + 1);
            Some(rendering(Hinting::None, true, true))
        };
        let got = resolve(&[&portal, &fontconfig], TextRendering::default());
        assert_eq!(got.hinting, Hinting::Full);
        assert_eq!(later_calls.get(), 0);
    }

    #[test]
    fn silent_readers_fall_through_to_the_next_then_the_fallback() {
        let silent = || None;
        let fontconfig = || Some(rendering(Hinting::Medium, true, true));
        let got = resolve(&[&silent, &fontconfig], TextRendering::default());
        assert_eq!(got.hinting, Hinting::Medium);

        let fallback = rendering(Hinting::None, false, false);
        assert_eq!(resolve(&[&silent, &silent], fallback), fallback);
        assert_eq!(resolve(&[], fallback), fallback);
    }

    #[test]
    fn platform_default_matches_the_platform() {
        let got = TextRendering::platform_default();
        let (hinting, coverage) = if cfg!(target_os = "macos") {
            (Hinting::None, Coverage::ThemeDefault)
        } else if cfg!(target_os = "windows") {
            (Hinting::Slight, Coverage::ThemeDefault)
        } else {
            (Hinting::Slight, Coverage::Linear)
        };
        assert_eq!(got.hinting, hinting);
        assert_eq!(got.coverage, coverage);
        assert!(got.antialias);
        assert!(got.subpixel_positioning);
    }
}
