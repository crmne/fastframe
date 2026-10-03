//! The desktop's media controls for an egui app.
//!
//! The app says what is playing, artwork included, and the desktop shows it
//! where people look for it: MPRIS on Linux (the panel's player widget,
//! `playerctl`, KDE Connect), the media overlay on Windows, and the Now
//! Playing panel and Control Centre on macOS. The keyboard's media keys, a
//! headset's buttons and those panels send [`Command`]s back.
//!
//! [`NowPlaying`] runs the controls on a thread of their own, so a slow or
//! missing session bus never holds up audio or the window. The app stays the
//! only one deciding what a command does.
//!
//! ```no_run
//! use fastframe_now_playing::{App, Command, NowPlaying, Playback, State, Track};
//!
//! // On the main thread (macOS delivers the controls' events there).
//! let mut controls = NowPlaying::start(App::new("example", "Example"), || {
//!     // Wake the window: a command is waiting.
//! });
//!
//! // Every frame, or whenever something changes:
//! for command in controls.commands() {
//!     match command {
//!         Command::PlayPause => { /* toggle */ }
//!         _ => {}
//!     }
//! }
//! controls.update(State {
//!     playback: Playback::Playing,
//!     track: Some(Track {
//!         id: "track-1".into(),
//!         title: "Song".into(),
//!         artists: vec!["Band".into()],
//!         art_file: Some("/home/me/.cache/example/art/1.jpg".into()),
//!         ..Track::default()
//!     }),
//!     ..State::default()
//! });
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod mpris;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native;
#[cfg(target_os = "windows")]
mod windows;

/// How often the position is republished while playing. Every client
/// interpolates between updates, so a jump is sent with
/// [`NowPlaying::seeked`] instead.
const POSITION_INTERVAL: Duration = Duration::from_secs(1);

/// How far a seek button that names no amount moves, in milliseconds.
#[cfg(any(target_os = "windows", target_os = "macos"))]
const SEEK_STEP_MS: i64 = 10_000;

/// Who the controls belong to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// The name on the session bus, after `org.mpris.MediaPlayer2.`: a
    /// lowercase word such as `spotifast`.
    pub bus_name: String,
    /// The name people see, such as `Spotifast`.
    pub identity: String,
    /// The desktop entry's name without `.desktop`. Inside a Flatpak the
    /// sandbox's app id is used instead, since that is the name the entry is
    /// exported under.
    pub desktop_entry: String,
    /// URI schemes [`Command::OpenUri`] takes, such as `spotify`.
    pub uri_schemes: Vec<String>,
    /// MIME types [`Command::OpenUri`] takes.
    pub mime_types: Vec<String>,
    /// Whether the controls offer to bring the window forward.
    pub can_raise: bool,
    /// Whether the controls offer to quit the app.
    pub can_quit: bool,
}

impl App {
    /// An app with this bus name and identity, whose desktop entry is named
    /// after the bus name, that can be raised and quit and opens no URIs.
    #[must_use]
    pub fn new(bus_name: &str, identity: &str) -> Self {
        Self {
            bus_name: bus_name.to_owned(),
            identity: identity.to_owned(),
            desktop_entry: bus_name.to_owned(),
            uri_schemes: Vec::new(),
            mime_types: Vec::new(),
            can_raise: true,
            can_quit: true,
        }
    }
}

/// Whether something is playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playback {
    /// Nothing is loaded, or playback was stopped.
    #[default]
    Stopped,
    /// Playing.
    Playing,
    /// Paused (or still loading), with a track that Play resumes.
    Paused,
}

/// What repeats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    /// Nothing repeats.
    #[default]
    Off,
    /// The track repeats.
    Track,
    /// The playlist, album or queue repeats.
    Playlist,
}

/// The track playing, or paused.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    /// The app's own id for the track. [`Command::SetPosition`] names it, so
    /// a request for a track that has since changed can be ignored.
    pub id: String,
    /// The title.
    pub title: String,
    /// The artists, in order.
    pub artists: Vec<String>,
    /// The album, or empty.
    pub album: String,
    /// The length, when known.
    pub duration: Option<Duration>,
    /// The cover as an image file on disk. Every platform shows it; Windows
    /// and macOS show only this, since they load the image themselves and
    /// macOS cannot survive a remote image that fails to load.
    pub art_file: Option<PathBuf>,
    /// The cover's web address, for MPRIS while [`art_file`](Self::art_file)
    /// is not there yet.
    pub art_url: Option<String>,
    /// The track's own address, such as a `spotify:` URI (MPRIS only).
    pub url: Option<String>,
    /// Genres (MPRIS only).
    pub genres: Vec<String>,
    /// Beats per minute (MPRIS only).
    pub bpm: Option<f64>,
    /// The listener's rating, from 0 to 1 (MPRIS only).
    pub rating: Option<f64>,
}

/// What the controls offer right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    /// Play is available.
    pub play: bool,
    /// Pause is available.
    pub pause: bool,
    /// Next is available.
    pub next: bool,
    /// Previous is available.
    pub previous: bool,
    /// Seeking is available (only ever with a track).
    pub seek: bool,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            play: true,
            pause: true,
            next: true,
            previous: true,
            seek: true,
        }
    }
}

/// Everything the controls show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Playing, paused or stopped.
    pub playback: Playback,
    /// The track, if any.
    pub track: Option<Track>,
    /// How far into the track playback is.
    pub position: Duration,
    /// The volume from 0 to 1, for an app whose volume the controls may set.
    pub volume: Option<f64>,
    /// Shuffle, for an app that shuffles.
    pub shuffle: Option<bool>,
    /// Repeat, for an app that repeats.
    pub repeat: Option<Repeat>,
    /// What the controls offer.
    pub controls: Controls,
}

/// What the desktop asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Play.
    Play,
    /// Pause.
    Pause,
    /// Play or pause, whichever it is not doing.
    PlayPause,
    /// Stop.
    Stop,
    /// The next track.
    Next,
    /// The previous track.
    Previous,
    /// Move by this many milliseconds; negative moves back.
    SeekBy(i64),
    /// Move to `position` in the track with this [`Track::id`].
    SetPosition {
        /// The track meant.
        track_id: String,
        /// Where to.
        position: Duration,
    },
    /// Set the volume, from 0 to 1.
    SetVolume(f64),
    /// Turn shuffle on or off.
    SetShuffle(bool),
    /// Set what repeats.
    SetRepeat(Repeat),
    /// Open this URI, of a scheme or type in [`App`].
    OpenUri(String),
    /// Bring the window forward, creating it if needed.
    Raise,
    /// Quit the app.
    Quit,
}

/// The desktop's media controls, for as long as the app keeps this.
pub struct NowPlaying {
    platform: Platform,
    throttle: Throttle,
}

#[cfg(target_os = "linux")]
type Platform = mpris::Service;
#[cfg(any(target_os = "windows", target_os = "macos"))]
type Platform = native::Service;
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
type Platform = Unsupported;

impl std::fmt::Debug for NowPlaying {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NowPlaying").finish_non_exhaustive()
    }
}

impl NowPlaying {
    /// Starts the controls. `wake` is called, from another thread, whenever
    /// a command arrives: wake the window there, so it reads
    /// [`commands`](Self::commands).
    ///
    /// Call it on the main thread: macOS delivers the controls' events
    /// there, through the app's event loop. Controls that cannot start (no
    /// session bus, say) log a warning, and the app runs without them.
    pub fn start(app: App, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            platform: Platform::start(app, std::sync::Arc::new(wake)),
            throttle: Throttle::default(),
        }
    }

    /// The commands that arrived since the last call, oldest first.
    pub fn commands(&self) -> Vec<Command> {
        let commands = self.platform.commands();
        if commands
            .iter()
            .any(|command| matches!(command, Command::SetVolume(_)))
        {
            self.throttle.volume_requested.set(true);
        }
        commands
    }

    /// Shows `state`. Cheap to call every frame: a change is sent at once,
    /// and the position alone at most once a second while playing.
    pub fn update(&mut self, state: State) {
        if let Some(volume_requested) = self.throttle.due(&state, Instant::now()) {
            self.platform.update(state.clone(), volume_requested);
            self.throttle.sent(state);
        }
    }

    /// Playback jumped to `position`, by a seek rather than by playing.
    pub fn seeked(&self, position: Duration) {
        self.platform.seeked(position);
    }

    /// Makes the app the one the media keys reach before anything plays,
    /// for a track remembered from the last run: macOS routes the keys only
    /// to an app that has played since it started. Does nothing elsewhere,
    /// or once the app has played.
    pub fn claim(&mut self, track_id: &str, position: Duration) {
        self.platform.claim(track_id, position);
    }
}

/// What the platform modules provide.
trait Controller: Sized {
    fn start(app: App, wake: Wake) -> Self;
    fn commands(&self) -> Vec<Command>;
    fn update(&mut self, state: State, volume_requested: bool);
    fn seeked(&self, position: Duration);
    fn claim(&mut self, _track_id: &str, _position: Duration) {}
}

type Wake = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Where there are no media controls to talk to.
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
struct Unsupported;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
impl Controller for Unsupported {
    fn start(_app: App, _wake: Wake) -> Self {
        Self
    }
    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }
    fn update(&mut self, _state: State, _volume_requested: bool) {}
    fn seeked(&self, _position: Duration) {}
}

/// Decides what is worth sending: any change at once, the position alone
/// at most once a second while playing.
#[derive(Debug, Default)]
struct Throttle {
    published: Option<State>,
    last_sent: Option<Instant>,
    /// A client set the volume, and now holds its level rather than the
    /// app's: the next update republishes whatever the app settled on, even
    /// when that is the level it already had.
    volume_requested: std::cell::Cell<bool>,
}

impl Throttle {
    /// Whether `state` should go out now, and if so whether a volume request
    /// is being answered.
    fn due(&self, state: &State, now: Instant) -> Option<bool> {
        let volume_requested = self.volume_requested.get();
        let changed = volume_requested
            || self
                .published
                .as_ref()
                .is_none_or(|published| !same_but_position(published, state));
        let moved = self
            .published
            .as_ref()
            .is_none_or(|published| published.position != state.position);
        let position_due = state.playback != Playback::Playing
            || self
                .last_sent
                .is_none_or(|at| now.duration_since(at) >= POSITION_INTERVAL);
        (changed || (moved && position_due)).then(|| {
            self.volume_requested.set(false);
            volume_requested
        })
    }

    fn sent(&mut self, state: State) {
        self.published = Some(state);
        self.last_sent = Some(Instant::now());
    }
}

fn same_but_position(left: &State, right: &State) -> bool {
    let volume_same = match (left.volume, right.volume) {
        (Some(a), Some(b)) => (a - b).abs() < 0.005,
        (a, b) => a.is_none() && b.is_none(),
    };
    left.playback == right.playback
        && left.track == right.track
        && volume_same
        && left.shuffle == right.shuffle
        && left.repeat == right.repeat
        && left.controls == right.controls
}

/// A `file://` URL for an artwork file. macOS reads the whole string as a
/// URL, so anything URL-significant in the path is escaped, or a `#` in a
/// home directory ends the URL early and the image fails to load (which
/// aborts the process there). MPRIS clients read it as a URL too. Windows
/// takes what follows `file://` as a plain path and opens it as written, so
/// escaping it there would break it.
fn file_url(path: &Path, escaped: bool) -> String {
    use std::fmt::Write;
    if !escaped {
        return format!("file://{}", path.display());
    }
    let mut url = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'/' | b'-' | b'.' | b'_' | b'~' => url.push(char::from(byte)),
            _ if byte.is_ascii_alphanumeric() => url.push(char::from(byte)),
            _ => {
                let _ = write!(url, "%{byte:02X}");
            }
        }
    }
    url
}

/// The desktop entry the media controls name for an app whose own entry is
/// `own` (without `.desktop`): the sandbox's app id inside a Flatpak, since
/// the entry is exported under it, and `own` elsewhere. An app that sets its
/// window's app id to match its entry uses the same name.
#[must_use]
pub fn desktop_entry(own: &str) -> String {
    let flatpak_id = std::env::var("FLATPAK_ID").ok();
    desktop_entry_for(flatpak_id.as_deref(), own).to_owned()
}

fn desktop_entry_for<'a>(flatpak_id: Option<&'a str>, own: &'a str) -> &'a str {
    flatpak_id.filter(|id| !id.is_empty()).unwrap_or(own)
}

#[cfg(test)]
mod tests;
