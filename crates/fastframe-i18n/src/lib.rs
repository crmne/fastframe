//! Bundled gettext catalogs for egui apps.
//!
//! Translations live in `assets/i18n/*.po`. At build time the app's
//! `build.rs` compiles each catalog into a Rust module (feature `build`, see
//! [`build`]), so the binary needs no libintl, parses no PO file at run time,
//! and makes no network request. English is the source language and the
//! fallback for any message a catalog has not translated yet.
//!
//! The app keeps its own `Locale` enum (the languages it ships, their tags
//! and native names) and implements [`Locale`] to name each one's catalog:
//!
//! ```ignore
//! include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));
//!
//! #[derive(Clone, Copy, Default)]
//! pub enum Locale { #[default] English, German }
//!
//! impl fastframe_i18n::Locale for Locale {
//!     fn catalog(self) -> Option<&'static dyn fastframe_i18n::Translator> {
//!         match self {
//!             Self::English => None,
//!             Self::German => Some(&de::Translator),
//!         }
//!     }
//! }
//!
//! pub use fastframe_i18n::{gettext, ngettext, pgettext};
//! ```
//!
//! [`gettext`], [`pgettext`] and [`ngettext`] keep the argument order the
//! apps' `xgettext` keywords expect (`gettext:2`, `pgettext:2c,3`,
//! `ngettext:2,3`).

use std::borrow::Cow;
use std::sync::OnceLock;

#[cfg(feature = "build")]
pub mod build;

/// The trait every compiled catalog implements, re-exported so apps and the
/// generated modules need no direct `tr` dependency.
pub use tr::Translator;

/// An interface language an app ships.
///
/// Implemented by the app's own `Locale` enum. The source language (English
/// in every app so far) has no catalog.
pub trait Locale: Copy {
    /// The compiled catalog for this language, or `None` for the source
    /// language.
    fn catalog(self) -> Option<&'static dyn Translator>;
}

/// Translates `source`, or returns it unchanged when the language has no
/// catalog or the catalog has not translated it.
pub fn gettext<L: Locale>(locale: L, source: &'static str) -> Cow<'static, str> {
    locale.catalog().map_or(Cow::Borrowed(source), |catalog| {
        catalog.translate(source, None)
    })
}

/// Translates a phrase whose meaning depends on where it appears, such as a
/// verb and a noun spelled alike. Falls back to `source`.
pub fn pgettext<L: Locale>(
    locale: L,
    context: &'static str,
    source: &'static str,
) -> Cow<'static, str> {
    locale.catalog().map_or(Cow::Borrowed(source), |catalog| {
        catalog.translate(source, Some(context))
    })
}

/// Chooses a whole translated phrase for `count` by the catalog's plural
/// rules. Without a catalog, English rules apply: `singular` for one,
/// `plural` otherwise (zero included).
pub fn ngettext<L: Locale>(
    locale: L,
    singular: &'static str,
    plural: &'static str,
    count: u32,
) -> Cow<'static, str> {
    locale.catalog().map_or(
        Cow::Borrowed(if count == 1 { singular } else { plural }),
        |catalog| catalog.ntranslate(count.into(), singular, plural, None),
    )
}

/// A language tag split into the parts that choose a catalog.
///
/// Reads BCP 47 (`pt-BR`, `zh-Hant-TW`, `es-419`) and POSIX locale names
/// (`es_UY.UTF-8`, `sr_RS.UTF-8@latin`, Windows' legacy `zh-CHT`). Every part
/// is lowercase.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LanguageTag {
    /// The language subtag: `pt` in `pt-BR`.
    pub language: String,
    /// The first four-letter script subtag: `hant` in `zh-Hant-TW`.
    pub script: Option<String>,
    /// The first region subtag: two letters (`br`), three digits (`419`), or
    /// Windows' three-letter Chinese scripts (`cht`, `chs`).
    pub region: Option<String>,
}

impl LanguageTag {
    /// Splits `tag`, or returns `None` when it names no language (empty,
    /// only separators). `C` and `POSIX` parse; no app has a catalog for them.
    #[must_use]
    pub fn parse(tag: &str) -> Option<Self> {
        let tag = tag.to_ascii_lowercase();
        // A POSIX name can carry an encoding and a modifier.
        let tag = tag.split(['.', '@']).next()?;
        let mut subtags = tag.split(['-', '_']).filter(|part| !part.is_empty());
        let language = subtags.next()?.to_owned();
        let mut script = None;
        let mut region = None;
        for subtag in subtags {
            match subtag.len() {
                4 => {
                    script.get_or_insert_with(|| subtag.to_owned());
                }
                2 | 3 => {
                    region.get_or_insert_with(|| subtag.to_owned());
                }
                _ => {}
            }
        }
        Some(Self {
            language,
            script,
            region,
        })
    }
}

/// The languages the operating system prefers, most preferred first.
///
/// `sys_locale` asks each platform its own way: the preferred languages on
/// macOS and Windows, and `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` and `LANG`
/// elsewhere. The answer is read once per process: on macOS it costs a Core
/// Foundation call, and apps ask again whenever the language setting changes.
pub fn system_languages() -> &'static [String] {
    static LANGUAGES: OnceLock<Vec<String>> = OnceLock::new();
    LANGUAGES.get_or_init(|| sys_locale::get_locales().collect())
}

/// The first of `tags` that `choose` maps to a supported language.
///
/// `tags` are in preference order, as [`system_languages`] returns them, so
/// a desktop that lists Norwegian and then German gets German when only
/// German has a catalog.
pub fn first_supported<L, S: AsRef<str>>(
    tags: impl IntoIterator<Item = S>,
    choose: impl Fn(&LanguageTag) -> Option<L>,
) -> Option<L> {
    tags.into_iter()
        .find_map(|tag| LanguageTag::parse(tag.as_ref()).and_then(|tag| choose(&tag)))
}

/// The supported language the operating system prefers, if any.
///
/// Unit tests that assert English strings should not call this: the language
/// of the machine running them would decide the outcome. Apps return their
/// source language under `cfg!(test)` before calling it.
pub fn detect<L>(choose: impl Fn(&LanguageTag) -> Option<L>) -> Option<L> {
    first_supported(system_languages(), choose)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalog written by hand, standing in for a compiled one.
    struct German;

    impl Translator for German {
        fn translate<'a>(&'a self, string: &'a str, context: Option<&'a str>) -> Cow<'a, str> {
            Cow::Borrowed(match (context, string) {
                (None, "Home") => "Start",
                (Some("lyrics"), "Follow") => "Folgen",
                _ => string,
            })
        }

        fn ntranslate<'a>(
            &'a self,
            n: u64,
            singular: &'a str,
            plural: &'a str,
            _context: Option<&'a str>,
        ) -> Cow<'a, str> {
            Cow::Borrowed(match singular {
                "{} member" if n == 1 => "{} Mitglied",
                "{} member" => "{} Mitglieder",
                _ if n == 1 => singular,
                _ => plural,
            })
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Language {
        English,
        German,
        PortugueseBrazil,
        PortuguesePortugal,
        ChineseSimplified,
        ChineseTraditional,
    }

    impl Locale for Language {
        fn catalog(self) -> Option<&'static dyn Translator> {
            match self {
                Self::German => Some(&German),
                _ => None,
            }
        }
    }

    /// Spotifast's mapping, the most detailed of the apps'.
    fn choose(tag: &LanguageTag) -> Option<Language> {
        Some(match tag.language.as_str() {
            "en" => Language::English,
            "de" => Language::German,
            "pt" => match tag.region.as_deref() {
                Some("pt" | "ao" | "mz") => Language::PortuguesePortugal,
                _ => Language::PortugueseBrazil,
            },
            "zh" => match (tag.script.as_deref(), tag.region.as_deref()) {
                (Some("hant"), _) => Language::ChineseTraditional,
                (Some("hans"), _) => Language::ChineseSimplified,
                (_, Some("tw" | "hk" | "mo" | "cht")) => Language::ChineseTraditional,
                _ => Language::ChineseSimplified,
            },
            _ => return None,
        })
    }

    #[test]
    fn missing_messages_and_the_source_language_use_the_english_source() {
        assert_eq!(gettext(Language::German, "Home"), "Start");
        assert_eq!(
            gettext(Language::German, "Not translated"),
            "Not translated"
        );
        assert_eq!(gettext(Language::English, "Home"), "Home");
    }

    #[test]
    fn contexts_do_not_leak_into_other_meanings() {
        assert_eq!(pgettext(Language::German, "lyrics", "Follow"), "Folgen");
        assert_eq!(pgettext(Language::German, "other", "Follow"), "Follow");
        assert_eq!(gettext(Language::German, "Follow"), "Follow");
        assert_eq!(pgettext(Language::English, "lyrics", "Follow"), "Follow");
    }

    #[test]
    fn plurals_use_the_catalog_or_english_rules() {
        assert_eq!(
            ngettext(Language::German, "{} member", "{} members", 1),
            "{} Mitglied"
        );
        assert_eq!(
            ngettext(Language::German, "{} member", "{} members", 0),
            "{} Mitglieder"
        );
        assert_eq!(
            ngettext(Language::English, "{} member", "{} members", 1),
            "{} member"
        );
        assert_eq!(
            ngettext(Language::English, "{} member", "{} members", 0),
            "{} members",
            "English treats zero as plural"
        );
    }

    #[test]
    fn tags_split_into_language_script_and_region() {
        let tag = LanguageTag::parse("zh-Hant-TW").unwrap();
        assert_eq!(tag.language, "zh");
        assert_eq!(tag.script.as_deref(), Some("hant"));
        assert_eq!(tag.region.as_deref(), Some("tw"));
        let tag = LanguageTag::parse("sr_RS.UTF-8@latin").unwrap();
        assert_eq!(
            (tag.language.as_str(), tag.region.as_deref()),
            ("sr", Some("rs"))
        );
        assert_eq!(
            LanguageTag::parse("es-419").unwrap().region.as_deref(),
            Some("419")
        );
        for tag in ["", "-", "_", ".UTF-8"] {
            assert_eq!(LanguageTag::parse(tag), None, "{tag:?}");
        }
    }

    #[test]
    fn system_tags_map_to_the_closest_catalog() {
        for (tag, expected) in [
            ("de", Language::German),
            ("de_CH.UTF-8", Language::German),
            ("en_US.UTF-8", Language::English),
            ("pt", Language::PortugueseBrazil),
            ("pt_BR.UTF-8", Language::PortugueseBrazil),
            ("pt_PT.UTF-8@euro", Language::PortuguesePortugal),
            ("pt-AO", Language::PortuguesePortugal),
            ("zh", Language::ChineseSimplified),
            ("zh_CN.GB2312", Language::ChineseSimplified),
            ("zh-Hans-HK", Language::ChineseSimplified),
            ("zh-TW", Language::ChineseTraditional),
            ("zh-Hant", Language::ChineseTraditional),
            ("zh-CHT", Language::ChineseTraditional),
        ] {
            assert_eq!(first_supported([tag], choose), Some(expected), "{tag}");
        }
        for tag in ["nb-NO", "ko-KR", "C", "POSIX", "", "und"] {
            assert_eq!(first_supported([tag], choose), None, "{tag}");
        }
    }

    #[test]
    fn the_first_preferred_language_with_a_catalog_wins() {
        assert_eq!(
            first_supported(["nb-NO", "de-DE", "en-US"], choose),
            Some(Language::German)
        );
        assert_eq!(first_supported(["ko-KR"], choose), None);
        assert_eq!(first_supported(Vec::<String>::new(), choose), None);
    }

    #[test]
    fn asking_the_system_never_panics() {
        // Whatever this machine prefers, including nothing.
        let _ = detect(choose);
        assert_eq!(system_languages(), system_languages());
    }
}
