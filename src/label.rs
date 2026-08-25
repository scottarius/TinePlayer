//! One way of writing a track's name, for audio and subtitles alike.
//!
//! Every list of tracks in the application comes through here: the two
//! soundtrack choosers, the subtitle chooser, the media page and
//! `--list-tracks`. They used to each assemble their own row out of whatever
//! the source happened to carry, which meant the same track read differently
//! depending on where the answer came from - a description found by its
//! Matroska flag looked like an ordinary track, while one found by its title
//! said so, and the two are the same fact.
//!
//! Three segments, and any that has nothing to say is left out rather than
//! shown empty:
//!
//! ```text
//! Русский - AAC 6ch - Audio Description
//! English - SRT - SDH
//! English - PGS - Forced
//! Français - AAC 2ch
//! ```
//!
//! **The third segment is the type where one could be worked out, and the
//! track's own title where it could not.** Dropping the title outright would
//! lose what no flag records - "Restored 1998 Mix", "Director's Cut" - and
//! showing both would stutter on the common case where the title is only the
//! type spelled out by hand.
//!
//! **A file beside the video is a track too**, and comes through
//! [`named_after_the_film`] or [`named_after_nothing`]
//! rather than reading as something else in the same list. It states no codec,
//! and everything it does state is in its name, but `Film.en.ad.mp3` and a
//! track inside the film flagged `FlagVisualImpaired` are the same fact and
//! now say so the same way.

use std::borrow::Cow;

use crate::languages;
use crate::{tr, trc};

/// Whether a track's own words claim a kind, for each kind separately.
///
/// One place deciding what the words mean, because two places would drift.
/// `probe` asks these of a track's title alongside the container's flags -
/// **a stated yes from either decides** - and `subtitles` asks them of a
/// subtitle file's whole name, which is all such a file has.
///
/// Separate predicates rather than one function returning a `Kind`, because
/// the caller in `probe` has a flag per kind to combine with each answer, and
/// a single verdict could not be combined with three flags.
pub fn says_forced(text: &str) -> bool {
    text.to_lowercase().contains("forced")
}

/// `hi` and `cc` are matched as whole words, split on anything that is not a
/// letter or a digit. As substrings they appear inside "Hindi", "Chinese" and
/// a great many titles, and a subtitle wrongly read as SDH is one offered to
/// somebody who did not ask for it.
pub fn says_sdh(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("sdh")
        || text.contains("hearing impaired")
        || text.contains("hard of hearing")
        || text
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == "hi" || word == "cc")
}

pub fn says_commentary(text: &str) -> bool {
    text.to_lowercase().contains("commentary")
}

/// What a track is *for*, as opposed to what it is.
///
/// Worked out once, from the container's flags first and the track's title
/// second, so that every list agrees. See [`crate::probe::AudioTrack`] and
/// [`crate::probe::SubtitleTrack`] for which flag answers which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A narrated account of what is on screen, for blind and low-vision
    /// viewers. Matroska's `FlagVisualImpaired`.
    Described,
    /// Subtitles carrying the sound as well as the speech, for deaf and
    /// hard-of-hearing viewers. Matroska's `FlagHearingImpaired`, and the
    /// thing a title spells "SDH".
    Sdh,
    /// Only the lines a viewer who follows the dialogue still needs: signs,
    /// and speech in another language.
    Forced,
    /// Somebody talking over the film about the film.
    Commentary,
}

impl Kind {
    /// How the row says it. Translated, because it is read rather than matched
    /// - nothing parses these back.
    pub fn name(self) -> Cow<'static, str> {
        match self {
            Kind::Described => tr!("Audio Description"),
            Kind::Sdh => trc!("subtitle type", "SDH"),
            Kind::Forced => trc!("subtitle type", "Forced"),
            Kind::Commentary => tr!("Commentary"),
        }
    }
}

/// Whether the language should be named as itself or shown as the file wrote
/// it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Naming {
    /// The language's own name for itself - `Русский`. What a chooser on a
    /// television uses: it is read by somebody looking for their own language,
    /// who scans for their own word, and it stays right whatever language the
    /// interface is in.
    Native,
    /// The tag as the file wrote it, with the language named after it -
    /// `rus (Русский)`. What `--list-tracks` uses, because that list is where
    /// somebody learns which code to hand to `--primary` or `--subtitle`.
    WithTag,
}

/// The pieces a row is built from, gathered by the caller because only the
/// caller knows which kind of track it has.
pub struct Parts<'a> {
    /// The language tag exactly as the container wrote it. Never decorated
    /// here: it is what a saved choice refers to and what `--primary` matches.
    pub language: &'a str,
    /// The technical middle - `AAC 6ch` for audio, `SRT` or `PGS` for a
    /// subtitle. Empty where the source would not say, which is every subtitle
    /// file sitting beside a video.
    pub technical: String,
    /// What the track is for, where that could be worked out.
    pub kind: Option<Kind>,
    /// What the file calls the track. Shown only when `kind` found nothing,
    /// and only when it adds something the language has not already said.
    pub title: &'a str,
}

/// A track's name, as every list should show it.
///
/// Never empty: a track that states nothing at all still has to be a row
/// somebody can point at, so the last resort is the language tag as written,
/// and after that the caller's own `unknown`.
pub fn line(parts: &Parts, naming: Naming, unknown: &str) -> String {
    // The last segment is settled first, because whether the title survives
    // decides how the language may be written. Suppressing "English" beside a
    // tag because the title says it, and then dropping that title for saying
    // only what the language said, loses both and leaves a bare `en` - which
    // is what this did until the listing showed it.
    let title = parts.title.trim();
    let last = match parts.kind {
        Some(kind) => Some(kind.name().into_owned()),
        None if title.is_empty() || restates_language(title, parts.language) => None,
        // Where the track said no language, the caller's wording holds the
        // first segment - and where that wording *is* the title, which is all
        // a library's own subtitle file ever said, showing it again says the
        // same thing twice: "Signs - Signs".
        None if !states_a_language(parts.language) && title == unknown.trim() => None,
        None => Some(title.to_string()),
    };

    let mut segments: Vec<String> = Vec::new();
    if !states_a_language(parts.language) {
        // A track that never said what language it is still has to be told
        // apart from the next one that did not say either. Leaving the segment
        // out looked reasonable in a printed list, which numbers its own rows,
        // and produced two identical rows in the chooser, which does not - two
        // untitled subtitles both read `SRT` and neither could be picked on
        // purpose. The caller's wording is a number, which is also what
        // `--primary` and `--subtitle` take.
        segments.push(unknown.to_string());
    } else {
        segments.push(match naming {
            Naming::Native => languages::native_of_tag(parts.language)
                .or_else(|| languages::name_of_tag(parts.language))
                .map(str::to_string)
                .unwrap_or_else(|| parts.language.to_string()),
            // Checked against the title only where the title is going to be
            // shown; against the tag itself otherwise, so that `en.English`
            // still does not become `en.English (English)`.
            Naming::WithTag => match &last {
                Some(shown) if shown == title => {
                    languages::describe_tag_unless(parts.language, title)
                }
                _ => languages::describe_tag(parts.language),
            },
        });
    }

    if !parts.technical.trim().is_empty() {
        segments.push(parts.technical.trim().to_string());
    }
    segments.extend(last);

    // A plain hyphen, not an em dash. Scott asked for this on 2026-08-20:
    // the interface used to be the one place em dashes were allowed, and is
    // not any more.
    segments.join(" - ")
}

/// Whether a tag says anything at all.
///
/// Deliberately not [`languages::known`], which asks whether the table carries
/// the tag. A tag the table has never heard of still came out of the file and
/// is still worth showing: it tells two tracks apart, and it is what somebody
/// would have to type. Only nothing, and the several spellings of "no idea",
/// count as unstated.
fn states_a_language(tag: &str) -> bool {
    let tag = tag.trim();
    !(tag.is_empty() || tag.eq_ignore_ascii_case("und") || tag.eq_ignore_ascii_case("unknown"))
}

/// Whether a title says only what the language segment already said.
///
/// Both names are checked, not just the one being shown: containers write
/// track titles in English far more often than not, so a row showing `Русский`
/// beside a title of "Russian" is the stutter this exists to catch, and
/// checking only the native name would miss every instance of it.
fn restates_language(title: &str, tag: &str) -> bool {
    let title = title.trim();
    [languages::name_of_tag(tag), languages::native_of_tag(tag)]
        .into_iter()
        .flatten()
        .any(|name| name.eq_ignore_ascii_case(title))
}

/// One row for a file a library named after the film, which is where every
/// external file that says anything about itself says it.
///
/// Every such file goes through here - a soundtrack or a subtitle beside the
/// video, and a library's own. They used to each write their own row, so the
/// same fact read three ways: `en.ad (English, Audio Description)` in a
/// soundtrack list, `en.hi (English) - SDH` in a subtitle list, and
/// `English - AAC 6ch - Audio Description` for the track inside the file that
/// meant the same thing.
///
/// `tag` is whatever the convention left between the film's name and the
/// extension - `en`, `en.hi`, `ad` - and is **empty for a file named exactly
/// after the film**, which is a real case and says nothing whatever about
/// itself. `kind_of` reads the type out of the tag and only ever out of the
/// tag: [`kind_of_audio_tag`] for a soundtrack, [`kind_of_subtitle_tag`] for a
/// subtitle, because `hi` means Hindi beside one and hearing-impaired beside
/// the other.
///
/// **`name` is shown but never read**, because it is the film's name with an
/// extension on the end rather than anything this file chose. Reading it would
/// read the film's title, and `Ad Astra (2019).mka` is not an audio
/// description. A file that named *itself* goes through
/// [`named_after_nothing`] instead, which does read it.
pub fn named_after_the_film(
    tag: &str,
    name: &str,
    kind_of: fn(&str) -> Option<Kind>,
    naming: Naming,
) -> String {
    let tag = tag.trim();
    // A tag is language-then-marks by convention, so the first component names
    // the language and whatever follows is left to say what no flag could -
    // `en.restored`. The printed list keeps the tag whole instead, because it
    // is what somebody types back at `--subtitle`, and naming the language
    // properly there would hide the very thing the list exists to show.
    let (language, rest) = match naming {
        Naming::WithTag => (tag, ""),
        Naming::Native => split_tag(tag),
    };
    line(
        &Parts {
            // Nothing to state: a file carries no codec, and a subtitle file's
            // format is its extension, which the name already shows.
            technical: String::new(),
            language,
            kind: kind_of(tag),
            title: rest,
        },
        naming,
        // The tag is what tells one of these from another and is short enough
        // to read across a room; the name is the film's over again with two
        // letters on the end. Only where there is no tag does the name have to
        // stand, and then it is standing as an identifier rather than as
        // anything anybody claimed.
        if tag.is_empty() { name } else { tag },
    )
}

/// One row for a file nothing named after the film: loose in its folder, or
/// picked by hand from anywhere on disk.
///
/// Its own name is the whole of what it has - it is what tells two of them
/// apart, and it is read for what it says, because a described soundtrack
/// downloaded by hand arrives called `AD.mp3` as often as it arrives named
/// after the film.
///
/// No language is taken from it. A name is not a tag: splitting `en.mp3` the
/// way `en.ad` is split would name the language and then show the extension as
/// if it meant something, and stripping the extension first would collapse
/// `English.srt` and `English.ass` into one unpickable row.
pub fn named_after_nothing(name: &str, kind_of: fn(&str) -> Option<Kind>) -> String {
    line(
        &Parts {
            language: "",
            technical: String::new(),
            kind: kind_of(name),
            title: "",
        },
        // Nothing to name either way, there being no language: whichever the
        // caller would have asked for, the row is the name and what it says.
        Naming::Native,
        name,
    )
}

/// A tag split into the language it names and whatever it says after it.
///
/// `("en", "hi")` for `en.hi`, `("en", "")` for `en`. Nothing at all where the
/// first component names no language - `ad`, `commentary`, or a file name
/// standing in for a tag - because the whole of it is then the caller's to
/// show as written: handing back a `rest` there would print it twice, once as
/// the row's own name and once as its reading.
pub(crate) fn split_tag(tag: &str) -> (&str, &str) {
    let (first, rest) = tag.split_once('.').unwrap_or((tag, ""));
    if languages::known(first) {
        (first, rest)
    } else {
        ("", "")
    }
}

/// The type a bare tag implies for a *soundtrack*.
///
/// The ladder [`crate::probe::AudioTrack::kind`] climbs, in the same order and
/// for the same reason: description first, because it is the one a preference
/// acts on and because the two overlap in the wild. The words are read the
/// same way too, so that `Film.en.ad.mp3` beside a video and a track inside it
/// flagged `FlagVisualImpaired` arrive at the same row.
///
/// Deliberately not the subtitle reading below. `hi` beside a soundtrack is
/// Hindi, and answering "hearing impaired" would be both wrong and unhelpful.
pub fn kind_of_audio_tag(tag: &str) -> Option<Kind> {
    if crate::probe::is_audio_description(tag) {
        return Some(Kind::Described);
    }
    tag.to_lowercase()
        .contains("commentary")
        .then_some(Kind::Commentary)
}

/// The type a bare tag implies for a *subtitle*, for the sources that carry
/// nothing else.
///
/// A subtitle file beside a video says everything it has to say in its name -
/// `Film.en.hi.srt`, `Film.en.forced.srt` - and a library hands over a title
/// and nothing more. Both go through the same words a track title is read for,
/// so that `en.hi` beside a file and `FlagHearingImpaired` inside one arrive at
/// the same row.
pub fn kind_of_subtitle_tag(tag: &str) -> Option<Kind> {
    let tag = tag.to_lowercase();
    if tag.contains("forced") {
        return Some(Kind::Forced);
    }
    if tag.contains("sdh")
        || tag.contains("hearing impaired")
        || tag
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == "hi" || word == "cc")
    {
        return Some(Kind::Sdh);
    }
    tag.contains("commentary").then_some(Kind::Commentary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts<'a>(
        language: &'a str,
        technical: &str,
        kind: Option<Kind>,
        title: &'a str,
    ) -> Parts<'a> {
        Parts {
            language,
            technical: technical.to_string(),
            kind,
            title,
        }
    }

    #[test]
    fn all_three_segments_when_all_three_are_known() {
        let p = parts("rus", "AAC 6ch", Some(Kind::Described), "");
        assert_eq!(
            line(&p, Naming::Native, "?"),
            "Русский - AAC 6ch - Audio Description"
        );
    }

    /// The whole point of the exercise: two tracks that are the same thing
    /// read the same, whether the answer came from a flag or from the title.
    #[test]
    fn a_flag_and_a_title_produce_the_same_row() {
        let by_flag = parts("eng", "AAC 2ch", Some(Kind::Described), "English Extra");
        let by_title = parts(
            "eng",
            "AAC 2ch",
            Some(Kind::Described),
            "English Audio Description",
        );
        assert_eq!(
            line(&by_flag, Naming::Native, "?"),
            line(&by_title, Naming::Native, "?")
        );
    }

    /// A title that no flag could have recorded is kept, because dropping it
    /// would lose the only thing that tells this track from the next.
    #[test]
    fn a_title_survives_when_no_type_was_worked_out() {
        let p = parts("eng", "FLAC 2ch", None, "Restored 1998 Mix");
        assert_eq!(
            line(&p, Naming::Native, "?"),
            "English - FLAC 2ch - Restored 1998 Mix"
        );
    }

    /// The commonest title of all says only what the language segment just
    /// said, in either language's word for it.
    #[test]
    fn a_title_that_only_repeats_the_language_is_dropped() {
        for title in ["English", "english"] {
            let p = parts("eng", "AAC 2ch", None, title);
            assert_eq!(line(&p, Naming::Native, "?"), "English - AAC 2ch");
        }
        let p = parts("rus", "AAC 2ch", None, "Russian");
        assert_eq!(line(&p, Naming::Native, "?"), "Русский - AAC 2ch");
    }

    /// Segments that have nothing to say are left out rather than shown empty,
    /// which is what a subtitle file beside the video looks like.
    #[test]
    fn empty_segments_are_left_out() {
        let p = parts("fre", "", None, "");
        assert_eq!(line(&p, Naming::Native, "?"), "Français");
        let p = parts("", "", Some(Kind::Forced), "");
        assert_eq!(
            line(&p, Naming::Native, "Subtitle 4"),
            "Subtitle 4 - Forced"
        );
    }

    /// A tag the table does not carry is shown as the file wrote it rather
    /// than guessed at: it still came out of the file, and it still tells two
    /// tracks apart.
    #[test]
    fn an_unknown_tag_is_shown_as_written() {
        let p = parts("qqq", "AAC 2ch", None, "");
        assert_eq!(line(&p, Naming::Native, "?"), "qqq - AAC 2ch");
    }

    /// A track that never said what language it is takes the caller's wording
    /// in the language's place, rather than losing the segment.
    ///
    /// Dropping it read acceptably in a numbered list and produced two
    /// identical, unpickable rows in the chooser, which numbers nothing: both
    /// of a file's untitled subtitles came out as "SRT".
    #[test]
    fn an_unstated_language_still_gets_a_segment() {
        let p = parts("und", "AAC 2ch", None, "");
        assert_eq!(line(&p, Naming::Native, "Track 3"), "Track 3 - AAC 2ch");

        let first = parts("und", "SRT", None, "");
        let second = parts("", "SRT", None, "");
        assert_ne!(
            line(&first, Naming::Native, "Subtitle 1"),
            line(&second, Naming::Native, "Subtitle 2")
        );
    }

    /// `--list-tracks` keeps the tag, because that list is where somebody
    /// learns what to type.
    #[test]
    fn the_cli_naming_keeps_the_tag() {
        let p = parts("rus", "AAC 6ch", Some(Kind::Commentary), "");
        assert_eq!(
            line(&p, Naming::WithTag, "?"),
            "rus (Русский) - AAC 6ch - Commentary"
        );
    }

    /// A title the first segment is already showing is not shown twice. Only
    /// where the language was unstated, which is the one case the two can be
    /// the same string.
    #[test]
    fn a_title_the_row_is_already_named_by_is_dropped() {
        let p = parts("", "", None, "Signs");
        assert_eq!(line(&p, Naming::Native, "Signs"), "Signs");
    }

    /// The whole point again, from the file end: a soundtrack beside the video
    /// reads exactly as the equivalent track inside it, bar the codec no file
    /// states.
    #[test]
    fn a_file_reads_as_the_track_it_matches() {
        assert_eq!(
            named_after_the_film(
                "en.ad",
                "Film (2019).en.ad.mp3",
                kind_of_audio_tag,
                Naming::Native
            ),
            "English - Audio Description"
        );
        assert_eq!(
            named_after_the_film(
                "en.hi",
                "Film (2019).en.hi.srt",
                kind_of_subtitle_tag,
                Naming::Native
            ),
            "English - SDH"
        );
    }

    /// A tag naming no language stands as it is, with its reading after it -
    /// which is how `ad` and `described` are told from each other at all.
    #[test]
    fn a_tag_that_names_no_language_stands_as_written() {
        for tag in ["ad", "described"] {
            assert_eq!(
                named_after_the_film(tag, "Film.mp3", kind_of_audio_tag, Naming::Native),
                format!("{tag} - Audio Description")
            );
        }
        assert_eq!(
            named_after_the_film("commentary", "Film.mp3", kind_of_audio_tag, Naming::Native),
            "commentary - Commentary"
        );
    }

    /// What a tag says beyond its language is kept where no type was worked
    /// out of it, for the same reason a track's title is: nothing else records
    /// it.
    #[test]
    fn a_tag_says_what_no_type_covers() {
        assert_eq!(
            named_after_the_film("en.restored", "Film.mp3", kind_of_audio_tag, Naming::Native),
            "English - restored"
        );
    }

    /// A file named exactly after the film says nothing about itself, and the
    /// name it is shown by is the film's rather than its own - so it stands as
    /// an identifier and is not read. `Ad Astra` is a film, not an audio
    /// description.
    #[test]
    fn a_film_named_file_is_shown_but_not_read() {
        assert_eq!(
            named_after_the_film("", "Ad Astra (2019).mka", kind_of_audio_tag, Naming::Native),
            "Ad Astra (2019).mka"
        );
        assert_eq!(
            named_after_the_film(
                "",
                "Hi, Mom (2021).srt",
                kind_of_subtitle_tag,
                Naming::Native
            ),
            "Hi, Mom (2021).srt"
        );
    }

    /// A file nothing named after the film is its own name, and that name *is*
    /// read: a downloaded soundtrack arrives called `AD.mp3` as often as it
    /// arrives named after the film.
    #[test]
    fn a_file_named_after_nothing_is_read_for_what_it_says() {
        assert_eq!(
            named_after_nothing("AD.mp3", kind_of_audio_tag),
            "AD.mp3 - Audio Description"
        );
        assert_eq!(
            named_after_nothing("English.srt", kind_of_subtitle_tag),
            "English.srt"
        );
        // A name is not a tag: nothing is split off it, so neither a language
        // nor an extension is picked out of one that happens to look like one.
        assert_eq!(named_after_nothing("en.mp3", kind_of_audio_tag), "en.mp3");
    }

    /// `hi` is Hindi beside a soundtrack and hearing-impaired beside a
    /// subtitle, which is why the two readings are separate functions.
    #[test]
    fn a_tag_is_read_for_the_kind_of_track_it_is() {
        assert_eq!(kind_of_audio_tag("en.hi"), None);
        assert_eq!(kind_of_subtitle_tag("en.hi"), Some(Kind::Sdh));
        assert_eq!(kind_of_audio_tag("en.ad"), Some(Kind::Described));
        assert_eq!(kind_of_subtitle_tag("en.ad"), None);
    }

    /// The printed list keeps the whole tag, because that is what
    /// `--subtitle` is matched against.
    #[test]
    fn the_cli_naming_keeps_a_files_tag_whole() {
        assert_eq!(
            named_after_the_film(
                "en.hi",
                "Film.en.hi.srt",
                kind_of_subtitle_tag,
                Naming::WithTag
            ),
            "en.hi (English) - SDH"
        );
    }
}
