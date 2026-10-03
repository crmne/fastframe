//! The decisions behind opening and keeping an output, as pure functions over
//! what the device reported, so they are tested on every platform without a
//! device.

use std::time::{Duration, Instant};

use crate::{BufferSize, ErrorKind};

/// One way to open the stream: a sample rate, and a fixed buffer or the
/// driver's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Attempt {
    pub(crate) sample_rate: u32,
    /// Frames, or `None` for the driver's buffer.
    pub(crate) buffer_frames: Option<u32>,
}

/// The order in which to try configurations: the app's preferred rate, then
/// the device's own rate (which Windows insists on for a shared device),
/// each with the fixed buffer when the app asked for one, then the first of
/// those with the driver's buffer, which a device that rejects the size still
/// accepts. Duplicates are left out. The device's other listed configurations
/// come after these, from the caller.
pub(crate) fn attempts(
    preferred_rate: Option<u32>,
    device_rate: u32,
    buffer: Option<BufferSize>,
    buffer_range: Option<(u32, u32)>,
) -> Vec<Attempt> {
    let mut rates = Vec::with_capacity(2);
    if let Some(rate) = preferred_rate.filter(|&rate| rate > 0) {
        rates.push(rate);
    }
    if !rates.contains(&device_rate) {
        rates.push(device_rate);
    }
    let mut out: Vec<Attempt> = Vec::new();
    let mut push = |attempt: Attempt| {
        if !out.contains(&attempt) {
            out.push(attempt);
        }
    };
    if let Some(size) = buffer {
        for &rate in &rates {
            push(Attempt {
                sample_rate: rate,
                buffer_frames: Some(buffer_frames(size, rate, buffer_range)),
            });
        }
    }
    for &rate in &rates {
        push(Attempt {
            sample_rate: rate,
            buffer_frames: None,
        });
    }
    out
}

/// A buffer size in frames at `sample_rate`, clamped to the range the device
/// reported, since CoreAudio rejects any other size. At least one frame.
pub(crate) fn buffer_frames(size: BufferSize, sample_rate: u32, range: Option<(u32, u32)>) -> u32 {
    let frames = match size {
        BufferSize::Frames(frames) => frames,
        BufferSize::Duration(duration) => {
            let frames = duration.as_nanos() * u128::from(sample_rate) / 1_000_000_000;
            u32::try_from(frames).unwrap_or(u32::MAX)
        }
    }
    .max(1);
    match range {
        Some((min, max)) if min <= max && max > 0 => frames.clamp(min.max(1), max),
        _ => frames,
    }
}

/// The device to open, given the output device names and the one the app
/// asked for. A name that is not there falls back to the default output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// The named device, at this position in the list.
    Named(usize),
    /// The default output: none was named, or the named one is missing.
    Default {
        /// The name asked for, when it was missing.
        missing: Option<String>,
    },
}

pub(crate) fn choose(names: &[String], wanted: Option<&str>) -> Choice {
    match wanted.map(str::trim).filter(|name| !name.is_empty()) {
        None => Choice::Default { missing: None },
        Some(wanted) => match names.iter().position(|name| name == wanted) {
            Some(index) => Choice::Named(index),
            None => Choice::Default {
                missing: Some(wanted.to_owned()),
            },
        },
    }
}

/// Whether an output paused since `paused_at` has rested long enough to let
/// the device go.
pub(crate) fn should_release(
    paused_at: Option<Instant>,
    now: Instant,
    release_after: Option<Duration>,
) -> bool {
    match (paused_at, release_after) {
        (Some(since), Some(after)) => now.saturating_duration_since(since) >= after,
        _ => false,
    }
}

/// How the app should treat an error, as Solco does (ADR 0165): a fatal one
/// fails what is playing; a recoverable one is logged. A lost device or an
/// invalidated stream is recoverable because the output reopens and the
/// renderer carries on.
pub(crate) fn kind_of(kind: cpal::ErrorKind) -> ErrorKind {
    use cpal::ErrorKind as Cpal;
    match kind {
        Cpal::HostUnavailable
        | Cpal::PermissionDenied
        | Cpal::ResourceExhausted
        | Cpal::UnsupportedConfig
        | Cpal::UnsupportedOperation
        | Cpal::InvalidInput => ErrorKind::Fatal,
        _ => ErrorKind::Recoverable,
    }
}

/// Whether a stream that reported `kind` has to be opened again. Only a
/// glitch, a route the backend already followed, and refused real-time
/// priority leave the stream working; anything else may have stopped it, as
/// Spotifast's output has always assumed.
pub(crate) fn needs_reopen(kind: cpal::ErrorKind) -> bool {
    use cpal::ErrorKind as Cpal;
    !matches!(
        kind,
        Cpal::Xrun | Cpal::DeviceChanged | Cpal::RealtimeDenied
    )
}

/// The interleaved sample ranges to render one callback's buffer in, of at
/// most `max_frames` frames each. Lengths in samples, so the last block
/// holds what is left.
pub(crate) fn blocks(
    samples: usize,
    channels: usize,
    max_frames: Option<u32>,
) -> Vec<(usize, usize)> {
    let channels = channels.max(1);
    let step = match max_frames {
        Some(frames) if frames > 0 => (frames as usize).saturating_mul(channels),
        _ => samples.max(1),
    };
    let mut out = Vec::new();
    let mut start = 0;
    while start < samples {
        let end = (start + step).min(samples);
        out.push((start, end));
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preferred_rate_comes_first_then_the_devices_then_the_drivers_buffer() {
        let tried = attempts(
            Some(44_100),
            48_000,
            Some(BufferSize::Frames(512)),
            Some((64, 4096)),
        );
        assert_eq!(
            tried,
            [
                Attempt {
                    sample_rate: 44_100,
                    buffer_frames: Some(512)
                },
                Attempt {
                    sample_rate: 48_000,
                    buffer_frames: Some(512)
                },
                Attempt {
                    sample_rate: 44_100,
                    buffer_frames: None
                },
                Attempt {
                    sample_rate: 48_000,
                    buffer_frames: None
                },
            ]
        );
    }

    #[test]
    fn without_a_preference_or_a_buffer_the_device_rate_is_tried_once() {
        assert_eq!(
            attempts(None, 48_000, None, None),
            [Attempt {
                sample_rate: 48_000,
                buffer_frames: None
            }]
        );
        assert_eq!(
            attempts(Some(48_000), 48_000, Some(BufferSize::Frames(256)), None),
            [
                Attempt {
                    sample_rate: 48_000,
                    buffer_frames: Some(256)
                },
                Attempt {
                    sample_rate: 48_000,
                    buffer_frames: None
                },
            ]
        );
    }

    #[test]
    fn buffer_sizes_are_frames_at_the_rate_and_stay_in_the_devices_range() {
        let ms = BufferSize::Duration(Duration::from_millis(100));
        assert_eq!(buffer_frames(ms, 44_100, None), 4410);
        assert_eq!(buffer_frames(ms, 48_000, None), 4800);
        // Solco's 512 frames are 512 at any rate.
        assert_eq!(buffer_frames(BufferSize::Frames(512), 48_000, None), 512);
        assert_eq!(
            buffer_frames(BufferSize::Frames(512), 44_100, Some((1024, 8192))),
            1024
        );
        assert_eq!(
            buffer_frames(BufferSize::Frames(9000), 44_100, Some((64, 8192))),
            8192
        );
        // A range a driver reports wrongly is ignored, and the size is never 0.
        assert_eq!(
            buffer_frames(BufferSize::Frames(512), 44_100, Some((900, 100))),
            512
        );
        assert_eq!(buffer_frames(BufferSize::Frames(0), 44_100, None), 1);
    }

    #[test]
    fn a_missing_named_device_falls_back_to_the_default() {
        let names = ["Speakers".to_owned(), "USB DAC".to_owned()];
        assert_eq!(choose(&names, Some("USB DAC")), Choice::Named(1));
        assert_eq!(choose(&names, None), Choice::Default { missing: None });
        assert_eq!(
            choose(&names, Some("  ")),
            Choice::Default { missing: None }
        );
        assert_eq!(
            choose(&names, Some("Headphones")),
            Choice::Default {
                missing: Some("Headphones".into())
            }
        );
    }

    #[test]
    fn a_long_pause_lets_the_device_go() {
        let start = Instant::now();
        let later = start + Duration::from_secs(301);
        let after = Some(Duration::from_secs(300));
        assert!(should_release(Some(start), later, after));
        assert!(!should_release(
            Some(start),
            start + Duration::from_secs(10),
            after
        ));
        assert!(!should_release(None, later, after));
        assert!(!should_release(Some(start), later, None));
    }

    #[test]
    fn errors_split_as_solco_handles_them() {
        use cpal::ErrorKind as Cpal;
        for fatal in [
            Cpal::HostUnavailable,
            Cpal::PermissionDenied,
            Cpal::ResourceExhausted,
            Cpal::UnsupportedConfig,
            Cpal::UnsupportedOperation,
            Cpal::InvalidInput,
        ] {
            assert_eq!(kind_of(fatal), ErrorKind::Fatal, "{fatal:?}");
            assert!(needs_reopen(fatal), "{fatal:?}");
        }
        // Logged, and the output carries on: reopened where the stream may
        // have stopped, left alone where it is still playing.
        for (kind, reopen) in [
            (Cpal::DeviceNotAvailable, true),
            (Cpal::StreamInvalidated, true),
            (Cpal::DeviceBusy, true),
            (Cpal::BackendError, true),
            (Cpal::Other, true),
            (Cpal::Xrun, false),
            (Cpal::DeviceChanged, false),
            (Cpal::RealtimeDenied, false),
        ] {
            assert_eq!(kind_of(kind), ErrorKind::Recoverable, "{kind:?}");
            assert_eq!(needs_reopen(kind), reopen, "{kind:?}");
        }
    }

    #[test]
    fn a_callback_is_rendered_in_blocks_of_at_most_the_asked_frames() {
        // 1200 stereo frames in blocks of 512 frames.
        assert_eq!(
            blocks(2400, 2, Some(512)),
            [(0, 1024), (1024, 2048), (2048, 2400)]
        );
        assert_eq!(blocks(2400, 2, None), [(0, 2400)]);
        assert_eq!(blocks(0, 2, Some(512)), []);
    }
}
