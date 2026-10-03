//! Audio output for egui apps: one device stream that costs nothing while
//! paused, follows the default output, and reopens after a failure, with the
//! app's own renderer (a rodio mixer, a kira renderer, a decoder) filling it.
//!
//! ```no_run
//! use fastframe_audio::{Output, OutputOptions, Render};
//!
//! struct Tone { phase: f32, step: f32, channels: usize }
//!
//! impl Render for Tone {
//!     fn configure(&mut self, sample_rate: u32, channels: u16) {
//!         self.step = 440.0 * std::f32::consts::TAU / sample_rate as f32;
//!         self.channels = usize::from(channels);
//!     }
//!     fn render(&mut self, out: &mut [f32]) {
//!         for frame in out.chunks_mut(self.channels) {
//!             self.phase = (self.phase + self.step) % std::f32::consts::TAU;
//!             frame.fill(self.phase.sin() * 0.1);
//!         }
//!     }
//! }
//!
//! // Opening asks the device, which can take a moment: off the UI thread.
//! let mut output = Output::open(
//!     OutputOptions::default(),
//!     Tone { phase: 0.0, step: 0.0, channels: 2 },
//! )?;
//! let clock = output.clock(); // for any thread
//!
//! // Pausing stops the device asking for sound: no callbacks, no CPU.
//! output.pause();
//! output.resume();
//!
//! // Now and then, on the thread that owns the output:
//! match output.maintain() {
//!     fastframe_audio::Maintained::Reopened { sample_rate, .. } => {
//!         // Another device, perhaps another rate: resample for it.
//!         let _ = sample_rate;
//!     }
//!     fastframe_audio::Maintained::Failed(error) => eprintln!("no audio: {error}"),
//!     _ => {}
//! }
//! for error in output.take_errors() {
//!     if error.is_fatal() { /* fail what is playing */ }
//! }
//! let _played = clock.played();
//! # Ok::<(), fastframe_audio::OpenError>(())
//! ```

mod clock;
mod plan;
mod stream;

use std::fmt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub use clock::Clock;

use stream::{DefaultWatch, Live, Shared};

/// What fills the output with sound. It runs on the audio thread.
///
/// The output keeps the same renderer across pauses and reopens, so what is
/// playing carries on from the same sample. The app reaches it through its
/// own handles (rodio's mixer, kira's manager, a channel), never through the
/// output, so nothing the app does can make the callback wait.
pub trait Render: Send + 'static {
    /// The stream's sample rate and channel count. Called before the first
    /// [`render`](Self::render), and again whenever the output opens a stream,
    /// which can be at another rate or channel count after a reopen. It may
    /// be called more than once while a stream is being opened; the last call
    /// is the one that holds.
    fn configure(&mut self, sample_rate: u32, channels: u16);

    /// Fills `out` with interleaved samples, in the channel count from
    /// [`configure`](Self::configure). Write silence (zeros) when there is
    /// nothing to play. Must not block.
    fn render(&mut self, out: &mut [f32]);
}

/// Which output device to open. Only this one is ever opened; other devices
/// are at most listed by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Device {
    /// The system's default output.
    #[default]
    Default,
    /// A device by the name [`output_device_names`] gives. When it is not
    /// there, the default output plays instead, and a warning is logged.
    Named(String),
}

/// A buffer size, in frames or as a duration at the stream's rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferSize {
    /// Frames, whatever the sample rate.
    Frames(u32),
    /// A duration, turned into frames at the stream's sample rate.
    Duration(Duration),
}

/// The device's buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Buffer {
    /// The driver's own size.
    #[default]
    Driver,
    /// A fixed size on every platform, asked for as given and clamped to
    /// the range the device reports only if it refuses that. PulseAudio
    /// otherwise targets about two seconds.
    Fixed(BufferSize),
    /// A fixed size on Windows, where shared-mode output underruns with the
    /// driver's; the driver's own elsewhere.
    FixedOnWindows(BufferSize),
}

/// How to open an [`Output`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputOptions {
    /// The device.
    pub device: Device,
    /// Channels to ask for; `0` takes the device's own.
    pub channels: u16,
    /// A sample rate to try first; `None` takes the device's default rate.
    pub sample_rate: Option<u32>,
    /// The device's buffer.
    pub buffer: Buffer,
    /// Calls [`Render::render`] in blocks of at most this many frames,
    /// whatever the device's buffer, so a renderer's clock moves in small
    /// steps.
    pub max_block_frames: Option<u32>,
    /// With [`Device::Default`], move to the new default output when it
    /// changes (checked every two seconds on macOS and Windows; PipeWire and
    /// PulseAudio move the stream themselves).
    pub follow_default: bool,
    /// Let the device go after the output has been paused this long; the next
    /// [`Output::resume`] opens it again. The device is let go by
    /// [`Output::maintain`], so an app whose audio thread sleeps while paused
    /// has to call it now and then during the pause (every few seconds is
    /// enough), or the device stays open, still costing nothing.
    pub release_after: Option<Duration>,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            device: Device::Default,
            channels: 2,
            sample_rate: None,
            buffer: Buffer::Driver,
            max_block_frames: None,
            follow_default: true,
            release_after: Some(Duration::from_secs(5 * 60)),
        }
    }
}

/// How the app should treat an [`OutputError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// What is playing cannot carry on: the audio system is missing, access
    /// was denied, a resource ran out, or the configuration or operation is
    /// not supported. The output still reopens.
    Fatal,
    /// Worth logging: a glitch, a busy or vanished device the output reopens,
    /// a rerouted stream, refused real-time priority, or what the backend
    /// could not classify.
    Recoverable,
}

/// An error from the audio device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputError {
    kind: ErrorKind,
    message: String,
}

impl OutputError {
    fn from_cpal(error: &cpal::Error) -> Self {
        Self {
            kind: plan::kind_of(error.kind()),
            message: error.to_string(),
        }
    }

    /// How to treat it.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Whether what is playing has to stop.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        self.kind == ErrorKind::Fatal
    }
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OutputError {}

/// Why an output could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// There is no output device.
    NoDevice,
    /// The device refused.
    Device(OutputError),
}

impl From<cpal::Error> for OpenError {
    fn from(error: cpal::Error) -> Self {
        Self::Device(OutputError::from_cpal(&error))
    }
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => f.write_str("There is no audio output device."),
            Self::Device(error) => write!(f, "Cannot open the audio output: {error}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Why the output opened its stream again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The stream failed or its device went away.
    Failed,
    /// The default output changed, and [`OutputOptions::follow_default`] is on.
    DefaultChanged,
    /// [`Output::resume`] took back a device let go after a long pause.
    Resumed,
}

/// What [`Output::maintain`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Maintained {
    /// Nothing changed.
    Unchanged,
    /// A new stream is playing, possibly on another device, at another rate
    /// or with other channels; [`Render::configure`] has been told.
    Reopened {
        /// The device's name.
        device: String,
        /// The new stream's sample rate.
        sample_rate: u32,
        /// The new stream's channel count.
        channels: u16,
        /// Why.
        reason: Reason,
    },
    /// The device was let go: paused past
    /// [`OutputOptions::release_after`], or failed while paused. The next
    /// [`Output::resume`] opens it again.
    Released,
    /// The stream had to be opened again and could not be. Nothing is
    /// playing; the next `maintain` or `resume` tries again.
    Failed(OpenError),
}

/// An audio output: one stream on one device, with the app's renderer.
///
/// Keep one for as long as the app may play. Pausing costs nothing, and
/// [`maintain`](Self::maintain) keeps it on a working device.
pub struct Output<R: Render> {
    options: OutputOptions,
    renderer: Arc<Mutex<R>>,
    live: Option<Live>,
    shared: Arc<Shared>,
    clock: Clock,
    paused: bool,
    paused_at: Option<Instant>,
    /// A reopen [`resume`](Self::resume) made, for the next `maintain` to report.
    resumed: bool,
    watch: Option<DefaultWatch>,
    device: String,
    sample_rate: u32,
    channels: u16,
}

impl<R: Render> fmt::Debug for Output<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Output")
            .field("device", &self.device)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("paused", &self.paused)
            .field("open", &self.live.is_some())
            .finish_non_exhaustive()
    }
}

impl<R: Render> Output<R> {
    /// Opens the device and starts playing through `renderer`.
    ///
    /// Asking the device can take a perceptible time, so call it off the UI
    /// thread. A failure is not remembered: the next `open` tries again.
    pub fn open(options: OutputOptions, renderer: R) -> Result<Self, OpenError> {
        let renderer = Arc::new(Mutex::new(renderer));
        let shared = Arc::new(Shared::default());
        let clock = Clock::default();
        let live = stream::open(&options, &renderer, &clock, &shared, false)?;
        let watch = (options.follow_default
            && options.device == Device::Default
            && cfg!(any(target_os = "macos", windows)))
        .then(|| DefaultWatch::start(live.default_at_open.clone()));
        Ok(Self {
            device: live.device.clone(),
            sample_rate: live.sample_rate,
            channels: live.channels,
            options,
            renderer,
            live: Some(live),
            shared,
            clock,
            paused: false,
            paused_at: None,
            resumed: false,
            watch,
        })
    }

    /// Stops the device asking for sound, so the callback and the renderer
    /// cost nothing, and keeps the device open for an instant
    /// [`resume`](Self::resume). Never fails: a stream that cannot pause keeps
    /// running.
    pub fn pause(&mut self) {
        if self.paused {
            return;
        }
        self.paused = true;
        self.paused_at = Some(Instant::now());
        if let Some(live) = &self.live {
            use cpal::traits::StreamTrait;
            if let Err(error) = live.stream.pause() {
                log::warn!("cannot pause the audio output: {error}");
                self.shared.report(error);
            }
        }
    }

    /// Has the device ask for sound again, opening it again if it was let go.
    /// Never fails: a device that will not start is reported by the next
    /// [`maintain`](Self::maintain), as is a reopen, with the new rate.
    pub fn resume(&mut self) {
        if !self.paused {
            return;
        }
        self.paused = false;
        self.paused_at = None;
        match &self.live {
            Some(live) => {
                use cpal::traits::StreamTrait;
                if let Err(error) = live.stream.play() {
                    log::warn!("cannot restart the audio output: {error}");
                    self.shared.report(error);
                }
            }
            None => {
                if self.reopen().is_ok() {
                    self.resumed = true;
                }
            }
        }
    }

    /// Whether the output is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Whether the stream has failed or could not be opened again. Cheap
    /// enough to ask before every write; [`maintain`](Self::maintain) repairs it.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire) || (self.live.is_none() && !self.paused)
    }

    /// Keeps the output on a working device. Call it now and then on the
    /// thread that owns the output (each frame, or before writing): it is
    /// cheap when nothing changed.
    ///
    /// It reopens a stream that failed or whose device went away, moves to a
    /// new default output, and lets the device go after a long pause. A
    /// failed reopen is not remembered: the next call tries again. Nothing
    /// happens between calls, so keep calling it while paused for
    /// [`OutputOptions::release_after`] to take effect.
    pub fn maintain(&mut self) -> Maintained {
        if std::mem::take(&mut self.resumed) {
            return self.reopened(Reason::Resumed);
        }
        if self.paused
            && self.live.is_some()
            && plan::should_release(self.paused_at, Instant::now(), self.options.release_after)
        {
            log::info!(
                "audio output: letting {} go after a long pause",
                self.device
            );
            self.live = None;
            return Maintained::Released;
        }
        let reason = if self.shared.failed.load(Ordering::Acquire) {
            Some(Reason::Failed)
        } else if self.default_changed() {
            Some(Reason::DefaultChanged)
        } else if self.live.is_none() && !self.paused {
            Some(Reason::Failed)
        } else {
            None
        };
        let Some(reason) = reason else {
            return Maintained::Unchanged;
        };
        if self.paused {
            // Nothing is playing: let the device go, and open the right one
            // on resume.
            self.live = None;
            self.shared.failed.store(false, Ordering::Release);
            return Maintained::Released;
        }
        match self.reopen() {
            Ok(()) => self.reopened(reason),
            Err(error) => Maintained::Failed(error),
        }
    }

    /// The errors the device reported since the last call, fatal and
    /// recoverable alike. A fatal one is still reported when the output has
    /// already reopened.
    pub fn take_errors(&mut self) -> Vec<OutputError> {
        std::mem::take(
            &mut *self
                .shared
                .errors
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// How long the output has played, readable from any thread.
    #[must_use]
    pub fn clock(&self) -> Clock {
        self.clock.clone()
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device
    }

    /// The stream's sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The stream's channel count.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    fn default_changed(&self) -> bool {
        let (Some(watch), Some(live)) = (&self.watch, &self.live) else {
            return false;
        };
        match (watch.name(), &live.default_at_open) {
            (Some(now), Some(then)) => &now != then,
            _ => false,
        }
    }

    /// Closes the stream, if any, and opens the one the options ask for.
    fn reopen(&mut self) -> Result<(), OpenError> {
        // Let the old device go first: some only take one stream.
        self.live = None;
        match stream::open(
            &self.options,
            &self.renderer,
            &self.clock,
            &self.shared,
            self.paused,
        ) {
            Ok(live) => {
                self.device = live.device.clone();
                self.sample_rate = live.sample_rate;
                self.channels = live.channels;
                self.live = Some(live);
                Ok(())
            }
            Err(error) => {
                log::warn!("audio output: {error}");
                Err(error)
            }
        }
    }

    fn reopened(&self, reason: Reason) -> Maintained {
        Maintained::Reopened {
            device: self.device.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            reason,
        }
    }
}

/// The output devices' names, for a device picker. Nothing is opened, so a
/// device another program or a USB peripheral is using is left alone.
pub fn output_device_names() -> Result<Vec<String>, OpenError> {
    stream::output_device_names()
}
