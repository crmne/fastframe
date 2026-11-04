//! When a newer release downloads and installs on its own.
//!
//! Extracted from Solco (its ADR 0343). Pure rules, so an app asks them with
//! what it knows: a newer release on the same major version installs itself
//! (downloaded in the background while nothing the app does would be
//! disturbed, installed at the next start before anything opens), unless the
//! person turned that off or went back to an older version on purpose. A new
//! major version asks, and a copy that can't replace itself is told where it
//! updates. What "nothing would be disturbed" means is the app's: a track
//! playing, an export, a command running.

use std::cmp::Ordering;

use crate::detect::Unsupported;
use crate::version::Semver;

/// Why the app asks rather than updating on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ask {
    /// A new major version (2.0 after 1.x), which may change what the person
    /// agreed to or paid for.
    NewMajor,
    /// The person turned automatic updates off.
    SwitchedOff,
    /// The person went back to an older version: the one they left, or
    /// anything up to it, is offered, never installed on its own.
    WentBack,
}

/// How a newer release reaches this copy.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Offer {
    /// Downloaded in the background while nothing would be disturbed, and
    /// installed at the next start, before anything opens.
    Automatic,
    /// The app offers it, and the person decides.
    Ask(Ask),
    /// This copy can't replace itself; the reason says where it updates
    /// (see [`Updater::managed_update`](crate::Updater::managed_update)).
    Elsewhere(Unsupported),
}

/// How `release` is offered to a copy running `current`.
///
/// `automatic` is the app's setting (on by default in the apps that use
/// it); `installable` is [`Updater::installable`](crate::Updater::installable)'s
/// answer; `newest_run` is the newest version that ever ran on this
/// computer, which the app remembers.
#[must_use]
pub fn offer(
    current: &str,
    release: &str,
    automatic: bool,
    installable: Result<(), &Unsupported>,
    newest_run: Option<&str>,
) -> Offer {
    if let Err(reason) = installable {
        return Offer::Elsewhere(reason.clone());
    }
    if major(release) != major(current) {
        return Offer::Ask(Ask::NewMajor);
    }
    if !automatic {
        return Offer::Ask(Ask::SwitchedOff);
    }
    if went_back(current, release, newest_run) {
        return Offer::Ask(Ask::WentBack);
    }
    Offer::Automatic
}

/// Whether a background download may run now: the release installs on its
/// own and nothing the app does would be disturbed (`quiet`).
#[must_use]
pub fn may_download(offer: &Offer, quiet: bool) -> bool {
    *offer == Offer::Automatic && quiet
}

/// Whether the update an earlier run downloaded, `kept` (from
/// [`Updater::kept`](crate::Updater::kept)), installs now, at the start
/// before anything opens: it is newer than `current`, it would install on its
/// own, and nothing the app does would be disturbed (`quiet`).
#[must_use]
pub fn installs_at_start(
    current: &str,
    kept: &str,
    automatic: bool,
    newest_run: Option<&str>,
    quiet: bool,
) -> bool {
    quiet
        && compare(kept, current) == Some(Ordering::Greater)
        && offer(current, kept, automatic, Ok(()), newest_run) == Offer::Automatic
}

/// The newest of `current` and what ran before, for the app to remember as
/// the next `newest_run`.
#[must_use]
pub fn newest_run(current: &str, before: Option<&str>) -> String {
    match before {
        Some(before) if compare(before, current) == Some(Ordering::Greater) => before.to_owned(),
        _ => current.to_owned(),
    }
}

/// The person runs an older version than one that ran before, and `release`
/// is no newer than that one: they went back on purpose.
fn went_back(current: &str, release: &str, newest_run: Option<&str>) -> bool {
    newest_run.is_some_and(|newest| {
        compare(current, newest) == Some(Ordering::Less)
            && compare(release, newest) != Some(Ordering::Greater)
    })
}

/// The major version: the number before the first dot.
fn major(version: &str) -> Option<u64> {
    version.split(['.', '-']).next()?.parse().ok()
}

/// Semantic-version precedence, or `None` when either isn't one.
fn compare(a: &str, b: &str) -> Option<Ordering> {
    Some(Semver::parse(a)?.cmp(&Semver::parse(b)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_release_on_the_same_major_installs_itself() {
        assert_eq!(
            offer("1.2.0", "1.3.0", true, Ok(()), None),
            Offer::Automatic
        );
        assert_eq!(
            offer("1.0.0-beta.3", "1.0.0-beta.8", true, Ok(()), None),
            Offer::Automatic
        );
        assert_eq!(
            offer("0.11.1", "0.12.0", true, Ok(()), Some("0.11.1")),
            Offer::Automatic
        );
    }

    #[test]
    fn a_new_major_the_switch_and_going_back_ask() {
        assert_eq!(
            offer("1.9.0", "2.0.0", true, Ok(()), None),
            Offer::Ask(Ask::NewMajor)
        );
        assert_eq!(
            offer("1.2.0", "1.3.0", false, Ok(()), None),
            Offer::Ask(Ask::SwitchedOff)
        );
        // Went back from 1.3.0 to 1.2.0: 1.3.0 isn't installed again.
        assert_eq!(
            offer("1.2.0", "1.3.0", true, Ok(()), Some("1.3.0")),
            Offer::Ask(Ask::WentBack)
        );
        // Anything newer than what they left is automatic again.
        assert_eq!(
            offer("1.2.0", "1.4.0", true, Ok(()), Some("1.3.0")),
            Offer::Automatic
        );
    }

    #[test]
    fn a_copy_that_cant_replace_itself_is_told_where() {
        assert_eq!(
            offer("1.2.0", "1.3.0", true, Err(&Unsupported::Homebrew), None),
            Offer::Elsewhere(Unsupported::Homebrew)
        );
    }

    #[test]
    fn downloads_and_installs_wait_for_quiet() {
        assert!(may_download(&Offer::Automatic, true));
        assert!(!may_download(&Offer::Automatic, false));
        assert!(!may_download(&Offer::Ask(Ask::SwitchedOff), true));
        assert!(installs_at_start("1.2.0", "1.3.0", true, None, true));
        assert!(!installs_at_start("1.2.0", "1.3.0", true, None, false));
        assert!(!installs_at_start("1.2.0", "1.3.0", false, None, true));
        assert!(!installs_at_start("1.2.0", "2.0.0", true, None, true));
        assert!(!installs_at_start("1.3.0", "1.3.0", true, None, true));
        assert!(!installs_at_start(
            "1.2.0",
            "1.3.0",
            true,
            Some("1.3.0"),
            true
        ));
    }

    #[test]
    fn the_newest_run_only_moves_forward() {
        assert_eq!(newest_run("1.2.0", None), "1.2.0");
        assert_eq!(newest_run("1.2.0", Some("1.3.0")), "1.3.0");
        assert_eq!(newest_run("1.4.0", Some("1.3.0")), "1.4.0");
        assert_eq!(newest_run("1.4.0", Some("not a version")), "1.4.0");
    }
}
