//! macOS: the traffic lights in the app's header, and a press on it.

#[cfg(target_os = "macos")]
mod appkit;

/// The width the traffic lights take at the start of the title bar, in
/// AppKit points: three buttons from 16 points in, 20 apart, and a margin.
pub const TRAFFIC_LIGHTS_WIDTH: f32 = 84.0;

/// The close button's usual left edge, in AppKit points.
const BUTTON_LEFT: f32 = 16.0;
/// Distance between the buttons' left edges, in AppKit points.
const BUTTON_SPACING: f64 = 20.0;

/// The room to leave at the left of the header for the traffic lights, in
/// egui points: [`TRAFFIC_LIGHTS_WIDTH`] on macOS, which does not scale with
/// egui's zoom, and zero elsewhere and in full screen, where the buttons are
/// gone.
pub(crate) fn traffic_light_inset(ctx: &egui::Context) -> f32 {
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
    align_traffic_lights_at(frame, ctx, title_bar_height, BUTTON_LEFT);
}

/// Like [`align_traffic_lights`], with the close button's left edge `left`
/// AppKit points from the window's edge instead of the usual 16. The buttons
/// stay 20 points apart, and `left` does not scale with egui's zoom.
///
/// For a header whose layout puts the buttons further in, a toolbar with
/// its own padding, say. [`footprint`](crate::footprint) still reports
/// [`TRAFFIC_LIGHTS_WIDTH`], so such a header leaves the extra room itself.
pub fn align_traffic_lights_at(
    frame: &eframe::Frame,
    ctx: &egui::Context,
    title_bar_height: f32,
    left: f32,
) {
    #[cfg(target_os = "macos")]
    if !ctx.input(|input| input.viewport().fullscreen.unwrap_or(false)) {
        appkit::align(
            frame,
            f64::from(title_bar_height * ctx.zoom_factor()),
            f64::from(left),
        );
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (frame, ctx, title_bar_height, left);
}

/// Where the button at `index` (close, minimize, zoom) goes in a bar `bar`
/// points tall, with the close button `left` points in, in the coordinates of
/// the buttons' parent view, which spans the bar with its origin at the
/// bottom.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn button_origin(index: usize, left: f64, bar: f64, button_height: f64) -> (f64, f64) {
    (
        left + index as f64 * BUTTON_SPACING,
        (bar - button_height) / 2.0,
    )
}

/// What a double-click on the title bar should do, from System Settings,
/// Desktop & Dock, "Double-click a window's title bar to".
// Only AppKit performs it; the parsing is tested everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DoubleClick {
    /// Zoom the window (the default).
    Zoom,
    /// Fill the screen.
    Fill,
    /// Minimize the window into the Dock.
    Minimize,
    /// Do nothing.
    Nothing,
}

/// The system's title-bar double-click setting. [`DoubleClick::Zoom`] off
/// macOS, where double-clicking a title bar maximizes.
///
/// AppKit performs the setting only on its own title bar, so on a header
/// drawn under a hidden one the app performs it.
// Only AppKit performs it; the parsing is tested everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn double_click_action() -> DoubleClick {
    #[cfg(target_os = "macos")]
    {
        let (action, legacy_minimize) = appkit::double_click_defaults();
        parse_double_click(action.as_deref(), legacy_minimize)
    }
    #[cfg(not(target_os = "macos"))]
    DoubleClick::Zoom
}

/// Handles a press on a header drawn under the hidden title bar, and says
/// whether it should move the window.
///
/// Call it on the press itself (`primary_pressed`), and on `true` send
/// `ViewportCommand::StartDrag`: AppKit starts a window drag only during the
/// mouse-down event that began it. The second press of a double-click
/// performs the system's setting ([`double_click_action`]) and returns
/// `false`. Fill needs nothing here: AppKit's own window drag, started by the
/// first press, fills the screen. Off macOS it always returns `true`.
pub(crate) fn title_bar_press() -> bool {
    #[cfg(target_os = "macos")]
    return appkit::title_bar_press();
    #[cfg(not(target_os = "macos"))]
    true
}

/// Reads `AppleActionOnDoubleClick`, where System Settings stores Zoom as
/// `Maximize`, falling back to the older `AppleMiniaturizeOnDoubleClick`
/// switch when it is unset. A value this does not know does nothing rather
/// than something unexpected.
// Only AppKit performs it; the parsing is tested everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_double_click(action: Option<&str>, legacy_minimize: bool) -> DoubleClick {
    match action {
        Some("Maximize") => DoubleClick::Zoom,
        Some("Fill") => DoubleClick::Fill,
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

    /// ZapFast's 60-point chat header and Solco's 48-point bar, with
    /// AppKit's 14-point buttons.
    #[test]
    fn buttons_sit_on_the_bars_centre_line_in_their_usual_columns() {
        let left = f64::from(BUTTON_LEFT);
        assert_eq!(button_origin(0, left, 60.0, 14.0), (16.0, 23.0));
        assert_eq!(button_origin(1, left, 48.0, 14.0), (36.0, 17.0));
        assert_eq!(button_origin(2, left, 28.0, 14.0), (56.0, 7.0));
    }

    /// Hindsight's 52-point toolbar, which puts the close button 20 points in.
    #[test]
    fn a_wider_inset_moves_every_button_and_keeps_their_spacing() {
        assert_eq!(button_origin(0, 20.0, 52.0, 14.0), (20.0, 19.0));
        assert_eq!(button_origin(1, 20.0, 52.0, 14.0), (40.0, 19.0));
        assert_eq!(button_origin(2, 20.0, 52.0, 14.0), (60.0, 19.0));
    }

    #[test]
    fn every_known_setting_maps_to_its_action() {
        for (value, action) in [
            (Some("Maximize"), DoubleClick::Zoom),
            (Some("Fill"), DoubleClick::Fill),
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
        assert_eq!(
            parse_double_click(Some("Maximize"), true),
            DoubleClick::Zoom
        );
    }

    #[test]
    fn off_macos_double_click_maximizes_and_alignment_does_nothing() {
        if !cfg!(target_os = "macos") {
            assert_eq!(double_click_action(), DoubleClick::Zoom);
        }
    }
}
