//! The Lucide icons ZapFast and Spotifast shipped byte for byte alike.
//!
//! Lucide is ISC licensed, and some of its icons derive from Feather (MIT);
//! both notices are in `icons/lucide/LICENSE.txt`, which apps ship with
//! their other licences. The files are Lucide's 24 px outlines drawn in
//! white (`stroke="#ffffff"`), so egui's tint colours them.
//!
//! Name one in [`crate::icons!`] with `lucide "name"`. The lookup runs at
//! compile time, so only the icons an app names reach its binary, and a
//! misspelt name fails the build.

/// Every shared icon: its Lucide name and its SVG.
pub const ALL: &[(&str, &[u8])] = &[
    (
        "arrow-left",
        include_bytes!("../icons/lucide/arrow-left.svg"),
    ),
    ("check", include_bytes!("../icons/lucide/check.svg")),
    (
        "chevron-down",
        include_bytes!("../icons/lucide/chevron-down.svg"),
    ),
    (
        "chevron-left",
        include_bytes!("../icons/lucide/chevron-left.svg"),
    ),
    (
        "chevron-right",
        include_bytes!("../icons/lucide/chevron-right.svg"),
    ),
    (
        "chevron-up",
        include_bytes!("../icons/lucide/chevron-up.svg"),
    ),
    (
        "circle-alert",
        include_bytes!("../icons/lucide/circle-alert.svg"),
    ),
    (
        "circle-check",
        include_bytes!("../icons/lucide/circle-check.svg"),
    ),
    ("circle-x", include_bytes!("../icons/lucide/circle-x.svg")),
    ("clock", include_bytes!("../icons/lucide/clock.svg")),
    ("copy", include_bytes!("../icons/lucide/copy.svg")),
    ("ellipsis", include_bytes!("../icons/lucide/ellipsis.svg")),
    (
        "external-link",
        include_bytes!("../icons/lucide/external-link.svg"),
    ),
    ("eye", include_bytes!("../icons/lucide/eye.svg")),
    ("eye-off", include_bytes!("../icons/lucide/eye-off.svg")),
    ("info", include_bytes!("../icons/lucide/info.svg")),
    ("lock", include_bytes!("../icons/lucide/lock.svg")),
    ("log-out", include_bytes!("../icons/lucide/log-out.svg")),
    (
        "maximize-2",
        include_bytes!("../icons/lucide/maximize-2.svg"),
    ),
    ("mic", include_bytes!("../icons/lucide/mic.svg")),
    (
        "minimize-2",
        include_bytes!("../icons/lucide/minimize-2.svg"),
    ),
    ("minus", include_bytes!("../icons/lucide/minus.svg")),
    ("monitor", include_bytes!("../icons/lucide/monitor.svg")),
    ("moon", include_bytes!("../icons/lucide/moon.svg")),
    (
        "panel-left",
        include_bytes!("../icons/lucide/panel-left.svg"),
    ),
    ("pause", include_bytes!("../icons/lucide/pause.svg")),
    ("pencil", include_bytes!("../icons/lucide/pencil.svg")),
    ("pin", include_bytes!("../icons/lucide/pin.svg")),
    ("pin-off", include_bytes!("../icons/lucide/pin-off.svg")),
    ("play", include_bytes!("../icons/lucide/play.svg")),
    ("plus", include_bytes!("../icons/lucide/plus.svg")),
    (
        "refresh-cw",
        include_bytes!("../icons/lucide/refresh-cw.svg"),
    ),
    ("search", include_bytes!("../icons/lucide/search.svg")),
    ("settings", include_bytes!("../icons/lucide/settings.svg")),
    (
        "smartphone",
        include_bytes!("../icons/lucide/smartphone.svg"),
    ),
    (
        "square-pen",
        include_bytes!("../icons/lucide/square-pen.svg"),
    ),
    ("sun", include_bytes!("../icons/lucide/sun.svg")),
    ("trash-2", include_bytes!("../icons/lucide/trash-2.svg")),
    ("user", include_bytes!("../icons/lucide/user.svg")),
    ("users", include_bytes!("../icons/lucide/users.svg")),
    ("volume-2", include_bytes!("../icons/lucide/volume-2.svg")),
    ("volume-x", include_bytes!("../icons/lucide/volume-x.svg")),
    ("x", include_bytes!("../icons/lucide/x.svg")),
];

/// The SVG of the shared icon called `name`.
///
/// # Panics
///
/// When no shared icon has that name. Evaluate it in a constant (as
/// [`crate::icons!`] does) and the panic is a compile error instead.
#[must_use]
pub const fn get(name: &str) -> &'static [u8] {
    let mut index = 0;
    while index < ALL.len() {
        if same(ALL[index].0, name) {
            return ALL[index].1;
        }
        index += 1;
    }
    panic!("not one of fastframe-icons' shared Lucide icons")
}

/// Whether two names are equal, in a form `const fn` allows.
const fn same(one: &str, other: &str) -> bool {
    let (one, other) = (one.as_bytes(), other.as_bytes());
    if one.len() != other.len() {
        return false;
    }
    let mut index = 0;
    while index < one.len() {
        if one[index] != other[index] {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shared_icon_is_a_white_24px_lucide_outline() {
        assert_eq!(ALL.len(), 43);
        for (name, bytes) in ALL {
            let svg = std::str::from_utf8(bytes).unwrap();
            assert!(svg.contains("viewBox=\"0 0 24 24\""), "{name}");
            assert!(svg.contains("stroke=\"#ffffff\""), "{name}");
            assert_eq!(get(name), *bytes, "{name}");
        }
    }

    #[test]
    fn names_are_unique_and_sorted() {
        for pair in ALL.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{} then {}", pair[0].0, pair[1].0);
        }
    }

    #[test]
    #[should_panic(expected = "not one of")]
    fn an_unknown_name_is_refused() {
        let name = String::from("no-such-icon");
        let _ = get(&name);
    }
}
