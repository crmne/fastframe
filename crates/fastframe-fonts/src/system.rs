//! Installed fonts for the scripts Inter does not draw.
//!
//! Inter covers Latin, Greek and Cyrillic. Bundling a font for every other
//! script would add hundreds of megabytes, so one suitable face per script
//! is borrowed from what the desktop already has.
//!
//! - macOS answers per script itself, in the language the user reads:
//!   CoreText names the face it would draw with, and its cascade behind it.
//! - Linux and Windows offer nothing equivalent, so every installed font is
//!   probed (a memory map touches only the header, names and character map)
//!   and the best regular sans face per script is kept. On Linux the
//!   directories come from fontconfig's configuration (NixOS lists its store
//!   paths only there), the XDG data directories, Flatpak's host fonts and
//!   the per-user directories.
//!
//! Each chosen face is then adjusted to sit with Inter: its baseline is
//! aligned (epaint centres a fallback's line box on Inter's, which shifts
//! faces with other vertical metrics), and Arabic is enlarged so it reads as
//! large as Latin text beside it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use skrifa::MetadataProvider as _;

#[cfg(target_os = "macos")]
mod macos;

/// An installed face registered as a fallback.
#[derive(Clone, Debug, PartialEq)]
pub struct Fallback {
    /// The font data key: `fallback-<script>`, after the first script that
    /// chose the face.
    pub name: String,
    /// The font file.
    pub bytes: Vec<u8>,
    /// The face within a collection.
    pub index: u32,
    /// Size relative to Inter, so the script reads as large as Latin text
    /// (above 1 only for small Arabic faces).
    pub scale: f32,
    /// Vertical shift, in ems, that puts the face's baseline on Inter's.
    pub y_offset_factor: f32,
}

/// A script Inter does not cover: its name, a character to probe for, and a
/// fragment of the family names drawn for it.
///
/// One face is chosen per entry, in this order. A face covering several
/// scripts (a pan-CJK collection covers three) is registered once, under
/// the first. Glyphs are rasterized only when used.
pub(crate) const SCRIPTS: &[(&str, char, &str)] = &[
    ("han", '\u{4e2d}', "cjk"),
    ("kana", '\u{3042}', "cjk"),
    ("hangul", '\u{d55c}', "cjk"),
    ("arabic", '\u{0627}', "arabic"),
    ("hebrew", '\u{05d0}', "hebrew"),
    ("thai", '\u{0e01}', "thai"),
    ("lao", '\u{0e81}', "lao"),
    ("khmer", '\u{1780}', "khmer"),
    ("myanmar", '\u{1000}', "myanmar"),
    ("devanagari", '\u{0915}', "devanagari"),
    ("bengali", '\u{0995}', "bengali"),
    ("gurmukhi", '\u{0a15}', "gurmukhi"),
    ("gujarati", '\u{0a95}', "gujarati"),
    ("tamil", '\u{0ba4}', "tamil"),
    ("telugu", '\u{0c15}', "telugu"),
    ("kannada", '\u{0c95}', "kannada"),
    ("malayalam", '\u{0d15}', "malayalam"),
    ("sinhala", '\u{0d9a}', "sinhala"),
    ("armenian", '\u{0531}', "armenian"),
    ("georgian", '\u{10d0}', "georgian"),
    ("ethiopic", '\u{1200}', "ethiopic"),
    ("cherokee", '\u{13a0}', "cherokee"),
    ("yi", '\u{a248}', "yi"),
    // Ornamental and styled characters people put in display names.
    ("javanese", '\u{a9c1}', "javanese"),
    ("math", '\u{1d4d0}', "math"),
    ("enclosed", '\u{24b6}', "symbol"),
    ("symbols", '\u{2605}', "symbol"),
    ("suits", '\u{2661}', "symbol"),
];

/// How deep to walk each font directory. Distributions nest a level or two
/// (`/usr/share/fonts/truetype/noto`); the bound also ends symlink loops.
const FONT_SCAN_DEPTH: usize = 4;

/// A collection says how many faces it holds, and a corrupt file can say
/// billions. No real one holds more than a few dozen.
pub(crate) const MAX_FACES: u32 = 64;

/// One installed face per script Inter does not draw.
///
/// Found once per process (a walk of every installed font, or a question per
/// script to CoreText) and reused by every window the app creates. The
/// result depends on the machine and may be empty.
pub fn fallbacks() -> &'static [Fallback] {
    static FONTS: OnceLock<Vec<Fallback>> = OnceLock::new();
    FONTS.get_or_init(|| {
        #[cfg(target_os = "macos")]
        let found = macos::load();
        #[cfg(not(target_os = "macos"))]
        let found = scan();
        found.into_iter().map(adjusted).collect()
    })
}

/// A face found for `script`, before adjusting it to Inter.
pub(crate) struct Found {
    pub(crate) script: &'static str,
    pub(crate) bytes: Vec<u8>,
    pub(crate) index: u32,
    /// The file it was read from, for the log.
    pub(crate) path: std::path::PathBuf,
}

fn adjusted(found: Found) -> Fallback {
    let scale = if found.script == "arabic" {
        arabic_scale(&found.bytes, found.index)
    } else {
        1.0
    };
    let y_offset_factor = baseline_offset(&found.bytes, found.index);
    // At info, so a user's ordinary log says which face draws each script
    // when one looks wrong.
    log::info!(
        "{} fallback: {} ({}, face {}), scale {scale:.2}, baseline {y_offset_factor:+.3} em",
        found.script,
        family_name(&found.bytes, found.index).unwrap_or_else(|| "unnamed".into()),
        found.path.display(),
        found.index,
    );
    Fallback {
        name: format!("fallback-{}", found.script),
        bytes: found.bytes,
        index: found.index,
        scale,
        y_offset_factor,
    }
}

/// A face's family name, as the font names it in English if it can.
fn family_name(bytes: &[u8], index: u32) -> Option<String> {
    skrifa::FontRef::from_index(bytes, index)
        .ok()?
        .localized_strings(skrifa::string::StringId::FAMILY_NAME)
        .english_or_first()
        .map(|name| name.to_string())
}

/// Every face in a font file, up to [`MAX_FACES`].
pub(crate) fn faces(data: &[u8]) -> Vec<(u32, skrifa::FontRef<'_>)> {
    match skrifa::raw::FileRef::new(data) {
        Ok(skrifa::raw::FileRef::Font(font)) => vec![(0, font)],
        Ok(skrifa::raw::FileRef::Collection(collection)) => (0..collection.len().min(MAX_FACES))
            .filter_map(|index| collection.get(index).ok().map(|font| (index, font)))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Whether a face draws `character`: a character map entry alone is not
/// enough, since bitmap and colour-only fonts map characters and draw
/// nothing epaint can rasterize.
pub(crate) fn draws(font: &skrifa::FontRef<'_>, character: char) -> bool {
    let outlines = font.outline_glyphs();
    font.charmap()
        .map(character)
        .is_some_and(|glyph| outlines.get(glyph).is_some())
}

/// Reads a font file through a read-only memory map, so probing touches only
/// the pages holding its header, names and character map. Reading every
/// font on a normal Linux tree whole costs half a second; mapping them costs
/// a tenth of that.
#[allow(
    unsafe_code,
    reason = "memory mapping is the only way to probe fonts without reading them whole"
)]
pub(crate) fn map(path: &Path) -> Option<memmap2::Mmap> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: the mapping is read-only and callers drop it before returning.
    // A font file rewritten underneath it during that window could fault,
    // the same bet every font enumerator on the platform makes.
    unsafe { memmap2::Mmap::map(&file) }.ok()
}

/// Inter's baseline measured from the centre of its line box, in ems:
/// ascender 1984, descender -494, no line gap, 2048 units per em.
const INTER_BASELINE_CENTER: f32 = (1984.0 / 2048.0) - 0.5 * ((1984.0 + 494.0) / 2048.0);

/// The shift that puts a fallback face's baseline on Inter's.
///
/// epaint places a fallback glyph at
/// `fallback.ascent + 0.5 * (primary.row_height - fallback.row_height)`, which
/// centres the two line boxes. A face with other vertical metrics (Hiragino
/// Sans declares a 0.5 em line gap) then sits above or below Latin text.
/// Offsetting by the difference in baseline-to-centre distances undoes the
/// centring at every size.
fn baseline_offset(bytes: &[u8], index: u32) -> f32 {
    let Ok(font) = skrifa::FontRef::from_index(bytes, index) else {
        return 0.0;
    };
    let metrics = font.metrics(
        skrifa::instance::Size::unscaled(),
        skrifa::instance::LocationRef::default(),
    );
    let units = f32::from(metrics.units_per_em);
    let height = metrics.ascent - metrics.descent + metrics.leading;
    if units <= 0.0 || height <= 0.0 {
        return 0.0;
    }
    let offset = INTER_BASELINE_CENTER - (metrics.ascent - 0.5 * height) / units;
    if offset.abs() > 0.001 { offset } else { 0.0 }
}

/// How much to enlarge an Arabic face so it reads as large as Inter.
///
/// Arabic letters sit lower than Latin ones, and many system faces draw them
/// small beside Inter. The body of heh (ه), a letter without ascenders or
/// descenders, plays the part of the x-height. Faces already drawn to match
/// Latin text are left alone.
fn arabic_scale(bytes: &[u8], index: u32) -> f32 {
    let height = |bytes: &[u8], index: u32, probe: char| -> Option<f32> {
        let font = skrifa::FontRef::from_index(bytes, index).ok()?;
        let glyph = font.charmap().map(probe)?;
        let bounds = font
            .glyph_metrics(
                skrifa::instance::Size::unscaled(),
                skrifa::instance::LocationRef::default(),
            )
            .bounds(glyph)?;
        let units = f32::from(
            font.metrics(
                skrifa::instance::Size::unscaled(),
                skrifa::instance::LocationRef::default(),
            )
            .units_per_em,
        );
        Some((bounds.y_max - bounds.y_min.max(0.0)) / units)
    };
    match (
        height(crate::INTER, 0, 'x'),
        height(bytes, index, '\u{0647}'),
    ) {
        (Some(latin), Some(arabic)) if arabic > 0.0 => scale_for(latin, arabic),
        _ => 1.0,
    }
}

/// The scale that brings `arabic` to `latin`: never smaller, at most 25%
/// larger so Arabic stays in proportion, and 1 for differences too small to
/// see.
fn scale_for(latin: f32, arabic: f32) -> f32 {
    let scale = (latin / arabic).clamp(1.0, 1.25);
    if scale < 1.04 {
        1.0
    } else {
        (scale * 100.0).round() / 100.0
    }
}

/// Whether a path names a font file this can open.
fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        })
}

#[cfg(not(target_os = "macos"))]
pub(crate) use scan::scan;

/// The ranking that serves platforms with no answer of their own. Compiled
/// everywhere so its tests run on every platform; only Linux and Windows
/// call it.
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod scan {
    use super::{FONT_SCAN_DEPTH, Found, SCRIPTS, draws, faces, is_font_file, map};
    use skrifa::MetadataProvider as _;
    use skrifa::raw::TableProvider as _;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// The regional cut of a pan-CJK font a locale reads, longest prefix
    /// first.
    const HAN_REGIONS: &[(&str, &str)] = &[
        ("zh_tw", "tc"),
        ("zh_hant", "tc"),
        ("zh_hk", "hk"),
        ("zh_mo", "hk"),
        ("zh", "sc"),
        ("ja", "jp"),
        ("ko", "kr"),
    ];

    /// A face that covers a script, and how well it suits the interface.
    pub(super) struct Candidate {
        /// Drawn for another Han region than the locale's. A Japanese face
        /// draws 中 and passes the probe, but every simplified character it
        /// lacks would come from whatever fallback follows, at another size
        /// and baseline, so it serves only when nothing else covers Han.
        pub(super) foreign: bool,
        pub(super) score: u32,
        pub(super) path: PathBuf,
        pub(super) index: u32,
    }

    /// Finds the best face for each script and reads the files they live in.
    pub(crate) fn scan() -> Vec<Found> {
        let han = han_region(&locale());
        let started = std::time::Instant::now();
        let mut best: BTreeMap<&'static str, Candidate> = BTreeMap::new();
        for dir in super::font_directories() {
            probe_dir(&dir, 0, han, &mut best);
        }
        log::debug!(
            "probed the system fonts in {:.1} ms, {} of {} scripts covered",
            started.elapsed().as_secs_f32() * 1e3,
            best.len(),
            SCRIPTS.len()
        );
        let mut found: Vec<Found> = Vec::new();
        let mut taken: Vec<(PathBuf, u32)> = Vec::new();
        for (script, _, _) in SCRIPTS {
            let Some(candidate) = best.get(script) else {
                log::debug!("no fallback face covers {script}");
                continue;
            };
            if taken.contains(&(candidate.path.clone(), candidate.index)) {
                continue;
            }
            let bytes = match std::fs::read(&candidate.path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    log::warn!("cannot read {}: {error}", candidate.path.display());
                    continue;
                }
            };
            taken.push((candidate.path.clone(), candidate.index));
            found.push(Found {
                script,
                bytes,
                index: candidate.index,
                path: candidate.path.clone(),
            });
        }
        found
    }

    /// Probes every font file below `dir`, keeping the best face per script.
    fn probe_dir(
        dir: &Path,
        depth: usize,
        han: &str,
        best: &mut BTreeMap<&'static str, Candidate>,
    ) {
        if depth >= FONT_SCAN_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            // The kind comes with the listing; only a symlink (Debian nests
            // its tree behind them, as does Flatpak's /run/host/fonts) costs
            // a look at the target.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() || (kind.is_symlink() && path.is_dir()) {
                probe_dir(&path, depth + 1, han, best);
            } else if is_font_file(&path) {
                probe_file(&path, han, best);
            }
        }
    }

    /// Offers every face in one font file to every script.
    pub(super) fn probe_file(path: &Path, han: &str, best: &mut BTreeMap<&'static str, Candidate>) {
        let Some(data) = map(path) else {
            return;
        };
        for (index, font) in faces(&data) {
            let attributes = font.attributes();
            if attributes.style != skrifa::attribute::Style::Normal {
                continue;
            }
            let family = font
                .localized_strings(skrifa::string::StringId::FAMILY_NAME)
                .english_or_first()
                .map(|name| name.to_string())
                .unwrap_or_default()
                .to_lowercase();
            for (script, probe, hint) in SCRIPTS {
                if !draws(&font, *probe) {
                    continue;
                }
                let score = face_score(&family, attributes.weight.value(), han, hint);
                let foreign = *script == "han" && {
                    let code_pages = font.os2().ok().and_then(|os2| os2.ul_code_page_range_1());
                    !covers_han_region(code_pages, han)
                };
                // Ties break on the path, so two machines with the same fonts
                // choose the same face whatever order their directories list.
                if best.get(script).is_none_or(|held| {
                    (foreign, score, path) < (held.foreign, held.score, held.path.as_path())
                }) {
                    best.insert(
                        script,
                        Candidate {
                            foreign,
                            score,
                            path: path.to_path_buf(),
                            index,
                        },
                    );
                }
            }
        }
    }

    /// Ranks a face as interface text for the script named by `hint`,
    /// lowest first.
    ///
    /// Serif, monospace and display cuts lose to a plain sans, and anything
    /// far from regular weight loses to something near it, so the fallback
    /// sits beside Inter rather than shouting over it.
    pub(super) fn face_score(family: &str, weight: f32, han: &str, hint: &str) -> u32 {
        let mut score = ((weight - 400.0).abs() / 25.0) as u32;
        // A face that names the script was drawn for it: Liberation Sans
        // carries enough Hebrew to pass the probe, but Noto Sans Hebrew is
        // the one a reader wants.
        if !family.contains(hint) {
            score += 25;
        }
        // A family that calls itself sans is meant for interface sizes; the
        // rest of a Noto set are specialised cuts (Nastaliq, Rashi, Kufi).
        // Symbol faces rarely say "sans".
        if !family.contains("sans") && hint != "emoji" && hint != "symbol" {
            score += 50;
        }
        for (fragment, penalty) in [
            ("serif", 200),
            ("mono", 120),
            ("kufi", 80),
            ("naskh", 80),
            ("looped", 80),
            ("display", 80),
            ("condensed", 60),
            ("caption", 40),
        ] {
            if family.contains(fragment) {
                score += penalty;
            }
        }
        // Han characters are unified in Unicode, so 直 has a Japanese shape
        // and a Chinese one and the font decides which a reader sees. A
        // pan-CJK family names its regional cut last ("Noto Sans CJK SC").
        if let Some(region) = family
            .rsplit(' ')
            .next()
            .filter(|region| han_code_page(region).is_some())
            && region != han
        {
            score += 40;
        }
        score
    }

    /// The OS/2 `ulCodePageRange1` bit a face sets to declare a region's
    /// legacy character set: Shift JIS, GB 2312, Wansung, or Big5.
    pub(super) fn han_code_page(region: &str) -> Option<u32> {
        match region {
            "jp" => Some(17),
            "sc" => Some(18),
            "kr" => Some(19),
            "tc" | "hk" => Some(20),
            _ => None,
        }
    }

    /// Whether a face's declared code pages include the region's character
    /// set. A face too old to declare any is taken at its word: none.
    pub(super) fn covers_han_region(code_pages: Option<u32>, han: &str) -> bool {
        han_code_page(han)
            .zip(code_pages)
            .is_some_and(|(bit, pages)| pages & (1 << bit) != 0)
    }

    /// The user's locale as [`han_region`] matches it, or an empty string.
    /// A variable that is set but empty does not count, as POSIX has it.
    fn locale() -> String {
        let named = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()));
        // A Windows desktop sets none of those; the variables still win when
        // someone sets one.
        #[cfg(windows)]
        let named = named.or_else(super::windows::language);
        normalise(named.unwrap_or_default())
    }

    /// Lowercase, with `_` between language and region: Windows and BCP 47
    /// hyphenate.
    pub(super) fn normalise(name: String) -> String {
        name.to_lowercase().replace('-', "_")
    }

    /// The pan-CJK cut a locale reads, defaulting to Simplified Chinese,
    /// the most widely read.
    pub(super) fn han_region(locale: &str) -> &'static str {
        HAN_REGIONS
            .iter()
            .find(|(prefix, _)| locale.starts_with(prefix))
            .map_or("sc", |(_, region)| *region)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_font_that_draws_a_yi_name_is_chosen() {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/yi/YiTest.ttf");
            let mut best = BTreeMap::new();
            probe_file(&path, "sc", &mut best);
            let chosen = best.get("yi").expect("the Yi face covers Yi");
            let bytes = std::fs::read(&chosen.path).unwrap();
            let font = skrifa::FontRef::from_index(&bytes, chosen.index).unwrap();
            for character in "ꉈꀧ꒒꒒ꁄꍈꍈꀧ꒦ꉈꉣꅔꎡꅔꁕꁄ".chars() {
                assert!(draws(&font, character), "{character}");
            }
            assert!(!best.contains_key("han"));
        }

        #[test]
        fn inter_covers_none_of_the_fallback_scripts() {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fonts/InterVariable.ttf");
            let mut best = BTreeMap::new();
            probe_file(&path, "sc", &mut best);
            let covered: Vec<_> = best.keys().collect();
            assert!(
                covered
                    .iter()
                    .all(|script| ["symbols", "suits", "enclosed", "math"].contains(script)),
                "Inter should not claim a script it lacks: {covered:?}"
            );
        }

        #[test]
        fn locales_choose_a_pan_cjk_cut() {
            assert_eq!(han_region("zh_cn.utf-8"), "sc");
            assert_eq!(han_region("zh_tw.utf-8"), "tc");
            assert_eq!(han_region("zh_hk.utf-8"), "hk");
            assert_eq!(han_region("ja_jp.utf-8"), "jp");
            assert_eq!(han_region("ko_kr.utf-8"), "kr");
            assert_eq!(han_region("en_us.utf-8"), "sc", "the default");
            assert_eq!(han_region(""), "sc", "no locale set");
        }

        #[test]
        fn hyphenated_language_names_choose_a_cut_too() {
            let cut = |name: &str| han_region(&normalise(name.to_owned()));
            assert_eq!(cut("ko-KR"), "kr");
            assert_eq!(cut("ja-JP"), "jp");
            assert_eq!(cut("zh-Hant-TW"), "tc");
            assert_eq!(cut("zh-HK"), "hk");
            assert_eq!(cut("en-US"), "sc");
        }

        #[test]
        fn a_face_declares_the_regions_it_covers() {
            // Hiragino Sans covers 中 but declares only Shift JIS.
            let japanese = Some(1 << 17);
            assert!(covers_han_region(japanese, "jp"));
            assert!(!covers_han_region(japanese, "sc"));
            assert!(!covers_han_region(None, "sc"), "no OS/2 table, no claim");
            for (_, region) in HAN_REGIONS {
                assert!(han_code_page(region).is_some(), "{region}");
            }
        }

        #[test]
        fn interface_faces_outrank_display_ones() {
            let sans = face_score("noto sans arabic", 400.0, "sc", "arabic");
            for other in [
                "noto naskh arabic",
                "noto kufi arabic",
                "noto nastaliq urdu",
                "noto serif arabic",
            ] {
                assert!(sans < face_score(other, 400.0, "sc", "arabic"), "{other}");
            }
            assert!(sans < face_score("noto sans arabic", 700.0, "sc", "arabic"));
        }

        #[test]
        fn a_face_drawn_for_the_script_wins() {
            assert!(
                face_score("noto sans hebrew", 400.0, "sc", "hebrew")
                    < face_score("liberation sans", 400.0, "sc", "hebrew")
            );
        }

        #[test]
        fn the_locale_picks_between_regional_cuts() {
            let simplified = face_score("noto sans cjk sc", 400.0, "sc", "cjk");
            assert!(simplified < face_score("noto sans cjk jp", 400.0, "sc", "cjk"));
            assert_eq!(
                face_score("noto sans cjk tc", 400.0, "tc", "cjk"),
                simplified
            );
        }

        #[test]
        fn symbol_faces_need_not_call_themselves_sans() {
            assert!(
                face_score("noto sans symbols", 400.0, "sc", "symbol")
                    < face_score("noto sans arabic", 400.0, "sc", "symbol")
            );
            assert_eq!(
                face_score("dejavu", 400.0, "sc", "symbol"),
                25,
                "only the missing hint counts against it, not a missing \"sans\""
            );
        }
    }
}

/// Where the platform keeps installed fonts, in the order they are probed.
///
/// On Linux: the directories fontconfig's configuration names, the XDG data
/// directories' `fonts`, Flatpak's `/run/host/fonts`, `~/.fonts` and
/// `$XDG_DATA_HOME/fonts`. For an app that looks for a particular family
/// itself (Spotifast's playlist face).
pub fn font_directories() -> Vec<PathBuf> {
    let user = directories::UserDirs::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut add = |dir: PathBuf| {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    if cfg!(target_os = "macos") {
        add(PathBuf::from("/System/Library/Fonts"));
        add(PathBuf::from("/Library/Fonts"));
    } else if cfg!(target_os = "windows") {
        add(std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
            .join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            add(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
    } else {
        let home = user.as_ref().map(|user| user.home_dir().to_path_buf());
        for dir in fontconfig_dirs(&fontconfig_root(), home.as_deref()) {
            add(dir);
        }
        // Every data directory fontconfig looks in, in its order (Guix keeps
        // its fonts outside the usual two).
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
            add(PathBuf::from(dir).join("fonts"));
        }
        // What a Flatpak sees of the host's fonts.
        add(PathBuf::from("/run/host/fonts"));
        // The pre-XDG per-user directory, which fontconfig still honours.
        if let Some(home) = home {
            add(home.join(".fonts"));
        }
    }
    // ~/Library/Fonts on macOS, $XDG_DATA_HOME/fonts on Linux.
    if let Some(font_dir) = user.as_ref().and_then(|user| user.font_dir()) {
        add(font_dir.to_path_buf());
    }
    dirs
}

/// fontconfig's main configuration file: `FONTCONFIG_FILE` when absolute,
/// else `fonts.conf` in `FONTCONFIG_PATH` or `/etc/fonts`.
fn fontconfig_root() -> PathBuf {
    std::env::var_os("FONTCONFIG_FILE")
        .map(PathBuf::from)
        .filter(|file| file.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("FONTCONFIG_PATH")
                .map_or_else(|| PathBuf::from("/etc/fonts"), PathBuf::from)
                .join("fonts.conf")
        })
}

/// How deeply configuration files may include one another.
const FONTCONFIG_INCLUDE_DEPTH: usize = 8;

/// The font directories fontconfig's own configuration names. NixOS keeps
/// each font package in its own store path and lists those paths only
/// there, so the data directories alone find none of them.
fn fontconfig_dirs(root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut read = Vec::new();
    read_fontconfig(root, home, 0, &mut read, &mut dirs);
    dirs
}

/// Collects the `<dir>` entries of one configuration file and of the files
/// and directories it includes. Paths relative to a file are relative to its
/// directory; `prefix="xdg"` entries are left out, because the data
/// directories are asked for separately.
fn read_fontconfig(
    path: &Path,
    home: Option<&Path>,
    depth: usize,
    read: &mut Vec<PathBuf>,
    dirs: &mut Vec<PathBuf>,
) {
    if depth > FONTCONFIG_INCLUDE_DEPTH || read.iter().any(|seen| seen == path) {
        return;
    }
    read.push(path.to_path_buf());
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|file| {
                file.extension()
                    .is_some_and(|extension| extension == "conf")
            })
            .collect();
        files.sort();
        for file in files {
            read_fontconfig(&file, home, depth + 1, read, dirs);
        }
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let base = path.parent().unwrap_or(Path::new("/"));
    for (tag, attributes, value) in fontconfig_elements(&text) {
        if attributes.contains("prefix=\"xdg\"") {
            continue;
        }
        let Some(resolved) = fontconfig_path(&value, base, home) else {
            continue;
        };
        if tag == "dir" {
            if !dirs.contains(&resolved) {
                dirs.push(resolved);
            }
        } else {
            read_fontconfig(&resolved, home, depth + 1, read, dirs);
        }
    }
}

/// The `<dir>` and `<include>` elements of a configuration file, with their
/// attributes and text, outside comments.
fn fontconfig_elements(text: &str) -> Vec<(&'static str, String, String)> {
    let mut elements = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let tag = ["dir", "include"].into_iter().find(|tag| {
            rest[1..].starts_with(tag)
                && rest[1 + tag.len()..].starts_with(|c: char| c == '>' || c.is_whitespace())
        });
        let Some(tag) = tag else {
            rest = &rest[1..];
            continue;
        };
        let Some(head_end) = rest.find('>') else {
            break;
        };
        let attributes = rest[1 + tag.len()..head_end].trim().to_owned();
        let body = &rest[head_end + 1..];
        let close = format!("</{tag}>");
        let Some(body_end) = body.find(&close) else {
            break;
        };
        let value = body[..body_end]
            .trim()
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&");
        elements.push((tag, attributes, value));
        rest = &body[body_end + close.len()..];
    }
    elements
}

/// Resolves a path from a configuration file: `~` is the home directory and
/// a relative path is relative to the file's own directory. The files are
/// POSIX, so a leading `/` is absolute on every platform the parser is tested
/// on (Windows would otherwise put it on the base's drive).
fn fontconfig_path(value: &str, base: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value.strip_prefix('~') {
        return Some(home?.join(rest.trim_start_matches('/')));
    }
    let path = PathBuf::from(value);
    Some(if path.is_absolute() || value.starts_with('/') {
        path
    } else {
        base.join(path)
    })
}

/// The language a Windows desktop is read in.
#[cfg(windows)]
mod windows {
    /// `LOCALE_NAME_MAX_LENGTH`, which windows-sys does not carry.
    const NAME_LENGTH: usize = 85;

    /// The display language first, since that is what `LANG` names
    /// elsewhere, then the user locale, which carries the region.
    #[allow(
        unsafe_code,
        reason = "Windows reports its languages through Win32 calls"
    )]
    pub(super) fn language() -> Option<String> {
        use windows_sys::Win32::Globalization::{
            GetUserDefaultLocaleName, GetUserPreferredUILanguages, MUI_LANGUAGE_NAME,
        };

        let mut names = [0u16; NAME_LENGTH * 8];
        let mut count = 0u32;
        let mut length = names.len() as u32;
        // SAFETY: the call writes at most `length` units into `names`, which
        // is the buffer's own length.
        let preferred = unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut count,
                names.as_mut_ptr(),
                &mut length,
            )
        };
        if preferred != 0
            && count > 0
            && let Some(first) = first_name(&names)
        {
            return Some(first);
        }
        let mut name = [0u16; NAME_LENGTH];
        // SAFETY: the call writes at most `name.len()` units into `name`.
        let read = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
        (read > 0).then(|| first_name(&name)).flatten()
    }

    /// The first string of a null-terminated list of them.
    fn first_name(names: &[u16]) -> Option<String> {
        let end = names.iter().position(|unit| *unit == 0)?;
        (end > 0).then(|| String::from_utf16_lossy(&names[..end]))
    }
}

/// Fixtures shared with the crate's other tests.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::{Fallback, Found, adjusted};

    /// The Yi test face with a 0.5 em line gap (as Hiragino Sans declares)
    /// as a fallback, adjusted as a system one would be.
    pub(crate) fn tall_yi_fallback() -> &'static [Fallback] {
        Box::leak(Box::new([adjusted(Found {
            script: "yi",
            bytes: include_bytes!("../tests/fixtures/yi/YiTallLineGap.ttf").to_vec(),
            index: 0,
            path: std::path::PathBuf::from("test.ttf"),
        })]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_font_files_are_probed() {
        assert!(is_font_file(Path::new("/x/NotoSans.ttf")));
        assert!(is_font_file(Path::new("/x/NotoSansCJK.TTC")));
        assert!(is_font_file(Path::new("/x/PingFang.otf")));
        assert!(!is_font_file(Path::new("/x/fonts.dir")));
        assert!(!is_font_file(Path::new("/x/README")));
    }

    #[test]
    fn arabic_is_enlarged_to_latin_size_within_limits() {
        assert!((scale_for(0.55, 0.50) - 1.1).abs() < 1e-6);
        assert!((scale_for(0.55, 0.54) - 1.0).abs() < 1e-6, "close enough");
        assert!((scale_for(0.55, 0.70) - 1.0).abs() < 1e-6, "never shrunk");
        assert!((scale_for(0.55, 0.30) - 1.25).abs() < 1e-6, "at most 25%");
    }

    #[test]
    fn inters_own_baseline_needs_no_shift() {
        assert!(baseline_offset(crate::INTER, 0).abs() < f32::EPSILON);
        let font = skrifa::FontRef::new(crate::INTER).unwrap();
        let metrics = font.metrics(
            skrifa::instance::Size::unscaled(),
            skrifa::instance::LocationRef::default(),
        );
        assert_eq!(metrics.units_per_em, 2048);
        assert!((metrics.ascent - 1984.0).abs() < f32::EPSILON);
        assert!((metrics.descent + 494.0).abs() < f32::EPSILON);
        assert!(metrics.leading.abs() < f32::EPSILON);
    }

    #[test]
    fn a_face_with_a_tall_line_box_is_shifted() {
        let yi = include_bytes!("../tests/fixtures/yi/YiTest.ttf");
        let tall = include_bytes!("../tests/fixtures/yi/YiTallLineGap.ttf");
        let offset = baseline_offset(yi, 0);
        assert!(offset.abs() < 0.05, "close to Inter's metrics: {offset}");
        // A 0.5 em line gap centres the face 0.25 em lower in its box.
        let shifted = baseline_offset(tall, 0) - offset;
        assert!((shifted - 0.25).abs() < 1e-4, "{shifted}");
        assert!(
            (arabic_scale(yi, 0) - 1.0).abs() < f32::EPSILON,
            "no heh, no scaling"
        );
        assert!((baseline_offset(b"not a font", 0)).abs() < f32::EPSILON);
    }

    #[test]
    fn adjusting_names_the_first_script() {
        let fallback = adjusted(Found {
            script: "yi",
            bytes: include_bytes!("../tests/fixtures/yi/YiTest.ttf").to_vec(),
            index: 0,
            path: std::path::PathBuf::from("test.ttf"),
        });
        assert_eq!(fallback.name, "fallback-yi");
        assert!((fallback.scale - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn scripts_are_listed_once() {
        for (position, (script, _, _)) in SCRIPTS.iter().enumerate() {
            assert!(
                SCRIPTS[position + 1..]
                    .iter()
                    .all(|(other, _, _)| other != script),
                "{script}"
            );
        }
    }

    #[test]
    fn fontconfig_configuration_names_the_font_directories() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::create_dir_all(root.join("conf.d")).unwrap();
        std::fs::write(
            root.join("fonts.conf"),
            r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <!-- <dir>/commented/out</dir> -->
  <dir>/nix/store/abc-noto-fonts-cjk-sans/share/fonts</dir>
  <dir prefix="xdg">fonts</dir>
  <dir>~/.local/fonts</dir>
  <include ignore_missing="yes">conf.d</include>
  <include ignore_missing="yes">missing.conf</include>
  <include>fonts.conf</include>
</fontconfig>"#,
        )
        .unwrap();
        std::fs::write(
            root.join("conf.d/10-extra.conf"),
            "<fontconfig><dir>relative/fonts</dir><dir>/a&amp;b</dir></fontconfig>",
        )
        .unwrap();
        std::fs::write(root.join("conf.d/README"), "<dir>/not/a/conf</dir>").unwrap();
        let home = Path::new("/home/reader");
        assert_eq!(
            fontconfig_dirs(&root.join("fonts.conf"), Some(home)),
            vec![
                PathBuf::from("/nix/store/abc-noto-fonts-cjk-sans/share/fonts"),
                home.join(".local/fonts"),
                root.join("conf.d/relative/fonts"),
                PathBuf::from("/a&b"),
            ]
        );
        assert_eq!(fontconfig_path("~/x", root, None), None, "no home");
        assert!(fontconfig_dirs(&root.join("absent.conf"), None).is_empty());
    }

    /// Lists the faces this machine offers:
    /// `cargo test -p fastframe-fonts -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads this machine's fonts"]
    fn which_scripts_have_faces_here() {
        for fallback in fallbacks() {
            println!(
                "{} ({} KB, scale {}, baseline {:+.3})",
                fallback.name,
                fallback.bytes.len() / 1024,
                fallback.scale,
                fallback.y_offset_factor
            );
        }
    }
}
