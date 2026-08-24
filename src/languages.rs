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

/// How a language is written wherever one is named: in its own words.
///
/// Every list in the application uses this - the Interface Language chooser,
/// the audio and subtitle preferences, and the track rows by way of
/// [`native_of_tag`]. There is exactly one way a language is spelled on
/// screen, and it is the way that language spells it.
///
/// **Both names used to be shown**, as `Russian (Русский)`, on the reasoning
/// that a list of fifty entries in scripts with nothing in common is easier to
/// work down with English beside it. Dropped on 2026-08-20. The English name
/// is only the *right* name when the interface is English, and since 1.5 that
/// is not a safe assumption: a Russian reading a Russian interface got a
/// foreign word bolted onto a language they already read. Naming it properly
/// would mean the interface language's word for every language, which is fifty
/// more strings per locale for a translator to carry.
///
/// Falls back to the English name for anything with no native form recorded,
/// and to the code itself for anything the table does not carry - which is an
/// honest answer rather than a guess.
pub fn display_name(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(stored, _, _, _)| *stored == code)
        .map(|(_, name, native, _)| if native.is_empty() { *name } else { *native }.to_string())
        .unwrap_or_else(|| code.to_string())
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

/// The same, but the language's own name for itself: `Русский` rather than
/// `Russian`, `日本語` rather than `Japanese`.
///
/// **What every list naming a language uses**, by way of this or of
/// [`display_name`]. The answer is most useful to whoever is looking for their
/// own language, who scans for their own word - and it stays right whatever
/// language the interface is in, which "Russian" does not: a Russian interface
/// listing "Russian" is a line nobody can read twice.
///
/// The chooser rows used to show both names and no longer do; see
/// [`display_name`] for what that cost and why it went.
pub fn native_of_tag(tag: &str) -> Option<&'static str> {
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
        .map(|(_, _, native, _)| *native)
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
///
/// **The name shown is the language's own**, so `rus` reads `rus (Русский)`.
/// A track list is scanned by somebody looking for their own language, and
/// their own word for it is what they are looking for - the same reasoning as
/// [`native_of_tag`], which the media page's summary lines use.
///
/// **Both names are checked against `already`**, not just the one shown. A
/// container's track titles are written in English far more often than not -
/// "Russian Commentary", not "Русский комментарий" - so checking only the
/// native name would miss every stutter it was written to catch, and the row
/// would read `rus (Русский) - AAC 2ch - Russian Commentary`.
pub fn describe_tag_unless(tag: &str, already: &str) -> String {
    let (Some(name), Some(native)) = (name_of_tag(tag), native_of_tag(tag)) else {
        return tag.to_string();
    };
    let already = already.to_lowercase();
    if already.contains(&name.to_lowercase()) || already.contains(&native.to_lowercase()) {
        return tag.to_string();
    }
    format!("{tag} ({native})")
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
    let leading = |text: &str| -> String {
        text.trim()
            .to_lowercase()
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect()
    };
    let tag = leading(tag);
    let code = leading(code);
    if tag.is_empty() || code.is_empty() {
        return false;
    }
    // The same tag on both sides is the same language, whatever the table has
    // heard of. A rip labelled with a code nobody standardised still matches
    // itself, which is the only honest answer available for one.
    if tag == code {
        return true;
    }
    // **Both sides are resolved through the aliases, not just the tag.** This
    // used to look `code` up in the stored column alone, so it only ever
    // worked when the caller already held a canonical code - which the
    // language *preferences* do, and a track's own language does not. A local
    // file says `en` and a Jellyfin stream says `eng` for the same soundtrack,
    // so casting a film matched no subtitle at all while browsing to it
    // matched the right one. Reported 2026-08-24.
    let entry = |wanted: &str| {
        LANGUAGES
            .iter()
            .find(|(_, _, _, aliases)| aliases.contains(&wanted))
    };
    match (entry(&tag), entry(&code)) {
        (Some(from_tag), Some(from_code)) => std::ptr::eq(from_tag, from_code),
        _ => false,
    }
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
    ///
    /// The name is the language's own, so Russian reads `Русский`. English is
    /// the case where that is invisible, its own name for itself being the
    /// same word - which is why the Russian line below is the one that says
    /// what this function actually does.
    #[test]
    fn a_known_tag_is_named() {
        assert_eq!(describe_tag("eng"), "eng (English)");
        assert_eq!(describe_tag("en"), "en (English)");
        // The whole label survives, so the "hi" that says hard-of-hearing is
        // still there to read.
        assert_eq!(describe_tag("en.hi"), "en.hi (English)");
        assert_eq!(describe_tag("ru"), "ru (Русский)");
        assert_eq!(describe_tag("ja"), "ja (日本語)");
    }

    /// Nothing is added when the text beside it already names the language,
    /// which track titles very often do.
    #[test]
    fn a_language_is_not_named_twice() {
        assert_eq!(describe_tag_unless("eng", "English SDH"), "eng");
        assert_eq!(describe_tag_unless("eng", "english commentary"), "eng");
        // Both names are checked, not only the one that would be shown. A
        // container's titles are written in English far more often than in
        // the language they describe, so checking `Русский` alone would let
        // "rus (Русский) - Russian Commentary" through.
        assert_eq!(describe_tag_unless("rus", "Russian Commentary"), "rus");
        assert_eq!(describe_tag_unless("rus", "Русский"), "rus");
        assert_eq!(
            describe_tag_unless("rus", "Director's Commentary"),
            "rus (Русский)"
        );
        // A label carrying the name itself rather than a separate title.
        assert_eq!(describe_tag("en.English"), "en.English");
        assert_eq!(describe_tag("eng - English SDH"), "eng - English SDH");
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

/// The order languages are offered in, as indices into [`LANGUAGES`].
///
/// The table itself stays sorted by English name, which is what makes it
/// maintainable to read and edit. That order stopped making sense on screen
/// the moment the lists began naming languages natively: sorted by their
/// English names but shown in their own, the list opened with Arabic, then
/// Armenian, then two Latin entries, then Bengali. Alphabetical by a word the
/// reader could no longer see.
///
/// Sorted by script first, then by name within it. Grouping by script is the
/// property worth having: somebody looking for their own language is looking
/// for their own letters, and fifty entries in fifteen scripts are worked down
/// by finding the block and then reading.
///
/// **Within a non-Latin script the order is by code point**, which is not what
/// a reader of that script would call alphabetical - it is merely stable and
/// keeps the block together. Doing better needs locale-aware collation, which
/// means a dependency and a table per language; the blocks are three to six
/// entries each, so the gain would be small.
pub fn display_order() -> Vec<usize> {
    let mut order: Vec<usize> = (0..LANGUAGES.len()).collect();
    order.sort_by_key(|&index| {
        let native = LANGUAGES[index].2;
        (script_rank(native), fold(native))
    });
    order
}

/// Which script a name is written in, as a sort position. Unicode lays the
/// scripts out in blocks, so this is the block the first character falls in.
fn script_rank(native: &str) -> u8 {
    const BLOCKS: [(u32, u32); 16] = [
        (0x0041, 0x024F), // Latin
        (0x0370, 0x03FF), // Greek
        (0x0400, 0x04FF), // Cyrillic
        (0x0530, 0x058F), // Armenian
        (0x0590, 0x05FF), // Hebrew
        (0x0600, 0x06FF), // Arabic, and the languages that borrow it
        (0x0900, 0x097F), // Devanagari
        (0x0980, 0x09FF), // Bengali
        (0x0A00, 0x0A7F), // Gurmukhi
        (0x0B80, 0x0BFF), // Tamil
        (0x0C00, 0x0C7F), // Telugu
        (0x0D00, 0x0D7F), // Malayalam
        (0x0E00, 0x0E7F), // Thai
        (0x10A0, 0x10FF), // Georgian
        (0x4E00, 0x9FFF), // Han
        (0xAC00, 0xD7AF), // Hangul
    ];
    let Some(first) = native.chars().next() else {
        return u8::MAX;
    };
    BLOCKS
        .iter()
        .position(|(low, high)| (*low..=*high).contains(&(first as u32)))
        .map(|rank| rank as u8)
        .unwrap_or(u8::MAX)
}

/// A name reduced to something that sorts the way a reader expects.
///
/// For ordering only, and never shown. Without it `Čeština` sorts after every
/// unaccented Latin name rather than beside `Català`, because `Č` lives past
/// `Z` in Unicode. Non-Latin letters are left alone: there is nothing to fold
/// them to, and within their own block they are already together.
fn fold(native: &str) -> String {
    native
        .chars()
        .map(|c| match c {
            'Č' | 'Ç' => 'C',
            'č' | 'ç' => 'c',
            'Í' => 'I',
            'í' => 'i',
            'Đ' => 'D',
            'đ' | 'ð' => 'd',
            'Ə' => 'E',
            'ə' | 'é' | 'ė' => 'e',
            'Ş' | 'Š' => 'S',
            'ş' | 'š' => 's',
            'Ğ' => 'G',
            'ğ' => 'g',
            'ü' | 'ū' | 'ų' => 'u',
            'ö' | 'ó' | 'ő' => 'o',
            'å' | 'á' | 'ą' | 'ã' => 'a',
            'ñ' => 'n',
            'ž' => 'z',
            'ł' => 'l',
            'ệ' | 'ế' => 'e',
            other => other,
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod order_tests {
    use super::*;

    fn shown() -> Vec<&'static str> {
        display_order()
            .into_iter()
            .map(|i| LANGUAGES[i].2)
            .collect()
    }

    /// Every language is offered exactly once, whatever the order.
    #[test]
    fn the_order_is_a_permutation() {
        let mut sorted = display_order();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..LANGUAGES.len()).collect::<Vec<_>>());
    }

    /// Each script arrives in one run rather than scattered through the list,
    /// which is the property somebody looking for their own letters needs.
    #[test]
    fn a_script_is_never_interrupted() {
        let mut seen: Vec<u8> = Vec::new();
        for name in shown() {
            let rank = script_rank(name);
            if seen.last() != Some(&rank) {
                assert!(
                    !seen.contains(&rank),
                    "{name} reopens a script already done"
                );
                seen.push(rank);
            }
        }
    }

    /// Latin comes first, being most of the table, and reads alphabetically -
    /// including the accented names, which sort past `Z` unfolded.
    #[test]
    fn the_latin_block_reads_alphabetically() {
        let latin: Vec<&str> = shown()
            .into_iter()
            .take_while(|name| script_rank(name) == 0)
            .collect();
        assert_eq!(latin.first(), Some(&"Azərbaycan"));
        let folded: Vec<String> = latin.iter().map(|n| fold(n)).collect();
        let mut expected = folded.clone();
        expected.sort();
        assert_eq!(folded, expected);
        assert!(latin.contains(&"Čeština") && latin.contains(&"Íslenska"));
    }
}

#[cfg(test)]
mod matching_tests {
    use super::matches;

    /// A track states its language in whatever form its source uses, and the
    /// two sides are often not the same form. A local file says `en` where
    /// Jellyfin says `eng` for the identical soundtrack.
    #[test]
    fn the_same_language_matches_in_either_form() {
        for (tag, code) in [
            ("en", "en"),
            ("eng", "en"),
            ("en", "eng"),
            ("eng", "eng"),
            ("rus", "ru"),
            ("ru", "rus"),
            ("eng - English (Forced)", "eng"),
            ("rus - Russian (Forced)", "ru"),
            ("en-US", "eng"),
        ] {
            assert!(matches(tag, code), "{tag:?} should match {code:?}");
        }
    }

    #[test]
    fn different_languages_do_not_match() {
        for (tag, code) in [
            ("es", "en"),
            ("spa", "eng"),
            ("es - Español (Forced)", "eng"),
            ("ru", "uk"),
        ] {
            assert!(!matches(tag, code), "{tag:?} should not match {code:?}");
        }
    }

    /// A code the table never heard of still matches itself, which is the only
    /// answer available for one.
    #[test]
    fn an_unknown_code_matches_itself_and_nothing_else() {
        assert!(matches("qya", "qya"));
        assert!(!matches("qya", "en"));
        assert!(!matches("en", "qya"));
    }

    #[test]
    fn nothing_matches_an_empty_side() {
        assert!(!matches("", "en"));
        assert!(!matches("en", ""));
        assert!(!matches("- Forced", "en"));
    }
}
