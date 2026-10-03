//! The tray-icon item shared by Windows and macOS.

use tray_icon::menu::{Menu, MenuEvent, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::{Config, DrawIcon, MenuItem, Router};

/// The icon's size in pixels; the system scales it for the tray.
const ICON_SIZE: usize = 32;

/// The item and its menu. Dropping it removes the item.
pub(crate) struct Item {
    icon: TrayIcon,
    entries: Entries,
}

impl Item {
    /// Changes an entry's label; unknown ids are ignored.
    pub(crate) fn set_label(&self, id: &str, label: &str) {
        self.entries.set_label(id, label);
    }

    /// Shows or hides an entry in its place; unknown ids are ignored.
    pub(crate) fn set_visible(&mut self, id: &str, visible: bool) {
        self.entries.set_visible(id, visible);
    }

    /// Greys an entry out or lets it be chosen; unknown ids are ignored.
    pub(crate) fn set_enabled(&self, id: &str, enabled: bool) {
        if let Some(entry) = self.entries.entry(id) {
            entry.set_enabled(enabled);
        }
    }

    /// Changes the icon, the macOS template image when there is one.
    pub(crate) fn set_icon(&self, icon: DrawIcon, template_icon: Option<DrawIcon>) {
        let (draw, template) = pick_icon(icon, template_icon);
        let size = ICON_SIZE as u32;
        let result = Icon::from_rgba(draw(ICON_SIZE), size, size)
            .map_err(|error| error.to_string())
            .and_then(|icon| {
                self.icon
                    .set_icon(Some(icon))
                    .map_err(|error| error.to_string())
            });
        #[cfg(target_os = "macos")]
        self.icon.set_icon_as_template(template);
        #[cfg(not(target_os = "macos"))]
        let _ = template;
        if let Err(error) = result {
            log::warn!("the tray icon could not be changed: {error}");
        }
    }

    /// Changes the tooltip.
    pub(crate) fn set_tooltip(&self, text: &str) {
        if let Err(error) = self.icon.set_tooltip(Some(text)) {
            log::warn!("the tray tooltip could not be changed: {error}");
        }
    }
}

/// The icon to draw, and whether it is a macOS template image.
fn pick_icon(icon: DrawIcon, template_icon: Option<DrawIcon>) -> (DrawIcon, bool) {
    match template_icon {
        Some(template) if cfg!(target_os = "macos") => (template, true),
        _ => (icon, false),
    }
}

/// The menu and its entries, kept for their labels and visibility.
struct Entries {
    /// The menu the item shows. muda has no hidden entries, so a hidden entry
    /// is taken out and put back.
    menu: Menu,
    /// What the menu holds, shown or not, in order.
    model: Vec<MenuItem>,
    entries: Vec<(&'static str, tray_icon::menu::MenuItem)>,
}

impl Entries {
    /// Makes the menu with the entries of `model` that are shown.
    fn new(model: &[MenuItem]) -> tray_icon::menu::Result<Self> {
        let menu = Menu::new();
        let mut entries = Vec::new();
        for item in model {
            match item {
                MenuItem::Action {
                    id,
                    label,
                    visible,
                    enabled,
                } => {
                    let entry = tray_icon::menu::MenuItem::with_id(
                        crate::menu_id(id),
                        label,
                        *enabled,
                        None,
                    );
                    if *visible {
                        menu.append(&entry)?;
                    }
                    entries.push((*id, entry));
                }
                MenuItem::Separator => menu.append(&PredefinedMenuItem::separator())?,
            }
        }
        Ok(Self {
            menu,
            model: model.to_vec(),
            entries,
        })
    }

    fn set_label(&self, id: &str, label: &str) {
        if let Some(entry) = self.entry(id) {
            entry.set_text(label);
        }
    }

    fn set_visible(&mut self, id: &str, visible: bool) {
        if !crate::set_visible(&mut self.model, id, visible) {
            return;
        }
        let Some(entry) = self.entry(id) else {
            return;
        };
        let result = if visible {
            crate::shown_position(&self.model, id)
                .map_or(Ok(()), |position| self.menu.insert(entry, position))
        } else {
            self.menu.remove(entry)
        };
        if let Err(error) = result {
            log::warn!("the tray entry {id} could not be shown or hidden: {error}");
        }
    }

    fn entry(&self, id: &str) -> Option<&tray_icon::menu::MenuItem> {
        self.entries
            .iter()
            .find(|(known, _)| *known == id)
            .map(|(_, entry)| entry)
    }
}

/// Makes the item on the current thread and routes its events to `router`.
pub(crate) fn build(config: &Config, router: Router) -> Result<Item, Box<dyn std::error::Error>> {
    let size = ICON_SIZE as u32;
    let (draw, template) = pick_icon(config.icon, config.template_icon);
    let icon = Icon::from_rgba(draw(ICON_SIZE), size, size)?;
    let entries = Entries::new(&config.menu)?;
    // Left click toggles (macOS) or shows (Windows) the window; the menu
    // opens on right click (Spotifast #310), or on any click on macOS when
    // the app asks for the menu-bar habit.
    let menu_on_click = config.menu_on_click && cfg!(target_os = "macos");
    let icon = TrayIconBuilder::new()
        .with_icon(icon)
        .with_icon_as_template(template)
        .with_tooltip(&config.title)
        .with_menu(Box::new(entries.menu.clone()))
        .with_menu_on_left_click(menu_on_click)
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
            && !menu_on_click
        {
            router.send(crate::left_click(cfg!(windows)));
        }
    }));

    Ok(Item { icon, entries })
}

// muda menus can be made off the main thread on Windows only; macOS needs
// AppKit's main thread, which tests do not run on.
#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn shown(entries: &Entries) -> Vec<String> {
        entries
            .menu
            .items()
            .iter()
            .map(|item| item.id().0.clone())
            .collect()
    }

    #[test]
    fn hidden_entries_leave_the_menu_and_come_back_in_place() {
        let mut entries = Entries::new(&[
            MenuItem::action("show", "Show"),
            MenuItem::action("lock", "Lock").visible(false),
            MenuItem::Separator,
            MenuItem::action("quit", "Quit"),
        ])
        .unwrap();
        let separator = shown(&entries)[1].clone();
        assert_eq!(
            shown(&entries),
            [
                crate::menu_id("show"),
                separator.clone(),
                crate::menu_id("quit")
            ]
        );
        entries.set_visible("lock", true);
        entries.set_visible("lock", true);
        assert_eq!(
            shown(&entries),
            [
                crate::menu_id("show"),
                crate::menu_id("lock"),
                separator.clone(),
                crate::menu_id("quit")
            ]
        );
        entries.set_visible("show", false);
        entries.set_visible("missing", false);
        assert_eq!(
            shown(&entries),
            [crate::menu_id("lock"), separator, crate::menu_id("quit")]
        );
    }
}
