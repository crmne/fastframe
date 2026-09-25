//! The tray-icon item shared by Windows and macOS.

use tray_icon::menu::{Menu, MenuEvent, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::{Config, MenuItem, Router};

/// The icon's size in pixels; the system scales it for the tray.
const ICON_SIZE: usize = 32;

/// The item and its entries, kept for their labels. Dropping it removes the
/// item.
pub(crate) struct Item {
    _icon: TrayIcon,
    entries: Vec<(&'static str, tray_icon::menu::MenuItem)>,
}

impl Item {
    /// Changes an entry's label; unknown ids are ignored.
    pub(crate) fn set_label(&self, id: &str, label: &str) {
        if let Some((_, entry)) = self.entries.iter().find(|(known, _)| *known == id) {
            entry.set_text(label);
        }
    }
}

/// Makes the item on the current thread and routes its events to `router`.
pub(crate) fn build(config: &Config, router: Router) -> Result<Item, Box<dyn std::error::Error>> {
    let size = ICON_SIZE as u32;
    let (draw, template) = match config.template_icon {
        Some(template) if cfg!(target_os = "macos") => (template, true),
        _ => (config.icon, false),
    };
    let icon = Icon::from_rgba(draw(ICON_SIZE), size, size)?;
    let menu = Menu::new();
    let mut entries = Vec::new();
    for item in &config.menu {
        match item {
            MenuItem::Action { id, label } => {
                let entry =
                    tray_icon::menu::MenuItem::with_id(crate::menu_id(id), label, true, None);
                menu.append(&entry)?;
                entries.push((*id, entry));
            }
            MenuItem::Separator => menu.append(&PredefinedMenuItem::separator())?,
        }
    }
    // Left click toggles (macOS) or shows (Windows) the window; the menu
    // opens on right click (Spotifast #310).
    let icon = TrayIconBuilder::new()
        .with_icon(icon)
        .with_icon_as_template(template)
        .with_tooltip(&config.title)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    router.install();
    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        crate::claim_menu_event(&event.id.0);
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            router.send(crate::left_click(cfg!(windows)));
        }
    }));

    Ok(Item {
        _icon: icon,
        entries,
    })
}
