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

/// Code stored in the config, the English name, the name in the language
/// itself, and the tags a container might label the track with.
///
/// The native name is there because a menu of languages is read by people
/// looking for their own: "Русский" is what a Russian speaker scans for, and
/// "Russian" is what everyone else does. Showing both means neither has to
/// know the other's word for it.
///
/// Both two and three letter forms are listed because files disagree: MKV
/// tends to carry `eng`, while GStreamer often reports `en`.
pub const LANGUAGES: &[(&str, &str, &str, &[&str])] = &[
    ("ar", "Arabic", "العربية", &["ar", "ara"]),
    ("hy", "Armenian", "Հայերեն", &["hy", "hye", "arm"]),
    ("az", "Azerbaijani", "Azərbaycan", &["az", "aze"]),
    ("bn", "Bengali", "বাংলা", &["bn", "ben"]),
    ("bs", "Bosnian", "Bosanski", &["bs", "bos"]),
    ("bg", "Bulgarian", "Български", &["bg", "bul"]),
    ("yue", "Cantonese", "粵語", &["yue"]),
    ("ca", "Catalan", "Català", &["ca", "cat"]),
    ("zh", "Chinese", "中文", &["zh", "zho", "chi", "cmn"]),
    ("hr", "Croatian", "Hrvatski", &["hr", "hrv"]),
    ("cs", "Czech", "Čeština", &["cs", "ces", "cze"]),
    ("da", "Danish", "Dansk", &["da", "dan"]),
    ("nl", "Dutch", "Nederlands", &["nl", "nld", "dut"]),
    ("en", "English", "English", &["en", "eng"]),
    ("et", "Estonian", "Eesti", &["et", "est"]),
    ("fi", "Finnish", "Suomi", &["fi", "fin"]),
    ("fr", "French", "Français", &["fr", "fra", "fre"]),
    ("ka", "Georgian", "ქართული", &["ka", "kat", "geo"]),
    ("de", "German", "Deutsch", &["de", "deu", "ger"]),
    ("el", "Greek", "Ελληνικά", &["el", "ell", "gre"]),
    ("he", "Hebrew", "עברית", &["he", "heb", "iw"]),
    ("hi", "Hindi", "हिन्दी", &["hi", "hin"]),
    ("hu", "Hungarian", "Magyar", &["hu", "hun"]),
    ("is", "Icelandic", "Íslenska", &["is", "isl", "ice"]),
    ("id", "Indonesian", "Bahasa Indonesia", &["id", "ind", "in"]),
    ("it", "Italian", "Italiano", &["it", "ita"]),
    ("ja", "Japanese", "日本語", &["ja", "jpn"]),
    ("kk", "Kazakh", "Қазақша", &["kk", "kaz"]),
    ("ko", "Korean", "한국어", &["ko", "kor"]),
    ("lv", "Latvian", "Latviešu", &["lv", "lav"]),
    ("lt", "Lithuanian", "Lietuvių", &["lt", "lit"]),
    ("ms", "Malay", "Bahasa Melayu", &["ms", "msa", "may"]),
    ("ml", "Malayalam", "മലയാളം", &["ml", "mal"]),
    (
        "no",
        "Norwegian",
        "Norsk",
        &["no", "nor", "nb", "nob", "nn", "nno"],
    ),
    ("fa", "Persian", "فارسی", &["fa", "fas", "per"]),
    ("pl", "Polish", "Polski", &["pl", "pol"]),
    ("pt", "Portuguese", "Português", &["pt", "por"]),
    ("pa", "Punjabi", "ਪੰਜਾਬੀ", &["pa", "pan"]),
    (
        "ro",
        "Romanian",
        "Română",
        &["ro", "ron", "rum", "mo", "mol"],
    ),
    ("ru", "Russian", "Русский", &["ru", "rus"]),
    ("sr", "Serbian", "Српски", &["sr", "srp"]),
    ("sk", "Slovak", "Slovenčina", &["sk", "slk", "slo"]),
    ("sl", "Slovenian", "Slovenščina", &["sl", "slv"]),
    ("es", "Spanish", "Español", &["es", "spa"]),
    ("sv", "Swedish", "Svenska", &["sv", "swe"]),
    ("tl", "Tagalog", "Tagalog", &["tl", "tgl", "fil"]),
    ("ta", "Tamil", "தமிழ்", &["ta", "tam"]),
    ("te", "Telugu", "తెలుగు", &["te", "tel"]),
    ("th", "Thai", "ไทย", &["th", "tha"]),
    ("tr", "Turkish", "Türkçe", &["tr", "tur"]),
    ("uk", "Ukrainian", "Українська", &["uk", "ukr"]),
    ("ur", "Urdu", "اردو", &["ur", "urd"]),
    ("vi", "Vietnamese", "Tiếng Việt", &["vi", "vie"]),
];

pub fn name_for(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(stored, _, _, _)| *stored == code)
        .map(|(_, name, _, _)| name.to_string())
        .unwrap_or_else(|| code.to_string())
}

/// How a language reads in a menu: the English name, and its own name after
/// it where the two differ. English gets "English", Russian gets
/// "Russian (Русский)".
pub fn menu_name(code: &str, name: &str, native: &str) -> String {
    let _ = code;
    if name == native {
        name.to_string()
    } else {
        format!("{name} ({native})")
    }
}

/// The English name for whatever a track or file called itself, taken
/// loosely: `eng`, `en` and `en-US` all answer "English".
///
/// `None` when the tag names nothing in the table, which is the honest answer
/// for a track tagged `und` or with something the list does not carry.
pub fn name_of_tag(tag: &str) -> Option<&'static str> {
    let tag: String = tag
        .trim()
        .to_lowercase()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if tag.is_empty() || tag == "und" {
        return None;
    }
    LANGUAGES
        .iter()
        .find(|(_, _, _, aliases)| aliases.contains(&tag.as_str()))
        .map(|(_, name, _, _)| *name)
}

/// A language tag as it should read on screen: the tag itself, with the
/// language named after it where the tag names one.
///
/// Track tags and file names are written for machines - `eng`, `en.hi`,
/// `en-US` - and knowing that one of those means English is a small piece of
/// knowledge not everyone has. A list of them is a list of things to decode.
///
/// For display only. The tag itself stays exactly as the file or container
/// wrote it, because it is what `--primary` and `--subtitle` are matched
/// against, what the forced check reads, and what a saved choice refers to.
/// Decorating the stored value would put that decoration in all of them.
///
/// Anything the table does not carry is left alone: a guess is worse than the
/// raw tag, which at least is what the file actually says.
pub fn describe_tag(tag: &str) -> String {
    describe_tag_unless(tag, tag)
}

/// The same, but silent when `already` says the language for itself.
///
/// Track titles very often name the language: "English SDH", "English
/// Commentary". Adding the name beside a tag whose title already carries it
/// produces "eng (English) - English SDH", which reads as a stutter and is
/// longer for no gain. `already` is the other text that will be shown, and the
/// name is added only when it is missing from it.
pub fn describe_tag_unless(tag: &str, already: &str) -> String {
    let Some(name) = name_of_tag(tag) else {
        return tag.to_string();
    };
    if already.to_lowercase().contains(&name.to_lowercase()) {
        return tag.to_string();
    }
    format!("{tag} ({name})")
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
        .any(|(_, _, _, aliases)| aliases.contains(&tag.as_str()))
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
        .find(|(stored, _, _, _)| *stored == code)
        .is_some_and(|(_, _, _, aliases)| aliases.contains(&tag.as_str()))
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

#[cfg(test)]
mod tag_descriptions {
    use super::*;

    /// A tag the table knows gains the language's name, and keeps the tag:
    /// the tag is what tells two English tracks apart.
    #[test]
    fn a_known_tag_is_named() {
        assert_eq!(describe_tag("eng"), "eng (English)");
        assert_eq!(describe_tag("en"), "en (English)");
        // The whole label survives, so the "hi" that says hard-of-hearing is
        // still there to read.
        assert_eq!(describe_tag("en.hi"), "en.hi (English)");
        assert_eq!(describe_tag("ru"), "ru (Russian)");
    }

    /// Nothing is added when the text beside it already names the language,
    /// which track titles very often do.
    #[test]
    fn a_language_is_not_named_twice() {
        assert_eq!(describe_tag_unless("eng", "English SDH"), "eng");
        assert_eq!(describe_tag_unless("eng", "english commentary"), "eng");
        // A label carrying the name itself rather than a separate title.
        assert_eq!(describe_tag("en.English"), "en.English");
        assert_eq!(describe_tag("eng — English SDH"), "eng — English SDH");
        // A title naming a different language is not the same language.
        assert_eq!(
            describe_tag_unless("eng", "Spanish Commentary"),
            "eng (English)"
        );
        assert_eq!(describe_tag_unless("eng", ""), "eng (English)");
    }

    /// Anything the table does not carry is left exactly as it was. A guess
    /// is worse than the raw tag, which is at least what the file says.
    #[test]
    fn an_unknown_tag_is_left_alone() {
        assert_eq!(describe_tag("und"), "und");
        assert_eq!(describe_tag("External"), "External");
        assert_eq!(describe_tag("forced"), "forced");
        assert_eq!(describe_tag(""), "");
    }

    /// Naming the language must not stop the label being matched by one.
    /// `--subtitle en` and the subtitle preference both compare against the
    /// label, and both take only its leading letters.
    #[test]
    fn a_named_label_still_matches_its_language() {
        assert!(matches(&describe_tag("en.hi"), "en"));
        assert!(matches(&describe_tag("eng"), "en"));
        assert!(!matches(&describe_tag("eng"), "ru"));
    }
}
