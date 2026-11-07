//! Open an egui window on the graphics a computer has, and say why when it
//! has none.
//!
//! eframe's glow renderer needs OpenGL 2 or later. A Windows computer
//! without its graphics driver (the Microsoft Basic Display Adapter, many
//! virtual machines, some Remote Desktop sessions) offers only OpenGL 1.1, so
//! `run_native` returns `egui_glow requires opengl 2.0+` before the window
//! draws anything. An app started from Explorer has no console, so it seemed
//! to quit without a word (Solco's Windows first-run test, 2026-10-27).
//!
//! [`Graphics::run_native`] opens the window with glow. When glow cannot draw
//! it and the `wgpu` feature is on, it opens it again with wgpu, which on
//! Windows draws through Direct3D 12 and, without a vendor driver, through
//! WARP, Windows' own software renderer. On macOS with the `wgpu` feature the
//! order turns around: wgpu draws through Metal first, and glow is the
//! fallback. When no renderer can draw, it shows
//! a native dialog that names the cause and what to do, with a link to the
//! app's help page, and returns the error.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // NSAlert and NSWorkspace, with safety notes at each call.
mod macos;
#[cfg(windows)]
#[allow(unsafe_code)] // MessageBoxW and ShellExecuteW, with safety notes.
mod windows;

use std::cell::Cell;
use std::fmt::Display;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub use eframe::Renderer;

/// The renderers [`Graphics::run_native`] tries, in order: glow, which every
/// app has drawn with so far, then wgpu when the `wgpu` feature is on.
///
/// On macOS with the `wgpu` feature, wgpu comes first. Apple deprecated
/// OpenGL, and on a 120 Hz ProMotion display a glow window misses refreshes
/// while it scrolls: Spotifast measured a p95 frame interval of 13 to 18 ms
/// with glow and 9 to 10 ms with wgpu on Metal, on the same MacBook, with
/// 1 to 4 ms of CPU per frame in both. Glow stays as the fallback.
pub const RENDERERS: &[Renderer] = &[
    #[cfg(all(feature = "wgpu", target_os = "macos"))]
    Renderer::Wgpu,
    Renderer::Glow,
    #[cfg(all(feature = "wgpu", not(target_os = "macos")))]
    Renderer::Wgpu,
];

/// The renderer that last opened a window in this process.
static WORKED: Mutex<Option<Renderer>> = Mutex::new(None);

/// An app's own words for the dialog, such as its translations.
type Words = Arc<dyn Fn(&Trouble<'_>) -> Dialog + Send + Sync>;

/// The app's name, help page and dialog words, for opening its windows.
#[derive(Clone)]
pub struct Graphics {
    app_name: String,
    help_url: Option<String>,
    words: Option<Words>,
}

impl std::fmt::Debug for Graphics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graphics")
            .field("app_name", &self.app_name)
            .field("help_url", &self.help_url)
            .field("words", &self.words.as_ref().map(|_| "app's own"))
            .finish()
    }
}

impl Graphics {
    /// For the app called `app_name`, which is also the name eframe gets.
    pub fn new(app_name: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            help_url: None,
            words: None,
        }
    }

    /// The page the dialog offers to open, explaining what to install.
    #[must_use]
    pub fn help(mut self, url: impl Into<String>) -> Self {
        self.help_url = Some(url.into());
        self
    }

    /// The app's own words for the dialog, in place of
    /// [`Dialog::english`]: a translation, or a different remedy or link.
    /// `words` gets the [`Trouble`] (the app, the [`Cause`], the error's
    /// details and the help page) and returns the [`Dialog`] to show. It
    /// can start from `Dialog::english(trouble)` and change only what it
    /// needs.
    #[must_use]
    pub fn dialog(
        mut self,
        words: impl Fn(&Trouble<'_>) -> Dialog + Send + Sync + 'static,
    ) -> Self {
        self.words = Some(Arc::new(words));
        self
    }

    /// Opens a window with `eframe::run_native`, as many times as it takes
    /// to find a renderer that draws it, and returns when it closes.
    ///
    /// `options` and `app_creator` are called once for each try, since a
    /// failed try consumes both. The renderer in `options` is replaced: the
    /// first window tries [`RENDERERS`] in order, and later windows use the
    /// one that worked. Another renderer is tried only when the window could
    /// not start (the app creator never ran) and the error is a graphics one
    /// ([`is_graphics_failure`]).
    ///
    /// # Errors
    ///
    /// The last try's error. When it is a graphics failure, the dialog has
    /// been shown and closed by then.
    pub fn run_native<'app>(
        &self,
        mut options: impl FnMut() -> eframe::NativeOptions,
        mut app_creator: impl FnMut() -> eframe::AppCreator<'app>,
    ) -> eframe::Result {
        let remembered = *WORKED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = try_renderers(
            remembered,
            RENDERERS,
            |renderer| {
                let started = Rc::new(Cell::new(false));
                let creator = app_creator();
                let flag = Rc::clone(&started);
                let creator: eframe::AppCreator<'app> = Box::new(move |context| {
                    // The renderer drew the window: the app can read it now,
                    // for its error reports.
                    flag.set(true);
                    *WORKED
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(renderer);
                    creator(context)
                });
                let mut options = options();
                options.renderer = renderer;
                // wgpu treats a surface it cannot configure as fatal and
                // panics: Direct3D 12 on WARP did in a Windows session with
                // no desktop (over SSH). Before the window started, that is
                // one more renderer that cannot draw; after, the app's own
                // panic goes on unchanged.
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    eframe::run_native(&self.app_name, options, creator)
                }));
                let result = match outcome {
                    Ok(result) => result.map_err(Tried::Error),
                    Err(payload) if !started.get() => Err(Tried::Panicked(panic_text(&*payload))),
                    Err(payload) => std::panic::resume_unwind(payload),
                };
                (result, started.get())
            },
            Tried::is_graphics_failure,
        );
        match result {
            Ok(_) => Ok(()),
            Err(Failure { error, graphics }) => {
                if graphics {
                    self.explain(&error);
                }
                Err(error.into())
            }
        }
    }

    /// Shows the dialog for a window that could not open, naming `error`
    /// under the explanation, and returns when it closes. If the person asks
    /// for the help page, it opens in their browser.
    ///
    /// [`Graphics::run_native`] calls this itself. It is public for an app
    /// that opens its window another way.
    pub fn explain(&self, error: &dyn Display) {
        let details = error.to_string();
        let trouble = Trouble {
            app_name: &self.app_name,
            cause: Cause::here(),
            details: details.trim(),
            help_url: self.help_url.as_deref(),
        };
        let dialog = match &self.words {
            Some(words) => words(&trouble),
            None => Dialog::english(&trouble),
        };
        log::error!("{} ({:?}): {}", dialog.title, trouble.cause, error);
        if show(&dialog)
            && let Some(link) = &dialog.link
        {
            open_url(link);
        }
    }
}

/// The Microsoft Store page of the OpenCL, OpenGL & Vulkan Compatibility
/// Pack, which gives Windows on ARM its OpenGL.
pub const COMPATIBILITY_PACK_PAGE: &str = "ms-windows-store://pdp/?ProductId=9NQPSL29BFFF";

/// Why no renderer could draw the window, which decides the remedy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cause {
    /// No graphics driver offers OpenGL 2: the Microsoft Basic Display
    /// Adapter, a virtual machine or remote session without 3D
    /// acceleration, or a Linux system without Mesa. The remedy is the
    /// graphics driver.
    NoGraphicsDriver,
    /// Windows on ARM, whatever the app was built for (an x64 build runs
    /// there emulated). Snapdragon graphics drivers have no OpenGL of their
    /// own; Windows provides it through Microsoft's OpenCL, OpenGL & Vulkan
    /// Compatibility Pack, a Store app that translates it to Direct3D 12.
    /// The remedy is installing or repairing that pack.
    WindowsOnArm,
}

impl Cause {
    /// The cause on this computer.
    pub fn here() -> Self {
        Self::for_machine(native_arm64())
    }

    /// The cause on a computer whose processor is, or is not, ARM64 Windows.
    pub fn for_machine(windows_on_arm: bool) -> Self {
        if windows_on_arm {
            Self::WindowsOnArm
        } else {
            Self::NoGraphicsDriver
        }
    }
}

/// What went wrong, for an app's own dialog words ([`Graphics::dialog`]).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Trouble<'a> {
    /// The app's name, as given to [`Graphics::new`].
    pub app_name: &'a str,
    /// Why no renderer could draw.
    pub cause: Cause,
    /// The last renderer's error, for the person to quote to support.
    pub details: &'a str,
    /// The app's help page, as given to [`Graphics::help`].
    pub help_url: Option<&'a str>,
}

impl<'a> Trouble<'a> {
    /// A trouble as [`Graphics::run_native`] would hand it to the app's
    /// words, so an app can test its [`Graphics::dialog`] closure. The struct
    /// stays `#[non_exhaustive]`, so later fields arrive without breaking
    /// this.
    ///
    /// ```
    /// use fastframe_graphics::{COMPATIBILITY_PACK_PAGE, Cause, Dialog, Trouble};
    ///
    /// // The app's words, the same function it passes to `Graphics::dialog`.
    /// fn words(trouble: &Trouble<'_>) -> Dialog {
    ///     Dialog {
    ///         title: format!("{} kann sein Fenster nicht öffnen", trouble.app_name),
    ///         ..Dialog::english(trouble)
    ///     }
    /// }
    ///
    /// let arm = Trouble::new("Example", Cause::WindowsOnArm, "egui_glow requires opengl 2.0+.", None);
    /// let dialog = words(&arm);
    /// assert_eq!(dialog.title, "Example kann sein Fenster nicht öffnen");
    /// assert_eq!(dialog.link.as_deref(), Some(COMPATIBILITY_PACK_PAGE));
    ///
    /// let driver = Trouble::new("Example", Cause::NoGraphicsDriver, "", Some("https://example.com/help"));
    /// assert_eq!(words(&driver).link.as_deref(), Some("https://example.com/help"));
    /// ```
    pub fn new(
        app_name: &'a str,
        cause: Cause,
        details: &'a str,
        help_url: Option<&'a str>,
    ) -> Self {
        Self {
            app_name,
            cause,
            details,
            help_url,
        }
    }
}

/// The dialog's words, and the page it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    /// The window's title.
    pub title: String,
    /// The explanation. With a `link`, it ends with the question the
    /// buttons answer: Windows' message box has only Yes and No, in the
    /// system's language.
    pub body: String,
    /// The page the first button opens. `None` shows only a close button.
    pub link: Option<String>,
    /// The button that opens the link, where the platform names its
    /// buttons (macOS and Linux).
    pub open_label: String,
    /// The button that closes the dialog, where the platform names it.
    pub close_label: String,
}

impl Dialog {
    /// fastframe's English words. Windows on ARM offers the compatibility
    /// pack's Store page; anywhere else the app's help page, when it has
    /// one.
    pub fn english(trouble: &Trouble<'_>) -> Self {
        let app = trouble.app_name;
        let details = trouble.details;
        let title = format!("{app} cannot open its window");
        if trouble.cause == Cause::WindowsOnArm {
            return Self {
                title,
                body: format!(
                    "On Windows on ARM, the OpenGL that {app} draws with comes from Microsoft's OpenCL, OpenGL & Vulkan Compatibility Pack. Install or repair it from the Microsoft Store, then open {app} again.\n\nDetails: {details}\n\nOpen its Microsoft Store page?"
                ),
                link: Some(COMPATIBILITY_PACK_PAGE.to_owned()),
                open_label: "Open Microsoft Store".to_owned(),
                close_label: "Close".to_owned(),
            };
        }
        let remedy = if cfg!(windows) {
            format!(
                "Install your graphics card's driver, from its maker or from Windows Update, then open {app} again. Remote Desktop and virtual machines need 3D acceleration turned on."
            )
        } else if cfg!(target_os = "macos") {
            format!("Update macOS, then open {app} again.")
        } else {
            format!(
                "Install your graphics card's driver or Mesa, then open {app} again. Remote sessions and virtual machines need 3D acceleration turned on."
            )
        };
        let mut body = format!(
            "{app} needs your graphics driver to draw its window (OpenGL 2 or later), and this computer only has basic graphics.\n\n{remedy}\n\nDetails: {details}"
        );
        if trouble.help_url.is_some() {
            body.push_str("\n\nOpen the help page?");
        }
        Self {
            title,
            body,
            link: trouble.help_url.map(str::to_owned),
            open_label: "Open Help".to_owned(),
            close_label: "Close".to_owned(),
        }
    }
}

/// Whether Windows runs on an ARM64 processor, asked at run time, so an
/// x64 build running emulated there counts too.
fn native_arm64() -> bool {
    #[cfg(windows)]
    return windows::native_arm64();
    #[cfg(not(windows))]
    false
}

/// The renderer drawing the window, or that drew the last one, for error
/// reports. Set before the app creator runs; `None` until a window opened
/// through [`Graphics::run_native`].
pub fn renderer() -> Option<Renderer> {
    *WORKED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether `error` says the computer's graphics cannot draw the window: no
/// usable OpenGL (glutin found no configuration or context, or the version
/// is below 2), or wgpu found no adapter or device. Window-system and app
/// errors are not.
pub fn is_graphics_failure(error: &eframe::Error) -> bool {
    match error {
        eframe::Error::Glutin(_)
        | eframe::Error::NoGlutinConfigs(..)
        | eframe::Error::OpenGL(_) => true,
        #[cfg(feature = "wgpu")]
        eframe::Error::Wgpu(_) => true,
        _ => false,
    }
}

/// How one try failed: eframe's error, or a panic before the window started.
#[derive(Debug)]
enum Tried {
    Error(eframe::Error),
    Panicked(String),
}

impl Tried {
    fn is_graphics_failure(&self) -> bool {
        match self {
            Self::Error(error) => is_graphics_failure(error),
            Self::Panicked(_) => true,
        }
    }
}

impl Display for Tried {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error(error) => error.fmt(f),
            Self::Panicked(text) => {
                write!(f, "the renderer failed while opening the window: {text}")
            }
        }
    }
}

/// A panic before the window started is returned as the app creation
/// error eframe would give, since eframe has no variant of its own for it.
impl From<Tried> for eframe::Error {
    fn from(tried: Tried) -> Self {
        match tried {
            Tried::Error(error) => error,
            Tried::Panicked(_) => Self::AppCreation(tried.to_string().into()),
        }
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned())
}

/// A try that failed, and whether it was a graphics failure.
#[derive(Debug)]
struct Failure<E> {
    error: E,
    graphics: bool,
}

/// Tries `order` (or only the `remembered` renderer that worked before),
/// moving to the next renderer only while the window had not started and
/// the error was a graphics one. Returns the renderer that worked.
fn try_renderers<R: Copy + std::fmt::Debug, E: Display>(
    remembered: Option<R>,
    order: &[R],
    mut open: impl FnMut(R) -> (Result<(), E>, bool),
    is_graphics: impl Fn(&E) -> bool,
) -> Result<R, Failure<E>> {
    let order = match remembered {
        Some(renderer) => std::slice::from_ref(&renderer).to_vec(),
        None => order.to_vec(),
    };
    let mut renderers = order.iter().peekable();
    while let Some(&renderer) = renderers.next() {
        let (result, started) = open(renderer);
        let Err(error) = result else {
            return Ok(renderer);
        };
        let graphics = !started && is_graphics(&error);
        match renderers.peek() {
            Some(next) if graphics => {
                log::warn!(
                    "the {renderer:?} renderer cannot draw the window ({error}); trying {next:?}"
                );
            }
            _ => return Err(Failure { error, graphics }),
        }
    }
    unreachable!("there is always at least one renderer to try")
}

/// Shows the dialog and returns whether the person asked for the help page.
fn show(message: &Dialog) -> bool {
    #[cfg(windows)]
    return windows::show(message);
    #[cfg(target_os = "macos")]
    return macos::show(message);
    #[cfg(target_os = "linux")]
    return linux::show(message);
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = message;
        false
    }
}

fn open_url(url: &str) {
    #[cfg(windows)]
    windows::open_url(url);
    #[cfg(target_os = "macos")]
    macos::open_url(url);
    #[cfg(target_os = "linux")]
    linux::open_url(url);
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let _ = url;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Fake {
        Glow,
        Wgpu,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Error {
        OldOpenGl,
        Window,
    }

    impl Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }

    fn graphics(error: &Error) -> bool {
        *error == Error::OldOpenGl
    }

    #[test]
    fn falls_back_to_the_next_renderer_when_opengl_is_too_old() {
        let mut tried = Vec::new();
        let worked = try_renderers(
            None,
            &[Fake::Glow, Fake::Wgpu],
            |renderer| {
                tried.push(renderer);
                match renderer {
                    Fake::Glow => (Err(Error::OldOpenGl), false),
                    Fake::Wgpu => (Ok(()), true),
                }
            },
            graphics,
        );
        assert_eq!(worked.ok(), Some(Fake::Wgpu));
        assert_eq!(tried, [Fake::Glow, Fake::Wgpu]);
    }

    #[test]
    fn the_first_renderer_that_works_is_the_only_one_tried() {
        let mut tried = Vec::new();
        let worked = try_renderers(
            None,
            &[Fake::Glow, Fake::Wgpu],
            |renderer| {
                tried.push(renderer);
                (Ok::<(), Error>(()), true)
            },
            graphics,
        );
        assert_eq!(worked.ok(), Some(Fake::Glow));
        assert_eq!(tried, [Fake::Glow]);
    }

    #[test]
    fn a_failure_after_the_window_started_is_never_retried_or_explained() {
        let mut tried = Vec::new();
        let failure = try_renderers(
            None,
            &[Fake::Glow, Fake::Wgpu],
            |renderer| {
                tried.push(renderer);
                (Err(Error::OldOpenGl), true)
            },
            graphics,
        )
        .expect_err("the window failed");
        assert_eq!(tried, [Fake::Glow]);
        assert!(!failure.graphics, "no dialog for a window that opened");
    }

    #[test]
    fn other_errors_are_returned_without_a_retry_or_a_dialog() {
        let mut tried = Vec::new();
        let failure = try_renderers(
            None,
            &[Fake::Glow, Fake::Wgpu],
            |renderer| {
                tried.push(renderer);
                (Err(Error::Window), false)
            },
            graphics,
        )
        .expect_err("the window failed");
        assert_eq!(tried, [Fake::Glow]);
        assert_eq!(failure.error, Error::Window);
        assert!(!failure.graphics);
    }

    #[test]
    fn the_dialog_explains_when_every_renderer_fails() {
        let mut tried = Vec::new();
        let failure = try_renderers(
            None,
            &[Fake::Glow, Fake::Wgpu],
            |renderer| {
                tried.push(renderer);
                (Err(Error::OldOpenGl), false)
            },
            graphics,
        )
        .expect_err("no renderer works");
        assert_eq!(tried, [Fake::Glow, Fake::Wgpu]);
        assert!(failure.graphics);
    }

    #[test]
    fn later_windows_use_the_renderer_that_worked() {
        let mut tried = Vec::new();
        let worked = try_renderers(
            Some(Fake::Wgpu),
            &[Fake::Glow, Fake::Wgpu],
            |renderer| {
                tried.push(renderer);
                (Ok::<(), Error>(()), true)
            },
            graphics,
        );
        assert_eq!(worked.ok(), Some(Fake::Wgpu));
        assert_eq!(tried, [Fake::Wgpu]);
    }

    #[test]
    fn glow_is_tried_first_except_on_macos_with_wgpu() {
        #[cfg(all(feature = "wgpu", target_os = "macos"))]
        let first = Renderer::Wgpu;
        #[cfg(not(all(feature = "wgpu", target_os = "macos")))]
        let first = Renderer::Glow;
        assert_eq!(RENDERERS.first(), Some(&first));
        assert_eq!(RENDERERS.len(), if cfg!(feature = "wgpu") { 2 } else { 1 });
        assert!(
            RENDERERS.contains(&Renderer::Glow),
            "glow is always a choice"
        );
    }

    #[cfg(all(feature = "wgpu", target_os = "macos"))]
    #[test]
    fn a_metal_startup_failure_falls_back_to_glow() {
        let mut tried = Vec::new();
        let worked = try_renderers(
            None,
            RENDERERS,
            |renderer| {
                tried.push(renderer);
                match renderer {
                    Renderer::Wgpu => (Err("no Metal adapter"), false),
                    Renderer::Glow => (Ok(()), true),
                }
            },
            |_| true,
        )
        .expect("OpenGL can still open the window");
        assert_eq!(tried, [Renderer::Wgpu, Renderer::Glow]);
        assert_eq!(worked, Renderer::Glow);
    }

    fn trouble(cause: Cause, help_url: Option<&'static str>) -> Trouble<'static> {
        Trouble {
            app_name: "Solco",
            cause,
            details: "egui_glow requires opengl 2.0+.",
            help_url,
        }
    }

    #[test]
    fn the_message_names_the_app_the_cause_and_the_remedy() {
        let dialog = Dialog::english(&trouble(Cause::NoGraphicsDriver, Some("https://help")));
        assert_eq!(dialog.title, "Solco cannot open its window");
        assert!(dialog.body.starts_with("Solco needs your graphics driver"));
        assert!(dialog.body.contains("OpenGL 2 or later"));
        assert!(dialog.body.contains("open Solco again"));
        assert!(
            dialog
                .body
                .contains("Details: egui_glow requires opengl 2.0+.")
        );
        assert!(dialog.body.ends_with("Open the help page?"));
        assert_eq!(dialog.link.as_deref(), Some("https://help"));

        let without_help = Dialog::english(&trouble(Cause::NoGraphicsDriver, None));
        assert!(!without_help.body.contains("help page"));
        assert_eq!(without_help.link, None);
    }

    /// Snapdragon drivers have no OpenGL: Windows on ARM gets it from
    /// Microsoft's compatibility pack, so that is the remedy, not a driver
    /// (Spotifast's dialog, upstreamed).
    #[test]
    fn windows_on_arm_points_to_the_compatibility_pack() {
        assert_eq!(Cause::for_machine(true), Cause::WindowsOnArm);
        assert_eq!(Cause::for_machine(false), Cause::NoGraphicsDriver);
        let dialog = Dialog::english(&trouble(Cause::WindowsOnArm, Some("https://help")));
        assert!(
            dialog
                .body
                .contains("OpenCL, OpenGL & Vulkan Compatibility Pack")
        );
        assert!(
            dialog
                .body
                .contains("Details: egui_glow requires opengl 2.0+.")
        );
        assert!(dialog.body.ends_with("Open its Microsoft Store page?"));
        assert!(!dialog.body.contains("graphics card's driver"));
        assert_eq!(dialog.link.as_deref(), Some(COMPATIBILITY_PACK_PAGE));
        assert_eq!(dialog.open_label, "Open Microsoft Store");
    }

    #[test]
    fn this_computer_is_not_windows_on_arm_unless_it_is_windows() {
        if !cfg!(windows) {
            assert_eq!(Cause::here(), Cause::NoGraphicsDriver);
        }
    }

    /// An app's own words replace fastframe's, here a translation that keeps
    /// the built-in link for Windows on ARM.
    #[test]
    fn an_app_supplies_its_own_words() {
        let graphics = Graphics::new("Solco")
            .help("https://help")
            .dialog(|trouble| {
                let english = Dialog::english(trouble);
                Dialog {
                    title: format!("{} kann sein Fenster nicht öffnen", trouble.app_name),
                    body: match trouble.cause {
                        Cause::WindowsOnArm => "Kompatibilitätspaket installieren?".to_owned(),
                        _ => "Grafiktreiber installieren?".to_owned(),
                    },
                    open_label: "Öffnen".to_owned(),
                    close_label: "Schließen".to_owned(),
                    ..english
                }
            });
        let words = graphics.words.as_ref().expect("the app's words");
        let arm = words(&trouble(Cause::WindowsOnArm, Some("https://help")));
        assert_eq!(arm.title, "Solco kann sein Fenster nicht öffnen");
        assert_eq!(arm.link.as_deref(), Some(COMPATIBILITY_PACK_PAGE));
        let driver = words(&trouble(Cause::NoGraphicsDriver, Some("https://help")));
        assert_eq!(driver.body, "Grafiktreiber installieren?");
        assert_eq!(driver.link.as_deref(), Some("https://help"));
        assert!(format!("{graphics:?}").contains("app's own"));
    }

    #[test]
    fn a_renderer_that_panicked_before_the_window_is_a_graphics_failure() {
        let tried = Tried::Panicked(panic_text(&"wgpu error: Validation Error"));
        assert!(tried.is_graphics_failure());
        let error = eframe::Error::from(tried);
        assert!(error.to_string().contains("wgpu error: Validation Error"));
        assert!(
            !is_graphics_failure(&error),
            "eframe's own errors stay as they are"
        );
    }

    #[test]
    fn app_and_window_errors_are_not_graphics_failures() {
        assert!(!is_graphics_failure(&eframe::Error::AppCreation(
            "the library is locked".into()
        )));
    }
}
