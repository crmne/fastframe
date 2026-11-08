//! Native notifications without blocking an application's UI or async executor.
//!
//! [`Client`] owns a bounded background worker. Apps keep their preferences,
//! visibility rules and routing; Fastframe owns identity, delivery and clicks.
//! See the crate README for daemon and macOS bundle requirements.

#![doc = include_str!("../README.md")]

use std::{path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, oneshot, watch};

mod logo;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "windows", test))]
mod mapping;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[path = "linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;
#[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "macos", target_os = "windows")))]
#[path = "unsupported.rs"]
mod platform;
mod worker;

/// An actionable delivery error. Notification text is never included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Invalid app identity, key, action or sound name.
    InvalidInput,
    /// This platform or execution environment cannot deliver notifications.
    Unsupported,
    /// macOS requires a signed application bundle.
    BundleRequired,
    /// Permission has not been granted, or was revoked.
    PermissionDenied,
    /// The bounded worker is full. The app may retry later.
    Busy,
    /// The worker has stopped.
    Stopped,
    /// The native service rejected or could not deliver the request.
    Delivery,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid notification configuration",
            Self::Unsupported => "native notifications are unavailable",
            Self::BundleRequired => "notifications require a signed application bundle",
            Self::PermissionDenied => "notification permission is not granted",
            Self::Busy => "notification worker is full",
            Self::Stopped => "notification worker stopped",
            Self::Delivery => "native notification delivery failed",
        })
    }
}
impl std::error::Error for Error {}

/// Current OS permission. Checking it never prompts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Delivery is allowed. Focus, Do Not Disturb and OS presentation still apply.
    Granted,
    /// The user has not answered a permission prompt yet.
    NotDetermined,
    /// The user or administrator disabled notifications.
    Denied,
    /// The platform does not expose an application permission query.
    Unknown,
}

/// The identity the desktop shows and groups notifications under.
#[derive(Clone, Debug)]
pub struct App {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) desktop_entry: String,
    pub(crate) icon: String,
    pub(crate) logo: Option<Logo>,
    /// The logo file written by [`Client::new`], for the platform to read.
    pub(crate) icon_file: Option<PathBuf>,
}
/// The app's logo, a PNG, and the colour of the tile Windows shows it on.
#[derive(Clone, Debug)]
pub(crate) struct Logo {
    pub(crate) png: Arc<[u8]>,
    #[cfg_attr(
        not(target_os = "windows"),
        expect(dead_code, reason = "only Windows composes the tile")
    )]
    pub(crate) background: [u8; 3],
}
impl App {
    /// `id` is the bundle ID / Windows shortcut AUMID; `name` is the display name.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            desktop_entry: id.clone(),
            icon: id.clone(),
            logo: None,
            icon_file: None,
            id,
            name: name.into(),
        }
    }
    /// Linux desktop file name, without `.desktop`. Flatpak identity is resolved automatically.
    pub fn desktop_entry(mut self, value: impl Into<String>) -> Self {
        self.desktop_entry = value.into();
        self
    }
    /// Linux theme icon name or absolute icon path.
    pub fn icon(mut self, value: impl Into<String>) -> Self {
        self.icon = value.into();
        self
    }
    /// The app's logo, a PNG of up to 1024 pixels a side, and `background`,
    /// an sRGB colour from the app's brand.
    ///
    /// Windows shows the logo on every toast, centred on an opaque square of
    /// `background`. Windows 11 inverts the colours of an icon it judges
    /// monochrome, a one-hue mark or a dark, mostly grey tile, in its dark
    /// theme; a band of a saturated colour around the logo keeps it as drawn,
    /// so pick a saturated colour, not a grey. Linux shows the PNG itself when
    /// no `hicolor` theme or `pixmaps` folder has an icon named [`App::icon`],
    /// as in a portable archive or a development build; installed packages
    /// keep following the theme. macOS always shows the application bundle's
    /// icon. [`Client::new`] writes the file the desktop reads to the per-user
    /// cache, in a folder named after the app's ID, and returns
    /// [`Error::InvalidInput`] when `png` is not such a PNG.
    pub fn logo(mut self, png: &[u8], background: [u8; 3]) -> Self {
        self.logo = Some(Logo {
            png: png.into(),
            background,
        });
        self
    }
    fn validate(&self) -> Result<(), Error> {
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            || self.name.is_empty()
            || self.name.len() > 1024
            || self.name.contains('\0')
            || self
                .logo
                .as_ref()
                .is_some_and(|logo| logo::decode(&logo.png).is_err())
        {
            Err(Error::InvalidInput)
        } else {
            Ok(())
        }
    }
}

/// Sound played by the OS, respecting its notification controls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sound {
    /// The system notification sound.
    #[default]
    Default,
    /// No sound.
    Silent,
    /// A Linux sound-theme name or a file name in the macOS bundle's Resources.
    /// Windows uses its default sound for named sounds.
    Named(String),
}

/// Plain text content. Markup in the body is escaped for desktops that support it.
#[derive(Clone, Debug)]
pub struct Notification {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) key: Option<String>,
    pub(crate) sound: Sound,
    pub(crate) image: Option<PathBuf>,
    pub(crate) category: Option<String>,
    pub(crate) progress: Option<u8>,
    pub(crate) persistent: bool,
    pub(crate) actions: Vec<(String, String)>,
}
impl Notification {
    /// A notification with the system sound and the desktop's default lifetime.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: String::new(),
            key: None,
            sound: Sound::Default,
            image: None,
            category: None,
            progress: None,
            persistent: false,
            actions: Vec::new(),
        }
    }
    /// Plain text message.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }
    /// Stable opaque application key for replacement and withdrawal. Do not put private content here.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
    /// Sound requested from the OS.
    pub fn sound(mut self, sound: Sound) -> Self {
        self.sound = sound;
        self
    }
    /// Local avatar or image. macOS uses the application icon.
    pub fn image(mut self, path: impl Into<PathBuf>) -> Self {
        self.image = Some(path.into());
        self
    }
    /// Freedesktop category, for example `im.received` or `transfer`.
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }
    /// Percentage for a Linux or FreeBSD progress notification, clamped to 100.
    pub fn progress(mut self, percent: u8) -> Self {
        self.progress = Some(percent.min(100));
        self
    }
    /// On Linux and FreeBSD, request a never-expiring notification that a
    /// compliant server may keep visible until dismissed. Other platforms use
    /// their normal lifetime.
    pub fn persistent(mut self, persistent: bool) -> Self {
        self.persistent = persistent;
        self
    }
    /// Add a button. `default` labels the ordinary click on Linux and FreeBSD;
    /// it is not a separate button elsewhere.
    pub fn action(mut self, id: impl Into<String>, label: impl Into<String>) -> Self {
        self.actions.push((id.into(), label.into()));
        self
    }
    fn validate(&self) -> Result<(), Error> {
        let invalid = self.title.len() > 4096
            || self.body.len() > 65536
            || self.title.is_empty()
            || self.title.contains('\0')
            || self.body.contains('\0')
            || self.key.as_ref().is_some_and(|key| !valid_key(key))
            || self.actions.len() > 4
            || self.actions.iter().enumerate().any(|(i, (id, label))| {
                !valid_key(id)
                    || label.len() > 512
                    || label.is_empty()
                    || label.contains('\0')
                    || self.actions[..i].iter().any(|(old, _)| old == id)
            })
            || matches!(&self.sound, Sound::Named(name) if name.len() > 256 || name.is_empty() || name.contains(['/', '\\', '\0']) || name == "." || name == "..");
        if invalid {
            Err(Error::InvalidInput)
        } else {
            Ok(())
        }
    }
}
fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 256 && !key.chars().any(char::is_control)
}

/// Events for one notification, delivered on the background worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Accepted by the OS, which may suppress presentation because of Focus or preferences.
    Shown,
    /// The user clicked the notification.
    Activated,
    /// The user clicked a named action.
    Action(String),
    /// The user dismissed it, or its native lifetime ended.
    Dismissed,
    /// The app withdrew it.
    Closed,
    /// Delivery or replacement failed.
    Failed(Error),
}

/// Platform differences an app can use without platform conditionals.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    /// Updating a live progress banner without posting a new ordinary banner.
    pub progress: bool,
    /// Withdrawal by stable key still works after a process restart.
    pub persistent_keys: bool,
}

/// A notification service, safe to call from a GUI, TUI, daemon, or Tokio task.
#[derive(Clone)]
pub struct Client {
    sender: mpsc::Sender<worker::Command>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl Client {
    /// Start one worker. No native calls or permission prompts run on the calling thread.
    /// The app's logo file is written on the worker too.
    pub fn new(mut app: App) -> Result<Self, Error> {
        app.validate()?;
        let (sender, receiver) = mpsc::channel(64);
        std::thread::Builder::new()
            .name("fastframe-notifications".into())
            .spawn(move || {
                app.icon_file = logo::file(&app);
                worker::run(app, receiver);
            })
            .map_err(|_| Error::Stopped)?;
        Ok(Self {
            sender,
            slots: Arc::new(tokio::sync::Semaphore::new(64)),
        })
    }
    /// Supported presentation features. Linux desktops may ignore optional hints.
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            progress: cfg!(any(target_os = "linux", target_os = "freebsd")),
            persistent_keys: cfg!(any(target_os = "macos", target_os = "windows")),
        }
    }
    /// Queue delivery. Keep the handle to update or close it. Dropping it leaves the OS notification visible.
    pub fn show(&self, notification: Notification) -> Result<Handle, Error> {
        self.show_with(notification, |_| {})
    }
    /// Queue delivery and receive events. Keep callbacks short and send into the app's own queue.
    /// No callbacks run on the UI thread, and a callback panic is contained.
    pub fn show_with(
        &self,
        notification: Notification,
        events: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Handle, Error> {
        notification.validate()?;
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let (sender, receiver) = watch::channel(worker::State {
            notification,
            revision: 0,
            stop: None,
        });
        let control = Arc::new(worker::Control(sender));
        self.sender
            .try_send(worker::Command::Show {
                control: control.0.clone(),
                state: receiver,
                events: worker::Events(Arc::new(events)),
                permit,
            })
            .map_err(worker::send_error)?;
        Ok(Handle {
            control,
            _service: self.sender.clone(),
        })
    }
    /// Check permission without prompting. An unavailable service returns an error.
    pub async fn permission(&self) -> Result<Permission, Error> {
        self.permission_impl(false).await
    }
    /// Request OS permission, normally in response to a user action.
    pub async fn request_permission(&self) -> Result<Permission, Error> {
        self.permission_impl(true).await
    }
    /// Request permission from a synchronous background thread. Never call on
    /// a UI thread or inside an async runtime; use the async method there.
    pub fn request_permission_blocking(&self) -> Result<Permission, Error> {
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(worker::Command::Permission {
                request: true,
                reply,
                permit,
            })
            .map_err(worker::send_error)?;
        receiver.blocking_recv().map_err(|_| Error::Stopped)?
    }
    async fn permission_impl(&self, request: bool) -> Result<Permission, Error> {
        let (reply, receiver) = oneshot::channel();
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        self.sender
            .try_send(worker::Command::Permission {
                request,
                reply,
                permit,
            })
            .map_err(worker::send_error)?;
        receiver.await.map_err(|_| Error::Stopped)?
    }
    /// Withdraw by stable key. On Linux only keys still tracked by this client can be withdrawn.
    pub fn withdraw(&self, key: impl Into<String>) -> Result<(), Error> {
        let key = key.into();
        if !valid_key(&key) {
            return Err(Error::InvalidInput);
        }
        self.sender
            .try_send(worker::Command::Withdraw(key))
            .map_err(worker::send_error)
    }
}

/// Control of one notification. Clones refer to the same delivery.
#[derive(Clone)]
pub struct Handle {
    control: Arc<worker::Control>,
    _service: mpsc::Sender<worker::Command>,
}
impl Handle {
    /// Replace content, coalescing pending changes. A closed handle cannot be reopened.
    /// The original stable key is retained.
    pub fn update(&self, mut notification: Notification) -> Result<(), Error> {
        notification.validate()?;
        if self.control.0.is_closed() {
            return Err(Error::Stopped);
        }
        let mut result = Err(Error::Stopped);
        self.control.0.send_modify(|state| {
            if state.stop.is_none() {
                notification.key.clone_from(&state.notification.key);
                state.notification = notification;
                state.revision = state.revision.wrapping_add(1);
                result = Ok(());
            }
        });
        result
    }
    /// Withdraw this notification. Closing before delivery suppresses the pending notification.
    pub fn close(&self) {
        self.control.stop(worker::Stop::Close);
    }
}

/// A bundled sound exists in this app's macOS Resources directory.
/// Other platforms return false. Useful for apps with optional built-in sounds.
pub fn bundled_sound_exists(name: &str) -> bool {
    if !cfg!(target_os = "macos") || name.is_empty() || name.contains(['/', '\\', '\0']) {
        return false;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("Resources").join(name)))
        .is_some_and(|path| path.is_file())
}

/// Run a headless broker's main future while pumping the macOS main run loop.
/// Call on the main thread of a signed `.app`; GUI apps already pump their loop.
/// On other platforms this simply blocks on the future. The future should wait
/// for work on its own runtime, rather than requiring a Tokio runtime here.
pub fn run_main<F: std::future::Future>(future: F) -> F::Output {
    #[cfg(target_os = "macos")]
    {
        mac_usernotifications::block_on_main(future)
    }
    #[cfg(not(target_os = "macos"))]
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("notification broker runtime")
            .block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_identities_and_keys_never_reach_native_apis() {
        for id in ["", "../other", "a\\b", "a\0b"] {
            assert_eq!(App::new(id, "Test").validate(), Err(Error::InvalidInput));
        }
        let app = App::new("com.example.app", "Test");
        assert_eq!(
            app.clone().logo(b"not a png", [0, 0, 255]).validate(),
            Err(Error::InvalidInput)
        );
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[1, 2, 3, 4]).unwrap();
        writer.finish().unwrap();
        assert_eq!(app.logo(&png, [0, 0, 255]).validate(), Ok(()));
        for key in ["", "a\nb", "a\0b"] {
            assert_eq!(
                Notification::new("Title").key(key).validate(),
                Err(Error::InvalidInput)
            );
        }
        assert_eq!(
            Notification::new("Title")
                .sound(Sound::Named("../outside.wav".into()))
                .validate(),
            Err(Error::InvalidInput)
        );
        assert_eq!(
            Notification::new("Title")
                .action("x", "One")
                .action("x", "Two")
                .validate(),
            Err(Error::InvalidInput)
        );
    }
    #[test]
    fn admission_is_bounded_without_starting_a_worker() {
        let (sender, _receiver) = mpsc::channel(64);
        let client = Client {
            sender,
            slots: Arc::new(tokio::sync::Semaphore::new(64)),
        };
        let handles: Vec<_> = (0..64)
            .map(|_| client.show(Notification::new("Title")).expect("slot"))
            .collect();
        assert!(matches!(
            client.show(Notification::new("Overflow")),
            Err(Error::Busy)
        ));
        handles[0].close();
        assert_eq!(
            handles[0].update(Notification::new("Closed")),
            Err(Error::Stopped)
        );
    }
    #[test]
    fn updates_preserve_routing_key_and_clamp_progress() {
        let (sender, mut receiver) = mpsc::channel(64);
        let client = Client {
            sender,
            slots: Arc::new(tokio::sync::Semaphore::new(64)),
        };
        let handle = client
            .show(Notification::new("Start").key("original"))
            .expect("queued");
        handle
            .update(Notification::new("Next").key("different").progress(200))
            .expect("updated");
        let worker::Command::Show { state, .. } = receiver.try_recv().expect("command") else {
            panic!("show")
        };
        assert_eq!(state.borrow().notification.key.as_deref(), Some("original"));
        assert_eq!(state.borrow().notification.progress, Some(100));
    }
}
