use super::*;
use egui::{CursorIcon as C, ResizeDirection as D, pos2};

#[test]
fn the_title_bar_is_drawn_only_where_supported_and_chosen() {
    assert!(enabled_for(true, true));
    assert!(!enabled_for(true, false));
    assert!(!enabled_for(false, true));
    assert!(!enabled_for(false, false));
}

/// The portal gives GNOME's button layout as a plain string, where gsettings
/// quoted it; both read the same, and a side may be empty.
#[test]
fn gnome_button_layouts_read_with_or_without_quotes() {
    use Button::*;
    for layout in [
        "'close,minimize:appmenu,maximize'",
        "close,minimize:appmenu,maximize",
    ] {
        assert_eq!(
            Layout::from_gnome(layout),
            Some(Layout {
                left: vec![Close, Minimize],
                right: vec![Maximize],
            }),
            "{layout}"
        );
    }
    assert_eq!(
        Layout::from_gnome("appmenu:close"),
        Some(Layout {
            left: vec![],
            right: vec![Close],
        })
    );
    assert_eq!(
        Layout::from_gnome("close:close,minimize"),
        Some(Layout {
            left: vec![Close],
            right: vec![Minimize],
        }),
        "a button appears once"
    );
    assert_eq!(Layout::from_gnome("close,minimize"), None);
}

#[test]
fn the_buttons_take_their_slots_and_a_margin() {
    let standard = Layout::standard();
    assert_eq!(
        footprint_for(false, &standard, 48.0),
        Footprint {
            left: 0.0,
            right: 3.0 * 36.0 + 5.0,
            height: 48.0,
        },
        "Windows centres its buttons in the header"
    );
    let gnome = Layout::from_gnome("close:minimize,maximize").unwrap();
    assert_eq!(
        footprint_for(true, &gnome, 48.0),
        Footprint {
            left: 36.0 + 14.0,
            right: 2.0 * 36.0 + 14.0,
            height: 41.0,
        }
    );
    let none = Layout::from_gnome("appmenu:").unwrap();
    assert_eq!(
        footprint_for(true, &none, 48.0),
        Footprint {
            left: 0.0,
            right: 0.0,
            height: 41.0,
        }
    );
}

/// A wide header shares the caption row with the buttons; a narrow one moves
/// under it. Spotifast's top bar needs 510 points beside them.
#[test]
fn a_header_goes_beside_the_buttons_while_both_fit() {
    let windows = footprint_for(false, &Layout::standard(), 48.0);
    assert_eq!(
        windows.reserve(800.0, 510.0),
        Reserve {
            inline_width: 113.0,
            push_down: 0.0,
        }
    );
    assert_eq!(
        windows.reserve(623.0, 510.0),
        Reserve {
            inline_width: 113.0,
            push_down: 0.0,
        }
    );
    assert_eq!(
        windows.reserve(622.0, 510.0),
        Reserve {
            inline_width: 0.0,
            push_down: 48.0,
        }
    );
    assert_eq!(
        Footprint::default().reserve(100.0, 510.0),
        Reserve::default(),
        "no buttons, no room taken"
    );
}

#[test]
fn edges_and_corners_resize_the_window() {
    let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
    let at = |x, y| resize_direction(window, pos2(x, y));
    assert_eq!(at(400.0, 300.0), None, "the middle");
    assert_eq!(at(900.0, 300.0), None, "outside");
    assert_eq!(at(2.0, 300.0), Some((D::West, C::ResizeHorizontal)));
    assert_eq!(at(798.0, 300.0), Some((D::East, C::ResizeHorizontal)));
    assert_eq!(at(400.0, 2.0), Some((D::North, C::ResizeVertical)));
    assert_eq!(at(400.0, 598.0), Some((D::South, C::ResizeVertical)));
    assert_eq!(at(2.0, 2.0), Some((D::NorthWest, C::ResizeNwSe)));
    assert_eq!(at(798.0, 598.0), Some((D::SouthEast, C::ResizeNwSe)));
    // Within the corner's length along an edge, both ways.
    assert_eq!(at(8.0, 2.0), Some((D::NorthWest, C::ResizeNwSe)));
    assert_eq!(at(2.0, 592.0), Some((D::SouthWest, C::ResizeNeSw)));
    assert_eq!(at(792.0, 2.0), Some((D::NorthEast, C::ResizeNeSw)));
    assert_eq!(at(20.0, 2.0), Some((D::North, C::ResizeVertical)));
}

/// With the title bar off, nothing is drawn or reserved on Windows and
/// Linux. macOS keeps the traffic lights, whose room is always reserved.
#[test]
fn nothing_is_reserved_while_the_title_bar_is_off_but_the_traffic_lights() {
    let ctx = egui::Context::default();
    set_enabled(false);
    assert!(!visible(&ctx));
    let expected = if cfg!(target_os = "macos") {
        Footprint {
            left: 84.0,
            right: 0.0,
            height: 48.0,
        }
    } else {
        Footprint::default()
    };
    assert_eq!(footprint(&ctx, 48.0), expected);
}

/// A title bar is no stop for the Tab key: the header's widgets take it.
#[test]
fn the_drag_area_takes_no_keyboard_focus() {
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(400.0, 40.0));
    let mut id = None;
    for events in [vec![], vec![tab()], vec![]] {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| id = Some(area(ui, rect).id));
        output.textures_delta.clear();
    }
    assert_ne!(ctx.memory(|memory| memory.focused()), id);
}

fn tab() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Tab,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
