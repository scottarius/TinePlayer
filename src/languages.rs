//! The languages a track can be preferred in.
//!
//! A fixed list rather than everything ISO defines: this is a menu read from
//! across a room, and a list of several hundred codes would be unusable with
//! a controller. Anything missing can still be played by choosing the track
//! by hand.

/// Code stored in the config, the name shown in the menu, and the tags a
/// container might label the track with.
///
/// Both two and three letter forms are listed because files disagree: MKV
/// tends to carry `eng`, while GStreamer often reports `en`.
pub const LANGUAGES: &[(&str, &str, &[&str])] = &[
    ("en", "English", &["en", "eng"]),
    ("ru", "Russian", &["ru", "rus"]),
    ("es", "Spanish", &["es", "spa"]),
    ("fr", "French", &["fr", "fra", "fre"]),
    ("de", "German", &["de", "deu", "ger"]),
    ("it", "Italian", &["it", "ita"]),
    ("pt", "Portuguese", &["pt", "por"]),
    ("nl", "Dutch", &["nl", "nld", "dut"]),
    ("pl", "Polish", &["pl", "pol"]),
    ("uk", "Ukrainian", &["uk", "ukr"]),
    ("cs", "Czech", &["cs", "ces", "cze"]),
    ("sv", "Swedish", &["sv", "swe"]),
    ("no", "Norwegian", &["no", "nor", "nb", "nob"]),
    ("da", "Danish", &["da", "dan"]),
    ("fi", "Finnish", &["fi", "fin"]),
    ("hu", "Hungarian", &["hu", "hun"]),
    ("tr", "Turkish", &["tr", "tur"]),
    ("el", "Greek", &["el", "ell", "gre"]),
    ("he", "Hebrew", &["he", "heb", "iw"]),
    ("ar", "Arabic", &["ar", "ara"]),
    ("hi", "Hindi", &["hi", "hin"]),
    ("ja", "Japanese", &["ja", "jpn"]),
    ("ko", "Korean", &["ko", "kor"]),
    ("zh", "Chinese", &["zh", "zho", "chi"]),
];

pub fn name_for(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(stored, _, _)| *stored == code)
        .map(|(_, name, _)| name.to_string())
        .unwrap_or_else(|| code.to_string())
}

/// Whether a tag names a language at all.
///
/// `und`, an empty tag, or anything not in the table is "not stated" rather
/// than a language of its own. Worth telling apart from a wrong language: a
/// track that never said what it is may still be the one wanted.
pub fn known(tag: &str) -> bool {
    let tag: String = tag
        .trim()
        .to_lowercase()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if tag.is_empty() || tag == "und" {
        return false;
    }
    LANGUAGES
        .iter()
        .any(|(_, _, aliases)| aliases.contains(&tag.as_str()))
}

/// Whether a track's language tag is the language stored as `code`.
///
/// The tag is taken loosely: subtitle files are named things like
/// `film.en.hi.srt`, and containers carry anything from `en` to `eng` to
/// `en-US`, so only the leading letters are compared.
pub fn matches(tag: &str, code: &str) -> bool {
    let tag: String = tag
        .trim()
        .to_lowercase()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if tag.is_empty() {
        return false;
    }
    LANGUAGES
        .iter()
        .find(|(stored, _, _)| *stored == code)
        .is_some_and(|(_, _, aliases)| aliases.contains(&tag.as_str()))
}

#[cfg(test)]
mod known_tests {
    use super::known;

    #[test]
    fn a_stated_language_is_known() {
        for tag in ["en", "eng", "en-US", "fr", "RU"] {
            assert!(known(tag), "{tag} should be known");
        }
    }

    #[test]
    fn an_unstated_one_is_not() {
        // What a container carries when nobody set a language, which is what
        // tools that add a description track tend to leave behind.
        for tag in ["und", "", "   ", "zzz"] {
            assert!(!known(tag), "{tag:?} should not be known");
        }
    }
}
