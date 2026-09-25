//! macOS window chrome for egui apps drawn under the title bar.
//!
//! ZapFast and RekordFlash hide the macOS title bar and draw their own header
//! to the top edge of the window (`with_fullsize_content_view(true)`,
//! `with_titlebar_shown(false)`). Two things then need doing by hand:
//!
//! - AppKit places the traffic lights for its own 28-point title bar, above
//!   the centre of a taller header. [`align_traffic_lights`] moves them to
//!   the header's centre line, and must be called every frame because
//!   AppKit restores them during its own layout passes. The header leaves
//!   [`traffic_light_inset`] free on its left for them.
//! - A double-click on the header should do what System Settings says
//!   ("Double-click a window's title bar to"). [`double_click_action`] reads
//!   that setting; the app performs it (Spotifast also reads it, for its
//!   hidden title bar).
//!
//! ```no_run
//! const HEADER_HEIGHT: f32 = 48.0;
//!
//! struct Window;
//!
//! impl eframe::App for Window {
//!     fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
//!         fastframe_macos::align_traffic_lights(frame, ctx, HEADER_HEIGHT);
//!     }
//!     fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
//!         let inset = fastframe_macos::traffic_light_inset(ui.ctx());
//!         ui.add_space(inset);
//!         // ... the header. On a double-click of its empty part:
//!         match fastframe_macos::double_click_action() {
//!             fastframe_macos::DoubleClick::Zoom | fastframe_macos::DoubleClick::Fill => {
//!                 let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
//!                 ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
//!             }
//!             fastframe_macos::DoubleClick::Minimize => {
//!                 ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
//!             }
//!             fastframe_macos::DoubleClick::Nothing => {}
//!         }
//!     }
//! }
//! ```
//!
//! Every function exists on every platform and does nothing (or answers the
//! platform-neutral default) off macOS.
//!
//! Application menus are not here: ZapFast builds its menu bar with muda and
//! Spotifast with AppKit directly, sharing too little to extract yet.

#[cfg(target_os = "macos")]
mod appkit;

/// The width the traffic lights take at the start of the title bar, in
/// AppKit points: three buttons from 16 points in, 20 apart, and a margin.
pub const TRAFFIC_LIGHTS_WIDTH: f32 = 84.0;

/// Left edge of the close button, in AppKit points.
const BUTTON_LEFT: f64 = 16.0;
/// Distance between the buttons' left edges, in AppKit points.
const BUTTON_SPACING: f64 = 20.0;

/// The room to leave at the left of the header for the traffic lights, in
/// egui points: [`TRAFFIC_LIGHTS_WIDTH`] on macOS, which does not scale with
/// egui's zoom, and zero elsewhere and in full screen, where the buttons are
/// gone.
pub fn traffic_light_inset(ctx: &egui::Context) -> f32 {
    let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
    inset(cfg!(target_os = "macos"), fullscreen, ctx.zoom_factor())
}

fn inset(on_macos: bool, fullscreen: bool, zoom: f32) -> f32 {
    if on_macos && !fullscreen {
        TRAFFIC_LIGHTS_WIDTH / zoom
    } else {
        0.0
    }
}

/// Centres the traffic lights vertically in a title bar `title_bar_height`
/// egui points tall (it scales with egui's zoom), keeping their usual
/// horizontal places.
///
/// Call it every frame: AppKit puts the buttons back during its own layout
/// passes (resizing, zooming, recreating the window). It changes nothing
/// when the buttons are already in place, in full screen, and off macOS.
///
/// A header that must not scale with zoom (AppKit's own 28-point strip, say)
/// passes its height divided by `ctx.zoom_factor()`.
pub fn align_traffic_lights(frame: &eframe::Frame, ctx: &egui::Context, title_bar_height: f32) {
    #[cfg(target_os = "macos")]
    if !ctx.input(|input| input.viewport().fullscreen.unwrap_or(false)) {
        appkit::align(frame, f64::from(title_bar_height * ctx.zoom_factor()));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (frame, ctx, title_bar_height);
}

/// Where the button at `index` (close, minimize, zoom) goes in a bar `bar`
/// points tall, in the coordinates of the buttons' parent view, which spans
/// the bar with its origin at the bottom.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn button_origin(index: usize, bar: f64, button_height: f64) -> (f64, f64) {
    (
        BUTTON_LEFT + index as f64 * BUTTON_SPACING,
        (bar - button_height) / 2.0,
    )
}

/// What a double-click on the title bar should do, from System Settings,
/// Desktop & Dock, "Double-click a window's title bar to".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoubleClick {
    /// Zoom the window (the default).
    Zoom,
    /// Fill the screen (`Fill`, or `Maximize` on older systems).
    ///
    /// AppKit performs this itself when the double-click starts a native
    /// window drag (`ViewportCommand::StartDrag` on mouse down, as Spotifast
    /// does); an app that drags only after movement zooms instead.
    Fill,
    /// Minimize the window into the Dock.
    Minimize,
    /// Do nothing.
    Nothing,
}

/// The system's title-bar double-click setting. [`DoubleClick::Zoom`] off
/// macOS, where double-clicking a title bar maximizes.
pub fn double_click_action() -> DoubleClick {
    #[cfg(target_os = "macos")]
    {
        let (action, legacy_minimize) = appkit::double_click_defaults();
        parse_double_click(action.as_deref(), legacy_minimize)
    }
    #[cfg(not(target_os = "macos"))]
    DoubleClick::Zoom
}

/// Reads `AppleActionOnDoubleClick`, falling back to the older
/// `AppleMiniaturizeOnDoubleClick` switch when it is unset. A value this does
/// not know does nothing rather than something unexpected.
pub fn parse_double_click(action: Option<&str>, legacy_minimize: bool) -> DoubleClick {
    match action {
        Some("Zoom") => DoubleClick::Zoom,
        Some("Fill" | "Maximize") => DoubleClick::Fill,
        Some("Minimize") => DoubleClick::Minimize,
        Some(_) => DoubleClick::Nothing,
        None if legacy_minimize => DoubleClick::Minimize,
        None => DoubleClick::Zoom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inset_clears_the_buttons_at_any_zoom_on_macos_only() {
        assert_eq!(inset(true, false, 1.0), 84.0);
        assert_eq!(inset(true, false, 2.0), 42.0, "the buttons do not zoom");
        assert_eq!(inset(true, true, 1.0), 0.0, "no buttons in full screen");
        assert_eq!(inset(false, false, 1.0), 0.0);
    }

    #[test]
    fn the_context_inset_follows_the_platform() {
        let ctx = egui::Context::default();
        let expected = if cfg!(target_os = "macos") { 84.0 } else { 0.0 };
        assert_eq!(traffic_light_inset(&ctx), expected);
    }

    /// ZapFast's 60-point chat header and RekordFlash's 48-point bar, with
    /// AppKit's 14-point buttons.
    #[test]
    fn buttons_sit_on_the_bars_centre_line_in_their_usual_columns() {
        assert_eq!(button_origin(0, 60.0, 14.0), (16.0, 23.0));
        assert_eq!(button_origin(1, 48.0, 14.0), (36.0, 17.0));
        assert_eq!(button_origin(2, 28.0, 14.0), (56.0, 7.0));
    }

    #[test]
    fn every_known_setting_maps_to_its_action() {
        for (value, action) in [
            (Some("Zoom"), DoubleClick::Zoom),
            (Some("Fill"), DoubleClick::Fill),
            (Some("Maximize"), DoubleClick::Fill),
            (Some("Minimize"), DoubleClick::Minimize),
            (Some("None"), DoubleClick::Nothing),
            (Some("FutureAction"), DoubleClick::Nothing),
            (None, DoubleClick::Zoom),
        ] {
            assert_eq!(parse_double_click(value, false), action, "{value:?}");
        }
    }

    #[test]
    fn the_older_minimize_switch_counts_only_when_the_new_key_is_unset() {
        assert_eq!(parse_double_click(None, true), DoubleClick::Minimize);
        assert_eq!(parse_double_click(Some("Zoom"), true), DoubleClick::Zoom);
    }

    #[test]
    fn off_macos_double_click_maximizes_and_alignment_does_nothing() {
        if !cfg!(target_os = "macos") {
            assert_eq!(double_click_action(), DoubleClick::Zoom);
        }
    }
}
