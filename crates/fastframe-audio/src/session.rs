//! The state of one stream on one device, whichever way it goes: pausing and
//! resuming it, and keeping it on a working device. [`Output`](crate::Output)
//! and [`Input`](crate::Input) are this with the app's renderer or capturer.

use std::fmt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::plan::{self, Direction};
use crate::stream::{self, DefaultWatch, Engine, Live, Opener, Settings, Shared};
use crate::{Clock, Device, DeviceId, Maintained, OpenError, OutputError, Reason};

pub(crate) struct Session<E: Engine> {
    settings: Settings,
    /// [`stream::open`], or in tests a stand-in.
    opener: Opener<E>,
    engine: Arc<Mutex<E>>,
    live: Option<Live>,
    shared: Arc<Shared>,
    clock: Clock,
    paused: bool,
    paused_at: Option<Instant>,
    /// A reopen [`resume`](Self::resume) made, for the next `maintain` to report.
    resumed: bool,
    /// Whether the clock still moves while running.
    stall: plan::Stall,
    /// How long the renderer has had nothing to play.
    idle: plan::Idle,
    watch: Option<DefaultWatch>,
    device: String,
    /// The device's own id and host, from which [`Self::device_id`] finds
    /// the id to report the first time it is asked.
    own_id: String,
    host: String,
    /// The id to report, found once per stream: on PipeWire's default it
    /// takes a connection to its PulseAudio server, which only an input's
    /// open waits for (for the source's channels), and an output's does not.
    id: OnceLock<DeviceId>,
    sample_rate: u32,
    channels: u16,
}

impl<E: Engine> fmt::Debug for Session<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match E::DIRECTION {
            Direction::Output => "Output",
            Direction::Input => "Input",
        };
        f.debug_struct(name)
            .field("device", &self.device)
            .field("own_id", &self.own_id)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("paused", &self.paused)
            .field("open", &self.live.is_some())
            .finish_non_exhaustive()
    }
}

impl<E: Engine> Session<E> {
    pub(crate) fn open(settings: Settings, engine: E) -> Result<Self, OpenError> {
        Self::open_with(settings, engine, stream::open)
    }

    fn open_with(settings: Settings, engine: E, opener: Opener<E>) -> Result<Self, OpenError> {
        let engine = Arc::new(Mutex::new(engine));
        let shared = Arc::new(Shared::new(E::DIRECTION));
        let clock = Clock::default();
        let live = opener(&settings, &engine, &clock, &shared, false)?;
        shared.follows_moves.store(
            plan::reopens_when_default_moves(E::DIRECTION, &live.host, &live.own_id),
            Ordering::Release,
        );
        let watch = (settings.follow_default
            && settings.device == Device::Default
            && cfg!(any(target_os = "macos", windows)))
        .then(|| DefaultWatch::start(live.default_at_open.clone(), E::DIRECTION));
        Ok(Self {
            device: live.device.clone(),
            own_id: live.own_id.clone(),
            host: live.host.clone(),
            id: live
                .reported_id
                .clone()
                .map(OnceLock::from)
                .unwrap_or_default(),
            sample_rate: live.sample_rate,
            channels: live.channels,
            settings,
            opener,
            engine,
            live: Some(live),
            shared,
            clock,
            paused: false,
            paused_at: None,
            resumed: false,
            stall: plan::Stall::new(Duration::ZERO, Instant::now()),
            idle: plan::Idle::default(),
            watch,
        })
    }

    pub(crate) fn pause(&mut self) {
        if self.paused {
            return;
        }
        self.paused = true;
        self.paused_at = Some(Instant::now());
        self.idle = plan::Idle::default();
        self.shared.epoch.fetch_add(1, Ordering::Relaxed);
        if let Some(live) = &self.live
            && let Err(error) = live.stream.pause()
        {
            log::warn!("cannot pause the audio {}: {error}", E::DIRECTION.label());
            self.shared.report(error);
        }
    }

    pub(crate) fn resume(&mut self) {
        // The app has just given the renderer something to play, so what it
        // said before is stale, even while playing: the next `maintain`
        // must not pause before the callback has rendered it.
        self.idle = plan::Idle::default();
        self.shared.wakes.fetch_add(1, Ordering::AcqRel);
        if !self.paused {
            return;
        }
        self.paused = false;
        self.paused_at = None;
        self.shared.epoch.fetch_add(1, Ordering::Relaxed);
        match &self.live {
            Some(live) => {
                if let Err(error) = live.stream.play() {
                    log::warn!("cannot restart the audio {}: {error}", E::DIRECTION.label());
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

    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire) || (self.live.is_none() && !self.paused)
    }

    pub(crate) fn maintain(&mut self) -> Maintained {
        if std::mem::take(&mut self.resumed) {
            return self.reopened(Reason::Resumed);
        }
        if let Some(after) = self.settings.pause_when_idle
            && !self.paused
            && self.live.is_some()
            && self.idle.observe(
                plan::idle_now(
                    self.shared.idle.load(Ordering::Acquire),
                    self.shared.wakes.load(Ordering::Acquire),
                ),
                Instant::now(),
                after,
            )
        {
            log::debug!(
                "audio {}: nothing to play for {after:?}; pausing",
                E::DIRECTION.label()
            );
            self.pause();
        }
        if self.paused
            && self.live.is_some()
            && plan::should_release(self.paused_at, Instant::now(), self.settings.release_after)
        {
            log::info!(
                "audio {}: letting {} go after a long pause",
                E::DIRECTION.label(),
                self.device
            );
            self.live = None;
            self.shared.default_moved.store(false, Ordering::Release);
            return Maintained::Released;
        }
        let reason = if self.shared.failed.load(Ordering::Acquire) {
            Some(Reason::Failed)
        } else if self.default_changed() || self.shared.default_moved.load(Ordering::Acquire) {
            Some(Reason::DefaultChanged)
        } else if self.live.is_none() && !self.paused {
            Some(Reason::Failed)
        } else if self.stalled() {
            log::warn!(
                "audio {}: {} stopped running; reopening",
                E::DIRECTION.label(),
                self.device
            );
            Some(Reason::Stalled)
        } else {
            None
        };
        let Some(reason) = reason else {
            return Maintained::Unchanged;
        };
        if self.paused {
            // Nothing is running: let the device go, and open the right one
            // on resume.
            self.live = None;
            self.shared.failed.store(false, Ordering::Release);
            self.shared.default_moved.store(false, Ordering::Release);
            return Maintained::Released;
        }
        match self.reopen() {
            Ok(()) => self.reopened(reason),
            Err(error) => Maintained::Failed(error),
        }
    }

    pub(crate) fn take_errors(&mut self) -> Vec<OutputError> {
        std::mem::take(
            &mut *self
                .shared
                .errors
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    pub(crate) fn clock(&self) -> Clock {
        self.clock.clone()
    }

    pub(crate) fn device_name(&self) -> &str {
        &self.device
    }

    pub(crate) fn device_id(&self) -> &DeviceId {
        self.id
            .get_or_init(|| stream::reported_id(&self.host, &self.own_id, E::DIRECTION))
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn channels(&self) -> u16 {
        self.channels
    }

    fn default_changed(&self) -> bool {
        let (Some(watch), Some(live)) = (&self.watch, &self.live) else {
            return false;
        };
        match (watch.id(), &live.default_at_open) {
            (Some(now), Some(then)) => &now != then,
            _ => false,
        }
    }

    /// Closes the stream, if any, and opens the one the settings ask for.
    fn reopen(&mut self) -> Result<(), OpenError> {
        // Let the old device go first: some only take one stream.
        self.live = None;
        match (self.opener)(
            &self.settings,
            &self.engine,
            &self.clock,
            &self.shared,
            self.paused,
        ) {
            Ok(live) => {
                self.shared.failed.store(false, Ordering::Release);
                self.shared.default_moved.store(false, Ordering::Release);
                self.shared.follows_moves.store(
                    plan::reopens_when_default_moves(E::DIRECTION, &live.host, &live.own_id),
                    Ordering::Release,
                );
                self.stall = plan::Stall::new(self.clock.played(), Instant::now());
                self.device = live.device.clone();
                self.own_id = live.own_id.clone();
                self.host = live.host.clone();
                self.id = live
                    .reported_id
                    .clone()
                    .map(OnceLock::from)
                    .unwrap_or_default();
                self.sample_rate = live.sample_rate;
                self.channels = live.channels;
                self.live = Some(live);
                Ok(())
            }
            Err(error) => {
                log::warn!("audio {}: {error}", E::DIRECTION.label());
                Err(error)
            }
        }
    }

    /// Whether the stream is running but its clock has stood still for
    /// [`plan::STALL`]. Paused, nothing is expected to move.
    fn stalled(&mut self) -> bool {
        let now = Instant::now();
        let played = self.clock.played();
        if self.paused || self.live.is_none() {
            self.stall = plan::Stall::new(played, now);
            return false;
        }
        self.stall.observe(played, now)
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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::AtomicU32;

    use super::*;
    use crate::stream::{Callback, FakeStream, Handle};
    use crate::{Buffer, OpenError};

    /// A renderer that only notes how it was configured.
    #[derive(Default)]
    struct Engineless {
        configured: Arc<AtomicU32>,
    }

    impl Engine for Engineless {
        const DIRECTION: Direction = Direction::Output;

        fn configure(&mut self, _sample_rate: u32, _channels: u16) {
            self.configured.fetch_add(1, Ordering::Relaxed);
        }

        fn build(
            _: &cpal::Device,
            _: &cpal::StreamConfig,
            _: cpal::SampleFormat,
            _: Arc<Mutex<Self>>,
            _: Callback,
        ) -> Result<cpal::Stream, cpal::Error> {
            unreachable!("the stand-in opener builds no stream")
        }
    }

    /// A capturer that captures nothing.
    #[derive(Default)]
    struct Captureless;

    impl Engine for Captureless {
        const DIRECTION: Direction = Direction::Input;

        fn configure(&mut self, _sample_rate: u32, _channels: u16) {}

        fn build(
            _: &cpal::Device,
            _: &cpal::StreamConfig,
            _: cpal::SampleFormat,
            _: Arc<Mutex<Self>>,
            _: Callback,
        ) -> Result<cpal::Stream, cpal::Error> {
            unreachable!("the stand-in opener builds no stream")
        }
    }

    thread_local! {
        /// The host and own id the stand-in opener reports, when not its
        /// own fake ones.
        static ON: Cell<Option<(&'static str, &'static str)>> = const { Cell::new(None) };
        /// The streams this test's opener opened, in order.
        static OPENED: RefCell<Vec<Arc<FakeStream>>> = const { RefCell::new(Vec::new()) };
        /// Whether the next open fails.
        static REFUSE: Cell<bool> = const { Cell::new(false) };
    }

    fn fake_open<E: Engine>(
        _: &Settings,
        engine: &Arc<Mutex<E>>,
        clock: &Clock,
        _: &Arc<Shared>,
        paused: bool,
    ) -> Result<Live, OpenError> {
        if REFUSE.get() {
            return Err(OpenError::NoDevice);
        }
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .configure(48_000, 2);
        clock.restart(48_000);
        let stream = Arc::new(FakeStream::default());
        stream.playing.store(!paused, Ordering::Relaxed);
        let count = OPENED.with_borrow_mut(|opened| {
            opened.push(Arc::clone(&stream));
            opened.len()
        });
        Ok(Live {
            stream: Handle::Fake(stream),
            device: "Fake".into(),
            own_id: ON
                .get()
                .map_or_else(|| format!("fake-{count}"), |(_, id)| id.to_owned()),
            host: ON.get().map_or("fake", |(host, _)| host).into(),
            sample_rate: 48_000,
            channels: 2,
            default_at_open: None,
            reported_id: None,
        })
    }

    fn opened(index: usize) -> Arc<FakeStream> {
        OPENED.with_borrow(|opened| Arc::clone(&opened[index]))
    }

    fn settings() -> Settings {
        Settings {
            device: Device::Default,
            channels: 2,
            sample_rate: None,
            buffer: Buffer::Driver,
            max_block_frames: None,
            // No default-device watch thread, which would ask the system.
            follow_default: false,
            release_after: None,
            pause_when_idle: None,
            media_name: None,
            role: None,
        }
    }

    fn open(settings: Settings) -> Session<Engineless> {
        Session::open_with(settings, Engineless::default(), fake_open).expect("the stand-in opens")
    }

    #[test]
    fn pause_and_resume_stop_and_start_the_stream_once_each() {
        let mut session = open(settings());
        let stream = opened(0);
        assert!(stream.playing.load(Ordering::Relaxed));
        let epoch = session.shared.epoch.load(Ordering::Relaxed);

        session.pause();
        session.pause();
        assert!(session.is_paused());
        assert!(!stream.playing.load(Ordering::Relaxed));
        assert_eq!(stream.pauses.load(Ordering::Relaxed), 1, "paused once");
        assert_eq!(session.shared.epoch.load(Ordering::Relaxed), epoch + 1);
        assert!(!session.failed());

        session.resume();
        session.resume();
        assert!(!session.is_paused());
        assert!(stream.playing.load(Ordering::Relaxed));
        assert_eq!(stream.plays.load(Ordering::Relaxed), 1, "started once");
        assert_eq!(session.shared.epoch.load(Ordering::Relaxed), epoch + 2);
        assert_eq!(session.maintain(), Maintained::Unchanged);
        assert_eq!(OPENED.with_borrow(Vec::len), 1, "never reopened");
    }

    #[test]
    fn a_long_pause_lets_the_device_go_and_resume_opens_it_again() {
        let mut session = open(Settings {
            release_after: Some(Duration::ZERO),
            ..settings()
        });
        assert_eq!(session.device_id().as_str(), "fake-1");
        session.pause();
        assert_eq!(session.maintain(), Maintained::Released);
        assert!(session.live.is_none());
        assert!(!session.failed(), "a paused output let go has not failed");
        assert_eq!(session.maintain(), Maintained::Unchanged);

        session.resume();
        assert!(opened(1).playing.load(Ordering::Relaxed));
        assert_eq!(
            session.maintain(),
            Maintained::Reopened {
                device: "Fake".into(),
                sample_rate: 48_000,
                channels: 2,
                reason: Reason::Resumed,
            }
        );
        assert_eq!(session.maintain(), Maintained::Unchanged);
        assert_eq!(session.device_id().as_str(), "fake-2", "the new stream's");
    }

    #[test]
    fn an_input_on_pipewires_default_reopens_when_the_default_moves() {
        ON.set(Some(("pipewire", "input_default")));
        let mut session =
            Session::open_with(settings(), Captureless, fake_open).expect("the stand-in opens");
        let moved = || cpal::Error::with_message(cpal::ErrorKind::DeviceChanged, "moved");
        session.shared.report(moved());
        assert_eq!(
            session.maintain(),
            Maintained::Reopened {
                device: "Fake".into(),
                sample_rate: 48_000,
                channels: 2,
                reason: Reason::DefaultChanged,
            }
        );
        assert_eq!(session.maintain(), Maintained::Unchanged, "once per move");
        assert_eq!(OPENED.with_borrow(Vec::len), 2);
        assert_eq!(session.take_errors().len(), 1, "still reported to the app");

        // Paused, the device is let go once, and resume opens the new one.
        session.pause();
        session.shared.report(moved());
        assert_eq!(session.maintain(), Maintained::Released);
        assert_eq!(session.maintain(), Maintained::Unchanged);
        session.resume();
        assert_eq!(OPENED.with_borrow(Vec::len), 3);

        // An output on PipeWire's default carries on: its stream moved.
        ON.set(Some(("pipewire", "output_default")));
        let mut output = open(settings());
        output.shared.report(moved());
        assert_eq!(output.maintain(), Maintained::Unchanged);
        assert_eq!(OPENED.with_borrow(Vec::len), 4);
        ON.set(None);
    }

    #[test]
    fn a_failed_stream_that_cannot_reopen_is_tried_again() {
        let mut session = open(settings());
        session.shared.failed.store(true, Ordering::Release);
        REFUSE.set(true);
        assert_eq!(session.maintain(), Maintained::Failed(OpenError::NoDevice));
        assert!(session.failed());
        REFUSE.set(false);
        assert!(matches!(
            session.maintain(),
            Maintained::Reopened {
                reason: Reason::Failed,
                ..
            }
        ));
        assert!(!session.failed());
        let configured = session
            .engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .configured
            .load(Ordering::Relaxed);
        assert_eq!(configured, 2, "configured for each stream");
    }

    #[test]
    fn a_paused_stream_that_fails_is_let_go_and_opens_on_resume() {
        let mut session = open(settings());
        session.pause();
        session.shared.failed.store(true, Ordering::Release);
        assert_eq!(session.maintain(), Maintained::Released);
        session.resume();
        assert!(matches!(
            session.maintain(),
            Maintained::Reopened {
                reason: Reason::Resumed,
                ..
            }
        ));
    }

    /// What the callback leaves after a render that began at `wake`.
    fn rendered(session: &Session<Engineless>, wake: u64, idle: bool) {
        session
            .shared
            .idle
            .store(plan::idle_mark(wake, idle), Ordering::Release);
    }

    #[test]
    fn an_idle_output_pauses_and_a_render_from_before_a_resume_does_not() {
        let mut session = open(Settings {
            pause_when_idle: Some(Duration::ZERO),
            ..settings()
        });
        let wakes = |session: &Session<Engineless>| session.shared.wakes.load(Ordering::Acquire);
        // Playing: nothing pauses it.
        rendered(&session, wakes(&session), false);
        assert_eq!(session.maintain(), Maintained::Unchanged);
        assert!(!session.is_paused());
        // Idle: the next look pauses it.
        rendered(&session, wakes(&session), true);
        assert_eq!(session.maintain(), Maintained::Unchanged);
        assert!(session.is_paused());
        assert!(!opened(0).playing.load(Ordering::Relaxed));

        // The app queues a sound and resumes, while a render that began
        // before the resume ends, saying idle.
        let before = wakes(&session);
        session.resume();
        rendered(&session, before, true);
        let _ = session.maintain();
        assert!(!session.is_paused(), "the stale mark does not pause it");
        // A render since the resume that finds the sound played out does.
        rendered(&session, wakes(&session), true);
        let _ = session.maintain();
        assert!(session.is_paused());

        // Resuming a playing output also makes earlier marks stale.
        session.resume();
        rendered(&session, wakes(&session), true);
        session.resume();
        let _ = session.maintain();
        assert!(!session.is_paused());
    }

    #[test]
    fn without_pause_when_idle_an_idle_renderer_keeps_playing() {
        let mut session = open(settings());
        rendered(&session, 0, true);
        assert_eq!(session.maintain(), Maintained::Unchanged);
        assert!(!session.is_paused());
    }
}
