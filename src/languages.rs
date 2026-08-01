//! The languages a track can be preferred in.
//!
//! A fixed list rather than everything ISO defines: this is a menu read from
//! across a room, and a list of several hundred codes would be unusable with
//! a controller. What is here is what actually turns up as an alternate audio
//! track on commercial discs and the rips made from them. Anything missing can
//! still be played by choosing the track by hand.
//!
//! Ordered by name rather than by how common each is. At fifty entries a
//! hand-tuned order is one nobody can predict, and looking for "Polish" in a
//! list sorted by popularity means reading all of it.

/// Code stored in the config, the name shown in the menu, and the tags a
/// container might label the track with.
///
/// Both two and three letter forms are listed because files disagree: MKV
/// tends to carry `eng`, while GStreamer often reports `en`.
pub const LANGUAGES: &[(&str, &str, &[&str])] = &[
    ("ar", "Arabic", &["ar", "ara"]),
    ("hy", "Armenian", &["hy", "hye", "arm"]),
    ("az", "Azerbaijani", &["az", "aze"]),
    ("bn", "Bengali", &["bn", "ben"]),
    ("bs", "Bosnian", &["bs", "bos"]),
    ("bg", "Bulgarian", &["bg", "bul"]),
    ("yue", "Cantonese", &["yue"]),
    ("ca", "Catalan", &["ca", "cat"]),
    ("zh", "Chinese", &["zh", "zho", "chi", "cmn"]),
    ("hr", "Croatian", &["hr", "hrv"]),
    ("cs", "Czech", &["cs", "ces", "cze"]),
    ("da", "Danish", &["da", "dan"]),
    ("nl", "Dutch", &["nl", "nld", "dut"]),
    ("en", "English", &["en", "eng"]),
    ("et", "Estonian", &["et", "est"]),
    ("fi", "Finnish", &["fi", "fin"]),
    ("fr", "French", &["fr", "fra", "fre"]),
    ("ka", "Georgian", &["ka", "kat", "geo"]),
    ("de", "German", &["de", "deu", "ger"]),
    ("el", "Greek", &["el", "ell", "gre"]),
    ("he", "Hebrew", &["he", "heb", "iw"]),
    ("hi", "Hindi", &["hi", "hin"]),
    ("hu", "Hungarian", &["hu", "hun"]),
    ("is", "Icelandic", &["is", "isl", "ice"]),
    ("id", "Indonesian", &["id", "ind", "in"]),
    ("it", "Italian", &["it", "ita"]),
    ("ja", "Japanese", &["ja", "jpn"]),
    ("kk", "Kazakh", &["kk", "kaz"]),
    ("ko", "Korean", &["ko", "kor"]),
    ("lv", "Latvian", &["lv", "lav"]),
    ("lt", "Lithuanian", &["lt", "lit"]),
    ("ms", "Malay", &["ms", "msa", "may"]),
    ("ml", "Malayalam", &["ml", "mal"]),
    ("no", "Norwegian", &["no", "nor", "nb", "nob", "nn", "nno"]),
    ("fa", "Persian", &["fa", "fas", "per"]),
    ("pl", "Polish", &["pl", "pol"]),
    ("pt", "Portuguese", &["pt", "por"]),
    ("pa", "Punjabi", &["pa", "pan"]),
    ("ro", "Romanian", &["ro", "ron", "rum", "mo", "mol"]),
    ("ru", "Russian", &["ru", "rus"]),
    ("sr", "Serbian", &["sr", "srp"]),
    ("sk", "Slovak", &["sk", "slk", "slo"]),
    ("sl", "Slovenian", &["sl", "slv"]),
    ("es", "Spanish", &["es", "spa"]),
    ("sv", "Swedish", &["sv", "swe"]),
    ("tl", "Tagalog", &["tl", "tgl", "fil"]),
    ("ta", "Tamil", &["ta", "tam"]),
    ("te", "Telugu", &["te", "tel"]),
    ("th", "Thai", &["th", "tha"]),
    ("tr", "Turkish", &["tr", "tur"]),
    ("uk", "Ukrainian", &["uk", "ukr"]),
    ("ur", "Urdu", &["ur", "urd"]),
    ("vi", "Vietnamese", &["vi", "vie"]),
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
