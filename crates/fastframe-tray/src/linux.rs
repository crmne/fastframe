//! The Linux StatusNotifierItem, on ksni's own thread.

use ksni::blocking::TrayMethods;

use crate::{Config, DrawIcon, Event, MenuItem, Router};

/// The size of the pixmap handed to the tray host, which scales it.
const ICON_SIZE: usize = 64;

pub(crate) struct Host {
    handle: ksni::blocking::Handle<Item>,
}

impl Host {
    pub(crate) fn start(config: Config, router: Router) -> Result<Self, ksni::Error> {
        let item = Item {
            id: config.id,
            title: config.title,
            icon: config.icon,
            menu: config.menu,
            router,
        };
        // Flatpak lets an app talk to the watcher but not own ksni's
        // generated StatusNotifierItem name; register the unique connection
        // name instead, as ksni requires for sandboxed apps (Spotifast
        // 1c03c21).
        let handle = item
            .disable_dbus_name(sandboxed(
                std::path::Path::new("/.flatpak-info").exists(),
                std::env::var_os("FLATPAK_ID").is_some(),
            ))
            .spawn()?;
        Ok(Self { handle })
    }

    pub(crate) fn set_label(&mut self, id: &str, label: String) {
        self.handle.update(|item| {
            crate::set_label(&mut item.menu, id, label);
        });
    }

    /// The item runs on its own thread from the start.
    pub(crate) fn attach(&mut self) {}
}

/// Whether the app runs inside Flatpak's sandbox.
fn sandboxed(flatpak_info: bool, flatpak_id: bool) -> bool {
    flatpak_info || flatpak_id
}

struct Item {
    id: &'static str,
    title: String,
    icon: DrawIcon,
    menu: Vec<MenuItem>,
    router: Router,
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        self.id.to_owned()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![ksni::Icon {
            width: ICON_SIZE as i32,
            height: ICON_SIZE as i32,
            data: argb(&(self.icon)(ICON_SIZE)),
        }]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.router.send(Event::Toggle);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.menu
            .iter()
            .map(|item| match item {
                MenuItem::Action { id, label } => {
                    let id: &'static str = id;
                    ksni::menu::StandardItem {
                        label: label.clone(),
                        activate: Box::new(move |item: &mut Self| {
                            item.router.send(Event::Menu(id));
                        }),
                        ..Default::default()
                    }
                    .into()
                }
                MenuItem::Separator => ksni::MenuItem::Separator,
            })
            .collect()
    }
}

/// RGBA pixels as the network-byte-order ARGB32 ksni expects.
fn argb(rgba: &[u8]) -> Vec<u8> {
    let (pixels, _) = rgba.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_become_argb_in_network_order() {
        assert_eq!(
            argb(&[1, 2, 3, 4, 5, 6, 7, 8, 9]),
            [4, 1, 2, 3, 8, 5, 6, 7],
            "a trailing partial pixel is dropped"
        );
    }

    #[test]
    fn either_flatpak_marker_means_sandboxed() {
        assert!(sandboxed(true, false));
        assert!(sandboxed(false, true));
        assert!(!sandboxed(false, false));
    }

    #[test]
    fn the_menu_mirrors_the_model_and_its_entries_send_their_ids() {
        use ksni::Tray as _;
        let (sender, events) = std::sync::mpsc::channel();
        let menu = vec![
            MenuItem::action("show", "Show or hide ZapFast"),
            MenuItem::Separator,
            MenuItem::action("quit", "Quit"),
        ];
        let router = Router::new(sender, std::sync::Arc::new(|| {}), &menu);
        let mut item = Item {
            id: "zapfast",
            title: "ZapFast".into(),
            icon: |size| vec![0; size * size * 4],
            menu,
            router,
        };
        assert_eq!(item.id(), "zapfast");
        assert_eq!(item.title(), "ZapFast");
        let pixmap = item.icon_pixmap();
        assert_eq!(pixmap[0].data.len(), ICON_SIZE * ICON_SIZE * 4);

        let entries = item.menu();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[1], ksni::MenuItem::Separator));
        let ksni::MenuItem::Standard(quit) = &entries[2] else {
            panic!("an entry");
        };
        assert_eq!(quit.label, "Quit");
        (quit.activate)(&mut item);
        item.activate(0, 0);
        assert_eq!(
            events.try_iter().collect::<Vec<_>>(),
            [Event::Menu("quit"), Event::Toggle]
        );
    }
}
