//! MPRIS on Linux. D-Bus runs on its own thread, on a current-thread tokio
//! runtime, and trades bounded messages with the app, which stays the only
//! owner of playback. A slow or absent session bus therefore cannot stall
//! audio or the window.

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use mpris_server::{LoopStatus, Metadata, PlaybackStatus, Player, Time, TrackId};
use tokio::sync::mpsc as tokio_mpsc;

use crate::{App, Command, Controller, Playback, Repeat, State, Track, Wake, file_url};

enum Update {
    State(Box<State>, bool),
    Seeked(Duration),
}

pub(crate) struct Service {
    updates: tokio_mpsc::UnboundedSender<Update>,
    commands: Receiver<Command>,
}

impl Controller for Service {
    fn start(app: App, wake: Wake) -> Self {
        let (updates, update_rx) = tokio_mpsc::unbounded_channel();
        let (command_tx, commands) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("fastframe-mpris".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        log::warn!("no media controls: {error}");
                        return;
                    }
                };
                let local = tokio::task::LocalSet::new();
                if let Err(error) = local.block_on(&runtime, run(app, update_rx, command_tx, wake))
                {
                    log::warn!("no media controls: {error}");
                }
            });
        if let Err(error) = spawned {
            log::warn!("no media controls: {error}");
        }
        Self { updates, commands }
    }

    fn commands(&self) -> Vec<Command> {
        self.commands.try_iter().collect()
    }

    fn update(&mut self, state: State, volume_requested: bool) {
        let _ = self
            .updates
            .send(Update::State(Box::new(state), volume_requested));
    }

    fn seeked(&self, position: Duration) {
        let _ = self.updates.send(Update::Seeked(position));
    }
}

fn millis(time: Time) -> i64 {
    time.as_millis()
}

fn time(duration: Duration) -> Time {
    Time::from_millis(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

async fn run(
    app: App,
    mut updates: tokio_mpsc::UnboundedReceiver<Update>,
    commands: Sender<Command>,
    wake: Wake,
) -> mpris_server::zbus::Result<()> {
    let flatpak_id = std::env::var("FLATPAK_ID").ok();
    let player = Player::builder(&app.bus_name)
        .identity(app.identity.clone())
        .desktop_entry(crate::desktop_entry(
            flatpak_id.as_deref(),
            &app.desktop_entry,
        ))
        .can_raise(app.can_raise)
        .can_quit(app.can_quit)
        .can_control(true)
        .can_play(true)
        .can_pause(true)
        .can_go_next(true)
        .can_go_previous(true)
        .can_seek(false)
        .supported_uri_schemes(app.uri_schemes.clone())
        .supported_mime_types(app.mime_types.clone())
        .build()
        .await?;

    // A client's volume write is published from the loop below, so the
    // `PropertiesChanged` that answers it carries the new level: Plasma
    // steps its wheel from the last one it heard.
    let (hold_tx, mut hold_rx) = tokio_mpsc::unbounded_channel::<f64>();
    let send = move |command: Command| {
        if commands.send(command).is_ok() {
            wake();
        }
    };
    let prefix = track_path_prefix(&app.bus_name);
    macro_rules! on {
        ($connect:ident, $command:expr) => {{
            let send = send.clone();
            player.$connect(move |_| send($command));
        }};
    }
    on!(connect_play, Command::Play);
    on!(connect_pause, Command::Pause);
    on!(connect_play_pause, Command::PlayPause);
    on!(connect_stop, Command::Stop);
    on!(connect_next, Command::Next);
    on!(connect_previous, Command::Previous);
    on!(connect_raise, Command::Raise);
    on!(connect_quit, Command::Quit);
    {
        let send = send.clone();
        player.connect_seek(move |_, offset| send(Command::SeekBy(millis(offset))));
    }
    {
        let send = send.clone();
        let prefix = prefix.clone();
        player.connect_set_position(move |_, track, position| {
            if let Some(track_id) = track_id_from_path(&prefix, track.as_str()) {
                send(Command::SetPosition {
                    track_id,
                    position: Duration::from_millis(u64::try_from(millis(position)).unwrap_or(0)),
                });
            }
        });
    }
    {
        let send = send.clone();
        player.connect_set_volume(move |_, volume| {
            if volume.is_nan() {
                return;
            }
            let volume = volume.clamp(0.0, 1.0);
            let _ = hold_tx.send(volume);
            send(Command::SetVolume(volume));
        });
    }
    {
        let send = send.clone();
        player.connect_set_shuffle(move |_, shuffle| send(Command::SetShuffle(shuffle)));
    }
    {
        let send = send.clone();
        player.connect_set_loop_status(move |_, status| {
            send(Command::SetRepeat(match status {
                LoopStatus::None => Repeat::Off,
                LoopStatus::Track => Repeat::Track,
                LoopStatus::Playlist => Repeat::Playlist,
            }));
        });
    }
    {
        let send = send.clone();
        player.connect_open_uri(move |_, uri| send(Command::OpenUri(uri.to_owned())));
    }

    let server = player.run();
    let apply = async {
        let mut published: Option<State> = None;
        loop {
            // A held level goes out before any update the app queued after
            // it, so the app's settled level is the last word.
            let update = tokio::select! {
                biased;
                Some(volume) = hold_rx.recv() => {
                    let _ = player.set_volume(volume).await;
                    continue;
                }
                update = updates.recv() => update,
            };
            let Some(update) = update else { break };
            match update {
                Update::Seeked(position) => {
                    let _ = player.seeked(time(position)).await;
                }
                Update::State(state, volume_requested) => {
                    publish(
                        &player,
                        published.as_ref(),
                        &state,
                        volume_requested,
                        &prefix,
                    )
                    .await;
                    published = Some(*state);
                }
            }
        }
    };
    tokio::select! {
        () = server => {}
        () = apply => {}
    }
    Ok(())
}

/// Sends what changed since `previous`.
async fn publish(
    player: &Player,
    previous: Option<&State>,
    state: &State,
    volume_requested: bool,
    prefix: &str,
) {
    let changed = |same: &dyn Fn(&State) -> bool| previous.is_none_or(|p| !same(p));
    if changed(&|p| p.playback == state.playback) {
        let _ = player
            .set_playback_status(playback_status(state.playback))
            .await;
    }
    if changed(&|p| p.track == state.track) {
        let _ = player
            .set_metadata(metadata(state.track.as_ref(), prefix))
            .await;
    }
    if changed(&|p| p.controls == state.controls)
        || changed(&|p| p.track.is_some() == state.track.is_some())
    {
        let controls = state.controls;
        let _ = player.set_can_play(controls.play).await;
        let _ = player.set_can_pause(controls.pause).await;
        let _ = player.set_can_go_next(controls.next).await;
        let _ = player.set_can_go_previous(controls.previous).await;
        let _ = player
            .set_can_seek(controls.seek && state.track.is_some())
            .await;
    }
    if let Some(volume) = state.volume
        && (volume_requested
            || previous.is_none_or(|p| p.volume.is_none_or(|v| (v - volume).abs() >= 0.005)))
    {
        let _ = player.set_volume(volume).await;
    }
    if let Some(shuffle) = state.shuffle
        && changed(&|p| p.shuffle == state.shuffle)
    {
        let _ = player.set_shuffle(shuffle).await;
    }
    if let Some(repeat) = state.repeat
        && changed(&|p| p.repeat == state.repeat)
    {
        let _ = player.set_loop_status(loop_status(repeat)).await;
    }
    player.set_position(time(state.position));
}

fn playback_status(playback: Playback) -> PlaybackStatus {
    match playback {
        Playback::Playing => PlaybackStatus::Playing,
        Playback::Paused => PlaybackStatus::Paused,
        Playback::Stopped => PlaybackStatus::Stopped,
    }
}

fn loop_status(repeat: Repeat) -> LoopStatus {
    match repeat {
        Repeat::Off => LoopStatus::None,
        Repeat::Track => LoopStatus::Track,
        Repeat::Playlist => LoopStatus::Playlist,
    }
}

/// The track's MPRIS metadata. The artwork is the file when there is one, as
/// a URL, and the web address otherwise.
pub(crate) fn metadata(track: Option<&Track>, prefix: &str) -> Metadata {
    let Some(track) = track else {
        return Metadata::new();
    };
    let mut builder = Metadata::builder().title(track.title.clone());
    if let Some(path) = track_path(prefix, &track.id) {
        builder = builder.trackid(path);
    }
    if let Some(duration) = track.duration {
        builder = builder.length(time(duration));
    }
    if let Some(url) = &track.url {
        builder = builder.url(url.clone());
    }
    if !track.artists.is_empty() {
        builder = builder.artist(track.artists.clone());
    }
    if !track.album.is_empty() {
        builder = builder.album(track.album.clone());
    }
    let art = track
        .art_file
        .as_deref()
        .map(|path| file_url(path, true))
        .or_else(|| track.art_url.clone());
    if let Some(art) = art {
        builder = builder.art_url(art);
    }
    if !track.genres.is_empty() {
        builder = builder.genre(track.genres.clone());
    }
    if let Some(bpm) = track.bpm.filter(|bpm| bpm.is_finite() && *bpm > 0.0) {
        builder = builder.audio_bpm(bpm.round() as i32);
    }
    if let Some(rating) = track.rating.filter(|rating| rating.is_finite()) {
        builder = builder.user_rating(rating.clamp(0.0, 1.0));
    }
    builder.build()
}

/// Where the app's tracks live on the bus: `/<bus name>/Track/`. MPRIS
/// reserves `/org/mpris`.
pub(crate) fn track_path_prefix(bus_name: &str) -> String {
    let name: String = bus_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("/{name}/Track/")
}

/// The object path naming a track: its id in hex, which any id survives.
pub(crate) fn track_path(prefix: &str, id: &str) -> Option<TrackId> {
    use std::fmt::Write;
    let mut path = format!("{prefix}t");
    for byte in id.bytes() {
        let _ = write!(path, "{byte:02x}");
    }
    TrackId::try_from(path).ok()
}

/// The track id an object path names, if it is one of the app's.
pub(crate) fn track_id_from_path(prefix: &str, path: &str) -> Option<String> {
    let hex = path.strip_prefix(prefix)?.strip_prefix('t')?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}
