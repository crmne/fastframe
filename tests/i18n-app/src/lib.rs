//! An app-shaped fixture for `fastframe-i18n`: its build script compiles the
//! catalogs in `assets/i18n`, and this module wires them into a `Locale` the
//! way ZapFast and Spotifast do.

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

pub use fastframe_i18n::{gettext, ngettext, pgettext};

/// The languages this fixture ships.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locale {
    /// The source language, with no catalog.
    #[default]
    English,
    /// Three plural forms.
    Polish,
    /// A constant plural rule.
    Japanese,
    /// A hyphenated tag.
    PortugueseBrazil,
}

impl fastframe_i18n::Locale for Locale {
    fn catalog(self) -> Option<&'static dyn fastframe_i18n::Translator> {
        match self {
            Self::English => None,
            Self::Polish => Some(&pl::Translator),
            Self::Japanese => Some(&ja::Translator),
            Self::PortugueseBrazil => Some(&pt_br::Translator),
        }
    }
}

/// The plural forms each compiled catalog declares.
#[must_use]
pub fn plural_forms() -> [usize; 3] {
    [pl::PLURALS, ja::PLURALS, pt_br::PLURALS]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_catalogs_omit_unfinished_messages() {
        assert_eq!(plural_forms(), [3, 1, 2]);
        assert_eq!(gettext(Locale::Polish, "Home"), "Start");
        assert_eq!(gettext(Locale::PortugueseBrazil, "Home"), "Início");
        for source in ["Search", "Library", "Missing", "Removed"] {
            assert_eq!(gettext(Locale::Polish, source), source);
        }
        assert_eq!(gettext(Locale::English, "Home"), "Home");
    }

    #[test]
    fn contexts_are_kept_apart() {
        assert_eq!(pgettext(Locale::Polish, "lyrics", "Follow"), "Śledź");
        assert_eq!(gettext(Locale::Polish, "Follow"), "Follow");
    }

    #[test]
    fn plural_rules_come_from_each_catalog() {
        for count in [0, 1, 2, 5, 12, 22, 101, 112] {
            let singular = "Playlist • {count} song";
            let plural = "Playlist • {count} songs";
            assert_eq!(
                ngettext(Locale::Polish, singular, plural, count),
                if count == 1 { singular } else { plural },
                "an incomplete plural falls back to a whole English phrase"
            );
            assert_eq!(
                ngettext(Locale::Polish, "{count} track", "{count} tracks", count),
                match count {
                    1 => "{count} utwór",
                    2 | 22 => "{count} utwory",
                    _ => "{count} utworów",
                }
            );
            assert_eq!(
                ngettext(Locale::Japanese, "{count} track", "{count} tracks", count),
                "{count}曲"
            );
        }
    }
}
