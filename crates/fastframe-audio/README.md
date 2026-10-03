# fastframe-audio

Audio output for egui apps: one device stream that costs nothing while
paused, follows the default output, and reopens after a failure. The app's
own renderer fills it: a rodio mixer, a kira renderer, or a decoder of its
own.

Spotifast found that rodio's output stream keeps the device asking for
sound until it is dropped, so a paused player still ran the audio callback
and mixed silence many times a second, about 1% of a core on a Mac
(crmne/spotifast#636). This crate opens the cpal stream itself and pauses
it, so a paused output gets no callbacks at all and resumes in
microseconds. It came out of Spotifast's output, with what Solco and ZapFast
need from theirs.

## Usage

```rust
use fastframe_audio::{Maintained, Output, OutputOptions, Render};

struct Player { /* the app's mixer, decoder or kira renderer */ }

impl Render for Player {
    fn configure(&mut self, sample_rate: u32, channels: u16) {
        // Called before the first render, and again after any reopen, which
        // can be at another rate or channel count.
    }
    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0); // interleaved; silence when there is nothing to play
    }
}

// Asking the device can take a moment: open it off the UI thread.
let mut output = Output::open(OutputOptions::default(), Player { /* .. */ })?;
let clock = output.clock(); // Send + Sync, for any thread

output.pause();  // no callbacks, no CPU, the device stays open
output.resume(); // plays again at once

// Now and then on the thread that owns the output:
if let Maintained::Reopened { sample_rate, .. } = output.maintain() {
    // Another device, perhaps another rate.
}
for error in output.take_errors() {
    if error.is_fatal() { /* fail what is playing */ }
}
```

The renderer runs on the audio thread and must not block. The app reaches it
through its own handles (rodio's mixer, kira's manager, a channel), never
through the output, so nothing the app does can make the callback wait. It
stays the same across pauses and reopens, so what is playing carries on from
the same sample.

## Options

`OutputOptions` (all have defaults):

- `device`: `Device::Default`, or `Device::Named(name)` from
  `output_device_names()`. Only that device is ever opened; a named device
  that is missing falls back to the default, with a warning in the log.
- `channels`: `2` by default; `0` takes the device's own.
- `sample_rate`: a rate to try first, such as Spotify's 44.1 kHz; by default
  the device's own rate.
- `buffer`: the driver's own size, `Buffer::Fixed(size)` everywhere (PulseAudio
  otherwise targets about two seconds), or `Buffer::FixedOnWindows(size)`,
  for Windows' shared-mode underruns. A size is `BufferSize::Frames(n)` or a
  `BufferSize::Duration`, clamped to the range the device reports.
- `max_block_frames`: render in blocks of at most this many frames, whatever
  the device's buffer, so a renderer's own clock moves in small steps.
- `follow_default`: move to a new default output (on macOS and Windows,
  checked every two seconds; PipeWire and PulseAudio move the stream
  themselves). On by default.
- `release_after`: let the device go after a pause this long (five minutes by
  default); the next `resume()` opens it again, and `maintain()` then reports
  `Reopened`.

The output opens at the preferred rate, then at the device's own, each with
the fixed buffer when one was asked for, then with the driver's buffer, then
in any configuration the device lists. Every cpal sample format is filled
from the renderer's `f32`.

## Keeping it working

`maintain()` is cheap when nothing changed. It reopens a stream that failed
or whose device went away, moves to a new default output, and lets the device
go after a long pause. It returns what it did:

- `Unchanged`;
- `Reopened { device, sample_rate, channels, reason }`, with `reason` one of
  `Failed`, `DefaultChanged` or `Resumed` (a reopen `resume()` made after an
  idle release, reported by the next `maintain()`);
- `Released`;
- `Failed(error)`. A failed reopen is not remembered: the next `maintain()`
  or `resume()` tries again.

`failed()` is a cheap check for a write path. `pause()` and `resume()` never
fail; a device that will not start shows up through `maintain()`.

## Errors

`take_errors()` drains what the device reported, each an `OutputError` whose
`kind()` is `Fatal` (the audio system is missing, access was denied, a
resource ran out, the configuration or operation is not supported) or
`Recoverable` (a glitch, a busy or vanished device, a rerouted stream,
refused real-time priority, an unclassified backend error). A fatal error is
reported even when the output has already reopened, so the app can fail what
was playing and let the next play use the reopened device.

## The clock

`Clock::played()` is how long the output has played: frames rendered at the
stream's rate, less the latency the device reports, so it counts what has
reached the device. It never goes backwards, stands still while paused, and
carries on across reopens (another device, another rate) without jumping.
Take differences between readings to follow a clip. `Clock::latency()` is the
latency itself, for lining up a picture with the sound.

## Features

- `pulseaudio`: cpal's PulseAudio backend (pure Rust).
- `pipewire`: cpal's PipeWire backend, which needs libpipewire's headers and
  clang to build.

On Linux, cpal uses PipeWire, then PulseAudio, then ALSA, whichever is
enabled and running; ALSA's headers (`libasound2-dev`, `alsa-lib`) are needed
in every case.

## Example

`cargo run -p fastframe-audio --example idle` opens the default output with
a renderer that writes silence, and prints the render calls and the clock
while playing, paused and resumed. On Linux, a paused output got no calls on
ALSA, PulseAudio and PipeWire alike.

## Not in this crate

Decoding, resampling, mixing, fades and visualiser taps belong to the
renderer, where rodio and kira already do them. Recording, and pausing other
media players while playing, stay in the app that needs them until a second
app does.
