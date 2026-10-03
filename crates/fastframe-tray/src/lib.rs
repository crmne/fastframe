//! A tray item with the app's own menu: a StatusNotifierItem on Linux, a
//! notification-area icon on Windows, a menu-bar item on macOS.
//!
//! The app supplies its name, its icon, and the menu; the tray reports what
//! was clicked as [`Event`]s on a channel and calls a wake function so the
//! interface reads them promptly, even while no window exists.
//!
//! ```no_run
//! use fastframe_tray::{Config, Event, MenuItem, Tray};
//!
//! fn icon(size: usize) -> Vec<u8> {
//!     vec![255; size * size * 4] // The app draws its RGBA icon here.
//! }
//!
//! let config = Config {
//!     id: "zapfast",
//!     title: "ZapFast".into(),
//!     icon,
//!     template_icon: None,
//!     themed_icon: true,
//!     menu_on_click: false,
//!     menu: vec![
//!         MenuItem::action("show", "Show or hide ZapFast"),
//!         MenuItem::Separator,
//!         MenuItem::action("quit", "Quit"),
//!     ],
//! };
//! // `None` when the tray cannot be made at all.
//! let mut tray = Tray::spawn(config, || { /* repaint the window */ });
//! // Whether closing the window should keep the app running: a panel shows
//! // the item now (on Linux one may appear later, as at login).
//! let keep_running = tray.as_ref().is_some_and(Tray::is_shown);
//!
//! // Each frame, and each headless tick:
//! if let Some(tray) = &tray {
//!     for event in tray.events() {
//!         match event {
//!             Event::Toggle => { /* show or hide the window */ }
//!             Event::Show => { /* show the window */ }
//!             Event::Menu("quit") => { /* quit */ }
//!             Event::Menu(_) => {}
//!         }
//!     }
//! }
//! // Entries can change their label, be greyed out, or come and go in their
//! // place, and the item can change its icon and tooltip:
//! if let Some(tray) = &mut tray {
//!     tray.set_label("show", "Show ZapFast");
//!     tray.set_enabled("show", true);
//!     tray.set_visible("show", true);
//!     tray.set_icon(icon, None);
//!     tray.set_tooltip("ZapFast\nConnected");
//! }
//! // When a window has been made (macOS creates the item then):
//! if let Some(tray) = &mut tray {
//!     tray.attach();
//! }
//! // While no window exists, wait with `fastframe_tray::idle` so macOS keeps
//! // serving the item.
//! fastframe_tray::idle(std::time::Duration::from_millis(150));
//! ```
//!
//! Per platform:
//!
//! - **Linux**: ksni on its own thread. The item registers even before the
//!   panel is up (an app started at login can beat it), and shows once a
//!   StatusNotifier host appears; [`Tray::is_shown`] says whether one shows
//!   it now. Inside Flatpak the item registers its unique bus name, since the
//!   sandbox does not let it own one.
//! - **Windows**: tray-icon on its own thread with a message loop. A left
//!   click asks to [`Event::Show`] (each release of a double-click arrives
//!   separately, possibly on both sides of window creation, so a toggle
//!   would flicker); the menu opens on right click.
//! - **macOS**: status items live on the main thread and only while AppKit's
//!   event loop runs, so the item is made by the first [`Tray::attach`] (or,
//!   for an app with no window yet, [`Tray::create_item`]), and
//!   [`idle`] runs AppKit's loop while no window exists. A Dock click asks to
//!   [`Event::Show`]. The menu opens on right click and left click toggles,
//!   or any click opens the menu with [`Config::menu_on_click`].
//!
//! The menu handler of tray-icon (muda) is process-wide, and muda keeps the
//! first one installed, ignoring the rest. An app that builds its own muda
//! menus (a macOS menu bar) must install its handler before the tray makes
//! its item, and pass each event to [`claim_menu_event`] first.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(windows, target_os = "macos"))]
mod native;
#[cfg(windows)]
mod windows;

/// Draws the app's icon: square RGBA pixels, `size` on each side.
pub type DrawIcon = fn(size: usize) -> Vec<u8>;

/// What the tray shows. The app supplies every label, already translated.
#[derive(Clone, Debug)]
pub struct Config {
    /// The app's short id (`zapfast`): the StatusNotifier id and the tray
    /// thread's name.
    pub id: &'static str,
    /// The app's name (`ZapFast`): the item's title and tooltip.
    pub title: String,
    /// The full-colour icon (Linux, Windows, and macOS without a template).
    pub icon: DrawIcon,
    /// A macOS template image: black on transparent, which macOS recolours
    /// to match the menu bar. `None` uses [`Config::icon`] as it is.
    pub template_icon: Option<DrawIcon>,
    /// On Linux, also name the app's installed icon to the panel, for the
    /// hosts that draw only icons they look up by name. Turn it off for an
    /// app whose tray icon differs from its app icon (a glyph, or one that
    /// changes with [`Tray::set_icon`]), since a host that has the name
    /// draws that instead of the pixels.
    pub themed_icon: bool,
    /// On macOS, open the menu on any click, as menu-bar items do, rather
    /// than toggling the window on a left click.
    pub menu_on_click: bool,
    /// The menu, top to bottom.
    pub menu: Vec<MenuItem>,
}

/// One entry of the tray menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// A clickable entry. Clicking it sends [`Event::Menu`] with `id`.
    Action {
        /// What [`Event::Menu`] carries back. Unique within the menu.
        id: &'static str,
        /// What the entry says. Change it with [`Tray::set_label`].
        label: String,
        /// Whether the menu shows it. Change it with [`Tray::set_visible`].
        visible: bool,
        /// Whether it can be chosen; a disabled entry is greyed out, as for a
        /// status line. Change it with [`Tray::set_enabled`].
        enabled: bool,
    },
    /// A line between groups of entries.
    Separator,
}

impl MenuItem {
    /// A clickable entry, shown and enabled.
    pub fn action(id: &'static str, label: impl Into<String>) -> Self {
        Self::Action {
            id,
            label: label.into(),
            visible: true,
            enabled: true,
        }
    }

    /// The same entry, enabled or greyed out from the start.
    #[must_use]
    pub fn enabled(mut self, can_choose: bool) -> Self {
        if let Self::Action { enabled, .. } = &mut self {
            *enabled = can_choose;
        }
        self
    }

    /// The same entry, shown or hidden from the start. A separator is always
    /// shown.
    #[must_use]
    pub fn visible(mut self, shown: bool) -> Self {
        if let Self::Action { visible, .. } = &mut self {
            *visible = shown;
        }
        self
    }
}

/// What the person did with the tray item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A left click on Linux or macOS: show the window if it is hidden,
    /// hide it otherwise.
    Toggle,
    /// Bring the window up: a left click on Windows, or a Dock click on
    /// macOS (even for a minimized window, which AppKit reports as visible).
    Show,
    /// A menu entry was chosen.
    Menu(&'static str),
}

/// The tray item. Dropping it removes the item on Linux and Windows; the
/// macOS item lives until the process ends.
pub struct Tray {
    events: Receiver<Event>,
    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    host: Host,
}

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray").finish_non_exhaustive()
    }
}

impl Tray {
    /// Registers the item, or returns `None` when it cannot be made (no
    /// session bus, or an OS error). On Linux the item registers even while
    /// no panel shows it yet; ask [`is_shown`](Self::is_shown) before letting
    /// a closed window keep the app running.
    ///
    /// `wake` is called after each event is queued, from the tray's thread.
    pub fn spawn(config: Config, wake: impl Fn() + Send + Sync + 'static) -> Option<Self> {
        let (sender, events) = std::sync::mpsc::channel();
        let router = Router::new(sender, Arc::new(wake), &config.menu);
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        match Host::start(config, router) {
            Ok(host) => Some(Self { events, host }),
            Err(error) => {
                log::info!("no system tray available: {error}");
                None
            }
        }
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        {
            let _ = (config, router, events);
            log::info!("no system tray on this platform");
            None
        }
    }

    /// The events since the last call, oldest first.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Changes what the entry `id` says (Play or Pause, say). Unknown ids
    /// are ignored.
    pub fn set_label(&mut self, id: &str, label: impl Into<String>) {
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        self.host.set_label(id, label.into());
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        let _ = (id, label);
    }

    /// Shows or hides the entry `id` (an action that only applies while a
    /// setting is on, say), keeping its place in the menu. Unknown ids are
    /// ignored.
    pub fn set_visible(&mut self, id: &str, visible: bool) {
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        self.host.set_visible(id, visible);
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        let _ = (id, visible);
    }

    /// Greys out the entry `id`, or lets it be chosen again. Unknown ids are
    /// ignored.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        self.host.set_enabled(id, enabled);
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        let _ = (id, enabled);
    }

    /// Changes the icon (dimmed while offline, say), with its macOS template
    /// image as in [`Config`].
    pub fn set_icon(&mut self, icon: DrawIcon, template_icon: Option<DrawIcon>) {
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        self.host.set_icon(icon, template_icon);
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        let _ = (icon, template_icon);
    }

    /// Changes the tooltip, which is the app's title until this is called.
    /// On Linux the first line is the tooltip's title and the rest its
    /// detail; elsewhere the text is shown as it is.
    pub fn set_tooltip(&mut self, text: impl Into<String>) {
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        self.host.set_tooltip(text.into());
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        let _ = text;
    }

    /// Whether a panel shows the item now, so closing the window can keep
    /// the app running. On Linux a panel may come and go (it starts after an
    /// app launched at login, or a desktop has none); on Windows and macOS
    /// the item is always shown.
    #[must_use]
    pub fn is_shown(&self) -> bool {
        #[cfg(target_os = "linux")]
        return self.host.is_shown();
        #[cfg(not(target_os = "linux"))]
        true
    }

    /// Makes the item now, without bringing the app forward, for an app that
    /// starts with no window (in the background at login). On macOS the item
    /// is otherwise made by the first [`attach`](Self::attach), which also
    /// activates the app; keep AppKit running with [`idle`] meanwhile.
    /// Elsewhere the item exists from [`spawn`](Self::spawn), and this does
    /// nothing.
    pub fn create_item(&mut self) {
        #[cfg(target_os = "macos")]
        self.host.create_item();
    }

    /// A window exists. On macOS the first call makes the item, and each
    /// call brings the application forward; elsewhere it does nothing.
    pub fn attach(&mut self) {
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        self.host.attach();
    }
}

/// Waits `duration` while no window exists.
///
/// On macOS this runs AppKit's event loop, so the menu-bar item, the Dock
/// and the app menus keep answering; elsewhere the tray has its own thread
/// and this sleeps.
pub fn idle(duration: Duration) {
    #[cfg(target_os = "macos")]
    macos::pump(duration);
    #[cfg(not(target_os = "macos"))]
    std::thread::sleep(duration);
}

/// Routes a muda menu event to the tray when its id is one of the tray's,
/// and returns whether it was.
///
/// The tray installs muda's process-wide handler when it makes its item
/// (Windows: [`Tray::spawn`]; macOS: the first [`Tray::attach`]). muda keeps
/// the first handler it is given and silently ignores later ones, so an app
/// with its own handler, for its macOS menu bar say, installs it before then
/// (ZapFast #215) and calls this first:
///
/// ```ignore
/// MenuEvent::set_event_handler(Some(|event: MenuEvent| {
///     if fastframe_tray::claim_menu_event(&event.id.0) {
///         return;
///     }
///     // The app's own menu ids.
/// }));
/// ```
///
/// Always `false` on Linux, where the tray does not use muda.
pub fn claim_menu_event(id: &str) -> bool {
    ROUTER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|router| router.menu(id))
}

#[cfg(target_os = "linux")]
use linux::Host;
#[cfg(target_os = "macos")]
use macos::Host;
#[cfg(windows)]
use windows::Host;

type Wake = Arc<dyn Fn() + Send + Sync>;

/// The router muda's process-wide handler reaches, once an item exists.
static ROUTER: Mutex<Option<Router>> = Mutex::new(None);

/// Prefixes the tray's muda ids so they never collide with the app's.
const ID_PREFIX: &str = "fastframe-tray:";

/// The muda id of the tray entry `id`.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn menu_id(id: &str) -> String {
    format!("{ID_PREFIX}{id}")
}

/// Sends events to the app and wakes it.
#[derive(Clone)]
struct Router {
    events: Sender<Event>,
    wake: Wake,
    ids: Vec<&'static str>,
}

impl Router {
    fn new(events: Sender<Event>, wake: Wake, menu: &[MenuItem]) -> Self {
        let ids = menu
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { id, .. } => Some(*id),
                MenuItem::Separator => None,
            })
            .collect();
        Self { events, wake, ids }
    }

    fn send(&self, event: Event) {
        if self.events.send(event).is_ok() {
            (self.wake)();
        }
    }

    /// Sends [`Event::Menu`] for a muda id of this tray.
    fn menu(&self, muda_id: &str) -> bool {
        let Some(id) = muda_id
            .strip_prefix(ID_PREFIX)
            .and_then(|id| self.ids.iter().find(|known| **known == id))
        else {
            return false;
        };
        self.send(Event::Menu(id));
        true
    }

    /// Makes this the router for [`claim_menu_event`].
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    fn install(&self) {
        *ROUTER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(self.clone());
    }
}

/// What a left click on the icon asks for.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn left_click(on_windows: bool) -> Event {
    if on_windows {
        Event::Show
    } else {
        Event::Toggle
    }
}

/// Changes the label of `id` in `menu`; whether it was there.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn set_label(menu: &mut [MenuItem], id: &str, new: String) -> bool {
    for item in menu {
        if let MenuItem::Action {
            id: known, label, ..
        } = item
            && *known == id
        {
            *label = new;
            return true;
        }
    }
    false
}

/// Enables or greys out `id` in `menu`; whether it was there.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn set_enabled(menu: &mut [MenuItem], id: &str, can_choose: bool) -> bool {
    for item in menu {
        if let MenuItem::Action {
            id: known, enabled, ..
        } = item
            && *known == id
        {
            *enabled = can_choose;
            return true;
        }
    }
    false
}

/// Shows or hides `id` in `menu`; whether its visibility changed.
#[cfg(any(target_os = "linux", target_os = "macos", windows, test))]
fn set_visible(menu: &mut [MenuItem], id: &str, shown: bool) -> bool {
    for item in menu {
        if let MenuItem::Action {
            id: known, visible, ..
        } = item
            && *known == id
        {
            let changed = *visible != shown;
            *visible = shown;
            return changed;
        }
    }
    false
}

/// Where the entry `id` sits among the entries `menu` shows: the position a
/// native menu, which holds only what it shows, inserts it at.
#[cfg(any(target_os = "macos", windows, test))]
fn shown_position(menu: &[MenuItem], id: &str) -> Option<usize> {
    let mut position = 0;
    for item in menu {
        match item {
            MenuItem::Action { id: known, .. } if *known == id => return Some(position),
            MenuItem::Action { visible: false, .. } => {}
            MenuItem::Action { .. } | MenuItem::Separator => position += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn router() -> (Router, Receiver<Event>, Arc<AtomicUsize>) {
        let (sender, events) = std::sync::mpsc::channel();
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let menu = [
            MenuItem::action("show", "Show"),
            MenuItem::Separator,
            MenuItem::action("quit", "Quit"),
        ];
        let wake: Wake = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (Router::new(sender, wake, &menu), events, woken)
    }

    #[test]
    fn every_event_is_queued_and_wakes_the_app() {
        let (router, events, woken) = router();
        router.send(Event::Show);
        router.send(Event::Toggle);
        assert_eq!(
            events.try_iter().collect::<Vec<_>>(),
            [Event::Show, Event::Toggle]
        );
        assert_eq!(woken.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_closed_app_is_not_woken() {
        let (router, events, woken) = router();
        drop(events);
        router.send(Event::Show);
        assert_eq!(woken.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn only_the_trays_own_menu_ids_are_claimed() {
        let (router, events, _) = router();
        assert!(router.menu(&menu_id("quit")));
        assert!(!router.menu("quit"), "an app menu id with the same name");
        assert!(!router.menu(&menu_id("unknown")));
        assert!(!router.menu("about"));
        assert_eq!(events.try_iter().collect::<Vec<_>>(), [Event::Menu("quit")]);
    }

    #[test]
    fn claiming_goes_through_the_installed_router() {
        let (router, events, _) = router();
        router.install();
        assert!(claim_menu_event(&menu_id("show")));
        assert!(!claim_menu_event("show"));
        assert_eq!(events.try_recv(), Ok(Event::Menu("show")));
        *ROUTER.lock().unwrap() = None;
        assert!(!claim_menu_event(&menu_id("show")));
    }

    /// Spotifast 5eb054e: each release of a Windows double-click arrives
    /// separately, possibly on both sides of window creation. Both must ask
    /// to show the window, or the second hides it again.
    #[test]
    fn a_windows_click_shows_and_elsewhere_toggles() {
        assert_eq!(left_click(true), Event::Show);
        assert_eq!(left_click(false), Event::Toggle);
    }

    #[test]
    fn labels_change_by_id() {
        let mut menu = vec![
            MenuItem::action("play", "Play"),
            MenuItem::Separator,
            MenuItem::action("quit", "Quit"),
        ];
        assert!(set_label(&mut menu, "play", "Pause".into()));
        assert!(!set_label(&mut menu, "missing", "x".into()));
        assert_eq!(menu[0], MenuItem::action("play", "Pause"));
        assert_eq!(menu[2], MenuItem::action("quit", "Quit"));
    }

    #[test]
    fn entries_start_shown_unless_asked_and_separators_stay() {
        assert_eq!(
            MenuItem::action("lock", "Lock").visible(false),
            MenuItem::Action {
                id: "lock",
                label: "Lock".into(),
                visible: false,
                enabled: true,
            }
        );
        assert_eq!(MenuItem::Separator.visible(false), MenuItem::Separator);
    }

    #[test]
    fn entries_can_be_greyed_out_from_the_start_or_later() {
        let mut menu = vec![
            MenuItem::action("status", "Online").enabled(false),
            MenuItem::action("pause", "Pause"),
        ];
        assert!(matches!(menu[0], MenuItem::Action { enabled: false, .. }));
        assert!(set_enabled(&mut menu, "pause", false));
        assert!(set_enabled(&mut menu, "status", true));
        assert!(!set_enabled(&mut menu, "missing", false));
        assert_eq!(menu[0], MenuItem::action("status", "Online"));
        assert_eq!(menu[1], MenuItem::action("pause", "Pause").enabled(false));
        assert_eq!(MenuItem::Separator.enabled(false), MenuItem::Separator);
    }

    #[test]
    fn visibility_changes_by_id_and_reports_a_change() {
        let mut menu = vec![
            MenuItem::action("show", "Show"),
            MenuItem::action("lock", "Lock").visible(false),
        ];
        assert!(set_visible(&mut menu, "lock", true));
        assert!(!set_visible(&mut menu, "lock", true), "already shown");
        assert!(!set_visible(&mut menu, "missing", false));
        assert_eq!(menu[1], MenuItem::action("lock", "Lock"));
        assert!(set_visible(&mut menu, "lock", false));
        assert_eq!(menu[1], MenuItem::action("lock", "Lock").visible(false));
    }

    /// muda menus hold only what they show, so a shown entry goes back in
    /// after the shown entries and separators before it.
    #[test]
    fn a_shown_entry_goes_back_after_what_is_shown_before_it() {
        let menu = [
            MenuItem::action("show", "Show"),
            MenuItem::action("play", "Play").visible(false),
            MenuItem::action("lock", "Lock"),
            MenuItem::Separator,
            MenuItem::action("quit", "Quit"),
        ];
        assert_eq!(shown_position(&menu, "show"), Some(0));
        assert_eq!(shown_position(&menu, "play"), Some(1));
        assert_eq!(shown_position(&menu, "lock"), Some(1));
        assert_eq!(shown_position(&menu, "quit"), Some(3));
        assert_eq!(shown_position(&menu, "missing"), None);
    }
}
