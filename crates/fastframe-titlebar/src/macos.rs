//! macOS: the traffic lights in the app's header, and a press on it.

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
    #[cfg(target_os = "macos")]
    {
        // Which window the root viewport's header presses belong to. `frame`
        // is always the root window's, even in a child viewport, which
        // therefore stays unnamed.
        if ctx.viewport_id() == egui::ViewportId::ROOT
            && let Some(number) = appkit::window_number(frame)
        {
            // Outside the closure: it holds the context's lock.
            let id = window_id(ctx);
            ctx.data_mut(|data| data.insert_temp(id, number));
        }
        if !ctx.input(|input| input.viewport().fullscreen.unwrap_or(false)) {
            appkit::align(frame, f64::from(title_bar_height * ctx.zoom_factor()));
        }
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

/// Keeps each left mouse-down as AppKit dispatches it, for
/// [`title_bar_press`]. Installs one application-wide monitor the first time.
pub(crate) fn record_presses() {
    #[cfg(target_os = "macos")]
    appkit::record_presses();
}

/// Handles a press on a header drawn under the hidden title bar, and says
/// whether the caller should still send `ViewportCommand::StartDrag`.
///
/// Call it on the press itself (`primary_pressed`); `down` says whether the
/// button is still held. egui sees the press after AppKit has dispatched it,
/// when `currentEvent` is often a Force Touch pressure or drag event, so the
/// press is the mouse-down [`record_presses`] kept for the window
/// [`align_traffic_lights`] named for this viewport, taken once. A viewport
/// it never named reads the current event, as before. The second
/// click performs the system's setting ([`double_click_action`]); any other
/// press starts AppKit's window drag from that mouse-down, as AppKit requires.
/// Fill needs nothing here: AppKit's own window drag, started by the first
/// press, fills the screen. Once another strip or egui pass has taken the
/// press it returns `false`; with no mouse-down seen in the window yet, and
/// off macOS, it returns `down`.
pub(crate) fn title_bar_press(ctx: &egui::Context, down: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        let id = window_id(ctx);
        appkit::title_bar_press(ctx.data(|data| data.get_temp(id)), down)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ctx;
        down
    }
}

/// Where [`align_traffic_lights`] keeps the viewport's window number.
#[cfg(target_os = "macos")]
fn window_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::new("fastframe-titlebar-window").with(ctx.viewport_id())
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
    fn a_press_with_nothing_kept_drags_while_held() {
        let ctx = egui::Context::default();
        assert!(title_bar_press(&ctx, true));
        assert!(!title_bar_press(&ctx, false));
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
        assert_eq!(button_origin(0, 60.0, 14.0), (16.0, 23.0));
        assert_eq!(button_origin(1, 48.0, 14.0), (36.0, 17.0));
        assert_eq!(button_origin(2, 28.0, 14.0), (56.0, 7.0));
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
