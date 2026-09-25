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
//!     menu: vec![
//!         MenuItem::action("show", "Show or hide ZapFast"),
//!         MenuItem::Separator,
//!         MenuItem::action("quit", "Quit"),
//!     ],
//! };
//! // `None` when the desktop has no tray: closing the window should quit.
//! let mut tray = Tray::spawn(config, || { /* repaint the window */ });
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
//! - **Linux**: ksni on its own thread. Without a StatusNotifier host,
//!   [`Tray::spawn`] returns `None`. Inside Flatpak the item registers its
//!   unique bus name, since the sandbox does not let it own one.
//! - **Windows**: tray-icon on its own thread with a message loop. A left
//!   click asks to [`Event::Show`] (each release of a double-click arrives
//!   separately, possibly on both sides of window creation, so a toggle
//!   would flicker); the menu opens on right click.
//! - **macOS**: status items live on the main thread and only while AppKit's
//!   event loop runs, so the item is made by the first [`Tray::attach`], and
//!   [`idle`] runs AppKit's loop while no window exists. A Dock click asks to
//!   [`Event::Show`]. The menu opens on right click; left click toggles.
//!
//! The menu handler of tray-icon (muda) is process-wide. An app that builds
//! its own muda menus (a macOS menu bar) and installs its own handler must
//! pass each event to [`claim_menu_event`] first.

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
    },
    /// A line between groups of entries.
    Separator,
}

impl MenuItem {
    /// A clickable entry.
    pub fn action(id: &'static str, label: impl Into<String>) -> Self {
        Self::Action {
            id,
            label: label.into(),
        }
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
    /// Registers the item, or returns `None` when the desktop has none to
    /// offer (no StatusNotifier host, or an OS error), in which case closing
    /// the window should quit.
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
/// (Windows, macOS). An app that later installs its own handler, for its
/// macOS menu bar say, replaces the tray's and must call this first:
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
        if let MenuItem::Action { id: known, label } = item
            && *known == id
        {
            *label = new;
            return true;
        }
    }
    false
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
}
