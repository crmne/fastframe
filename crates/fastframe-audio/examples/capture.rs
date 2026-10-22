//! Opens the default input and shows what it captures, without keeping any
//! of it: `cargo run -p fastframe-audio --example capture`. The samples are
//! only measured (how many, and the loudest) and then dropped; nothing is
//! saved or sent.
//!
//! It captures for a second, pauses for a second to show that a paused input
//! gets no callbacks, and captures for another second. `--device <id>` opens
//! an input by an id from the `devices` example instead of the default,
//! `--name <name>` by its name, and `--channels <n>` asks for that many
//! channels instead of the device's own. The loudest sample is shown for
//! each channel (the first eight). `--watch <seconds>` instead captures for
//! that long, reporting each second and calling `maintain()` before each
//! report, to show the input following a new default.
//!
//! `--app <name>`, `--icon <icon>`, `--media-name <name>` and `--role <role>`
//! name it in the system's volume mixer.
//!
//! It also says how long after `Input::open` returned the first block, and
//! the first block that is not all silence, reached the capturer: the sound
//! an app recording from the moment it opens would lose.

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use fastframe_audio::{Capture, Device, DeviceId, Input, InputOptions};

#[derive(Default)]
struct Stats {
    calls: AtomicU64,
    samples: AtomicU64,
    /// The channels the capturer was configured with.
    channels: AtomicU16,
    /// The loudest sample of each channel since the last report, as `f32`
    /// bits.
    peaks: [AtomicU32; 8],
    /// When the first block, and the first block that is not all silence,
    /// came, in nanoseconds after [`Meter::since`] plus one (`0` until then).
    first_block: AtomicU64,
    first_sound: AtomicU64,
}

struct Meter {
    stats: Arc<Stats>,
    /// Taken just before `Input::open`.
    since: Instant,
}

impl Meter {
    fn mark(&self, first: &AtomicU64) {
        if first.load(Ordering::Relaxed) == 0 {
            let nanos = u64::try_from(self.since.elapsed().as_nanos()).unwrap_or(u64::MAX);
            first.store(nanos.saturating_add(1), Ordering::Relaxed);
        }
    }
}

impl Capture for Meter {
    fn configure(&mut self, _sample_rate: u32, channels: u16) {
        self.stats.channels.store(channels, Ordering::Relaxed);
    }

    fn capture(&mut self, input: &[f32]) {
        self.mark(&self.stats.first_block);
        if input.iter().any(|sample| *sample != 0.0) {
            self.mark(&self.stats.first_sound);
        }
        let channels = usize::from(self.stats.channels.load(Ordering::Relaxed)).max(1);
        for (channel, peak) in self.stats.peaks.iter().enumerate().take(channels) {
            let loudest = input
                .iter()
                .skip(channel)
                .step_by(channels)
                .fold(0.0_f32, |loudest, sample| loudest.max(sample.abs()));
            peak.fetch_max(loudest.to_bits(), Ordering::Relaxed);
        }
        self.stats
            .samples
            .fetch_add(input.len() as u64, Ordering::Relaxed);
        self.stats.calls.fetch_add(1, Ordering::Relaxed);
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
    let device = args
        .iter()
        .position(|arg| arg == "--device")
        .and_then(|at| args.get(at + 1))
        .map(|id| Device::Id(DeviceId::new(id.as_str())))
        .or_else(|| {
            args.iter()
                .position(|arg| arg == "--name")
                .and_then(|at| args.get(at + 1))
                .map(|name| Device::Named(name.clone()))
        })
        .unwrap_or(Device::Default);
    let channels = args
        .iter()
        .position(|arg| arg == "--channels")
        .and_then(|at| args.get(at + 1))
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);

    let stats = Arc::new(Stats::default());
    let opening = Instant::now();
    let meter = Meter {
        stats: Arc::clone(&stats),
        since: opening,
    };
    let (media_name, role) = naming(&args);
    let mut input = Input::open(
        InputOptions {
            device,
            channels,
            media_name,
            role,
            ..InputOptions::default()
        },
        meter,
    )?;
    let opened = opening.elapsed();
    let clock = input.clock();
    println!("opened in {opened:?}");
    println!(
        "{} ({}) at {} Hz, {} channels, through {}",
        input.device_name(),
        input.device_id(),
        input.sample_rate(),
        input.channels(),
        fastframe_audio::host()
    );

    let report = |what: &str| {
        for peak in &stats.peaks {
            peak.store(0, Ordering::Relaxed);
        }
        let before = (
            stats.calls.load(Ordering::Relaxed),
            stats.samples.load(Ordering::Relaxed),
            clock.played(),
        );
        std::thread::sleep(Duration::from_secs(1));
        let after = (
            stats.calls.load(Ordering::Relaxed),
            stats.samples.load(Ordering::Relaxed),
            clock.played(),
        );
        let channels = usize::from(stats.channels.load(Ordering::Relaxed)).clamp(1, 8);
        let level = stats.peaks[..channels]
            .iter()
            .map(|peak| {
                let peak = f32::from_bits(peak.load(Ordering::Relaxed));
                if peak > 0.0 {
                    format!("{:.1} dBFS", 20.0 * peak.log10())
                } else {
                    "silence".to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" | ");
        println!(
            "{what}: {} callbacks, {} samples, loudest {level}, captured {:?} -> {:?}, latency {:?}",
            after.0 - before.0,
            after.1 - before.1,
            before.2,
            after.2,
            clock.latency()
        );
    };
    // How long after `open` returned a first came, or that none has.
    let after_open = |first: &AtomicU64| match first.load(Ordering::Relaxed) {
        0 => "none yet".to_owned(),
        nanos => format!(
            "{:?} after open returned",
            Duration::from_nanos(nanos - 1).saturating_sub(opened)
        ),
    };
    let firsts = || {
        println!(
            "first block {}, first sound {}",
            after_open(&stats.first_block),
            after_open(&stats.first_sound)
        );
    };
    let watch = args
        .iter()
        .position(|arg| arg == "--watch")
        .and_then(|at| args.get(at + 1))
        .and_then(|seconds| seconds.parse::<u32>().ok());
    if let Some(seconds) = watch {
        for _ in 0..seconds {
            match input.maintain() {
                fastframe_audio::Maintained::Unchanged => {}
                maintained => println!(
                    "maintain: {maintained:?}, now {} at {} channels",
                    input.device_id(),
                    input.channels()
                ),
            }
            report("capturing");
        }
        firsts();
        return Ok(());
    }
    report("capturing");
    firsts();
    input.pause();
    std::thread::sleep(Duration::from_millis(200));
    report("paused");
    input.resume();
    report("resumed");
    println!(
        "maintain: {:?}, errors: {:?}, overruns: {}",
        input.maintain(),
        input.take_errors(),
        clock.underruns()
    );
    Ok(())
}
