//! Opens the default output with a renderer that writes silence, and shows
//! that a paused output gets no callbacks: `cargo run -p fastframe-audio
//! --example idle`. Nothing is audible.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fastframe_audio::{Output, OutputOptions, Render};

struct Silence(Arc<AtomicU64>);

impl Render for Silence {
    fn configure(&mut self, _sample_rate: u32, _channels: u16) {}
    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[allow(clippy::print_stdout)]
fn main() -> Result<(), fastframe_audio::OpenError> {
    let calls = Arc::new(AtomicU64::new(0));
    let options = OutputOptions {
        // As Solco opens it: 512 frames, rendered in blocks of at most 512.
        buffer: fastframe_audio::Buffer::Fixed(fastframe_audio::BufferSize::Frames(512)),
        max_block_frames: Some(512),
        ..OutputOptions::default()
    };
    let mut output = Output::open(options, Silence(Arc::clone(&calls)))?;
    let clock = output.clock();
    println!(
        "{} at {} Hz, {} channels",
        output.device_name(),
        output.sample_rate(),
        output.channels()
    );

    let report = |what: &str| {
        let before = (calls.load(Ordering::Relaxed), clock.played());
        std::thread::sleep(Duration::from_secs(1));
        let after = (calls.load(Ordering::Relaxed), clock.played());
        println!(
            "{what}: {} render calls, played {:?} -> {:?}, latency {:?}",
            after.0 - before.0,
            before.1,
            after.1,
            clock.latency()
        );
    };
    report("playing");
    output.pause();
    std::thread::sleep(Duration::from_millis(200));
    report("paused");
    output.resume();
    report("resumed");
    println!(
        "maintain: {:?}, errors: {:?}",
        output.maintain(),
        output.take_errors()
    );
    Ok(())
}
