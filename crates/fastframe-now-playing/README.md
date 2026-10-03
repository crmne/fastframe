# fastframe-now-playing

The desktop's media controls for an egui app. The app says what is playing,
artwork included, and the desktop shows it where people look for it:

- **Linux:** MPRIS, which the panel's player widget, the lock screen,
  `playerctl` and KDE Connect read.
- **Windows:** the media overlay (the System Media Transport Controls), with
  the cover, which outlives the app's window closing to the tray.
- **macOS:** the Now Playing panel and Control Centre.

The keyboard's media keys, a headset's buttons and those panels send
commands back: play, pause, play/pause, stop, next, previous, seek by or to
a position, volume, shuffle, repeat, open a URI, raise the window, quit.

It came out of Spotifast, which had all three platforms, with the extra
track details and per-moment controls Solco publishes.

## Usage

```rust
use fastframe_now_playing::{App, Command, NowPlaying, Playback, State, Track};

// On the main thread: macOS delivers the controls' events there.
let mut app = App::new("example", "Example");
app.uri_schemes = vec!["example".into()];
let mut controls = NowPlaying::start(app, move || waker.wake());

// Every frame, or whenever something changes:
for command in controls.commands() {
    match command {
        Command::PlayPause => player.toggle(),
        Command::SeekBy(ms) => player.seek_by(ms),
        Command::Raise => show_window(),
        _ => {}
    }
}
controls.update(State {
    playback: Playback::Playing,
    position: player.position(),
    track: Some(Track {
        id: track.id.clone(),
        title: track.title.clone(),
        artists: track.artists.clone(),
        album: track.album.clone(),
        duration: Some(track.duration),
        art_file: art_cache.file_for(&track), // the cover, on disk
        ..Track::default()
    }),
    volume: Some(player.volume()),
    ..State::default()
});
// After a seek, so clients jump rather than glide:
controls.seeked(player.position());
```

`update` is cheap to call every frame: a change goes out at once, and the
position alone at most once a second while playing, since every client
interpolates between updates. The controls run on a thread of their own,
so a slow or missing session bus never holds up audio or the window.

## Artwork

Give the cover as `art_file`, an image on disk. Every platform shows it.
Windows and macOS show only a file: they load the image themselves, and
macOS aborts the process when a remote image it was handed fails to load.
`art_url` is the cover's web address, which MPRIS shows while the file is
not there yet; set `art_file` when it lands and the controls update.

## What each platform shows

| | Linux (MPRIS) | Windows | macOS |
| --- | --- | --- | --- |
| Title, artists, album, length, cover | yes | yes | yes |
| Genres, BPM, rating, the track's URL | yes | | |
| Volume, shuffle, repeat (when set) | yes | | |
| Which controls are available now | yes | | |

`State::volume`, `shuffle` and `repeat` are `Option`s: leave them `None` for
an app without them. `Controls` says what is available at the moment (no
next track, nothing to seek in).

## Platform notes

- **Linux:** the player appears as `org.mpris.MediaPlayer2.<bus_name>`.
  Inside a Flatpak, the desktop entry is the sandbox's app id. The track id a
  `SetPosition` names is the app's own `Track::id`, whatever its shape.
- **Windows:** the controls belong to a hidden window on their own thread,
  with a message loop.
- **macOS:** start the controls on the main thread. macOS sends the media
  keys only to the app that played last; `claim` makes a track remembered
  from the last run take them before anything plays.
