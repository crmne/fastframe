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
        let sandbox = sandboxed(
            std::path::Path::new("/.flatpak-info").exists(),
            std::env::var_os("FLATPAK_ID").is_some(),
        );
        let icon_name = icon_name(config.id, sandbox, &icon_dirs());
        let item = Item {
            id: config.id,
            icon_name,
            title: config.title,
            icon: config.icon,
            menu: config.menu,
            router,
        };
        // Flatpak lets an app talk to the watcher but not own ksni's
        // generated StatusNotifierItem name; register the unique connection
        // name instead, as ksni requires for sandboxed apps (Spotifast
        // 1c03c21).
        let handle = item.disable_dbus_name(sandbox).spawn()?;
        Ok(Self { handle })
    }

    pub(crate) fn set_label(&mut self, id: &str, label: String) {
        self.handle.update(|item| {
            crate::set_label(&mut item.menu, id, label);
        });
    }

    pub(crate) fn set_visible(&mut self, id: &str, visible: bool) {
        self.handle.update(|item| {
            crate::set_visible(&mut item.menu, id, visible);
        });
    }

    /// The item runs on its own thread from the start.
    pub(crate) fn attach(&mut self) {}
}

/// Whether the app runs inside Flatpak's sandbox.
fn sandboxed(flatpak_info: bool, flatpak_id: bool) -> bool {
    flatpak_info || flatpak_id
}

/// The icon theme name a tray host can look up, or empty for none.
///
/// Many hosts draw only what they find through the icon theme and never fall
/// back to the pixmap (fastframe#4), so name the app's icon when the desktop
/// has it: a Flatpak exports its icon under the app id, and a package
/// installs one under the app's own id. A portable copy has neither, and its
/// host gets the pixmap alone, as before.
fn icon_name(id: &str, sandbox: bool, dirs: &[std::path::PathBuf]) -> String {
    if sandbox {
        return std::env::var("FLATPAK_ID").unwrap_or_default();
    }
    let installed = dirs.iter().any(|dir| {
        let hicolor = dir.join("icons/hicolor");
        let sizes = std::fs::read_dir(&hicolor).into_iter().flatten().flatten();
        sizes
            .map(|size| size.path().join("apps"))
            .chain([dir.join("pixmaps")])
            .any(|apps| {
                ["svg", "png"]
                    .iter()
                    .any(|ext| apps.join(format!("{id}.{ext}")).is_file())
            })
    });
    if installed {
        id.to_owned()
    } else {
        String::new()
    }
}

/// The XDG data directories an icon theme is read from, the user's first.
fn icon_dirs() -> Vec<std::path::PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".local/share"))
        });
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    home.into_iter()
        .chain(
            system
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(std::path::PathBuf::from),
        )
        .collect()
}

struct Item {
    id: &'static str,
    icon_name: String,
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

    fn icon_name(&self) -> String {
        self.icon_name.clone()
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
                MenuItem::Action { id, label, visible } => {
                    let id: &'static str = id;
                    ksni::menu::StandardItem {
                        label: dbusmenu_label(label),
                        visible: *visible,
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

/// A label as DBusMenu reads it: a single `_` marks the next letter as the
/// keyboard shortcut and disappears, so a literal one is doubled. Labels
/// carry names such as `q3_plan.md`, never shortcuts.
fn dbusmenu_label(label: &str) -> String {
    label.replace('_', "__")
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
    fn an_installed_icon_is_named_and_a_missing_one_is_not() {
        let root =
            std::env::temp_dir().join(format!("fastframe-tray-icons-{}", std::process::id()));
        let apps = root.join("icons/hicolor/scalable/apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("zapfast.svg"), "<svg/>").unwrap();
        let dirs = [root.join("missing"), root.clone()];
        assert_eq!(icon_name("zapfast", false, &dirs), "zapfast");
        assert_eq!(icon_name("spotifast", false, &dirs), "");
        std::fs::remove_dir_all(&root).unwrap();
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
            icon_name: String::new(),
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

    /// DBusMenu takes `_` for a shortcut marker and drops it, so a file name
    /// in the menu would lose its underscores.
    #[test]
    fn underscores_in_labels_are_shown_as_written() {
        assert_eq!(dbusmenu_label("Open q3_plan.md"), "Open q3__plan.md");
        assert_eq!(dbusmenu_label("Quit"), "Quit");
    }

    /// ksni keeps hidden entries in the menu with `visible` off, so the host
    /// hides and shows them in place.
    #[test]
    fn hidden_entries_are_sent_hidden_and_can_be_shown() {
        use ksni::Tray as _;
        let (sender, _events) = std::sync::mpsc::channel();
        let menu = vec![
            MenuItem::action("show", "Show or hide ZapFast"),
            MenuItem::action("lock", "Lock ZapFast").visible(false),
        ];
        let router = Router::new(sender, std::sync::Arc::new(|| {}), &menu);
        let mut item = Item {
            id: "zapfast",
            icon_name: String::new(),
            title: "ZapFast".into(),
            icon: |size| vec![0; size * size * 4],
            menu,
            router,
        };
        let shown = |item: &Item| -> Vec<bool> {
            item.menu()
                .iter()
                .map(|entry| match entry {
                    ksni::MenuItem::Standard(entry) => entry.visible,
                    _ => panic!("an entry"),
                })
                .collect()
        };
        assert_eq!(shown(&item), [true, false]);
        crate::set_visible(&mut item.menu, "lock", true);
        assert_eq!(shown(&item), [true, true]);
    }
}
