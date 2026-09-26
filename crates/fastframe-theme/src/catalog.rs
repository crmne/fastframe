//! The palette catalogue, loaded off the interface thread.

use std::fmt;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use crate::{CustomTheme, Palette, omarchy, parse_palette, presets};

/// The largest palette file read, in bytes. A palette has a few dozen
/// colours; the limit also bounds the work a hostile file can cause.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// A themes directory with more entries than this is not listed at all,
/// rather than listing a subset that depends on the filesystem's order.
pub const MAX_DIRECTORY_ENTRIES: usize = 512;
/// At most this many palettes are listed.
pub const MAX_THEMES: usize = 128;

/// Wakes the interface when a scan finishes or a watched file changes.
///
/// The app passes its own repaint or event-loop wake-up:
/// `Waker::new(move || ctx.request_repaint())`. The default does nothing.
#[derive(Clone)]
pub struct Waker(Arc<dyn Fn() + Send + Sync>);

impl Waker {
    /// A waker that calls `wake`, from a background thread.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(wake))
    }

    /// Wakes the interface.
    pub fn wake(&self) {
        (self.0)();
    }
}

impl Default for Waker {
    fn default() -> Self {
        Self::new(|| {})
    }
}

impl fmt::Debug for Waker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Waker")
    }
}

/// Why the palettes could not all be listed. The app words each for its
/// settings, in its own language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Problem {
    /// The themes directory exists and could not be read.
    Unreadable,
    /// The themes directory has more than [`MAX_DIRECTORY_ENTRIES`] entries.
    TooManyEntries,
    /// More than [`MAX_THEMES`] palettes; the rest are not listed.
    TooManyThemes,
    /// The loader thread could not start or stopped without an answer. The
    /// app suggests `<app> reload-themes`.
    LoaderFailed,
    /// Omarchy is followed and its current palette could not be read; the
    /// last usable appearance stays.
    OmarchyUnreadable,
}

/// What the theme setting should say under it, when anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// A scan is running.
    Loading,
    /// The selected palette is not in the catalogue; the app keeps its last
    /// usable appearance (its cached palette).
    SelectedUnavailable,
    /// Something went wrong listing the palettes.
    Problem(Problem),
}

/// What a desktop launch adds to the catalogue. Demo and test launches
/// leave it off and stay isolated from the desktop and its files.
#[derive(Clone, Copy, Debug)]
pub struct DesktopThemes {
    /// The app's name in paths and commands: `zapfast`. Names the packaged
    /// assets (`share/<slug>/omarchy`), the Omarchy template
    /// (`<slug>.json.tpl`) and hook (`<slug>-theme`), and the palette Omarchy
    /// renders (`<slug>.json`).
    pub slug: &'static str,
    /// The app's Omarchy template, used when neither Omarchy nor the user
    /// has one ([`omarchy::BASE_TEMPLATE`] for apps with only the base
    /// colours).
    pub omarchy_template: &'static str,
    /// Whether to list the [`presets`] too.
    pub presets: bool,
}

/// Results from one scan.
struct Loaded<P> {
    themes: Vec<CustomTheme<P>>,
    problem: Option<Problem>,
    follows_omarchy: bool,
    system_theme: Option<CustomTheme<P>>,
    #[cfg(target_os = "linux")]
    watch: Option<crate::watch::ThemeWatch>,
}

impl<P> Default for Loaded<P> {
    fn default() -> Self {
        Self {
            themes: Vec::new(),
            problem: None,
            follows_omarchy: false,
            system_theme: None,
            #[cfg(target_os = "linux")]
            watch: None,
        }
    }
}

struct Scan {
    directory: PathBuf,
    selected: Option<String>,
    waker: Waker,
}

/// The palettes an app can choose from.
///
/// [`Catalog::start`] lists the themes directory on a background thread,
/// adds the [`presets`] and, on Linux with Omarchy, the live desktop
/// palette, and watches both for changes. At most one scan runs and one
/// request waits; rapid changes collapse into the latest. The selection
/// itself lives in the app's settings, never here.
pub struct Catalog<P> {
    themes: Vec<CustomTheme<P>>,
    problem: Option<Problem>,
    receiver: Option<mpsc::Receiver<Loaded<P>>>,
    pending: Option<Scan>,
    follows_omarchy: bool,
    system_theme: Option<CustomTheme<P>>,
    desktop: Option<DesktopThemes>,
    #[cfg(target_os = "linux")]
    watch: Option<crate::watch::ThemeWatch>,
    #[cfg(target_os = "linux")]
    setup: Option<omarchy::Setup>,
    #[cfg(target_os = "linux")]
    setup_pending: bool,
}

impl<P> Default for Catalog<P> {
    fn default() -> Self {
        Self {
            themes: Vec::new(),
            problem: None,
            receiver: None,
            pending: None,
            follows_omarchy: false,
            system_theme: None,
            desktop: None,
            #[cfg(target_os = "linux")]
            watch: None,
            #[cfg(target_os = "linux")]
            setup: None,
            #[cfg(target_os = "linux")]
            setup_pending: false,
        }
    }
}

impl<P> fmt::Debug for Catalog<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Catalog")
            .field("themes", &self.themes.len())
            .field("problem", &self.problem)
            .field("loading", &self.receiver.is_some())
            .field("follows_omarchy", &self.follows_omarchy)
            .finish_non_exhaustive()
    }
}

impl<P: Palette> Catalog<P> {
    /// A catalogue already holding `themes`, for demos, screenshots and
    /// tests, without touching the desktop. With `follows_omarchy`, the
    /// [`omarchy::FILENAME`] entry among them is the live desktop palette.
    #[must_use]
    pub fn preview(themes: Vec<CustomTheme<P>>, follows_omarchy: bool) -> Self {
        let system_theme = follows_omarchy
            .then(|| {
                themes
                    .iter()
                    .find(|theme| theme.filename == omarchy::FILENAME)
                    .cloned()
            })
            .flatten();
        Self {
            themes,
            follows_omarchy,
            system_theme,
            ..Self::default()
        }
    }

    /// Adds what a normal launch shows: the presets if asked for, and on
    /// Linux the Omarchy palette, with the packaged template and hook
    /// installed for the user on the next scan.
    pub fn enable_desktop_themes(&mut self, desktop: DesktopThemes) {
        self.desktop = Some(desktop);
        #[cfg(target_os = "linux")]
        {
            self.setup = omarchy::Setup::discover(desktop.slug, desktop.omarchy_template);
            self.setup_pending = true;
        }
    }

    /// Whether a watched file changed since the last call, so the app
    /// should [`Self::start`] a scan. Always `false` outside Linux.
    pub fn needs_reload(&self) -> bool {
        #[cfg(target_os = "linux")]
        if let Some(watch) = &self.watch {
            return watch.take_changed();
        }
        false
    }

    /// Lists `directory` in the background. `selected` (a filename) is read
    /// first, so a large directory cannot push the saved choice out. When a
    /// scan is running, this waits for it and replaces any earlier waiting
    /// request.
    pub fn start(&mut self, directory: PathBuf, selected: Option<String>, waker: &Waker) {
        let scan = Scan {
            directory,
            selected,
            waker: waker.clone(),
        };
        if self.loading() {
            self.pending = Some(scan);
        } else {
            self.scan(scan);
        }
    }

    fn scan(&mut self, scan: Scan) {
        let presets = self.desktop.is_some_and(|desktop| desktop.presets);
        #[cfg(target_os = "linux")]
        let needs_watch = self.desktop.is_some() && self.watch.is_none();
        #[cfg(target_os = "linux")]
        let setup = self.setup.clone();
        #[cfg(target_os = "linux")]
        let install = std::mem::take(&mut self.setup_pending);
        let waker = scan.waker.clone();
        self.spawn(&waker, move || {
            #[cfg(target_os = "linux")]
            if install
                && let Some(setup) = &setup
                && let Err(error) = setup.install::<P>(&scan.directory)
            {
                log::warn!("unable to prepare the optional Omarchy theme: {error}");
            }
            if presets && let Err(error) = presets::write_examples(&scan.directory) {
                log::warn!("unable to write the example themes: {error}");
            }
            let loaded = with_presets(&scan.directory, scan.selected.as_deref(), presets);
            #[cfg(target_os = "linux")]
            let loaded = {
                let mut loaded = loaded;
                if needs_watch {
                    let system = setup
                        .as_ref()
                        .filter(|setup| setup.active())
                        .map(omarchy::Setup::watch_directory);
                    let watch = std::fs::create_dir_all(&scan.directory)
                        .map_err(notify::Error::io)
                        .and_then(|()| {
                            crate::watch::ThemeWatch::new(
                                &scan.directory,
                                system.as_deref(),
                                &scan.waker,
                            )
                        });
                    match watch {
                        Ok(watch) => loaded.watch = Some(watch),
                        Err(error) => log::warn!("unable to watch theme changes: {error}"),
                    }
                }
                if let Some(setup) = &setup
                    && setup.active()
                {
                    loaded.follows_omarchy = true;
                    match setup.current_theme::<P>() {
                        Ok(theme) => {
                            loaded
                                .themes
                                .retain(|old| old.filename != omarchy::FILENAME);
                            loaded.themes.push(theme.clone());
                            loaded.system_theme = Some(theme);
                        }
                        Err(error) => {
                            log::warn!("unable to read the current Omarchy palette: {error}");
                            loaded.problem.get_or_insert(Problem::OmarchyUnreadable);
                        }
                    }
                }
                loaded
            };
            loaded
        });
    }

    fn spawn(&mut self, waker: &Waker, load: impl FnOnce() -> Loaded<P> + Send + 'static) {
        let (sender, receiver) = mpsc::channel();
        let wake = waker.clone();
        let name = match self.desktop {
            Some(desktop) => format!("{}-themes", desktop.slug),
            None => "themes".to_owned(),
        };
        match std::thread::Builder::new().name(name).spawn(move || {
            let result = load();
            if sender.send(result).is_ok() {
                wake.wake();
            }
        }) {
            Ok(_) => self.receiver = Some(receiver),
            Err(error) => {
                log::warn!("unable to start the theme loader: {error}");
                self.problem = Some(Problem::LoaderFailed);
            }
        }
    }

    /// Takes a finished scan's result. Returns `true` when the catalogue
    /// changed, so the app re-resolves its selection; call it every frame
    /// (or on wake-up) while [`Self::loading`].
    pub fn poll(&mut self) -> bool {
        let Some(receiver) = &self.receiver else {
            return false;
        };
        let result = receiver.try_recv();
        if matches!(result, Err(mpsc::TryRecvError::Empty)) {
            return false;
        }
        self.receiver = None;
        if let Some(scan) = self.pending.take() {
            // Keep the accepted palettes until the latest request finishes:
            // publishing this superseded result could flash old colours.
            self.scan(scan);
            return false;
        }
        match result {
            Ok(loaded) => {
                self.themes = loaded.themes;
                self.problem = loaded.problem;
                self.follows_omarchy = loaded.follows_omarchy;
                self.system_theme = loaded.system_theme;
                #[cfg(target_os = "linux")]
                if loaded.watch.is_some() {
                    self.watch = loaded.watch;
                }
            }
            Err(_) => self.problem = Some(Problem::LoaderFailed),
        }
        true
    }

    /// Every palette, sorted by filename.
    pub fn themes(&self) -> &[CustomTheme<P>] {
        &self.themes
    }

    /// The palettes in picker order: the live Omarchy palette first when it
    /// is followed, then the rest. A leftover `omarchy.json` is not offered
    /// when the desktop is not followed.
    pub fn picker_themes(&self) -> impl Iterator<Item = &CustomTheme<P>> {
        let live = self
            .themes
            .iter()
            .filter(|theme| self.follows_omarchy && theme.filename == omarchy::FILENAME);
        live.chain(
            self.themes
                .iter()
                .filter(|theme| theme.filename != omarchy::FILENAME),
        )
    }

    /// The palette called `filename`, if listed.
    pub fn find(&self, filename: &str) -> Option<&CustomTheme<P>> {
        self.themes.iter().find(|theme| theme.filename == filename)
    }

    /// Whether the desktop's palette is followed (Omarchy is set up).
    pub fn follows_omarchy(&self) -> bool {
        self.follows_omarchy
    }

    /// The desktop's current palette, for a "follow system" setting.
    pub fn system_theme(&self) -> Option<&CustomTheme<P>> {
        self.system_theme.as_ref()
    }

    /// Whether a scan is running.
    pub fn loading(&self) -> bool {
        self.receiver.is_some()
    }

    /// What to say under the theme setting, given the selected filename.
    pub fn status(&self, selected: Option<&str>) -> Option<Status> {
        if self.loading() {
            return Some(Status::Loading);
        }
        if selected.is_some_and(|filename| self.find(filename).is_none()) {
            return Some(Status::SelectedUnavailable);
        }
        self.problem.map(Status::Problem)
    }
}

/// Lists the directory and, when asked, adds the presets it does not
/// override.
fn with_presets<P: Palette>(directory: &Path, selected: Option<&str>, presets: bool) -> Loaded<P> {
    // A preset selected without a local override is added below; reading it
    // from the directory would only log that it is missing.
    let selected_file = selected.filter(|filename| {
        !presets || !presets::contains(filename) || directory.join(filename).exists()
    });
    let mut loaded = discover(directory, selected_file);
    if presets {
        for theme in presets::themes::<P>() {
            let listed = loaded
                .themes
                .iter()
                .any(|local| local.filename == theme.filename);
            if selected_file == Some(theme.filename.as_str())
                && directory.join(&theme.filename).exists()
                && !listed
            {
                // A broken user override keeps the cached selection; it must
                // not silently turn back into the shared palette.
                continue;
            }
            if !listed {
                loaded.themes.push(theme);
            }
        }
        loaded.themes.sort_by(|a, b| a.filename.cmp(&b.filename));
    }
    loaded
}

/// Whether `filename` is a single JSON file name, never a path.
fn filename_is_local(filename: &str) -> bool {
    let mut parts = Path::new(filename).components();
    matches!(parts.next(), Some(Component::Normal(_)))
        && parts.next().is_none()
        && !filename.contains('\\')
        && filename.ends_with(".json")
}

/// Reads one palette file: a regular file (not a link) in `directory`, at
/// most [`MAX_FILE_BYTES`], UTF-8, and a valid palette.
fn read_theme<P: Palette>(directory: &Path, filename: &str) -> Result<CustomTheme<P>, String> {
    if !filename_is_local(filename) {
        return Err("expected a JSON filename in the themes folder".into());
    }
    let path = directory.join(filename);
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("expected a regular file, not a directory or symbolic link".into());
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err("theme exceeds the 64 KiB file limit".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("theme exceeds the 64 KiB file limit".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "expected UTF-8 JSON".to_owned())?;
    Ok(CustomTheme {
        filename: filename.into(),
        palette: parse_palette(text)?,
    })
}

/// Lists the palette files in `directory`, never recursing.
fn discover<P: Palette>(directory: &Path, selected: Option<&str>) -> Loaded<P> {
    let mut loaded = Loaded::default();
    // Read the saved choice directly so a large directory cannot displace
    // it. It is still a single validated filename, never a path.
    if let Some(filename) = selected {
        match read_theme(directory, filename) {
            Ok(theme) => loaded.themes.push(theme),
            Err(error) => log::warn!("unable to load selected theme {filename:?}: {error}"),
        }
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                loaded.problem = Some(Problem::Unreadable);
                log::warn!("unable to read themes at {}: {error}", directory.display());
            }
            return loaded;
        }
    };
    let mut names = Vec::new();
    for (index, entry) in entries.take(MAX_DIRECTORY_ENTRIES + 1).enumerate() {
        if index == MAX_DIRECTORY_ENTRIES {
            // Offer no subset that depends on the filesystem's order.
            loaded.problem = Some(Problem::TooManyEntries);
            return loaded;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!("unable to read theme entry: {error}");
                continue;
            }
        };
        let filename = entry.file_name();
        let Some(filename) = filename.to_str() else {
            continue;
        };
        if filename_is_local(filename) && Some(filename) != selected {
            match entry.file_type() {
                Ok(kind) if kind.is_file() => names.push(filename.to_owned()),
                Ok(_) => {}
                Err(error) => log::warn!("unable to inspect theme {filename:?}: {error}"),
            }
        }
    }
    names.sort();
    if names.len() + loaded.themes.len() > MAX_THEMES {
        loaded.problem = Some(Problem::TooManyThemes);
    }
    for filename in names
        .into_iter()
        .take(MAX_THEMES.saturating_sub(loaded.themes.len()))
    {
        match read_theme(directory, &filename) {
            Ok(theme) => loaded.themes.push(theme),
            Err(error) => log::warn!("unable to load theme {filename:?}: {error}"),
        }
    }
    loaded.themes.sort_by(|a, b| a.filename.cmp(&b.filename));
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Base;
    use crate::test_palette::Colors;
    use egui::Color32;
    use std::time::{Duration, Instant};

    fn theme(filename: &str, base: Base) -> CustomTheme<Colors> {
        CustomTheme {
            filename: filename.into(),
            palette: Colors::base(base),
        }
    }

    fn wait(catalog: &mut Catalog<Colors>) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !catalog.poll() {
            assert!(Instant::now() < deadline, "the scan did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    impl Catalog<Colors> {
        fn load_test(&mut self, load: impl FnOnce() -> Vec<CustomTheme<Colors>> + Send + 'static) {
            self.spawn(&Waker::default(), move || Loaded {
                themes: load(),
                ..Loaded::default()
            });
        }
    }

    #[test]
    fn shared_palettes_need_no_local_files_and_valid_overrides_win() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("Nord.json"),
            r##"{"base":"light","colors":{"accent":"#102030"}}"##,
        )
        .unwrap();
        let mut catalog = Catalog::<Colors> {
            desktop: Some(DesktopThemes {
                slug: "app",
                omarchy_template: omarchy::BASE_TEMPLATE,
                presets: true,
            }),
            ..Catalog::default()
        };
        catalog.start(
            directory.path().into(),
            Some("Nord.json".into()),
            &Waker::default(),
        );
        wait(&mut catalog);
        assert_eq!(catalog.themes().len(), 8);
        let nord = catalog.find("Nord.json").unwrap();
        assert!(!nord.palette.dark);
        assert_eq!(
            nord.palette.get("accent"),
            Color32::from_rgb(0x10, 0x20, 0x30)
        );
        assert!(catalog.find("Rose Pine Dawn.json").is_some());
        // The only palette file where themes are loaded from is the user's
        // own, unchanged; the shared ones are copied into the examples
        // folder, which is never loaded.
        let files: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .collect();
        assert_eq!(files, ["Nord.json"], "shared palettes create no user files");
        assert!(
            directory
                .path()
                .join(presets::EXAMPLES)
                .join("Rose Pine Dawn.json")
                .is_file()
        );
        assert_eq!(catalog.themes().len(), 8, "the examples are not loaded");
    }

    #[test]
    fn a_broken_override_of_a_selected_preset_keeps_the_selection_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Nord.json"), "broken").unwrap();
        let loaded: Loaded<Colors> = with_presets(directory.path(), Some("Nord.json"), true);
        assert!(
            loaded
                .themes
                .iter()
                .all(|theme| theme.filename != "Nord.json")
        );
        let loaded: Loaded<Colors> = with_presets(directory.path(), Some("Tokyo Night.json"), true);
        assert!(
            loaded
                .themes
                .iter()
                .any(|theme| theme.filename == "Nord.json")
        );
        let loaded: Loaded<Colors> = with_presets(directory.path(), None, false);
        assert!(loaded.themes.is_empty(), "presets only when asked for");
    }

    #[test]
    fn the_picker_puts_live_omarchy_first_only_when_it_is_followed() {
        for followed in [false, true] {
            let catalog = Catalog::preview(
                ["Catppuccin.json", "Tokyo Night.json", "omarchy.json"]
                    .into_iter()
                    .map(|filename| theme(filename, Base::Dark))
                    .collect(),
                followed,
            );
            let names: Vec<_> = catalog
                .picker_themes()
                .map(|theme| theme.filename.as_str())
                .collect();
            if followed {
                assert_eq!(
                    names,
                    ["omarchy.json", "Catppuccin.json", "Tokyo Night.json"]
                );
                assert_eq!(catalog.system_theme().unwrap().filename, "omarchy.json");
            } else {
                assert_eq!(names, ["Catppuccin.json", "Tokyo Night.json"]);
                assert!(catalog.system_theme().is_none());
            }
            assert!(
                catalog.find("omarchy.json").is_some(),
                "filtering the menu keeps cached selections"
            );
        }
    }

    #[test]
    fn reloads_coalesce_and_never_publish_a_superseded_palette() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("latest.json"), br#"{"base":"light"}"#).unwrap();
        let accepted = theme("accepted.json", Base::Dark);
        let mut catalog = Catalog::preview(vec![accepted.clone()], false);
        let (finish, worker) = mpsc::channel::<()>();
        catalog.load_test(move || {
            worker.recv().unwrap();
            vec![theme("stale.json", Base::Light)]
        });
        for _ in 0..100 {
            catalog.start(directory.path().join("superseded"), None, &Waker::default());
        }
        catalog.start(
            directory.path().into(),
            Some("latest.json".into()),
            &Waker::default(),
        );
        assert!(!catalog.poll(), "requests do not replace the running scan");
        assert_eq!(catalog.status(None), Some(Status::Loading));
        finish.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !catalog.poll() {
            assert_eq!(
                catalog.themes(),
                std::slice::from_ref(&accepted),
                "hold the accepted colours"
            );
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!catalog.loading());
        assert_eq!(catalog.themes().len(), 1);
        assert_eq!(catalog.themes()[0].filename, "latest.json");
        assert!(!catalog.themes()[0].palette.dark);
        assert_eq!(catalog.status(Some("latest.json")), None);
        assert_eq!(
            catalog.status(Some("gone.json")),
            Some(Status::SelectedUnavailable)
        );
    }

    #[test]
    fn scans_wake_the_interface() {
        let directory = tempfile::tempdir().unwrap();
        let (sender, woken) = mpsc::channel();
        let sender = std::sync::Mutex::new(sender);
        let waker = Waker::new(move || {
            let _ = sender.lock().unwrap().send(());
        });
        let mut catalog = Catalog::<Colors>::default();
        catalog.start(directory.path().into(), None, &waker);
        woken.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(catalog.poll());
        assert!(catalog.themes().is_empty());
        assert!(
            !catalog.needs_reload(),
            "nothing is watched without the desktop"
        );
    }

    #[test]
    fn discovery_sorts_valid_files_and_skips_invalid_ones() {
        let directory = tempfile::tempdir().unwrap();
        let dir = directory.path().join("themes");
        assert!(discover::<Colors>(&dir, None).themes.is_empty());
        assert_eq!(
            discover::<Colors>(&dir, None).problem,
            None,
            "a missing folder is fine"
        );
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in [
            ("z.json", "{}"),
            ("a.json", "{}"),
            ("bad.json", "invalid"),
            ("ignored.txt", "{}"),
        ] {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let names: Vec<_> = discover::<Colors>(&dir, None)
            .themes
            .into_iter()
            .map(|theme| theme.filename)
            .collect();
        assert_eq!(names, ["a.json", "z.json"]);
    }

    #[test]
    fn reads_are_bounded_and_cannot_escape_to_other_files() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("themes");
        std::fs::create_dir_all(&dir).unwrap();
        let mut boundary = vec![b' '; MAX_FILE_BYTES as usize];
        boundary[..2].copy_from_slice(b"{}");
        std::fs::write(dir.join("boundary.json"), &boundary).unwrap();
        assert_eq!(
            read_theme::<Colors>(&dir, "boundary.json").unwrap().palette,
            Colors::base(Base::Dark)
        );
        boundary.push(b' ');
        std::fs::write(dir.join("too-large.json"), boundary).unwrap();
        std::fs::write(dir.join("invalid-utf8.json"), [0xff, 0xfe]).unwrap();
        std::fs::create_dir(dir.join("directory.json")).unwrap();
        std::fs::write(root.path().join("outside.json"), b"{}").unwrap();
        for filename in [
            "too-large.json",
            "invalid-utf8.json",
            "directory.json",
            "../outside.json",
            "..\\outside.json",
            "/outside.json",
            "not-json.txt",
        ] {
            assert!(read_theme::<Colors>(&dir, filename).is_err(), "{filename}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path().join("outside.json"), dir.join("link.json"))
                .unwrap();
            assert!(read_theme::<Colors>(&dir, "link.json").is_err());
            assert!(
                discover::<Colors>(&dir, Some("link.json"))
                    .themes
                    .iter()
                    .all(|theme| theme.filename != "link.json")
            );
        }
    }

    #[test]
    fn limits_keep_the_saved_selection_without_an_arbitrary_partial_listing() {
        let directory = tempfile::tempdir().unwrap();
        let dir = directory.path();
        for index in 0..=MAX_THEMES {
            std::fs::write(dir.join(format!("{index:03}.json")), b"{}").unwrap();
        }
        std::fs::write(dir.join("selected.json"), b"{}").unwrap();
        let loaded = discover::<Colors>(dir, Some("selected.json"));
        assert_eq!(loaded.themes.len(), MAX_THEMES);
        assert!(
            loaded
                .themes
                .iter()
                .any(|theme| theme.filename == "selected.json")
        );
        assert_eq!(loaded.problem, Some(Problem::TooManyThemes));
        for index in 0..MAX_DIRECTORY_ENTRIES {
            std::fs::write(dir.join(format!("ignored-{index}.txt")), b"ignored").unwrap();
        }
        let loaded = discover::<Colors>(dir, Some("selected.json"));
        let names: Vec<_> = loaded
            .themes
            .iter()
            .map(|theme| theme.filename.as_str())
            .collect();
        assert_eq!(names, ["selected.json"]);
        assert_eq!(loaded.problem, Some(Problem::TooManyEntries));
    }

    #[test]
    fn an_unreadable_folder_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("not-a-folder");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(
            discover::<Colors>(&file, None).problem,
            Some(Problem::Unreadable)
        );
    }
}
