//! The controls on Windows and macOS: the System Media Transport Controls
//! (the media overlay), and MPNowPlayingInfoCenter with
//! MPRemoteCommandCenter (Now Playing and Control Centre), through souvlaki.
//!
//! Windows ties the controls to a window, so they get a hidden one of their
//! own, on a thread with a message loop (`windows.rs`), and outlive the
//! app's own window closing to the tray. macOS delivers their events on the
//! main thread, which the app's event loop keeps running.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use crate::{App, Command, Controller, Playback, State, Wake, file_url};

/// What a control event asks of the app. `track_id` is the track showing,
/// for a "set position" to name.
pub(crate) fn command_for(event: MediaControlEvent, track_id: &str) -> Command {
    let step = |direction: SeekDirection, ms: i64| match direction {
        SeekDirection::Forward => Command::SeekBy(ms),
        SeekDirection::Backward => Command::SeekBy(-ms),
    };
    match event {
        MediaControlEvent::Play => Command::Play,
        MediaControlEvent::Pause => Command::Pause,
        MediaControlEvent::Toggle => Command::PlayPause,
        MediaControlEvent::Next => Command::Next,
        MediaControlEvent::Previous => Command::Previous,
        MediaControlEvent::Stop => Command::Stop,
        MediaControlEvent::Seek(direction) => step(direction, crate::SEEK_STEP_MS),
        MediaControlEvent::SeekBy(direction, amount) => step(
            direction,
            i64::try_from(amount.as_millis()).unwrap_or(i64::MAX),
        ),
        MediaControlEvent::SetPosition(position) => Command::SetPosition {
            track_id: track_id.to_owned(),
            position: position.0,
        },
        MediaControlEvent::SetVolume(volume) => Command::SetVolume(volume.clamp(0.0, 1.0)),
        MediaControlEvent::OpenUri(uri) => Command::OpenUri(uri),
        MediaControlEvent::Raise => Command::Raise,
        MediaControlEvent::Quit => Command::Quit,
    }
}

/// Whether a remembered, paused track should make the app the Now Playing
/// owner: macOS routes the media keys only to the app that last played, and
/// attaching handlers alone does not make it that.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn should_claim(claimed: bool, state: &State) -> bool {
    !claimed && state.track.is_some() && state.playback != Playback::Playing
}

/// The controls, and what they were last told, so only changes are sent.
pub(crate) struct Bridge {
    controls: MediaControls,
    last: State,
    /// The track showing, for a "set position" request to name.
    track_id: Arc<Mutex<String>>,
    claimed: bool,
}

impl Bridge {
    pub(crate) fn new(
        app: &App,
        hwnd: Option<*mut std::ffi::c_void>,
        sender: Sender<Command>,
        wake: Wake,
    ) -> Result<Self, String> {
        let mut controls = MediaControls::new(PlatformConfig {
            display_name: &app.identity,
            dbus_name: &app.bus_name,
            hwnd,
        })
        .map_err(|error| format!("{error:?}"))?;
        let track_id: Arc<Mutex<String>> = Arc::default();
        let current = Arc::clone(&track_id);
        controls
            .attach(move |event| {
                let id = current
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if sender.send(command_for(event, &id)).is_ok() {
                    wake();
                }
            })
            .map_err(|error| format!("{error:?}"))?;
        Ok(Self {
            controls,
            last: State::default(),
            track_id,
            claimed: false,
        })
    }

    pub(crate) fn apply(&mut self, state: State) {
        let track_changed = state.track != self.last.track;
        if track_changed {
            *self.track_id.lock().unwrap_or_else(PoisonError::into_inner) = state
                .track
                .as_ref()
                .map(|track| track.id.clone())
                .unwrap_or_default();
            let artist = state
                .track
                .as_ref()
                .map(|track| track.artists.join(", "))
                .unwrap_or_default();
            // Only ever a local file: handed a remote URL, macOS fetches it
            // itself and aborts the process when it fails to arrive. Art
            // that is not on disk yet is left out, and set when a state
            // arrives with the file.
            let cover = state
                .track
                .as_ref()
                .and_then(|track| track.art_file.as_deref())
                .map(|path| file_url(path, cfg!(target_os = "macos")));
            let metadata = match &state.track {
                Some(track) => MediaMetadata {
                    title: Some(track.title.as_str()),
                    album: Some(track.album.as_str()),
                    artist: Some(artist.as_str()),
                    cover_url: cover.as_deref(),
                    duration: track.duration,
                },
                None => MediaMetadata::default(),
            };
            if let Err(error) = self.controls.set_metadata(metadata) {
                log::debug!("the media controls refused the metadata: {error:?}");
            }
        }
        // A paused remembered track is worth the keys: Play resumes it.
        #[cfg(target_os = "macos")]
        if should_claim(self.claimed, &state) {
            self.set_playback(Playback::Playing, state.position);
            self.claimed = true;
        }
        if track_changed || state.playback != self.last.playback {
            self.set_playback(state.playback, state.position);
        }
        if state.playback == Playback::Playing {
            self.claimed = true;
        }
        self.last = state;
    }

    pub(crate) fn seeked(&mut self, position: Duration) {
        self.last.position = position;
        self.set_playback(self.last.playback, position);
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn claim(&mut self, track_id: &str, position: Duration) {
        if self.claimed {
            return;
        }
        *self.track_id.lock().unwrap_or_else(PoisonError::into_inner) = track_id.to_owned();
        self.set_playback(Playback::Playing, position);
        self.set_playback(Playback::Paused, position);
        self.claimed = true;
    }

    fn set_playback(&mut self, playback: Playback, position: Duration) {
        let progress = Some(MediaPosition(position));
        let playback = match playback {
            Playback::Playing => MediaPlayback::Playing { progress },
            Playback::Paused => MediaPlayback::Paused { progress },
            Playback::Stopped => MediaPlayback::Stopped,
        };
        if let Err(error) = self.controls.set_playback(playback) {
            log::debug!("the media controls refused the playback state: {error:?}");
        }
    }
}

/// What the app sends to the Windows controls' thread.
#[cfg(target_os = "windows")]
pub(crate) enum Update {
    State(Box<State>),
    Seeked(Duration),
}

/// The Windows controls, on their own thread.
#[cfg(target_os = "windows")]
pub(crate) struct Service {
    commands: Receiver<Command>,
    /// Where updates go, and the thread to wake for them; `None` when the
    /// controls could not be made.
    updates: Option<(Sender<Update>, u32)>,
}

#[cfg(target_os = "windows")]
impl Controller for Service {
    fn start(app: App, wake: Wake) -> Self {
        let (sender, commands) = std::sync::mpsc::channel();
        let (update_tx, update_rx) = std::sync::mpsc::channel();
        let updates = match crate::windows::start(app, sender, wake, update_rx) {
            Ok(thread) => Some((update_tx, thread)),
            Err(error) => {
                log::warn!("no media controls: {error}");
                None
            }
        };
        Self { commands, updates }
    }

    fn commands(&self) -> Vec<Command> {
        self.commands.try_iter().collect()
    }

    fn update(&mut self, state: State, _volume_requested: bool) {
        self.send(Update::State(Box::new(state)));
    }

    fn seeked(&self, position: Duration) {
        self.send(Update::Seeked(position));
    }
}

#[cfg(target_os = "windows")]
impl Service {
    fn send(&self, update: Update) {
        if let Some((updates, thread)) = &self.updates
            && updates.send(update).is_ok()
        {
            crate::windows::poke(*thread);
        }
    }
}

/// The macOS controls, used on the main thread.
#[cfg(target_os = "macos")]
pub(crate) struct Service {
    commands: Receiver<Command>,
    bridge: Mutex<Option<Bridge>>,
}

#[cfg(target_os = "macos")]
impl Service {
    fn with_bridge(&self, act: impl FnOnce(&mut Bridge)) {
        if let Some(bridge) = self
            .bridge
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            act(bridge);
        }
    }
}

#[cfg(target_os = "macos")]
impl Controller for Service {
    fn start(app: App, wake: Wake) -> Self {
        let (sender, commands) = std::sync::mpsc::channel();
        let bridge = match Bridge::new(&app, None, sender, wake) {
            Ok(bridge) => Some(bridge),
            Err(error) => {
                log::warn!("no media controls: {error}");
                None
            }
        };
        Self {
            commands,
            bridge: Mutex::new(bridge),
        }
    }

    fn commands(&self) -> Vec<Command> {
        self.commands.try_iter().collect()
    }

    fn update(&mut self, state: State, _volume_requested: bool) {
        self.with_bridge(|bridge| bridge.apply(state));
    }

    fn seeked(&self, position: Duration) {
        self.with_bridge(|bridge| bridge.seeked(position));
    }

    fn claim(&mut self, track_id: &str, position: Duration) {
        self.with_bridge(|bridge| bridge.claim(track_id, position));
    }
}
