//! The thin layer that talks to cpal: choosing the device, opening the
//! stream in the device's sample format, and watching the default output.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::plan::{self, Attempt, Choice};
use crate::{Clock, Device, OpenError, OutputError, OutputOptions, Render};

/// What the callback and the error callback share with the [`Output`](crate::Output).
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// A stream error means the stream has to be opened again.
    pub(crate) failed: AtomicBool,
    /// Errors not yet taken by the app.
    pub(crate) errors: Mutex<Vec<OutputError>>,
}

impl Shared {
    pub(crate) fn report(&self, error: cpal::Error) {
        let reopen = plan::needs_reopen(error.kind());
        let error = OutputError::from_cpal(&error);
        if reopen {
            log::warn!("audio output error, reopening: {error}");
            self.failed.store(true, Ordering::Release);
        } else {
            log::debug!("audio output: {error}");
        }
        self.errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error);
    }
}

/// An open stream and what it was opened with.
pub(crate) struct Live {
    pub(crate) stream: cpal::Stream,
    pub(crate) device: String,
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    /// The default output's name when this stream opened on it, for noticing
    /// that the default changed.
    pub(crate) default_at_open: Option<String>,
}

/// Opens the stream the options ask for and starts it, unless `paused`.
pub(crate) fn open<R: Render>(
    options: &OutputOptions,
    renderer: &Arc<Mutex<R>>,
    clock: &Clock,
    shared: &Arc<Shared>,
    paused: bool,
) -> Result<Live, OpenError> {
    let host = cpal::default_host();
    let (device, default_at_open) = pick(&host, &options.device)?;
    let name = device.to_string();
    let default = device.default_output_config().map_err(OpenError::from)?;
    let format = default.sample_format();
    let range = match *default.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => Some((min, max)),
        cpal::SupportedBufferSize::Unknown => None,
    };
    let buffer = match options.buffer {
        crate::Buffer::Driver => None,
        crate::Buffer::Fixed(size) => Some(size),
        crate::Buffer::FixedOnWindows(size) => cfg!(windows).then_some(size),
    };
    let channels = if options.channels == 0 {
        default.channels()
    } else {
        options.channels
    };

    let mut tried = plan::attempts(options.sample_rate, default.sample_rate(), buffer, range)
        .into_iter()
        .map(|attempt| (attempt, channels, format))
        .collect::<Vec<_>>();
    // Then anything else the device lists, with the driver's buffer.
    if let Ok(configs) = device.supported_output_configs() {
        for config in configs {
            let rate = default
                .sample_rate()
                .clamp(config.min_sample_rate(), config.max_sample_rate());
            let candidate = (
                Attempt {
                    sample_rate: rate,
                    buffer_frames: None,
                },
                config.channels(),
                config.sample_format(),
            );
            if !tried.contains(&candidate) {
                tried.push(candidate);
            }
        }
    }

    let mut last_error = None;
    for (attempt, channels, format) in tried {
        let config = cpal::StreamConfig {
            channels,
            sample_rate: attempt.sample_rate,
            buffer_size: attempt
                .buffer_frames
                .map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed),
        };
        renderer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .configure(attempt.sample_rate, channels);
        match build(
            &device,
            &config,
            format,
            Arc::clone(renderer),
            clock.clone(),
            Arc::clone(shared),
            options.max_block_frames,
        ) {
            Ok(stream) => {
                clock.restart(attempt.sample_rate);
                shared.failed.store(false, Ordering::Release);
                if !paused {
                    stream.play().map_err(OpenError::from)?;
                }
                log::info!(
                    "audio output: {name} at {} Hz, {channels} channels, {}",
                    attempt.sample_rate,
                    attempt
                        .buffer_frames
                        .map_or("the driver's buffer".to_owned(), |frames| format!(
                            "{frames} frames"
                        ))
                );
                return Ok(Live {
                    stream,
                    device: name,
                    sample_rate: attempt.sample_rate,
                    channels,
                    default_at_open,
                });
            }
            Err(error) => {
                log::debug!("audio output: {name} refused {config:?} as {format}: {error}");
                last_error = Some(error);
            }
        }
    }
    Err(last_error.map_or(OpenError::NoDevice, OpenError::from))
}

/// The device to open, and the default output's name when that is the one.
fn pick(host: &cpal::Host, wanted: &Device) -> Result<(cpal::Device, Option<String>), OpenError> {
    let wanted = match wanted {
        Device::Default => None,
        Device::Named(name) => Some(name.as_str()),
    };
    if wanted.is_some() {
        let devices: Vec<cpal::Device> = host.output_devices().map_err(OpenError::from)?.collect();
        let names: Vec<String> = devices.iter().map(ToString::to_string).collect();
        match plan::choose(&names, wanted) {
            Choice::Named(index) => {
                if let Some(device) = devices.into_iter().nth(index) {
                    return Ok((device, None));
                }
            }
            Choice::Default { missing } => {
                if let Some(missing) = missing {
                    log::warn!(
                        "audio device {missing:?} is not available; using the default output"
                    );
                }
            }
        }
    }
    let device = host.default_output_device().ok_or(OpenError::NoDevice)?;
    let name = device.to_string();
    Ok((device, Some(name)))
}

/// Builds a stream that renders through `renderer` in the device's sample
/// format.
fn build<R: Render>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    renderer: Arc<Mutex<R>>,
    clock: Clock,
    shared: Arc<Shared>,
    max_block_frames: Option<u32>,
) -> Result<cpal::Stream, cpal::Error> {
    use cpal::SampleFormat as F;
    let args = (device, config, renderer, clock, shared, max_block_frames);
    match format {
        F::F32 => typed::<f32, R>(args),
        F::F64 => typed::<f64, R>(args),
        F::I8 => typed::<i8, R>(args),
        F::I16 => typed::<i16, R>(args),
        F::I24 => typed::<cpal::I24, R>(args),
        F::I32 => typed::<i32, R>(args),
        F::I64 => typed::<i64, R>(args),
        F::U8 => typed::<u8, R>(args),
        F::U16 => typed::<u16, R>(args),
        F::U24 => typed::<cpal::U24, R>(args),
        F::U32 => typed::<u32, R>(args),
        F::U64 => typed::<u64, R>(args),
        other => Err(cpal::Error::with_message(
            cpal::ErrorKind::UnsupportedConfig,
            format!("unsupported sample format {other}"),
        )),
    }
}

type BuildArgs<'a, R> = (
    &'a cpal::Device,
    &'a cpal::StreamConfig,
    Arc<Mutex<R>>,
    Clock,
    Arc<Shared>,
    Option<u32>,
);

fn typed<T, R>(
    (device, config, renderer, clock, shared, max_block): BuildArgs<'_, R>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
    R: Render,
{
    let channels = usize::from(config.channels);
    let mut scratch: Vec<f32> = Vec::new();
    let errors = Arc::clone(&shared);
    device.build_output_stream::<T, _, _>(
        *config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            scratch.clear();
            scratch.resize(data.len(), 0.0);
            // The app's thread only takes the lock while (re)opening, before
            // the stream starts; a contended lock renders silence rather
            // than wait.
            let rendered = match renderer.try_lock() {
                Ok(mut renderer) => {
                    for (start, end) in plan::blocks(scratch.len(), channels, max_block) {
                        renderer.render(&mut scratch[start..end]);
                    }
                    true
                }
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    let mut renderer = poisoned.into_inner();
                    for (start, end) in plan::blocks(scratch.len(), channels, max_block) {
                        renderer.render(&mut scratch[start..end]);
                    }
                    true
                }
                Err(std::sync::TryLockError::WouldBlock) => false,
            };
            for (out, sample) in data.iter_mut().zip(&scratch) {
                *out = if rendered {
                    T::from_sample(*sample)
                } else {
                    T::EQUILIBRIUM
                };
            }
            let timestamp = info.timestamp();
            let latency = timestamp
                .playback
                .saturating_duration_since(timestamp.callback);
            clock.rendered((data.len() / channels.max(1)) as u64, latency);
        },
        move |error| errors.report(error),
        None,
    )
}

/// The default output's name, or `None` when there is none.
pub(crate) fn default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .map(|device| device.to_string())
}

/// Every output device's name. Nothing is opened.
pub(crate) fn output_device_names() -> Result<Vec<String>, OpenError> {
    Ok(cpal::default_host()
        .output_devices()
        .map_err(OpenError::from)?
        .map(|device| device.to_string())
        .collect())
}

/// The default output's name, polled on a thread of its own because
/// enumerating devices can block on Windows. The thread ends with the watch.
#[derive(Debug)]
pub(crate) struct DefaultWatch(Arc<Mutex<Option<String>>>);

/// How often the default output is asked for.
const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(2);

impl DefaultWatch {
    pub(crate) fn start(initial: Option<String>) -> Self {
        let shared = Arc::new(Mutex::new(initial));
        let weak = Arc::downgrade(&shared);
        let started = std::thread::Builder::new()
            .name("audio-default-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(DEFAULT_CHECK_INTERVAL);
                    let Some(shared) = weak.upgrade() else {
                        break;
                    };
                    let name = default_output_name();
                    *shared.lock().unwrap_or_else(PoisonError::into_inner) = name;
                }
            });
        if let Err(error) = started {
            log::warn!("cannot watch the default audio output: {error}");
        }
        Self(shared)
    }

    /// The last polled name.
    pub(crate) fn name(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
