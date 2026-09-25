//! Release version numbers.

/// `major.minor.patch`, and whether a suffix marks it as a pre-release;
/// anything else is `None`.
pub(crate) fn parse(version: &str) -> Option<([u64; 3], bool)> {
    let version = version.trim();
    let (numbers, pre_release) = match version.split_once('-') {
        Some((numbers, _)) => (numbers, true),
        None => (version, false),
    };
    let mut parts = numbers.split('.').map(|part| part.parse::<u64>().ok());
    let numbers = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some((numbers, pre_release))
}

/// Whether `candidate` is a newer stable version than `current`.
///
/// Stable releases supersede their release candidates (`0.11.0` is newer
/// than `0.11.0-rc1`). Pre-releases are never offered, and anything that is
/// not `major.minor.patch` is ignored.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some((candidate, false)), Some((current, current_pre))) => {
            candidate > current || (candidate == current && current_pre)
        }
        _ => false,
    }
}

/// A stable version made only of digits and dots, safe to put in a file
/// name, a URL path and a tag.
pub(crate) fn is_plain_release(version: &str) -> bool {
    parse(version).is_some_and(|(_, pre)| !pre)
        && version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
}

/// The longest version string the pre-release channel accepts. Release tags
/// are far shorter; the bound keeps asset and file names sane.
const MAX_LENGTH: usize = 64;

/// A semantic version without build metadata, as the pre-release channel
/// reads tags: `major.minor.patch`, optionally followed by `-` and
/// dot-separated identifiers. Ordered by semver precedence.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Semver<'a> {
    core: [u64; 3],
    pre: Vec<Identifier<'a>>,
}

/// One pre-release identifier. Numeric identifiers sort numerically and
/// before alphanumeric ones, which sort in ASCII order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier<'a> {
    Numeric(u64),
    Alphanumeric(&'a str),
}

impl<'a> Semver<'a> {
    /// Parses exactly `major.minor.patch[-pre.release]`: numbers without
    /// leading zeros, identifiers of ASCII letters, digits and hyphens.
    /// Anything else (a `v` prefix, spaces, `+build` metadata, empty
    /// identifiers, slashes) is `None`, so an accepted version only holds
    /// `[0-9A-Za-z.-]`, starts with a digit and never contains `..`: safe in
    /// a file name, a URL path and a tag.
    pub(crate) fn parse(version: &'a str) -> Option<Self> {
        if version.is_empty() || version.len() > MAX_LENGTH {
            return None;
        }
        let (core, pre) = match version.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (version, None),
        };
        let number = |part: &str| {
            (!part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part == "0" || !part.starts_with('0')))
            .then(|| part.parse::<u64>().ok())
            .flatten()
        };
        let mut parts = core.split('.');
        let core = [
            number(parts.next()?)?,
            number(parts.next()?)?,
            number(parts.next()?)?,
        ];
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|identifier| {
                    if identifier.is_empty()
                        || !identifier
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    {
                        None
                    } else if identifier.bytes().all(|byte| byte.is_ascii_digit()) {
                        number(identifier).map(Identifier::Numeric)
                    } else {
                        Some(Identifier::Alphanumeric(identifier))
                    }
                })
                .collect::<Option<_>>()?,
        };
        Some(Self { core, pre })
    }

    /// Whether it has a pre-release part, as `0.3.0-alpha.4` does.
    pub(crate) fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Semver<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.core.cmp(&other.core).then_with(|| {
            // A release sorts after its own pre-releases; otherwise compare
            // identifier by identifier, and a longer list wins a tie.
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Semver<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.10.1", "0.10.0"));
        assert!(is_newer("0.11.0", "0.10.9"));
        assert!(is_newer("1.0.0", "0.99.9"));
        assert!(is_newer("0.10.10", "0.10.9"));
        assert!(!is_newer("0.10.0", "0.10.0"));
        assert!(!is_newer("0.9.9", "0.10.0"));
        assert!(
            !is_newer("0.11.0-rc1", "0.10.0"),
            "pre-releases are not announced"
        );
        assert!(!is_newer("nightly", "0.10.0"));
        assert!(!is_newer("99.0.0.1", "0.10.0"));
        assert!(!is_newer("1.0", "0.10.0"));
    }

    #[test]
    fn a_release_candidate_hears_about_its_release_and_nothing_older() {
        assert!(is_newer("0.11.0", "0.11.0-rc1"));
        assert!(is_newer("0.11.1", "0.11.0-rc1"));
        assert!(!is_newer("0.11.0-rc1", "0.11.0"));
        assert!(!is_newer("0.11.0-rc2", "0.11.0-rc1"));
        assert!(!is_newer("0.10.0", "0.11.0-rc1"));
    }

    #[test]
    fn only_plain_stable_versions_reach_paths_and_urls() {
        assert!(is_plain_release("0.16.3"));
        for version in [
            "0.16.3-rc1",
            "0.16",
            "0.16.3/../x",
            " 0.16.3",
            "v0.16.3",
            "+1.2.3",
        ] {
            assert!(!is_plain_release(version), "{version}");
        }
    }

    fn semver(version: &str) -> Semver<'_> {
        Semver::parse(version).unwrap_or_else(|| panic!("{version}"))
    }

    #[test]
    fn prerelease_versions_follow_semver_precedence() {
        // The example chain from the semver specification, plus alphas past 9.
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.9",
            "1.0.0-alpha.10",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1-alpha.1",
            "1.10.0",
        ];
        for pair in ordered.windows(2) {
            assert!(
                semver(pair[0]) < semver(pair[1]),
                "{} < {}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(semver("0.3.0-alpha.4"), semver("0.3.0-alpha.4"));
        assert!(semver("0.3.0-alpha.4").is_prerelease());
        assert!(!semver("0.3.0").is_prerelease());
    }

    #[test]
    fn only_safe_semantic_versions_are_accepted() {
        for version in ["0.3.0", "0.3.0-alpha.4", "10.0.0-rc-1.x-y", "0.0.0-0"] {
            assert!(Semver::parse(version).is_some(), "{version}");
        }
        for version in [
            "",
            "v0.3.0-alpha.4",
            " 0.3.0-alpha.4",
            "0.3.0-alpha.4 ",
            "0.3.0-alpha/../../x",
            "0.3.0-alpha\\x",
            "0.3.0-alpha..4",
            "0.3.0-",
            "0.3.0-alpha.",
            "0.3.0-.alpha",
            "0.3.0-alpha_4",
            "0.3.0-alpha.04",
            "0.3.0+build.1",
            "0.3.0-alpha.4+build",
            "0.3.0-älpha",
            "0.3",
            "0.3.0.1",
            "03.0.0",
            "nightly",
            "0.3.0-alpha.99999999999999999999999",
            &format!("0.3.0-{}", "a".repeat(64)),
        ] {
            assert!(Semver::parse(version).is_none(), "{version:?}");
        }
    }
}
