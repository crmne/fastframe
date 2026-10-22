//! Opens the default output with a renderer that writes silence, and shows
//! that a paused output gets no callbacks: `cargo run -p fastframe-audio
//! --example idle`. Nothing is audible.
//!
//! By default it opens the output as Solco does (512 frames, rendered in
//! blocks of at most 512). `--buffer-ms 100` asks for a fixed 100 ms buffer
//! instead, as Spotifast does on Windows, without blocks, so the largest
//! callback shows the buffer the device gave. `--device <id>` opens a
//! device by an id from the `devices` example instead of the default.
//! `--idle-ms <ms>` also shows `pause_when_idle`: the renderer says it has
//! nothing to play, and the output pauses itself after that long.
//! `--app <name>`, `--icon <icon>`, `--media-name <name>` and `--role <role>`
//! name it in the system's volume mixer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use fastframe_audio::{Buffer, BufferSize, Device, DeviceId, Output, OutputOptions, Render};

#[derive(Default)]
struct Stats {
    calls: AtomicU64,
    /// The most frames one render call was asked for.
    max_frames: AtomicU64,
}

struct Silence {
    stats: Arc<Stats>,
    channels: u64,
}

impl Render for Silence {
    fn configure(&mut self, _sample_rate: u32, channels: u16) {
        self.channels = u64::from(channels.max(1));
    }
    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() as u64 / self.channels;
        self.stats.max_frames.fetch_max(frames, Ordering::Relaxed);
        self.stats.calls.fetch_add(1, Ordering::Relaxed);
    }
    fn idle(&self) -> bool {
        // Silence and nothing queued: idle, which matters only with
        // `pause_when_idle`.
        true
    }
}

/// `--app <name>` (with `--icon <icon>`) names the app in the volume mixer,
/// `--media-name <name>` the stream, and `--role <role>` (`music`, `movie`,
/// `game`, `communication`, `notification`, `production` or
/// `accessibility`) says what it carries.
fn naming(args: &[String]) -> (Option<String>, Option<fastframe_audio::StreamRole>) {
    use fastframe_audio::StreamRole;
    let value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    if let Some(name) = value("--app") {
        let _ = fastframe_audio::set_app_info(fastframe_audio::AppInfo {
            name,
            icon: value("--icon"),
        });
    }
    let role = value("--role").and_then(|role| match role.as_str() {
        "music" => Some(StreamRole::Music),
        "movie" => Some(StreamRole::Movie),
        "game" => Some(StreamRole::Game),
        "communication" => Some(StreamRole::Communication),
        "notification" => Some(StreamRole::Notification),
        "production" => Some(StreamRole::Production),
        "accessibility" => Some(StreamRole::Accessibility),
        _ => None,
    });
    (value("--media-name"), role)
}

#[allow(clippy::print_stdout)]
fn main() -> Result<(), fastframe_audio::OpenError> {
    let args: Vec<String> = std::env::args().collect();
    let buffer_ms = args
        .iter()
        .position(|arg| arg == "--buffer-ms")
        .and_then(|at| args.get(at + 1))
        .and_then(|ms| ms.parse::<u64>().ok());
    let device = args
        .iter()
        .position(|arg| arg == "--device")
        .and_then(|at| args.get(at + 1))
        .map_or(Device::Default, |id| Device::Id(DeviceId::new(id.as_str())));
    let idle_ms = args
        .iter()
        .position(|arg| arg == "--idle-ms")
        .and_then(|at| args.get(at + 1))
        .and_then(|ms| ms.parse::<u64>().ok());
    let mut options = match buffer_ms {
        Some(ms) => OutputOptions {
            device,
            buffer: Buffer::Fixed(BufferSize::Duration(Duration::from_millis(ms))),
            ..OutputOptions::default()
        },
        None => OutputOptions {
            device,
            buffer: Buffer::Fixed(BufferSize::Frames(512)),
            max_block_frames: Some(512),
            ..OutputOptions::default()
        },
    };

    options.pause_when_idle = idle_ms.map(Duration::from_millis);
    (options.media_name, options.role) = naming(&args);

    let stats = Arc::new(Stats::default());
    let renderer = Silence {
        stats: Arc::clone(&stats),
        channels: 1,
    };
    let opening = Instant::now();
    let mut output = Output::open(options, renderer)?;
    let opened = opening.elapsed();
    let clock = output.clock();
    let asking = Instant::now();
    let id = output.device_id().clone();
    println!(
        "opened in {opened:?}; device_id() took {:?}",
        asking.elapsed()
    );
    println!(
        "{} ({id}) at {} Hz, {} channels, through {}",
        output.device_name(),
        output.sample_rate(),
        output.channels(),
        fastframe_audio::host()
    );

    let report = |what: &str| {
        stats.max_frames.store(0, Ordering::Relaxed);
        let before = (stats.calls.load(Ordering::Relaxed), clock.played());
        std::thread::sleep(Duration::from_secs(1));
        let after = (stats.calls.load(Ordering::Relaxed), clock.played());
        println!(
            "{what}: {} render calls of at most {} frames, played {:?} -> {:?}, latency {:?}",
            after.0 - before.0,
            stats.max_frames.load(Ordering::Relaxed),
            before.1,
            after.1,
            clock.latency()
        );
    };
    report("playing");
    output.pause();
    std::thread::sleep(Duration::from_millis(200));
    report("paused");

    let calls = stats.calls.load(Ordering::Relaxed);
    let start = Instant::now();
    output.resume();
    let resumed = start.elapsed();
    while stats.calls.load(Ordering::Relaxed) == calls && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_micros(200));
    }
    println!(
        "resume() took {resumed:?}; the first callback came {:?} after it",
        start.elapsed()
    );
    report("resumed");
    println!(
        "maintain: {:?}, errors: {:?}",
        output.maintain(),
        output.take_errors()
    );
    if let Some(ms) = idle_ms {
        // The renderer has been idle all along: the next looks pause it.
        let start = Instant::now();
        while !output.is_paused() && start.elapsed() < Duration::from_millis(ms + 2000) {
            let _ = output.maintain();
            std::thread::sleep(Duration::from_millis(50));
        }
        println!(
            "pause_when_idle {ms} ms: paused {} after {:?}",
            output.is_paused(),
            start.elapsed()
        );
        report("idle");
        output.resume();
        report("resumed after a sound was queued");
    }
    Ok(())
}
