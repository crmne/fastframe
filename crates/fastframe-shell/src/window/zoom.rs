//! The interface zoom, kept in the app's settings and given to every window.

/// The range a saved zoom is held to: egui's own keyboard zoom goes no
/// further. Anything outside it is a corrupt setting.
const RANGE: std::ops::RangeInclusive<f32> = 0.2..=5.0;

/// Changes smaller than this are rounding, not the user zooming.
const SETTLING: f32 = 0.001;

/// Gives this window the zoom the app keeps in its settings, and returns the
/// zoom to save when the user changes it (Ctrl and plus or minus).
///
/// Call it every frame, in `logic`, for each window that follows the saved
/// zoom. A new window has a new egui context, which starts at zoom 1: the
/// first call on it applies `saved`. Every call returns `Some(zoom)` while
/// the window's zoom differs from `saved`, so the app stores it and passes
/// the new value from then on. That includes a corrupt `saved` value, which
/// is applied as the nearest usable zoom.
///
/// The app keeps the zoom in its own settings rather than in egui's memory,
/// with `eframe::App::persist_egui_memory` off, so that
/// [`Geometry`](super::Geometry) opens windows at the right size. A window
/// with a scale of its own, such as a mini player, does not call it.
pub fn keep_zoom(ctx: &egui::Context, saved: f32) -> Option<f32> {
    let applied = egui::Id::unique("fastframe_shell::zoom_applied");
    let zoom = if ctx.data(|data| data.get_temp::<bool>(applied).unwrap_or(false)) {
        ctx.zoom_factor()
    } else {
        ctx.data_mut(|data| data.insert_temp(applied, true));
        // egui takes a new zoom at the start of the next pass.
        let zoom = applied_zoom(saved);
        ctx.set_zoom_factor(zoom);
        zoom
    };
    (saved.is_nan() || (zoom - saved).abs() > SETTLING).then_some(zoom)
}

/// `saved`, held to [`RANGE`], or 1 when it is no number at all.
fn applied_zoom(saved: f32) -> f32 {
    if saved.is_finite() {
        saved.clamp(*RANGE.start(), *RANGE.end())
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pass of the window, calling [`keep_zoom`] as an app's `logic`
    /// does, with what it returned.
    fn pass(ctx: &egui::Context, saved: f32) -> Option<f32> {
        ctx.begin_pass(egui::RawInput::default());
        let kept = keep_zoom(ctx, saved);
        // The font atlas this pass made has no renderer to go to.
        ctx.end_pass().textures_delta.clear();
        kept
    }

    #[test]
    fn each_new_window_takes_the_saved_zoom() {
        for _ in 0..2 {
            let ctx = egui::Context::default();
            assert_eq!(ctx.zoom_factor(), 1.0);
            assert_eq!(pass(&ctx, 1.5), None);
            assert_eq!(pass(&ctx, 1.5), None, "nothing to save");
            assert_eq!(ctx.zoom_factor(), 1.5);
        }
    }

    #[test]
    fn a_zoom_the_user_picks_is_returned_until_it_is_saved() {
        let ctx = egui::Context::default();
        pass(&ctx, 1.25);
        ctx.set_zoom_factor(1.5);
        assert_eq!(pass(&ctx, 1.25), Some(1.5));
        assert_eq!(pass(&ctx, 1.5), None);
        assert_eq!(ctx.zoom_factor(), 1.5, "applied once, never forced back");
    }

    #[test]
    fn a_corrupt_saved_zoom_is_held_to_a_usable_one() {
        for (saved, applied) in [
            (f32::NAN, 1.0),
            (f32::INFINITY, 1.0),
            (0.0, 0.2),
            (40.0, 5.0),
        ] {
            let ctx = egui::Context::default();
            assert_eq!(pass(&ctx, saved), Some(applied), "saved again: {saved}");
            assert_eq!(pass(&ctx, applied), None);
            assert_eq!(ctx.zoom_factor(), applied, "{saved}");
        }
    }
}
