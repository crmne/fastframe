//! Audio output and input for egui apps: one device stream that costs nothing
//! while paused, follows the default device, and reopens after a failure,
//! with the app's own renderer (a rodio mixer, a kira renderer, a decoder)
//! filling an [`Output`], and the app's [`Capture`] taking what an [`Input`]
//! records.
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
//! // On the thread that decodes and queues what the renderer plays, so a
//! // busy machine does not keep it waiting until the queue runs dry.
//! fastframe_audio::raise_feeder_priority();
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

#[cfg(test)]
mod allocations;
mod clock;
mod feeder;
#[cfg(any(target_os = "linux", test))]
mod kept;
#[cfg(feature = "rodio")]
mod mixer;
#[cfg(target_os = "windows")]
mod mmcss;
mod naming;
mod plan;
#[cfg(target_os = "linux")]
mod realtime;
mod session;
mod stream;

use std::fmt;
use std::time::Duration;

pub use clock::Clock;
pub use feeder::raise_feeder_priority;
#[cfg(feature = "rodio")]
pub use mixer::{MixerRender, MixerSlot, NewMixer};
pub use naming::{AppInfo, StreamRole, set_app_info};

use session::Session;
use stream::{Capturing, Rendering, Settings};

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

    /// Whether the renderer has nothing to play: nothing queued, nothing
    /// sounding. Asked on the audio thread after each callback's render
    /// when [`OutputOptions::pause_when_idle`] is set, so it must be cheap
    /// and must not block.
    ///
    /// Answer `true` only when nothing will sound until the app queues more
    /// and calls [`Output::resume`]: an idle output is paused, and a paused
    /// output never renders again by itself. Silence within a sound is not
    /// idle. By default, never.
    fn idle(&self) -> bool {
        false
    }
}

/// What takes the sound an [`Input`] captures. It runs on the audio thread.
///
/// The input keeps the same capturer across pauses and reopens. As with a
/// [`Render`], the app reaches what it captured through its own handles (a
/// ring buffer, a channel), never through the input, so nothing the app does
/// can make the callback wait.
pub trait Capture: Send + 'static {
    /// The stream's sample rate and channel count. Called before the first
    /// [`capture`](Self::capture), and again whenever the input opens a
    /// stream, which can be at another rate or channel count after a reopen.
    /// It may be called more than once while a stream is being opened; the
    /// last call is the one that holds.
    fn configure(&mut self, sample_rate: u32, channels: u16);

    /// Takes interleaved samples, in the channel count from
    /// [`configure`](Self::configure), converted to `f32` from whatever the
    /// device captures. Must not block.
    fn capture(&mut self, input: &[f32]);
}

/// A device's own id, to save in settings and open the same device again
/// later, whatever its name or its place in the list. It names no audio
/// system, so a saved one still reads on another machine, where it simply
/// matches nothing and the default plays.
///
/// What it holds is the system's own id for the device:
///
/// - PipeWire: the node's `node.name`, such as
///   `alsa_output.pci-0000_00_1f.3.analog-stereo`, which is also the sink or
///   source name PulseAudio clients see under `pipewire-pulse`;
/// - PulseAudio: the sink or source name, the same string;
/// - ALSA: the PCM name, such as `hw:CARD=PCH,DEV=0` or `default`;
/// - WASAPI: the endpoint id, `{0.0.0.00000000}.{guid}`;
/// - CoreAudio: the device's UID.
///
/// These stay the same across restarts and reconnections, as long as the
/// device is plugged into the same port (ALSA and PipeWire derive theirs
/// from the card and the port).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    /// An id read back from settings.
    pub fn new(saved: impl Into<String>) -> Self {
        Self(saved.into())
    }

    /// The id as a string, to save.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which device to open. Only this one is ever opened; other devices are at
/// most listed.
///
/// An [`Input`] is opened by id or name only among the devices
/// [`input_devices`] offers, so a name a source shares with a sink (both
/// "Built-in Audio Analog Stereo" on PipeWire) opens the source, and a sink's
/// id never records what the sink plays: the default input opens instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Device {
    /// The system's default device.
    #[default]
    Default,
    /// A device by the id [`output_devices`] or [`input_devices`] gives. When
    /// it is not there, the default device plays instead, and a warning is
    /// logged.
    Id(DeviceId),
    /// An output device by the name [`output_device_names`] gives, or any
    /// device by its [`DeviceInfo::name`]. When it is not there, the default
    /// device plays instead, and a warning is logged. Two devices can share a
    /// name; prefer [`Device::Id`].
    Named(String),
}

/// A device a picker can offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Its id, to save and open it by ([`Device::Id`]).
    pub id: DeviceId,
    /// Its name, to show.
    pub name: String,
    /// Whether it is the system's default device just now.
    pub is_default: bool,
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
    /// The driver's own size. On PulseAudio, whose own target is about two
    /// seconds, 20 ms instead.
    #[default]
    Driver,
    /// A fixed size on every platform, asked for as given and clamped to
    /// the range the device reports only if it refuses that. PulseAudio
    /// otherwise targets about two seconds.
    Fixed(BufferSize),
    /// A fixed size on Windows, where shared-mode output underruns with the
    /// driver's; the driver's own elsewhere, as for [`Buffer::Driver`].
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
    /// Pause the output once [`Render::idle`] has said it has nothing to play
    /// for this long, so a sound effect player that is quiet most of the
    /// time costs nothing between sounds. `None`, the default, never does.
    ///
    /// [`Output::maintain`] makes the pause, so it comes at the first call
    /// after that long; give it longer than the output's latency, so the end
    /// of the last sound has played. From then on the output is paused as by
    /// [`Output::pause`]: [`Output::is_paused`] says so, and after
    /// [`release_after`](Self::release_after) the device is let go. The app
    /// calls [`Output::resume`] whenever it queues a sound: it costs nothing
    /// when the output is playing, and plays at once (or reopens a device
    /// let go) when it is paused. See [`Output`] for handles on other
    /// threads.
    pub pause_when_idle: Option<Duration>,
    /// The stream's name in the system's volume mixer, such as `"Voice
    /// call"`. `None`, the default, takes the app's name from
    /// [`set_app_info`] (and, without that, the system's own). It is given
    /// when the stream opens and stays until it reopens: cpal cannot rename
    /// an open stream, so it is no place for a song title that changes.
    pub media_name: Option<String>,
    /// What the stream carries, which the sound server uses to route it and
    /// to lower other streams (for example during a call). `None`, the
    /// default, gives none.
    pub role: Option<StreamRole>,
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
            pause_when_idle: None,
            media_name: None,
            role: None,
        }
    }
}

impl OutputOptions {
    fn settings(&self) -> Settings {
        Settings {
            device: self.device.clone(),
            channels: self.channels,
            sample_rate: self.sample_rate,
            buffer: self.buffer,
            max_block_frames: self.max_block_frames,
            follow_default: self.follow_default,
            release_after: self.release_after,
            pause_when_idle: self.pause_when_idle,
            media_name: self.media_name.clone(),
            role: self.role,
        }
    }
}

/// How to open an [`Input`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputOptions {
    /// The device, an id from [`input_devices`] or its name.
    pub device: Device,
    /// Channels the capturer takes; `0`, the default, takes the source's
    /// own. The stream always opens at the source's own channels, and other
    /// counts are made from them: a mono source in every channel, many
    /// channels averaged down to mono, and otherwise the channels both have,
    /// with the rest silent. (cpal's PipeWire host would link a stereo
    /// stream to a mono microphone's one channel and leave the other
    /// silent.)
    pub channels: u16,
    /// A sample rate to try first; `None` takes the device's default rate.
    pub sample_rate: Option<u32>,
    /// The device's buffer, as for an output: on PulseAudio, whose own
    /// target is about two seconds, [`Buffer::Driver`] asks for 20 ms.
    pub buffer: Buffer,
    /// With [`Device::Default`], move to the new default input when it
    /// changes (checked every two seconds on macOS and Windows; PipeWire and
    /// PulseAudio move the stream themselves).
    pub follow_default: bool,
    /// The stream's name in the system's volume mixer, such as `"Voice
    /// call"`. `None`, the default, takes the app's name from
    /// [`set_app_info`] (and, without that, the system's own). It is given
    /// when the stream opens and stays until it reopens: cpal cannot rename
    /// an open stream, so it is no place for a song title that changes.
    pub media_name: Option<String>,
    /// What the stream carries, which the sound server uses to route it and
    /// to lower other streams (for example during a call). `None`, the
    /// default, gives none.
    pub role: Option<StreamRole>,
}

impl Default for InputOptions {
    fn default() -> Self {
        Self {
            device: Device::Default,
            channels: 0,
            sample_rate: None,
            buffer: Buffer::Driver,
            follow_default: true,
            media_name: None,
            role: None,
        }
    }
}

impl InputOptions {
    fn settings(&self) -> Settings {
        Settings {
            device: self.device.clone(),
            channels: self.channels,
            sample_rate: self.sample_rate,
            buffer: self.buffer,
            max_block_frames: None,
            follow_default: self.follow_default,
            release_after: None,
            pause_when_idle: None,
            media_name: self.media_name.clone(),
            role: self.role,
        }
    }
}

/// How the app should treat an [`OutputError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// What is playing or recording cannot carry on: the audio system is
    /// missing, access was denied, a resource ran out, or the configuration
    /// or operation is not supported. The stream still reopens.
    Fatal,
    /// Worth logging: a glitch, a busy or vanished device the stream reopens,
    /// a rerouted stream, refused real-time priority, or what the backend
    /// could not classify.
    Recoverable,
}

/// An error from the audio device, of an [`Output`] or an [`Input`].
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

    /// Whether what is playing or recording has to stop.
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

/// Why an output or an input could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// There is no device that way: no output for an output, no input for an
    /// input.
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
            Self::NoDevice => f.write_str("There is no audio device."),
            Self::Device(error) => write!(f, "Cannot open the audio device: {error}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Why the output or input opened its stream again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The stream failed or its device went away.
    Failed,
    /// The default device changed, and [`OutputOptions::follow_default`] or
    /// [`InputOptions::follow_default`] is on; or, on PipeWire's default
    /// input, which PipeWire moves to a new default source itself, the
    /// default source changed, and the input reopened at its channels.
    DefaultChanged,
    /// [`Output::resume`] took back a device let go after a long pause, or
    /// [`Input::resume`] one let go after it failed while paused.
    Resumed,
    /// The stream stopped asking for or delivering sound while running,
    /// without reporting an error: the [`Clock`] stood still for two seconds.
    Stalled,
}

/// What [`Output::maintain`] or [`Input::maintain`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Maintained {
    /// Nothing changed.
    Unchanged,
    /// A new stream is playing, possibly on another device, at another rate
    /// or with other channels; [`Render::configure`] or
    /// [`Capture::configure`] has been told. The new device's id is
    /// [`Output::device_id`] or [`Input::device_id`] from now on.
    Reopened {
        /// The device's name.
        device: String,
        /// The new stream's sample rate.
        sample_rate: u32,
        /// The new stream's channel count: for an input, the channels its
        /// capturer gets.
        channels: u16,
        /// Why.
        reason: Reason,
    },
    /// The device was let go: paused past
    /// [`OutputOptions::release_after`], or failed while paused. The next
    /// [`Output::resume`] opens it again.
    Released,
    /// The stream had to be opened again and could not be. Nothing is
    /// playing or recording; the next `maintain` or `resume` tries again.
    Failed(OpenError),
}

/// An audio output: one stream on one device, with the app's renderer.
///
/// Keep one for as long as the app may play. Pausing costs nothing, and
/// [`maintain`](Self::maintain) keeps it on a working device.
///
/// With [`OutputOptions::pause_when_idle`], the output pauses itself when
/// the renderer has nothing to play, and the app resumes it when it queues a
/// sound. The thread that owns the output does both: queue, then
/// [`resume`](Self::resume). A handle on another thread (a sound effect
/// player called from the UI) cannot reach the output, so it queues and
/// tells the owning thread, which calls `resume` before its next
/// `maintain`: through a channel, or an [`AtomicBool`] it swaps:
///
/// ```no_run
/// # use std::sync::Arc;
/// # use std::sync::atomic::{AtomicBool, Ordering};
/// # struct Silence;
/// # impl fastframe_audio::Render for Silence {
/// #     fn configure(&mut self, _: u32, _: u16) {}
/// #     fn render(&mut self, out: &mut [f32]) { out.fill(0.0) }
/// # }
/// # let mut output = fastframe_audio::Output::open(Default::default(), Silence)?;
/// let wake = Arc::new(AtomicBool::new(false));
/// // The handle, after it queues a sound with the renderer:
/// wake.store(true, Ordering::Release);
/// // The owning thread, each turn:
/// if wake.swap(false, Ordering::AcqRel) {
///     output.resume();
/// }
/// let _ = output.maintain();
/// # Ok::<(), fastframe_audio::OpenError>(())
/// ```
///
/// A sound queued while the output pauses itself waits for that `resume`,
/// so wake it after every sound, not only when it was paused: `resume` on a
/// playing output also tells it the renderer has something to play, so the
/// next `maintain` does not pause it before the callback has rendered it.
///
/// [`AtomicBool`]: std::sync::atomic::AtomicBool
pub struct Output<R: Render> {
    session: Session<Rendering<R>>,
}

impl<R: Render> fmt::Debug for Output<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.session.fmt(f)
    }
}

impl<R: Render> Output<R> {
    /// Opens the device and starts playing through `renderer`.
    ///
    /// Asking the device can take a perceptible time, so call it off the UI
    /// thread. A failure is not remembered: the next `open` tries again.
    pub fn open(options: OutputOptions, renderer: R) -> Result<Self, OpenError> {
        Ok(Self {
            session: Session::open(options.settings(), Rendering(renderer))?,
        })
    }

    /// Stops the device asking for sound, so the callback and the renderer
    /// cost nothing, and keeps the device open for an instant
    /// [`resume`](Self::resume). Never fails: a stream that cannot pause keeps
    /// running.
    pub fn pause(&mut self) {
        self.session.pause();
    }

    /// Has the device ask for sound again, opening it again if it was let go.
    /// Never fails: a device that will not start is reported by the next
    /// [`maintain`](Self::maintain), as is a reopen, with the new rate.
    pub fn resume(&mut self) {
        self.session.resume();
    }

    /// Whether the output is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.session.is_paused()
    }

    /// Whether the stream has failed or could not be opened again. Cheap
    /// enough to ask before every write; [`maintain`](Self::maintain) repairs it.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.session.failed()
    }

    /// Keeps the output on a working device. Call it now and then on the
    /// thread that owns the output (each frame, or before writing): it is
    /// cheap when nothing changed.
    ///
    /// It reopens a stream that failed or whose device went away, moves to a
    /// new default output, pauses an idle output
    /// ([`OutputOptions::pause_when_idle`]), and lets the device go after a
    /// long pause. A failed reopen is not remembered: the next call tries again. Nothing
    /// happens between calls, so keep calling it while paused for
    /// [`OutputOptions::release_after`] to take effect.
    pub fn maintain(&mut self) -> Maintained {
        self.session.maintain()
    }

    /// The errors the device reported since the last call, fatal and
    /// recoverable alike. A fatal one is still reported when the output has
    /// already reopened.
    pub fn take_errors(&mut self) -> Vec<OutputError> {
        self.session.take_errors()
    }

    /// How long the output has played, readable from any thread.
    #[must_use]
    pub fn clock(&self) -> Clock {
        self.session.clock()
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> &str {
        self.session.device_name()
    }

    /// The device's id, to save and open it again by ([`Device::Id`]).
    ///
    /// For the default device, the device it is just now. On PipeWire the
    /// stream follows the default itself and its node is not named, so this
    /// is the default's node as PipeWire's PulseAudio server names it, or
    /// cpal's own `output_default` without one. Finding it asks that server
    /// (the process's first question connects, about 10 ms, longer if it
    /// does not answer), so it is done on the first call after each open,
    /// not by the open itself; call it where waiting is fine.
    #[must_use]
    pub fn device_id(&self) -> &DeviceId {
        self.session.device_id()
    }

    /// The stream's sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.session.sample_rate()
    }

    /// The stream's channel count.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.session.channels()
    }
}

/// An audio input: one stream from one device, into the app's capturer.
///
/// It works as an [`Output`] does, the other way: a paused input gets no
/// callbacks, [`maintain`](Self::maintain) reopens a stream that failed or
/// stalled and moves to a new default input, and its [`Clock`] counts what
/// was captured. The callback runs with the same priority as an output's:
/// real-time on Linux on devices that wait for the hardware, in MMCSS on
/// Windows, on CoreAudio's I/O thread on macOS. A paused input keeps its
/// device open; drop it to let the device go.
///
/// Opening it may make the system ask the user for the microphone (macOS
/// asks once per app, Windows follows its privacy settings). Refused, the
/// device captures silence or fails to open, as the system decides.
pub struct Input<C: Capture> {
    session: Session<Capturing<C>>,
}

impl<C: Capture> fmt::Debug for Input<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.session.fmt(f)
    }
}

impl<C: Capture> Input<C> {
    /// Opens the device and starts capturing into `capturer`.
    ///
    /// Asking the device can take a perceptible time, so call it off the UI
    /// thread. On PipeWire, opening the default input also asks PipeWire's
    /// PulseAudio server how many channels the default source has (cpal's
    /// PipeWire default input does not say): about 6 ms, and up to two
    /// seconds when that server does not answer. It is asked on the thread
    /// that opens, never on the audio thread. A failure is not remembered:
    /// the next `open` tries again.
    ///
    /// It returns before the first block comes. A source that has been idle
    /// has to start again first, which took a few hundred milliseconds on
    /// PipeWire, whichever way it was opened; the [`clock`](Self::clock)
    /// moves once capturing has begun.
    pub fn open(options: InputOptions, capturer: C) -> Result<Self, OpenError> {
        Ok(Self {
            session: Session::open(options.settings(), Capturing(capturer))?,
        })
    }

    /// Stops the device delivering sound, so the callback and the capturer
    /// cost nothing, and keeps the device open for an instant
    /// [`resume`](Self::resume). Never fails.
    pub fn pause(&mut self) {
        self.session.pause();
    }

    /// Has the device deliver sound again. Never fails: a device that will
    /// not start is reported by the next [`maintain`](Self::maintain).
    pub fn resume(&mut self) {
        self.session.resume();
    }

    /// Whether the input is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.session.is_paused()
    }

    /// Whether the stream has failed or could not be opened again.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.session.failed()
    }

    /// Keeps the input on a working device, as [`Output::maintain`] does:
    /// call it now and then on the thread that owns the input.
    ///
    /// On PipeWire's default input it also reopens the input when the
    /// default source changes, so it captures at the new source's own
    /// channels ([`Maintained::Reopened`] with [`Reason::DefaultChanged`]).
    /// That reopen asks PipeWire's PulseAudio server again, as
    /// [`open`](Self::open) does, so it can wait as long.
    pub fn maintain(&mut self) -> Maintained {
        self.session.maintain()
    }

    /// The errors the device reported since the last call, classified as an
    /// output's are.
    pub fn take_errors(&mut self) -> Vec<OutputError> {
        self.session.take_errors()
    }

    /// How long the input has captured, readable from any thread.
    #[must_use]
    pub fn clock(&self) -> Clock {
        self.session.clock()
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> &str {
        self.session.device_name()
    }

    /// The device's id, to save and open it again by ([`Device::Id`]). On
    /// PipeWire the default is reported as for [`Output::device_id`].
    #[must_use]
    pub fn device_id(&self) -> &DeviceId {
        self.session.device_id()
    }

    /// The stream's sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.session.sample_rate()
    }

    /// The channels the capturer gets, as [`Capture::configure`] was told:
    /// what [`InputOptions::channels`] asked for, or the source's own.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.session.channels()
    }
}

/// The name of the audio system the crate plays through, for the log:
/// `PipeWire`, `PulseAudio` or `ALSA` on Linux, `CoreAudio` on macOS,
/// `WASAPI` on Windows.
///
/// The process chooses it the first time it opens or lists a device (cpal's
/// default host: on Linux PipeWire, then PulseAudio, then ALSA, whichever is
/// running) and keeps it. Asking before that makes the choice, which can take
/// a moment, so ask off the UI thread or after opening.
#[must_use]
pub fn host() -> &'static str {
    stream::host_name()
}

/// The output devices' names, for a device picker. Nothing is opened, so a
/// device another program or a USB peripheral is using is left alone.
///
/// The names are every output cpal lists, as before [`output_devices`]: on
/// PipeWire that includes cpal's own default devices and other apps'
/// playback streams. Prefer [`output_devices`], whose ids also tell apart
/// two devices with the same name.
pub fn output_device_names() -> Result<Vec<String>, OpenError> {
    stream::output_device_names()
}

/// The output devices a picker can offer, with their ids and which one is
/// the default. Nothing is opened. Listing asks the audio system, which can
/// take a moment: call it off the UI thread.
///
/// Left out: what is not a device of its own, such as PipeWire's default
/// devices (which [`Device::Default`] stands for) and other apps' playback
/// streams, and ALSA's `null` device. A device that cannot give its id or
/// name just now (it is going away) is left out too.
///
/// On PipeWire, the default is found through its PulseAudio server, over
/// one connection the process keeps (and makes again after it fails);
/// without that server, no device is marked the default.
pub fn output_devices() -> Result<Vec<DeviceInfo>, OpenError> {
    stream::devices(plan::Direction::Output)
}

/// The input devices a picker can offer: microphones, line inputs and other
/// sources, with their ids and which one is the default. Nothing is opened.
///
/// Left out: the monitors of output devices (what an output is playing),
/// PipeWire's default devices and ALSA's `null` device, as for
/// [`output_devices`]. On PipeWire another app's recording stream can
/// still be listed: cpal does not tell it apart from a source.
pub fn input_devices() -> Result<Vec<DeviceInfo>, OpenError> {
    stream::devices(plan::Direction::Input)
}
