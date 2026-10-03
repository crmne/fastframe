//! Publishes a made-up track, with a cover if one is named, and prints the
//! commands the desktop sends: `cargo run -p fastframe-now-playing --example
//! controls -- [cover.png] [seconds]`. Try the media keys, `playerctl`, or the
//! system's media panel while it runs.

use std::time::{Duration, Instant};

use fastframe_now_playing::{App, Command, NowPlaying, Playback, State, Track};

#[allow(clippy::print_stdout)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cover = args.first().filter(|arg| !arg.is_empty()).map(Into::into);
    let seconds = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut controls = NowPlaying::start(App::new("fastframedemo", "fastframe demo"), || {});
    let mut state = State {
        playback: Playback::Playing,
        track: Some(Track {
            id: "demo:1".into(),
            title: "A Made-Up Song".into(),
            artists: vec!["The Examples".into()],
            album: "Demonstration".into(),
            duration: Some(Duration::from_secs(200)),
            art_file: cover,
            genres: vec!["Techno".into()],
            bpm: Some(124.0),
            ..Track::default()
        }),
        volume: Some(0.5),
        shuffle: Some(false),
        ..State::default()
    };
    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        for command in controls.commands() {
            println!("command: {command:?}");
            match command {
                Command::PlayPause => {
                    state.playback = match state.playback {
                        Playback::Playing => Playback::Paused,
                        _ => Playback::Playing,
                    };
                }
                Command::Pause => state.playback = Playback::Paused,
                Command::Play => state.playback = Playback::Playing,
                Command::SetVolume(volume) => state.volume = Some(volume),
                Command::SeekBy(ms) => {
                    let at = state.position.as_millis() as i64 + ms;
                    state.position = Duration::from_millis(at.max(0) as u64);
                    controls.seeked(state.position);
                }
                _ => {}
            }
        }
        if state.playback == Playback::Playing {
            state.position += last.elapsed();
        }
        last = Instant::now();
        controls.update(state.clone());
        std::thread::sleep(Duration::from_millis(50));
    }
}
