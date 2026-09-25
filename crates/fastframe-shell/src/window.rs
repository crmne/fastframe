//! Bring back a window restored where no monitor shows it.
//!
//! eframe restores a window's saved position. When the displays were
//! rearranged, or a secondary display comes up late after a restart, that
//! position can be on no monitor at all, and Windows then leaves the window
//! unreachable (ZapFast #171). Call [`recover_offscreen`] once, on a new
//! window's first frame:
//!
//! ```no_run
//! struct Window { recovery_checked: bool }
//!
//! impl eframe::App for Window {
//!     fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
//!         if !std::mem::replace(&mut self.recovery_checked, true) {
//!             fastframe_shell::window::recover_offscreen(ctx, frame);
//!         }
//!     }
//!     fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
//! }
//! ```
//!
//! Wayland does not reveal global window positions, so nothing happens
//! there. macOS keeps windows on a screen itself, and winit's macOS
//! coordinates disagree between displays with different scale factors, so it
//! is left alone. Windows and X11 report the window and every monitor in the
//! same physical pixels.

use egui::{Pos2, Rect};

/// Where to move a window that no connected monitor shows, in physical
/// virtual-desktop pixels, or `None` to leave it where it is.
///
/// Any overlap with any monitor counts as visible, so a valid position on a
/// secondary display (including negative coordinates left of or above the
/// primary one) is never moved. The window goes to the middle of the first
/// monitor listed (put the primary one first), pinned to its top-left corner
/// when it is larger than that monitor. Without monitors nothing moves.
pub fn recovered_position(window: Rect, monitors: &[Rect]) -> Option<Pos2> {
    let target = *monitors.first()?;
    if monitors
        .iter()
        .any(|monitor| window.intersect(*monitor).area() > 0.0)
    {
        return None;
    }
    let slack = (target.size() - window.size()).max(egui::Vec2::ZERO);
    Some((target.min + slack / 2.0).round())
}

/// Moves the window onto the primary monitor when it is on none of the
/// connected ones. Does nothing on macOS, on Wayland, for a minimized
/// window, and without a native window (tests).
pub fn recover_offscreen(ctx: &egui::Context, frame: &eframe::Frame) {
    #[cfg(not(target_os = "macos"))]
    recover(ctx, frame);
    #[cfg(target_os = "macos")]
    let _ = (ctx, frame);
}

#[cfg(not(target_os = "macos"))]
fn recover(ctx: &egui::Context, frame: &eframe::Frame) {
    let Some(window) = frame.winit_window() else {
        return;
    };
    // A minimized window on Windows reports a parking position far off
    // screen; restoring it brings back its real one.
    if window.is_minimized() == Some(true) {
        return;
    }
    let Ok(position) = window.outer_position() else {
        return;
    };
    let size = window.outer_size();
    let rect = |x: i32, y: i32, width: u32, height: u32| {
        Rect::from_min_size(
            egui::pos2(x as f32, y as f32),
            egui::vec2(width as f32, height as f32),
        )
    };
    // The primary monitor comes first, as the place a lost window goes; a
    // platform without one falls back to the first connected monitor.
    let monitors: Vec<_> = window
        .primary_monitor()
        .into_iter()
        .chain(window.available_monitors())
        .map(|monitor| {
            let (position, size) = (monitor.position(), monitor.size());
            rect(position.x, position.y, size.width, size.height)
        })
        .collect();
    let Some(target) = recovered_position(
        rect(position.x, position.y, size.width, size.height),
        &monitors,
    ) else {
        return;
    };
    log::warn!("the restored window was on no connected monitor; moving it to the primary one");
    let pixels_per_point = ctx.input(|input| input.pixels_per_point);
    ctx.send_viewport_cmd(move_to_physical(target, pixels_per_point));
}

/// The command that moves the window to `target` physical pixels.
///
/// egui-winit multiplies the position by the window's pixels per point, so
/// dividing by the same factor asks for exactly these physical pixels,
/// whatever the scale of the monitor the window is on now.
#[cfg(any(not(target_os = "macos"), test))]
fn move_to_physical(target: Pos2, pixels_per_point: f32) -> egui::ViewportCommand {
    egui::ViewportCommand::OuterPosition(target / pixels_per_point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn monitor(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(width, height))
    }

    #[test]
    fn a_window_above_every_monitor_moves_to_the_middle_of_the_first() {
        let window = Rect::from_min_size(pos2(100.0, -500.0), vec2(400.0, 300.0));
        assert_eq!(
            recovered_position(window, &[monitor(0.0, 0.0, 1920.0, 1080.0)]),
            Some(pos2(760.0, 390.0))
        );
    }

    #[test]
    fn a_window_partly_on_a_monitor_stays() {
        let window = Rect::from_min_size(pos2(-100.0, 100.0), vec2(400.0, 300.0));
        assert_eq!(
            recovered_position(window, &[monitor(0.0, 0.0, 1920.0, 1080.0)]),
            None
        );
    }

    #[test]
    fn a_window_on_a_monitor_left_of_or_above_the_primary_stays() {
        let monitors = [
            monitor(0.0, 0.0, 1920.0, 1080.0),
            monitor(-1920.0, 0.0, 1920.0, 1080.0),
            monitor(0.0, -1440.0, 2560.0, 1440.0),
        ];
        let left = Rect::from_min_size(pos2(-1600.0, 100.0), vec2(800.0, 600.0));
        let above = Rect::from_min_size(pos2(200.0, -1300.0), vec2(800.0, 600.0));
        assert_eq!(recovered_position(left, &monitors), None);
        assert_eq!(recovered_position(above, &monitors), None);
    }

    /// ZapFast #171: a window saved on a display to the right, which is gone
    /// after a restart. The primary one here is not at the origin.
    #[test]
    fn a_window_left_where_a_disconnected_monitor_was_moves_to_the_primary() {
        let window = Rect::from_min_size(pos2(3891.0, -358.0), vec2(1180.0, 780.0));
        let monitors = [
            monitor(1920.0, 0.0, 1920.0, 1080.0),
            monitor(0.0, 0.0, 1920.0, 1080.0),
        ];
        assert_eq!(
            recovered_position(window, &monitors),
            Some(pos2(2290.0, 150.0))
        );
    }

    #[test]
    fn a_window_larger_than_the_monitor_is_pinned_to_its_corner() {
        let window = Rect::from_min_size(pos2(5000.0, 5000.0), vec2(2000.0, 1200.0));
        assert_eq!(
            recovered_position(window, &[monitor(-1280.0, 0.0, 1280.0, 1024.0)]),
            Some(pos2(-1280.0, 0.0))
        );
    }

    #[test]
    fn without_monitors_nothing_moves() {
        let window = Rect::from_min_size(pos2(-5000.0, -5000.0), vec2(400.0, 300.0));
        assert_eq!(recovered_position(window, &[]), None);
    }

    #[test]
    fn the_move_asks_for_physical_pixels_at_any_scale() {
        assert_eq!(
            move_to_physical(pos2(2290.0, 150.0), 1.25),
            egui::ViewportCommand::OuterPosition(pos2(1832.0, 120.0))
        );
    }
}
