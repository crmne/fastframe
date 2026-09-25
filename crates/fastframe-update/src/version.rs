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
}
