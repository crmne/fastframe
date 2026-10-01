//! Fontconfig's rendering settings for a family (Linux and other Unix).
//!
//! No fontconfig binding is common to the apps' dependency trees, and the C
//! library would add a build dependency, so `read` runs
//! `fc-match -f '%{hintstyle}|%{hinting}|%{antialias}' <family>` and parses
//! its one line with [`parse_fc_match`]. When `fc-match` is not installed,
//! fails, or has not answered within a second, the reader has no answer.
//! `rgba` is not asked for: egui renders grayscale only.
//!
//! `hintstyle` is fontconfig's integer: 0 none, 1 slight, 2 medium, 3 full.
//! `hinting` false means no hinting whatever the style. A field fontconfig
//! leaves empty keeps its default.

#[cfg(any(unix, test))]
use std::time::{Duration, Instant};

use crate::{Hinting, TextRendering};

/// The `fc-match` format `read` asks for and [`parse_fc_match`] expects.
pub const FORMAT: &str = "%{hintstyle}|%{hinting}|%{antialias}";

/// Maps fontconfig's `hintstyle` (`0`..`3`, or the `hintslight` style
/// constants) to a hinting level.
#[must_use]
pub fn parse_hintstyle(value: &str) -> Option<Hinting> {
    match value.trim() {
        "0" | "hintnone" => Some(Hinting::None),
        "1" | "hintslight" => Some(Hinting::Slight),
        "2" | "hintmedium" => Some(Hinting::Medium),
        "3" | "hintfull" => Some(Hinting::Full),
        _ => None,
    }
}

/// Parses a fontconfig boolean as `fc-match` prints it (`True`, `False`;
/// `DontCare` and anything else is no answer).
#[must_use]
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Parses `fc-match` output in [`FORMAT`].
///
/// Returns `None` when no field holds a known value.
#[must_use]
pub fn parse_fc_match(output: &str) -> Option<TextRendering> {
    let line = output.lines().next()?;
    let mut fields = line.split('|');
    let hintstyle = fields.next().and_then(parse_hintstyle);
    let hinting = fields.next().and_then(parse_bool);
    let antialias = fields.next().and_then(parse_bool);
    if hintstyle.is_none() && hinting.is_none() && antialias.is_none() {
        return None;
    }
    let default = TextRendering::default();
    let hinting = match hinting {
        Some(false) => Hinting::None,
        _ => hintstyle.unwrap_or(default.hinting),
    };
    Some(TextRendering {
        hinting,
        antialias: antialias.unwrap_or(default.antialias),
        ..default
    })
}

/// `family` as an `fc-match` pattern. In fontconfig's pattern syntax `-`
/// starts a point size and `:` and `,` separate elements, so they are
/// escaped: `sans-serif` unescaped asks for a family called `sans`.
#[cfg_attr(not(unix), allow(dead_code))]
fn pattern(family: &str) -> String {
    let mut out = String::with_capacity(family.len());
    for c in family.chars() {
        if matches!(c, '\\' | '-' | ':' | ',') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// How long `read` waits for `fc-match`: as long as a portal call gets. It
/// answers in a few milliseconds, but the caller is the app's startup, and a
/// fontconfig that is scanning its font folders again, or cannot reach one
/// of them, can take far longer.
#[cfg(unix)]
const LIMIT: Duration = Duration::from_secs(1);

/// The pause between two looks at a process that has not ended.
#[cfg(any(unix, test))]
const PAUSE: Duration = Duration::from_millis(1);

/// Asks fontconfig how `family` is rendered, through `fc-match`.
///
/// Returns `None` when `fc-match` is missing, fails, prints nothing useful,
/// or has not answered within a second.
#[cfg(unix)]
#[must_use]
pub fn read(family: &str) -> Option<TextRendering> {
    let child = std::process::Command::new("fc-match")
        .arg("-f")
        .arg(FORMAT)
        .arg(pattern(family))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    parse_fc_match(&printed(child, Instant::now() + LIMIT)?)
}

/// What `child` printed, when it has ended well by `deadline`. One that has
/// not ended is killed and reaped, so it neither keeps running nor stays a
/// zombie. Only the process itself is stopped: what a wrapper script in
/// `fc-match`'s place started is not followed.
///
/// The output is read once the process has ended, which is enough for the
/// one line `fc-match` prints: a process that filled the pipe would wait for
/// a reader, and be stopped at the deadline.
#[cfg(unix)]
fn printed(mut child: std::process::Child, deadline: Instant) -> Option<String> {
    use std::io::Read as _;

    let Some(Ok(status)) = wait_until(deadline, || child.try_wait().transpose()) else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    if !status.success() {
        return None;
    }
    let mut output = Vec::new();
    child.stdout.take()?.read_to_end(&mut output).ok()?;
    Some(String::from_utf8_lossy(&output).into_owned())
}

/// Asks `finished` until it answers or `deadline` passes. It is asked at
/// least once, so what has already ended is never given up on.
#[cfg(any(unix, test))]
fn wait_until<T>(deadline: Instant, mut finished: impl FnMut() -> Option<T>) -> Option<T> {
    loop {
        if let Some(answer) = finished() {
            return Some(answer);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(PAUSE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_names_are_escaped_for_fc_match() {
        // Unescaped, fc-match reads `sans-serif` as the family `sans` at a
        // size `serif`, and answers for a family nobody asked about.
        assert_eq!(pattern("sans-serif"), r"sans\-serif");
        assert_eq!(pattern("system-ui"), r"system\-ui");
        assert_eq!(pattern("Noto Sans"), "Noto Sans");
        assert_eq!(pattern(r"a:b,c\d"), r"a\:b\,c\\d");
    }

    #[test]
    fn hintstyles() {
        assert_eq!(parse_hintstyle("0"), Some(Hinting::None));
        assert_eq!(parse_hintstyle("1"), Some(Hinting::Slight));
        assert_eq!(parse_hintstyle("2"), Some(Hinting::Medium));
        assert_eq!(parse_hintstyle("3"), Some(Hinting::Full));
        assert_eq!(parse_hintstyle("hintslight"), Some(Hinting::Slight));
        assert_eq!(parse_hintstyle("4"), None);
        assert_eq!(parse_hintstyle(""), None);
    }

    #[test]
    fn booleans() {
        assert_eq!(parse_bool("True"), Some(true));
        assert_eq!(parse_bool("False"), Some(false));
        assert_eq!(parse_bool("DontCare"), None);
        assert_eq!(parse_bool(""), None);
    }

    #[test]
    fn slight_hinting_on_this_desktop() {
        // hintstyle=hintslight, hinting on, grayscale antialiasing.
        assert_eq!(
            parse_fc_match("1|True|True"),
            Some(TextRendering::default())
        );
    }

    #[test]
    fn full_hinting_without_antialiasing() {
        let got = parse_fc_match("3|True|False").unwrap();
        assert_eq!(got.hinting, Hinting::Full);
        assert!(!got.antialias);
    }

    #[test]
    fn hinting_off_overrides_the_style() {
        let got = parse_fc_match("3|False|True").unwrap();
        assert_eq!(got.hinting, Hinting::None);
    }

    #[test]
    fn missing_fields_keep_their_defaults() {
        let got = parse_fc_match("|True|").unwrap();
        assert_eq!(got, TextRendering::default());
        let got = parse_fc_match("2").unwrap();
        assert_eq!(got.hinting, Hinting::Medium);
        assert!(got.antialias);
        let got = parse_fc_match("||False").unwrap();
        assert_eq!(got.hinting, Hinting::Slight);
        assert!(!got.antialias);
    }

    #[test]
    fn extra_fields_are_ignored() {
        // An older format that also asked for rgba.
        assert_eq!(
            parse_fc_match("1|True|True|1\n"),
            Some(TextRendering::default())
        );
    }

    #[test]
    fn nothing_useful_is_no_answer() {
        assert_eq!(parse_fc_match(""), None);
        assert_eq!(parse_fc_match("||"), None);
        assert_eq!(parse_fc_match("Fontconfig error"), None);
    }

    #[test]
    fn waiting_ends_with_the_answer_or_at_the_deadline() {
        let far = Instant::now() + Duration::from_secs(60);
        let mut looks = 0;
        let answer = wait_until(far, || {
            looks += 1;
            (looks == 3).then_some("ended")
        });
        assert_eq!(answer, Some("ended"));
        assert_eq!(looks, 3);

        // No answer ever: it gives up, and not before the deadline.
        let started = Instant::now();
        let limit = Duration::from_millis(20);
        assert_eq!(wait_until(started + limit, || None::<()>), None);
        assert!(started.elapsed() >= limit);

        // A deadline already past still gets one look.
        assert_eq!(wait_until(started, || Some("ended")), Some("ended"));
    }
}
