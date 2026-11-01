//! Which parts of a text are emoji: grapheme clusters, by Unicode's
//! presentation rules. From ZapFast's `src/emoji.rs`.

use unicode_segmentation::UnicodeSegmentation as _;

/// Plain text or one emoji sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// A run of text drawn by the text fonts.
    Text(&'a str),
    /// One grapheme cluster drawn as a picture.
    Emoji(&'a str),
}

/// Splits text into plain runs and emoji sequences.
#[must_use]
pub fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut run_start = 0;
    let mut offset = 0;
    for cluster in text.graphemes(true) {
        if is_emoji(cluster) {
            if run_start < offset {
                pieces.push(Piece::Text(&text[run_start..offset]));
            }
            pieces.push(Piece::Emoji(&text[offset..offset + cluster.len()]));
            run_start = offset + cluster.len();
        }
        offset += cluster.len();
    }
    if run_start < text.len() {
        pieces.push(Piece::Text(&text[run_start..]));
    }
    pieces
}

/// Whether a grapheme cluster is drawn as an emoji picture.
#[must_use]
pub fn is_emoji(cluster: &str) -> bool {
    let Some(first) = cluster.chars().next() else {
        return false;
    };
    if (first as u32) < 0xA9 {
        // Digits, `#`, and `*` are emoji only in keycap sequences.
        return cluster.contains('\u{20E3}');
    }
    if is_presentation(first) {
        return true;
    }
    // Variation selectors, emoji modifiers, and joiners can request emoji
    // presentation. Writing systems that use joiners stay text.
    let capable = matches!(first, '\u{A9}' | '\u{AE}')
        || ('\u{2000}'..='\u{33FF}').contains(&first)
        || ('\u{1F000}'..='\u{1FAFF}').contains(&first);
    capable
        && cluster
            .chars()
            .any(|c| matches!(c, '\u{FE0F}' | '\u{20E3}' | '\u{200D}') || is_skin_tone(c))
}

fn is_skin_tone(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Characters with default emoji presentation in Unicode data.
fn is_presentation(c: char) -> bool {
    let code = c as u32;
    if (0x1F000..=0x1FAFF).contains(&code) {
        return !matches!(
            code,
            0x1F170 | 0x1F171 | 0x1F17E | 0x1F17F | 0x1F202 | 0x1F237
        );
    }
    matches!(
        code,
        0x231A | 0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD | 0x25FE | 0x2614 | 0x2615
            | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1 | 0x26AA | 0x26AB | 0x26BD | 0x26BE
            | 0x26C4 | 0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2 | 0x26F3 | 0x26F5 | 0x26FA
            | 0x26FD | 0x2705 | 0x270A | 0x270B | 0x2728 | 0x274C | 0x274E | 0x2753..=0x2755
            | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55
    )
}

/// How many emoji a text holds when it holds nothing else but spaces.
#[must_use]
pub fn only_emoji(text: &str) -> Option<usize> {
    let mut count = 0;
    for piece in pieces(text) {
        match piece {
            Piece::Emoji(_) => count += 1,
            Piece::Text(run) if run.trim().is_empty() => {}
            Piece::Text(_) => return None,
        }
    }
    (count > 0).then_some(count)
}

/// The forms of a cluster to look for, best first: the whole sequence, the
/// sequence without U+FE0F, then its leading parts, so an unsupported joined
/// sequence still shows its first emoji. A flag is only itself: its first
/// part is a letter, and a flag no font has is spelled out instead.
pub(crate) fn candidates(cluster: &str) -> Vec<String> {
    let chars: Vec<char> = cluster.chars().collect();
    let mut forms = vec![cluster.to_owned()];
    if flag_letters(cluster).is_some() {
        return forms;
    }
    let mut stripped: Vec<char> = chars
        .iter()
        .copied()
        .filter(|character| *character != '\u{FE0F}')
        .collect();
    if stripped.len() != chars.len() && !stripped.is_empty() {
        forms.push(stripped.iter().collect());
    }
    while stripped.len() > 1 {
        stripped.pop();
        while stripped.last().is_some_and(|c| *c == '\u{200D}') {
            stripped.pop();
        }
        if !stripped.is_empty() {
            forms.push(stripped.iter().collect());
        }
    }
    forms
}

/// The two letters a country flag spells (🇳🇱 is "NL"), or `None` for
/// anything but a pair of regional indicators.
pub(crate) fn flag_letters(cluster: &str) -> Option<String> {
    let letters = cluster
        .chars()
        .map(|c| {
            let offset = u32::from(c).checked_sub(0x1F1E6)?;
            char::from_u32(u32::from('A') + offset).filter(char::is_ascii_uppercase)
        })
        .collect::<Option<String>>()?;
    (letters.len() == 2).then_some(letters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_spells_its_two_letters() {
        assert_eq!(flag_letters("🇳🇱").as_deref(), Some("NL"));
        assert_eq!(flag_letters("🇦🇿").as_deref(), Some("AZ"));
        assert_eq!(flag_letters("🇳"), None, "one letter is no flag");
        assert_eq!(flag_letters("😀"), None);
        assert_eq!(flag_letters("NL"), None);
        assert_eq!(
            flag_letters("\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}"),
            None
        );
    }

    #[test]
    fn a_flag_is_looked_for_whole_only() {
        assert_eq!(candidates("🇳🇱"), vec!["🇳🇱".to_owned()]);
    }

    #[test]
    fn pieces_split_emoji_out_of_text() {
        assert_eq!(
            pieces("hi 😀 there"),
            vec![
                Piece::Text("hi "),
                Piece::Emoji("😀"),
                Piece::Text(" there")
            ]
        );
        assert_eq!(pieces("plain"), vec![Piece::Text("plain")]);
        assert_eq!(pieces("🇩🇪"), vec![Piece::Emoji("🇩🇪")]);
        assert_eq!(pieces("👨‍👩‍👧"), vec![Piece::Emoji("👨‍👩‍👧")]);
        assert_eq!(pieces("👍🏽!"), vec![Piece::Emoji("👍🏽"), Piece::Text("!")]);
    }

    #[test]
    fn presentation_follows_unicode() {
        assert!(is_emoji("❤️"));
        assert!(!is_emoji("❤"));
        assert!(is_emoji("⭐"));
        assert!(is_emoji("1️⃣"));
        assert!(!is_emoji("1"));
        assert!(!is_emoji("©"));
        assert!(is_emoji("©️"));
        assert!(!is_emoji("a"));
    }

    #[test]
    fn emoji_only_texts_are_counted() {
        assert_eq!(only_emoji("😂"), Some(1));
        assert_eq!(only_emoji("😂 🙏"), Some(2));
        assert_eq!(only_emoji("ok 😂"), None);
        assert_eq!(only_emoji(""), None);
    }

    #[test]
    fn joiners_in_ordinary_words_are_not_emoji() {
        for word in [
            "\u{0DC1}\u{0DCA}\u{200D}\u{0DBB}\u{0DD3}",
            "\u{09B0}\u{200D}\u{09CD}\u{09AF}",
            "\u{0D28}\u{0D4D}\u{200D}",
        ] {
            assert!(!is_emoji(word), "{word:?} is text");
            assert_eq!(only_emoji(word), None);
        }
    }

    #[test]
    fn sequences_that_ask_to_be_pictures_still_are() {
        for picture in [
            "\u{2764}\u{FE0F}",
            "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",
            "\u{1F9D1}\u{1F3FD}\u{200D}\u{1F4BB}",
            "\u{23}\u{FE0F}\u{20E3}",
            "\u{1F1EE}\u{1F1F9}",
            "\u{A9}\u{FE0F}",
        ] {
            assert!(is_emoji(picture), "{picture:?} is a picture");
        }
    }

    #[test]
    fn a_sequence_is_tried_whole_then_shorter() {
        assert_eq!(
            candidates("\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}"),
            vec![
                "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}".to_owned(),
                "\u{1F3F3}\u{200D}\u{1F308}".to_owned(),
                "\u{1F3F3}".to_owned(),
            ]
        );
        assert_eq!(candidates("😀"), vec!["😀".to_owned()]);
    }
}
