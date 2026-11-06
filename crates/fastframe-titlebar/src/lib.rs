//! The title bar an egui app draws itself.
//!
//! On macOS the apps run their content under a hidden title bar
//! (`with_fullsize_content_view(true)`, `with_titlebar_shown(false)`) and keep
//! the native traffic lights, which only AppKit's own buttons can offer in
//! full (the zoom button's tiling menu, Option-click, the inactive look). On
//! Windows and Linux the user can turn the platform's frame off
//! (`with_decorations(false)`), and the app then draws the buttons too. This
//! crate supplies what such a header needs:
//!
//! - [`drag`] makes a header area move the window, and answers a double
//!   click: on macOS as System Settings says, through AppKit's own drag;
//!   elsewhere by maximizing or restoring;
//! - [`footprint`] says how much room the buttons take at each end (the
//!   traffic lights on macOS), so the header keeps its controls clear of
//!   them;
//! - [`controls`] draws the minimize, maximize and close buttons on Windows
//!   and Linux: Windows' square caption buttons at the top right, or GNOME's
//!   round ones in the desktop's button layout, with the icon theme's glyphs;
//! - [`resize`] lets the window's edges and corners resize it on Windows and
//!   Linux.
//!
//! On Windows and Linux the choice is the user's: [`set_enabled`] records it
//! before the window is made, and the drawn title bar does nothing while it
//! is off and in full screen. On macOS [`drag`] and [`footprint`] always
//! work outside full screen, and [`align_traffic_lights`], called every
//! frame, centres the traffic lights in the header.
//!
//! ```no_run
//! # struct Settings { custom_titlebar: bool }
//! # let settings = Settings { custom_titlebar: true };
//! // At startup, before the first window:
//! fastframe_titlebar::init();
//! fastframe_titlebar::set_enabled(settings.custom_titlebar);
//! let viewport = egui::ViewportBuilder::default()
//!     .with_decorations(!fastframe_titlebar::enabled());
//! # let _ = viewport;
//!
//! // Once per context, with the app's other icons:
//! # let ctx = egui::Context::default();
//! fastframe_titlebar::install(&ctx);
//!
//! // Each frame, in the header:
//! # fn header(ui: &mut egui::Ui, header: egui::Rect) {
//! fastframe_titlebar::drag(ui, header); // before the header's own widgets
//! // ... the header's widgets ...
//! let style = fastframe_titlebar::Style {
//!     icon: egui::Color32::GRAY,
//!     button: egui::Color32::from_gray(60),
//!     header_height: 48.0,
//! };
//! fastframe_titlebar::controls(ui, &style, &fastframe_titlebar::Labels::default());
//! fastframe_titlebar::resize(ui);
//! # }
//! ```

use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Align2, Rect, vec2};

#[cfg(target_os = "linux")]
mod linux;
mod macos;

pub use macos::{TRAFFIC_LIGHTS_WIDTH, align_traffic_lights};

/// One caption button's slot: Windows' button width, and the room a GNOME
/// button takes with its spacing.
const BUTTON: f32 = 36.0;
/// How far from the window's edge a press resizes it.
const RESIZE_BORDER: f32 = 5.0;
/// How far along an edge from a corner a press resizes both ways.
const RESIZE_CORNER: f32 = 10.0;
/// GNOME's buttons sit this far from the window's edges.
const LINUX_MARGIN: f32 = 14.0;
/// GNOME's caption row: a button with its margin.
const LINUX_HEIGHT: f32 = BUTTON + RESIZE_BORDER;

static CHOSEN: AtomicBool = AtomicBool::new(false);

/// Records whether the user wants the app's own title bar. Windows made from
/// now on follow it: create them with `with_decorations(!enabled())`.
pub fn set_enabled(on: bool) {
    CHOSEN.store(on, Ordering::Relaxed);
}

/// Whether windows draw their own title bar: chosen, and on Windows or
/// Linux.
pub fn enabled() -> bool {
    enabled_for(
        cfg!(any(windows, target_os = "linux")),
        CHOSEN.load(Ordering::Relaxed),
    )
}

const fn enabled_for(supported: bool, chosen: bool) -> bool {
    supported && chosen
}

/// Whether this window shows the app's own title bar now: enabled, and not
/// in fullscreen.
pub fn visible(ctx: &egui::Context) -> bool {
    let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
    enabled() && !fullscreen
}

/// Reads the desktop's preferences the buttons follow: on GNOME, the button
/// layout and the icon theme's window icons. Call it once at startup, before
/// the first frame; it asks the desktop portal, which can take up to half a
/// second per setting. Elsewhere it does nothing.
pub fn init() {
    #[cfg(target_os = "linux")]
    linux::init();
}

/// Serves the buttons' fallback icons to egui. Call it once per context,
/// after installing egui's SVG loaders, as for the app's own icons.
pub fn install(ctx: &egui::Context) {
    fastframe_icons::install::<Icon>(ctx);
}

fastframe_icons::icons! {
    /// The buttons' icons where the desktop gives none.
    enum Icon {
        prefix: "fastframe-titlebar-",
        directory: "",
        Minimize => lucide "minus",
        Maximize => lucide "square",
        Restore => lucide "copy",
        Close => lucide "x",
    }
}

/// A caption button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Minimizes the window.
    Minimize,
    /// Maximizes the window, or restores a maximized one.
    Maximize,
    /// Closes the window.
    Close,
}

/// Which buttons sit at each end of the title bar, outside in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The buttons at the left end, from the edge inwards.
    pub left: Vec<Button>,
    /// The buttons at the right end, from the inside out to the edge.
    pub right: Vec<Button>,
}

impl Layout {
    /// Minimize, maximize and close on the right, as Windows and most
    /// desktops place them.
    pub fn standard() -> Self {
        Self {
            left: Vec::new(),
            right: vec![Button::Minimize, Button::Maximize, Button::Close],
        }
    }

    /// Reads a GNOME `button-layout` such as `appmenu:minimize,close`,
    /// quoted or not. Names other than the three buttons are skipped, and a
    /// button named twice keeps its first place. `None` without the colon
    /// that separates the two ends.
    pub fn from_gnome(layout: &str) -> Option<Self> {
        let layout = layout.trim().trim_matches('\'');
        let (left, right) = layout.split_once(':')?;
        let mut seen = Vec::new();
        let mut buttons = |side: &str| {
            side.split(',')
                .filter_map(|name| match name.trim() {
                    "minimize" => Some(Button::Minimize),
                    "maximize" => Some(Button::Maximize),
                    "close" => Some(Button::Close),
                    _ => None,
                })
                .filter(|button| {
                    let first = !seen.contains(button);
                    seen.push(*button);
                    first
                })
                .collect()
        };
        Some(Self {
            left: buttons(left),
            right: buttons(right),
        })
    }
}

/// The buttons this desktop shows: GNOME's layout as [`init`] read it on
/// Linux, otherwise [`Layout::standard`].
pub fn layout() -> &'static Layout {
    #[cfg(target_os = "linux")]
    return linux::layout();
    #[cfg(not(target_os = "linux"))]
    {
        static STANDARD: std::sync::OnceLock<Layout> = std::sync::OnceLock::new();
        STANDARD.get_or_init(Layout::standard)
    }
}

/// How the buttons look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// The glyphs' colour. Windows' buttons look right in the app's
    /// secondary text colour, GNOME's in its text colour.
    pub icon: egui::Color32,
    /// Linux: the round buttons' fill.
    pub button: egui::Color32,
    /// Windows: the height of the header the buttons are centred in.
    pub header_height: f32,
}

/// The buttons' tooltips and accessible names, in the app's language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Labels<'a> {
    /// Minimize.
    pub minimize: &'a str,
    /// Maximize.
    pub maximize: &'a str,
    /// Restore, on Maximize while the window is maximized.
    pub restore: &'a str,
    /// Close.
    pub close: &'a str,
}

impl Default for Labels<'static> {
    fn default() -> Self {
        Self {
            minimize: "Minimize",
            maximize: "Maximize",
            restore: "Restore",
            close: "Close",
        }
    }
}

/// The room the buttons take at each end of the title bar: the traffic
/// lights on macOS, the drawn buttons while the title bar is [`visible`] on
/// Windows and Linux, and zero where there are none and in full screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Footprint {
    /// Width taken at the left end.
    pub left: f32,
    /// Width taken at the right end.
    pub right: f32,
    /// Height of the caption row the buttons sit in.
    pub height: f32,
}

/// Where a header puts its own controls beside the buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reserve {
    /// Width to leave free at the header's right end, beside the buttons.
    pub inline_width: f32,
    /// How far to move the header down, under the caption row, when it is
    /// too narrow to share the row with the buttons.
    pub push_down: f32,
}

impl Footprint {
    /// Where a header `width` wide, whose own controls need `min_content`,
    /// goes beside the right-hand buttons: in the same row when both fit,
    /// otherwise under the caption row.
    pub fn reserve(&self, width: f32, min_content: f32) -> Reserve {
        if self.right <= 0.0 {
            Reserve::default()
        } else if width < min_content + self.right {
            Reserve {
                inline_width: 0.0,
                push_down: self.height,
            }
        } else {
            Reserve {
                inline_width: self.right,
                push_down: 0.0,
            }
        }
    }
}

/// The buttons' room in this window. `header_height` is the height of the
/// header the buttons sit in (as in [`Style`]); on macOS it is the footprint's
/// height, beside the traffic lights' width.
pub fn footprint(ctx: &egui::Context, header_height: f32) -> Footprint {
    if cfg!(target_os = "macos") {
        let inset = macos::traffic_light_inset(ctx);
        return Footprint {
            left: inset,
            right: 0.0,
            height: if inset > 0.0 { header_height } else { 0.0 },
        };
    }
    if !visible(ctx) {
        return Footprint::default();
    }
    footprint_for(cfg!(target_os = "linux"), layout(), header_height)
}

fn footprint_for(linux: bool, layout: &Layout, header_height: f32) -> Footprint {
    let (margin, height) = if linux {
        (LINUX_MARGIN, LINUX_HEIGHT)
    } else {
        (RESIZE_BORDER, header_height)
    };
    let side = |buttons: &[Button]| {
        if buttons.is_empty() {
            0.0
        } else {
            buttons.len() as f32 * BUTTON + margin
        }
    };
    Footprint {
        left: side(&layout.left),
        right: side(&layout.right),
        height,
    }
}

/// Makes `rect` move the window, and answers a double click on it. Call it
/// before the widgets inside `rect`, so they keep their clicks.
///
/// On macOS the drag starts on the press itself, as AppKit requires, and a
/// double click does what System Settings says (zoom, fill, minimize or
/// nothing). On Windows and Linux a double click maximizes or restores the
/// window, and the drag works only while the drawn title bar is [`visible`].
/// Nothing happens in full screen.
pub fn drag(ui: &mut egui::Ui, rect: Rect) {
    let fullscreen = ui
        .ctx()
        .input(|input| input.viewport().fullscreen.unwrap_or(false));
    let macos = cfg!(target_os = "macos");
    if fullscreen || !(macos || visible(ui.ctx())) {
        return;
    }
    if macos {
        macos::record_presses();
    }
    let response = ui.interact(
        rect,
        ui.id().with("fastframe-titlebar-drag"),
        egui::Sense::click_and_drag(),
    );
    if macos {
        // A quick click can be pressed and released within one pass.
        let (pressed, down) = ui.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
            )
        });
        if pressed
            && (response.is_pointer_button_down_on() || response.clicked())
            && macos::title_bar_press(ui.ctx(), down)
        {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    } else if response.double_clicked() {
        let maximized = ui
            .ctx()
            .input(|input| input.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if response.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

/// Draws the caption buttons over the window's top corners, above every
/// panel, and acts on their clicks.
pub fn controls(ui: &mut egui::Ui, style: &Style, labels: &Labels<'_>) {
    if !visible(ui.ctx()) {
        return;
    }
    let maximized = ui
        .ctx()
        .input(|input| input.viewport().maximized.unwrap_or(false));
    let linux = cfg!(target_os = "linux");
    // Windows centres its buttons in the header's height.
    let (margin, top, spacing) = if linux {
        (LINUX_MARGIN, LINUX_MARGIN, 8.0)
    } else {
        (
            RESIZE_BORDER,
            (style.header_height - BUTTON) / 2.0 + 2.0,
            0.0,
        )
    };
    let layout = layout();
    for (right, buttons) in [(false, &layout.left), (true, &layout.right)] {
        if buttons.is_empty() {
            continue;
        }
        let (id, anchor, offset) = if right {
            ("fastframe-titlebar-right", Align2::RIGHT_TOP, -margin)
        } else {
            ("fastframe-titlebar-left", Align2::LEFT_TOP, margin)
        };
        egui::Area::new(egui::Id::new(id))
            .anchor(anchor, vec2(offset, top))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().item_spacing.x = spacing;
                // Equal padding keeps a 28-point GNOME button round.
                if linux {
                    ui.spacing_mut().button_padding = vec2(6.0, 6.0);
                }
                ui.horizontal(|ui| {
                    for &button in buttons {
                        caption_button(ui, button, maximized, style, labels);
                    }
                });
            });
    }
}

fn caption_button(
    ui: &mut egui::Ui,
    button: Button,
    maximized: bool,
    style: &Style,
    labels: &Labels<'_>,
) {
    let (icon, label, command) = match button {
        Button::Minimize => (
            Icon::Minimize,
            labels.minimize,
            egui::ViewportCommand::Minimized(true),
        ),
        Button::Maximize if maximized => (
            Icon::Restore,
            labels.restore,
            egui::ViewportCommand::Maximized(false),
        ),
        Button::Maximize => (
            Icon::Maximize,
            labels.maximize,
            egui::ViewportCommand::Maximized(true),
        ),
        Button::Close => (Icon::Close, labels.close, egui::ViewportCommand::Close),
    };
    let response = if cfg!(target_os = "linux") {
        let image = theme_icon(icon)
            .unwrap_or_else(|| icon.image(style.icon, 16.0))
            .fit_to_exact_size(egui::Vec2::splat(16.0))
            .tint(style.icon)
            .alt_text(label);
        ui.add_sized(
            egui::Vec2::splat(28.0),
            egui::Button::image(image)
                .fill(style.button)
                .corner_radius(egui::CornerRadius::same(14)),
        )
    } else {
        let image = icon.image(style.icon, 14.0).alt_text(label);
        ui.scope(|ui| {
            caption_feedback(ui, button == Button::Close);
            ui.add_sized(
                egui::Vec2::splat(BUTTON),
                egui::Button::image(image).frame_when_inactive(false),
            )
        })
        .inner
    };
    if response.on_hover_text(label).clicked() {
        ui.ctx().send_viewport_cmd(command);
    }
}

/// The icon theme's glyph for `icon`, on GNOME.
fn theme_icon(icon: Icon) -> Option<egui::Image<'static>> {
    #[cfg(target_os = "linux")]
    return linux::icon(match icon {
        Icon::Minimize => linux::MINIMIZE,
        Icon::Maximize => linux::MAXIMIZE,
        Icon::Restore => linux::RESTORE,
        Icon::Close => linux::CLOSE,
    });
    #[cfg(not(target_os = "linux"))]
    {
        let _ = icon;
        None
    }
}

/// Windows' caption feedback: a light wash over Minimize and Maximize while
/// hovered, red under Close, each dimmer while pressed.
fn caption_feedback(ui: &mut egui::Ui, close: bool) {
    let (hover, pressed) = if close {
        (
            egui::Color32::from_rgb(232, 17, 35),
            egui::Color32::from_rgb(174, 13, 26),
        )
    } else {
        (
            egui::Color32::from_white_alpha(55),
            egui::Color32::from_white_alpha(32),
        )
    };
    let visuals = &mut ui.style_mut().visuals.widgets;
    visuals.hovered.bg_fill = hover;
    visuals.hovered.weak_bg_fill = hover;
    visuals.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.active.bg_fill = pressed;
    visuals.active.weak_bg_fill = pressed;
    visuals.active.bg_stroke = egui::Stroke::NONE;
}

/// Lets a press on the window's edges or corners resize it, with the
/// matching cursor while hovering there. Call it each frame after the
/// window's content. Does nothing unless the title bar is [`visible`], or
/// while the window is maximized.
pub fn resize(ui: &mut egui::Ui) {
    let maximized = ui
        .ctx()
        .input(|input| input.viewport().maximized.unwrap_or(false));
    if !visible(ui.ctx()) || maximized {
        return;
    }
    let Some(position) = ui.input(|input| input.pointer.hover_pos()) else {
        return;
    };
    let Some((direction, cursor)) = resize_direction(ui.ctx().content_rect(), position) else {
        return;
    };
    ui.ctx().set_cursor_icon(cursor);
    if ui.input(|input| input.pointer.primary_pressed()) {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

fn resize_direction(
    window: Rect,
    position: egui::Pos2,
) -> Option<(egui::ResizeDirection, egui::CursorIcon)> {
    if !window.contains(position) {
        return None;
    }
    let [left, right, top, bottom] = [
        position.x - window.left(),
        window.right() - position.x,
        position.y - window.top(),
        window.bottom() - position.y,
    ];
    let mut x = i8::from(right <= RESIZE_BORDER) - i8::from(left <= RESIZE_BORDER);
    let mut y = i8::from(bottom <= RESIZE_BORDER) - i8::from(top <= RESIZE_BORDER);
    // Near a corner, an edge press resizes both ways.
    if y != 0 {
        x = i8::from(right <= RESIZE_CORNER) - i8::from(left <= RESIZE_CORNER);
    }
    if x != 0 {
        y = i8::from(bottom <= RESIZE_CORNER) - i8::from(top <= RESIZE_CORNER);
    }
    use egui::{CursorIcon as C, ResizeDirection as D};
    match (x, y) {
        (-1, -1) => Some((D::NorthWest, C::ResizeNwSe)),
        (1, -1) => Some((D::NorthEast, C::ResizeNeSw)),
        (-1, 1) => Some((D::SouthWest, C::ResizeNeSw)),
        (1, 1) => Some((D::SouthEast, C::ResizeNwSe)),
        (-1, 0) => Some((D::West, C::ResizeHorizontal)),
        (1, 0) => Some((D::East, C::ResizeHorizontal)),
        (0, -1) => Some((D::North, C::ResizeVertical)),
        (0, 1) => Some((D::South, C::ResizeVertical)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
