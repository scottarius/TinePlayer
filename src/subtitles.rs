//! Choosing subtitles, from inside the file or from alongside it.
//!
//! Kept apart from the audio settings deliberately: the subtitle language is
//! an independent choice, and may well be a third language rather than a copy
//! of either soundtrack.

use std::borrow::Cow;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::probe::SubtitleTrack;
use crate::{tr, trc};

/// Formats GStreamer can parse from a plain file. Blu-ray `.sup` and the
/// VOBSUB `.sub`/`.idx` pair are deliberately absent: both are bitmap
/// formats with no decoder in the shipped GStreamer.
pub const EXTENSIONS: [&str; 4] = ["srt", "ass", "ssa", "vtt"];

/// One entry in the subtitle chooser.
#[derive(Clone, Debug, PartialEq)]
pub enum Subtitle {
    Embedded {
        index: u32,
        /// The whole of what the track says, as one string: what `--subtitle`
        /// is matched against and what a saved choice refers to. Kept exactly
        /// as it was, which is why the two halves below are carried beside it
        /// rather than being cut back out of it.
        label: String,
        /// The language tag alone, for the row's first segment.
        language: String,
        /// The title alone, for its last - where no type was worked out.
        title: String,
        /// What the stream is - `SRT`, `PGS`. The technical middle of a row.
        format: String,
        /// What this subtitle is for, worked out once where the track was in
        /// hand: flags, then sidecar, then title. Carried rather than
        /// re-derived because this is the only place all three were available.
        kind: Option<crate::label::Kind>,
    },
    External {
        /// The file's own name, which is how the choice is stored and found
        /// again.
        name: String,
        /// The tag the convention left in that name, exactly as written, and
        /// **empty for a file named exactly after the film** - which is a real
        /// case and says nothing about itself. For a file named after nothing
        /// this is the name over again, that being the whole of what it says.
        ///
        /// What `--subtitle` is matched against, and what the forced check and
        /// the language preferences read.
        label: String,
    },
    /// A file chosen by hand, from anywhere on disk. Kept apart from
    /// `External`, which is a name and means "beside the video": the two look
    /// alike and resolve differently, and collapsing them would make every
    /// subtitle found beside a video into an absolute path that breaks the
    /// moment the library is mounted somewhere else.
    File {
        path: std::path::PathBuf,
        label: String,
    },
    /// A subtitle file the media server holds beside the video, fetched over
    /// HTTP rather than found on disk.
    ///
    /// Carried as the server's own stream index and never as a URL. The URL
    /// would have to contain the access token, and this choice is written to
    /// disk with the resume position - which is exactly the leak the token is
    /// kept out of `config.yaml` to avoid. The index is meaningless to anyone
    /// without the pairing.
    Library {
        index: u32,
        /// The language and title run together, as [`Subtitle::Embedded`]
        /// carries it: what `--subtitle` is matched against, and what the row
        /// falls back to where the stream stated neither.
        label: String,
        /// The language alone, for the row's first segment.
        language: String,
        /// The title alone, for its last - and where the type is read from,
        /// the server's own flags being unreliable.
        title: String,
    },
}

impl Subtitle {
    pub fn label(&self) -> &str {
        match self {
            Subtitle::Embedded { label, .. }
            | Subtitle::External { label, .. }
            | Subtitle::File { label, .. }
            | Subtitle::Library { label, .. } => label,
        }
    }

    pub fn choice(&self) -> SubtitleChoice {
        match self {
            Subtitle::Embedded { index, .. } => SubtitleChoice::Embedded(*index),
            Subtitle::External { name, .. } => SubtitleChoice::External(name.clone()),
            Subtitle::File { path, .. } => SubtitleChoice::File(path.clone()),
            Subtitle::Library { index, .. } => SubtitleChoice::Library(*index),
        }
    }
}

/// The persisted form. Stored by stream index or by file name rather than by
/// menu position, so it still resolves when the list changes.
///
/// A file name rather than a path: subtitle files live beside the video, so the
/// folder is already known, and storing only the name means the choice survives
/// the whole library moving or being mounted somewhere else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SubtitleChoice {
    Embedded(u32),
    External(String),
    /// A file anywhere on disk, chosen by hand or named on the command line.
    /// Stored whole, unlike `External`: there is no folder to imply it from.
    File(std::path::PathBuf),
    /// The media server's own index for a subtitle file it holds. Resolved to
    /// a URL only when a pipeline is built, so no token is ever written here.
    Library(u32),
}

/// A subtitle choice with its location worked out, which is all a pipeline
/// needs to open one.
///
/// Separate from [`SubtitleChoice`] because resolving needs things the
/// pipeline has no business knowing: which folder the video sits in, and for a
/// library's subtitle the server address and access token. Doing it before the
/// pipeline is built also means a subtitle that cannot be found fails while
/// there is still somewhere to say so.
#[derive(Clone, Debug, PartialEq)]
pub enum SubtitleSource {
    /// A stream inside the video, by position among the embedded subtitles.
    Embedded(u32),
    /// Anything that can be opened by URI: a file beside the video, one picked
    /// by hand, or a library's, which is an HTTP address carrying its own
    /// credential and so must never be written down.
    Uri(String),
}

/// Everything on offer for a video: what is inside it, then what sits beside
/// it on disk.
///
/// `video` is `None` for a remote source, which offers only what is embedded -
/// there is no folder to look in, and a media server hands its own subtitles
/// over inside the stream anyway.
pub fn options(
    video: Option<&Path>,
    embedded: &[SubtitleTrack],
    library: &[Subtitle],
) -> Vec<Subtitle> {
    let mut options: Vec<Subtitle> = embedded
        .iter()
        .map(|track| Subtitle::Embedded {
            index: track.index,
            label: if track.title.is_empty() {
                track.language.clone()
            } else {
                format!("{} - {}", track.language, track.title)
            },
            language: track.language.clone(),
            title: track.title.clone(),
            format: track.format.clone(),
            kind: track.kind(),
        })
        .collect();
    if let Some(video) = video {
        options.extend(external(video));
    }
    // After what is in the file, matching how subtitles beside a local video
    // are listed: what the container holds first, what sits alongside it next.
    options.extend(library.iter().cloned());
    options
}

/// An entry for a subtitle file chosen by hand, labelled by its own name.
///
/// The label is the file name rather than the language tag `external` reads
/// out of one: a file picked from somewhere else is not named to the
/// convention, and guessing a language from it would be guessing.
pub fn chosen_file(path: &Path) -> Subtitle {
    Subtitle::File {
        path: path.to_path_buf(),
        label: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string()),
    }
}

/// Subtitle files sitting next to the video and named after it.
///
/// The convention media libraries use is `<video name>.<language>.srt`, with
/// optional extra tags such as `.hi` for hearing-impaired versions. Whatever
/// sits between the video's name and the extension is shown as the label,
/// since that is where the language ended up.
///
/// Found by [`crate::beside`], which is the one place that convention is
/// written down: separate soundtracks are named the same way and are found by
/// the same code, so the two lists cannot drift apart.
fn external(video: &Path) -> Vec<Subtitle> {
    let mut found: Vec<Subtitle> = crate::beside::files(video, &EXTENSIONS[..])
        .into_iter()
        .map(|file| Subtitle::External {
            // The tag as written, empty and all. It used to become the word
            // "External" where there was none, which named nothing, could not
            // be translated without breaking what `--subtitle` matches, and
            // read as a row that had failed rather than one with nothing to
            // say. An empty tag is shown as the file's own name instead - see
            // `row` - which at least identifies it.
            label: file.tag,
            name: file.name,
        })
        .collect();

    found.sort_by(|a, b| a.label().cmp(b.label()));

    // Then any subtitle file loose in the folder, where the folder holds only
    // this film - the same rule separate soundtracks get, and the same reason.
    // A subtitle downloaded from a subtitle site arrives called `English.srt`
    // or `2_eng.srt`, named after nothing, so holding out for the convention
    // means the commonest way one arrives is the one way it is not offered.
    //
    // Safe only because it is the only film there. The convention earns its
    // keep telling one film's files from another's in a shared folder; where
    // there is nothing to tell apart, a subtitle in the folder can only be for
    // the film in the folder.
    let mut loose: Vec<Subtitle> = crate::beside::in_a_lone_film_folder(video, is_subtitle_file)
        .into_iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|name| {
            !found
                .iter()
                .any(|subtitle| matches!(subtitle, Subtitle::External { name: found, .. } if found == name))
        })
        // Named to no convention, so there is no tag to read and nothing to
        // dress it up with: the file's own name is both the label and the only
        // thing that tells two of them apart.
        .map(|name| Subtitle::External {
            label: name.clone(),
            name,
        })
        .collect();
    loose.sort_by(|a, b| a.label().cmp(b.label()));

    // Capped, and sorted before it is capped so the same ones survive every
    // time rather than whatever order the directory happened to answer in.
    //
    // The named files above are not capped and should not be: a release with
    // twenty languages beside it named after the film is unambiguous, and
    // every one of them is a real choice. These are the unnamed ones, where
    // the folder is being taken at its word - a person who has downloaded
    // subtitles by hand has a few, and a folder with more than this in it is
    // not a film folder with extras in it, it is something else that a
    // subtitle chooser should not try to be a file browser for.
    if loose.len() > MAX_LOOSE {
        log::error!(
            "{} unnamed subtitle files beside the video; offering the first {} by name",
            loose.len(),
            MAX_LOOSE
        );
        loose.truncate(MAX_LOOSE);
    }

    found.extend(loose);
    found
}

/// How many subtitle files named after nothing are offered from a folder
/// holding a single film. Above what anybody downloads by hand, below the
/// point where the chooser stops being readable from across a room.
const MAX_LOOSE: usize = 10;

/// Whether a path is a subtitle this build can read, by its extension.
fn is_subtitle_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

impl Subtitle {
    /// Whether this is a forced track: one carrying only the lines a viewer
    /// who understands the dialogue still needs, such as alien speech or
    /// signs.
    ///
    /// Read from the name, and from a sidecar when one says so.
    ///
    /// What the file said, then what it is called.
    ///
    /// The flag is the better answer where there is one, and there is one far
    /// more often than this used to assume: `matroskademux` reads
    /// `TrackForced` and discards it, so the flag looked absent when it was
    /// only unreachable. [`crate::matroska`] reads it back, and a `.nfo`
    /// answers for the files that have one.
    ///
    /// Where nothing stated it - an MP4, a subtitle file beside the video, a
    /// library's stream - the name still carries the intent, because the
    /// convention of writing "Forced" into the title predates anyone relying
    /// on the flag. An explicit *no* from the file is believed over a name,
    /// which is the one case that changed: a track the file says is not forced
    /// but somebody titled "Forced" is not forced.
    /// What this subtitle is for, or `None` for a plain one - which is what
    /// the preference calls "full".
    ///
    /// An embedded track was answered when it was probed, on the full ladder
    /// with its flags in hand. A file beside the video has no flags at all, so
    /// its name is the only evidence there is, read with the same words.
    pub fn kind(&self) -> Option<crate::label::Kind> {
        if let Subtitle::Embedded { kind, .. } = self {
            return *kind;
        }
        let label = self.label();
        if crate::label::says_forced(label) {
            return Some(crate::label::Kind::Forced);
        }
        if crate::label::says_sdh(label) {
            return Some(crate::label::Kind::Sdh);
        }
        crate::label::says_commentary(label).then_some(crate::label::Kind::Commentary)
    }

    pub fn is_forced(&self) -> bool {
        self.kind() == Some(crate::label::Kind::Forced)
    }
}

/// What kind of subtitle to prefer, and what is acceptable instead.
///
/// **Stored values only.** These go into `config.yaml` and are matched by
/// `--subtitle`, so they are never translated. What each reads as on screen is
/// [`kind_label`].
///
/// The list used to be five entries crossing *kind* with *which output*, which
/// meant every new kind multiplied it: adding SDH would have made seven, and
/// commentary nine. Splitting the two apart on 2026-08-24 made SDH reachable
/// at all - it had no entry before, which is a poor gap in a player that
/// otherwise takes accessibility seriously.
pub const KINDS: [&str; 5] = ["none", "forced_only", "forced", "full", "sdh"];

/// Which language, and whether the other output's will do.
///
/// Stored values, as [`KINDS`] are. Anything not in this list is a language
/// code, which is the rest of what the chooser offers.
pub const PLACES: [&str; 4] = ["first_only", "second_only", "first", "second"];

/// Forced subtitles for whatever the room is hearing, and nothing if there are
/// none. A dub usually speaks every sign and foreign line aloud, so the only
/// gap worth filling is in the original language - which the primary output is
/// most likely to be carrying.
pub const DEFAULT_KIND: &str = "forced_only";
pub const DEFAULT_PLACE: &str = "first";

/// How one of [`KINDS`] reads on screen, or `None` if that is not one.
pub fn kind_label(value: &str) -> Option<Cow<'static, str>> {
    Some(match value {
        // A third sense of "None" in this interface, after an output device
        // and a list of languages. English spells all three the same way and
        // several languages do not, which is what the context is for.
        "none" => trc!("subtitle preference", "None"),
        // "Only" against two "Prefer"s, deliberately: the word is the whole
        // difference between them, and making the three read alike would hide
        // the one thing somebody needs to know before choosing.
        "forced_only" => tr!("Forced Only"),
        "forced" => tr!("Prefer Forced"),
        "full" => tr!("Prefer Full"),
        "sdh" => tr!("Prefer SDH"),
        _ => return None,
    })
}

/// How one of [`PLACES`] reads on screen, or `None` if that is not one.
pub fn place_label(value: &str) -> Option<Cow<'static, str>> {
    Some(match value {
        "first_only" => tr!("First Output Only"),
        "second_only" => tr!("Second Output Only"),
        "first" => tr!("Prefer First Output"),
        "second" => tr!("Prefer Second Output"),
        _ => return None,
    })
}

/// How the language setting reads on screen: one of [`PLACES`] or a language.
///
/// Not `languages::display_name` alone, which hands back whatever it was given
/// when it recognizes nothing - and it recognizes none of these, so the
/// settings list would show `first_only` rather than what it means.
pub fn describe_place(setting: Option<&str>) -> String {
    let setting = setting.unwrap_or(DEFAULT_PLACE);
    place_label(setting)
        .map(Cow::into_owned)
        .unwrap_or_else(|| crate::languages::display_name(setting))
}

/// How the kind setting reads on screen.
pub fn describe_kind(setting: Option<&str>) -> String {
    let setting = setting.unwrap_or(DEFAULT_KIND);
    kind_label(setting)
        .map(Cow::into_owned)
        .unwrap_or_else(|| kind_label(DEFAULT_KIND).unwrap().into_owned())
}

/// What to look for, in the order it is acceptable.
///
/// **The ladder is derived rather than configured**, from one rule: after the
/// kind that was asked for, take the nearest on `Forced ⊂ Full ⊂ SDH`. Forced
/// carries signs alone, full adds the dialogue, SDH adds the sound - so the
/// nearest is always the one that changes least about what appears on screen.
///
/// Only one step is ambiguous. From full, SDH and forced are each one away;
/// SDH wins, because it keeps the dialogue and forced does not.
///
/// `Forced Only` is its own entry rather than a flag on `forced`, because the
/// two are different intentions. Somebody who understands the audio wants the
/// signs and nothing else, and a wall of dialogue is worse than silence. But a
/// film with foreign speech and no forced track leaves that person with
/// nothing at all - which is the case `Prefer Forced` exists for, and which is
/// why both are offered rather than one being chosen for everybody.
///
/// Commentary is never in a ladder. Nobody wants it selected on every film,
/// and it is one press away in the chooser.
#[derive(Clone, Debug, PartialEq)]
pub enum Wanted {
    /// Show none.
    None,
    /// The kinds to try, in order. `None` in the list means a plain subtitle -
    /// no flag, no marker in the title - which is what "full" is.
    Ladder(Vec<Option<crate::label::Kind>>),
}

impl Wanted {
    /// Reads the kind setting. Anything unrecognized is the default, since
    /// this half of the preference has no free-text form the way the language
    /// half does.
    pub fn parse(setting: &str) -> Self {
        use crate::label::Kind;
        match setting.trim().to_lowercase().as_str() {
            "" | "none" => Self::None,
            "forced_only" => Self::Ladder(vec![Some(Kind::Forced)]),
            "forced" => Self::Ladder(vec![Some(Kind::Forced), None, Some(Kind::Sdh)]),
            "sdh" => Self::Ladder(vec![Some(Kind::Sdh), None, Some(Kind::Forced)]),
            // Full, and the fallback for anything unrecognized.
            _ => Self::Ladder(vec![None, Some(Kind::Sdh), Some(Kind::Forced)]),
        }
    }
}

/// Which languages to try, in order.
///
/// The two "Prefer" entries used to be hardwired into asking for forced
/// subtitles and available to nothing else: that mode tried the other output's
/// language and the others did not, which was right and invisible. Here it is
/// a choice, and available to every kind.
pub fn places(setting: &str, primary: Option<&str>, secondary: Option<&str>) -> Vec<String> {
    let one = |language: Option<&str>| language.map(str::to_string).into_iter().collect::<Vec<_>>();
    match setting.trim().to_lowercase().as_str() {
        "first_only" => one(primary),
        "second_only" => one(secondary),
        "first" => [one(primary), one(secondary)].concat(),
        "second" => [one(secondary), one(primary)].concat(),
        // A named language, which is the rest of the chooser.
        other => vec![other.to_string()],
    }
}

/// Whether changing an output's soundtrack should re-choose the subtitle.
///
/// A question about the *language* half alone now: the kind does not depend on
/// what anybody is hearing. Both "Prefer" entries follow either output,
/// because either language can be the one that supplies the answer, and a
/// named language follows neither.
pub fn follows_output(place: &str, secondary_output: bool) -> bool {
    match place.trim().to_lowercase().as_str() {
        "first_only" => !secondary_output,
        "second_only" => secondary_output,
        "first" | "second" => true,
        _ => false,
    }
}

pub fn automatic(
    wanted: &Wanted,
    place: &str,
    options: &[Subtitle],
    primary_language: Option<&str>,
    secondary_language: Option<&str>,
) -> Option<SubtitleChoice> {
    let Wanted::Ladder(ladder) = wanted else {
        return None;
    };

    // **Language outside, kind inside.** Exhaust what is acceptable in the
    // first language before moving to the second, rather than the reverse:
    // dialogue in a language somebody cannot read is no use to them, so a
    // different *kind* in their own language is nearly always the better
    // answer. Forced is the exception that proves it - signs carry no dialogue
    // and are worth having in any language - but its ladder is one entry long,
    // so both orders give the same answer for it and no special case is
    // needed.
    for language in places(place, primary_language, secondary_language) {
        for kind in ladder {
            let found = options.iter().find(|option| {
                option.kind() == *kind && crate::languages::matches(option.label(), &language)
            });
            if let Some(option) = found {
                return Some(option.choice());
            }
        }
    }
    None
}

/// What `--subtitle` was given, resolved against what this video offers.
///
/// Accepts, in this order:
///
/// * any of [`MODES`], meaning the same as choosing it in the settings but for
///   this run only.
/// * a position as `--list-tracks` prints it, for acting on a file you are
///   looking at now. Positions are not stable: the external entries come from
///   a directory listing, so another subtitle file appearing beside the video
///   renumbers everything after it.
/// * the name of a subtitle file beside the video, or the label as printed.
/// * a language code, taking the first subtitle in that language.
///
/// A name is reduced to its last component, so a path pasted in from somewhere
/// else still resolves against the video's own folder rather than pointing
/// outside it.
///
/// `Ok(None)` means no subtitles were asked for. `Err` describes what was
/// offered, which is more use than silently playing without them.
pub fn resolve(
    spec: &str,
    options: &[Subtitle],
    primary_language: Option<&str>,
    secondary_language: Option<&str>,
) -> Result<Option<SubtitleChoice>, String> {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("none") || spec == "0" {
        return Ok(None);
    }

    // Either half of the setting is accepted here and means the same: one run
    // set that way, rather than changing the setting itself. Naming one half
    // leaves the other at its default, since this has no config to read - so
    // `--subtitle sdh` is "SDH, following the first output" and
    // `--subtitle second_only` is "the usual kind, second output only".
    let names_a_kind = KINDS.iter().any(|value| value.eq_ignore_ascii_case(spec));
    let names_a_place = PLACES.iter().any(|value| value.eq_ignore_ascii_case(spec));
    if names_a_kind || names_a_place {
        let (kind, place) = match names_a_kind {
            true => (spec, DEFAULT_PLACE),
            false => (DEFAULT_KIND, spec),
        };
        return Ok(automatic(
            &Wanted::parse(kind),
            place,
            options,
            primary_language,
            secondary_language,
        ));
    }

    let offered = || {
        options
            .iter()
            .map(Subtitle::label)
            // A file named exactly after the film has no label of its own.
            // Reachable by its file name and by position, both of which this
            // message's own list would be lying about if it showed a gap.
            .filter(|label| !label.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    };

    if let Ok(position) = spec.parse::<usize>() {
        return options
            .get(position.wrapping_sub(1))
            .map(|option| Some(option.choice()))
            .ok_or_else(|| {
                format!(
                    "There is no subtitle {position}. This video offers {}.",
                    options.len()
                )
            });
    }

    // A path to a file that exists is taken as itself, wherever it points.
    // Checked before the name matching below, which deliberately keeps to the
    // video's own folder: an argument naming a file outside that folder can
    // only mean that file, and answering with a same-named one from beside the
    // video would be answering a different question.
    let named_path = Path::new(spec);
    if named_path.is_file() {
        return Ok(Some(SubtitleChoice::File(named_path.to_path_buf())));
    }

    // Only the last component, so neither a relative nor an absolute path can
    // send this outside the video's folder.
    let wanted = spec.rsplit(['/', '\\']).next().unwrap_or(spec);

    if let Some(named) = options.iter().find(|option| match option {
        Subtitle::External { name, .. } => name.eq_ignore_ascii_case(wanted),
        Subtitle::File { path, .. } => path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(wanted)),
        // Neither has a file name to match. A library subtitle is still
        // reachable by the label below, which is what somebody would type.
        Subtitle::Embedded { .. } | Subtitle::Library { .. } => false,
    }) {
        return Ok(Some(named.choice()));
    }

    // The label as printed, which is how a file with extra tags is named:
    // "en.hi" is not a language code, but it is what the list shows.
    if let Some(labelled) = options
        .iter()
        .find(|option| option.label().eq_ignore_ascii_case(wanted))
    {
        return Ok(Some(labelled.choice()));
    }

    // A language code, matched the way the language preferences are: on the
    // leading letters, so "en" finds a track labelled "eng" or "en.hi".
    if let Some(spoken) = options
        .iter()
        .find(|option| crate::languages::matches(option.label(), wanted))
    {
        return Ok(Some(spoken.choice()));
    }

    Err(format!(
        "No subtitle matches \"{spec}\". This video offers: {}.",
        offered()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<Subtitle> {
        vec![
            Subtitle::Embedded {
                index: 0,
                label: "en".to_string(),
                language: "en".to_string(),
                title: String::new(),
                format: String::new(),
                kind: None,
            },
            Subtitle::External {
                name: "Film (2019).en.hi.srt".to_string(),
                label: "en.hi".to_string(),
            },
            Subtitle::External {
                name: "Film (2019).ru.hi.srt".to_string(),
                label: "ru.hi".to_string(),
            },
        ]
    }

    #[test]
    fn none_is_explicit() {
        assert_eq!(resolve("0", &options(), None, None), Ok(None));
        assert_eq!(resolve("none", &options(), None, None), Ok(None));
        assert_eq!(resolve("None", &options(), None, None), Ok(None));
    }

    #[test]
    fn a_position_indexes_the_printed_list() {
        assert_eq!(
            resolve("1", &options(), None, None),
            Ok(Some(SubtitleChoice::Embedded(0)))
        );
        assert_eq!(
            resolve("3", &options(), None, None),
            Ok(Some(SubtitleChoice::External(
                "Film (2019).ru.hi.srt".to_string()
            )))
        );
        assert!(resolve("9", &options(), None, None).is_err());
        assert!(resolve("0", &options(), None, None).is_ok());
    }

    #[test]
    fn a_language_code_takes_the_first_of_that_language() {
        // "ru" is not a label here; it matches ru.hi on the leading letters.
        assert_eq!(
            resolve("ru", &options(), None, None),
            Ok(Some(SubtitleChoice::External(
                "Film (2019).ru.hi.srt".to_string()
            )))
        );
        // "en" is a label exactly, and the embedded track comes first.
        assert_eq!(
            resolve("en", &options(), None, None),
            Ok(Some(SubtitleChoice::Embedded(0)))
        );
    }

    #[test]
    fn a_label_with_tags_resolves() {
        assert_eq!(
            resolve("en.hi", &options(), None, None),
            Ok(Some(SubtitleChoice::External(
                "Film (2019).en.hi.srt".to_string()
            )))
        );
    }

    #[test]
    fn a_file_name_resolves_and_paths_are_reduced_to_it() {
        let expected = Ok(Some(SubtitleChoice::External(
            "Film (2019).ru.hi.srt".to_string(),
        )));
        assert_eq!(
            resolve("Film (2019).ru.hi.srt", &options(), None, None),
            expected
        );
        // A path from anywhere still means the file beside the video, never
        // somewhere else on disk.
        assert_eq!(
            resolve(
                "C:\\elsewhere\\Film (2019).ru.hi.srt",
                &options(),
                None,
                None
            ),
            expected
        );
        assert_eq!(
            resolve("/tmp/Film (2019).ru.hi.srt", &options(), None, None),
            expected
        );
    }

    #[test]
    fn anything_else_is_reported() {
        assert!(resolve("nonsense", &options(), None, None).is_err());
        assert!(resolve("../../etc/passwd", &options(), None, None).is_err());
        assert!(resolve("de", &options(), None, None).is_err());
    }
}

#[cfg(test)]
mod automatic_tests {
    use super::*;

    fn options() -> Vec<Subtitle> {
        vec![
            Subtitle::Embedded {
                index: 0,
                label: "ru - Forced".to_string(),
                language: "ru - Forced".to_string(),
                title: String::new(),
                format: String::new(),
                kind: Some(crate::label::Kind::Forced),
            },
            Subtitle::Embedded {
                index: 1,
                label: "ru - Full".to_string(),
                language: "ru - Full".to_string(),
                title: String::new(),
                format: String::new(),
                kind: None,
            },
            Subtitle::Embedded {
                index: 2,
                label: "en - Full".to_string(),
                language: "en - Full".to_string(),
                title: String::new(),
                format: String::new(),
                kind: None,
            },
            Subtitle::External {
                name: "f.en.forced.srt".to_string(),
                label: "en.forced".to_string(),
            },
        ]
    }

    /// Forcedness is carried on the option now, worked out once from the
    /// track. That it can be read out of a *name* is a fact about
    /// `SubtitleTrack::kind`, and is tested there; this is only that the
    /// answer survives onto the row and is what the preference sees.
    #[test]
    fn forced_is_carried_by_the_option() {
        let o = options();
        assert!(o[0].is_forced());
        assert!(!o[1].is_forced());
        assert!(o[3].is_forced());
    }

    #[test]
    fn following_an_output_takes_its_language() {
        let o = options();
        let forced = Wanted::parse("forced_only");
        assert_eq!(
            automatic(&forced, "first", &o, Some("ru"), Some("en")),
            Some(SubtitleChoice::Embedded(0))
        );
        let full = Wanted::parse("full");
        assert_eq!(
            automatic(&full, "second_only", &o, Some("ru"), Some("en")),
            Some(SubtitleChoice::Embedded(2))
        );
    }

    #[test]
    fn forced_and_full_never_substitute_for_each_other() {
        let o = options();
        // German is not present at all.
        assert_eq!(
            automatic(&Wanted::parse("full"), "first_only", &o, Some("de"), None),
            None
        );
        // Only a full English track exists besides the forced file, so asking
        // for forced English gets the file, and asking for full gets the track.
        assert_eq!(
            automatic(&Wanted::parse("forced_only"), "first", &o, Some("en"), None),
            Some(SubtitleChoice::External("f.en.forced.srt".to_string()))
        );
        assert_eq!(
            automatic(&Wanted::parse("full"), "first_only", &o, Some("en"), None),
            Some(SubtitleChoice::Embedded(2))
        );
    }

    #[test]
    fn no_forced_track_means_none_rather_than_a_full_one() {
        let only_full = vec![Subtitle::Embedded {
            index: 0,
            label: "ru - Full".to_string(),
            language: "ru - Full".to_string(),
            title: String::new(),
            format: String::new(),
            kind: None,
        }];
        assert_eq!(
            automatic(
                &Wanted::parse("forced_only"),
                "first",
                &only_full,
                Some("ru"),
                None
            ),
            None
        );
    }

    #[test]
    fn none_and_a_named_language() {
        let o = options();
        assert_eq!(
            automatic(&Wanted::parse("none"), "first", &o, Some("ru"), None),
            None
        );
        assert_eq!(
            automatic(&Wanted::parse("full"), "en", &o, Some("ru"), None),
            Some(SubtitleChoice::Embedded(2))
        );
    }

    #[test]
    fn the_default_is_forced_with_no_fallback() {
        assert_eq!(
            Wanted::parse(DEFAULT_KIND),
            Wanted::Ladder(vec![Some(crate::label::Kind::Forced)])
        );
        // And the default language follows the first output, trying the
        // second if it has nothing.
        assert_eq!(
            places(DEFAULT_PLACE, Some("en"), Some("ru")),
            vec!["en".to_string(), "ru".to_string()]
        );
    }
}

#[cfg(test)]
mod argument_tests {
    use super::*;

    fn options() -> Vec<Subtitle> {
        vec![
            Subtitle::Embedded {
                index: 0,
                label: "ru - Forced".to_string(),
                language: "ru - Forced".to_string(),
                title: String::new(),
                format: String::new(),
                kind: Some(crate::label::Kind::Forced),
            },
            Subtitle::Embedded {
                index: 1,
                label: "ru - Full".to_string(),
                language: "ru - Full".to_string(),
                title: String::new(),
                format: String::new(),
                kind: None,
            },
            Subtitle::Embedded {
                index: 2,
                label: "en - Full".to_string(),
                language: "en - Full".to_string(),
                title: String::new(),
                format: String::new(),
                kind: None,
            },
        ]
    }

    #[test]
    fn the_argument_accepts_either_half_of_the_setting() {
        let o = options();
        // A kind alone, with the language half left at its default - which
        // follows the first output and tries the second.
        assert_eq!(
            resolve("forced_only", &o, Some("ru"), Some("en")),
            Ok(Some(SubtitleChoice::Embedded(0)))
        );
        // A place alone, with the kind left at its default. "Only" is the
        // point here: the second output is English, this file has no forced
        // English track, and the forced Russian one is not taken because
        // nothing was said about falling back.
        assert_eq!(resolve("second_only", &o, Some("ru"), Some("en")), Ok(None));
        // The same place against the output that does have one.
        assert_eq!(
            resolve("first_only", &o, Some("ru"), Some("en")),
            Ok(Some(SubtitleChoice::Embedded(0)))
        );
    }

    #[test]
    fn modes_do_not_shadow_the_other_forms() {
        let o = options();
        assert_eq!(
            resolve("2", &o, None, None),
            Ok(Some(SubtitleChoice::Embedded(1)))
        );
        assert_eq!(
            resolve("en", &o, None, None),
            Ok(Some(SubtitleChoice::Embedded(2)))
        );
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;

    #[test]
    fn forced_prefers_one_output_but_takes_the_other() {
        // Only an English forced track exists; the room is hearing Russian.
        let o = vec![
            Subtitle::Embedded {
                index: 0,
                label: "ru - Full".to_string(),
                language: "ru - Full".to_string(),
                title: String::new(),
                format: String::new(),
                kind: None,
            },
            Subtitle::Embedded {
                index: 1,
                label: "en - Forced".to_string(),
                language: "en - Forced".to_string(),
                title: String::new(),
                format: String::new(),
                kind: Some(crate::label::Kind::Forced),
            },
        ];
        assert_eq!(
            automatic(
                &Wanted::parse("forced_only"),
                "first",
                &o,
                Some("ru"),
                Some("en")
            ),
            Some(SubtitleChoice::Embedded(1))
        );
    }

    #[test]
    fn the_preferred_output_still_wins_when_both_have_one() {
        let o = vec![
            Subtitle::Embedded {
                index: 0,
                label: "en - Forced".to_string(),
                language: "en - Forced".to_string(),
                title: String::new(),
                format: String::new(),
                kind: Some(crate::label::Kind::Forced),
            },
            Subtitle::Embedded {
                index: 1,
                label: "ru - Forced".to_string(),
                language: "ru - Forced".to_string(),
                title: String::new(),
                format: String::new(),
                kind: Some(crate::label::Kind::Forced),
            },
        ];
        assert_eq!(
            automatic(
                &Wanted::parse("forced_only"),
                "first",
                &o,
                Some("ru"),
                Some("en")
            ),
            Some(SubtitleChoice::Embedded(1))
        );
        assert_eq!(
            automatic(
                &Wanted::parse("forced_only"),
                "second",
                &o,
                Some("ru"),
                Some("en")
            ),
            Some(SubtitleChoice::Embedded(0))
        );
    }

    #[test]
    fn a_full_translation_never_falls_back() {
        let o = vec![Subtitle::Embedded {
            index: 0,
            label: "en - Full".to_string(),
            language: "en - Full".to_string(),
            title: String::new(),
            format: String::new(),
            kind: None,
        }];
        assert_eq!(
            automatic(
                &Wanted::parse("full"),
                "first_only",
                &o,
                Some("ru"),
                Some("en")
            ),
            None
        );
    }

    #[test]
    fn falling_back_still_refuses_an_unforced_track() {
        let o = vec![Subtitle::Embedded {
            index: 0,
            label: "en - Full".to_string(),
            language: "en - Full".to_string(),
            title: String::new(),
            format: String::new(),
            kind: None,
        }];
        assert_eq!(
            automatic(
                &Wanted::parse("forced_only"),
                "first",
                &o,
                Some("ru"),
                Some("en")
            ),
            None
        );
    }
}

/// Finding the subtitle files that sit beside a video, as opposed to choosing
/// between them once found.
#[cfg(test)]
mod external_tests {
    use super::*;

    /// A folder holding one film offers the subtitles loose in it, the same
    /// way it offers loose soundtracks - because a downloaded subtitle arrives
    /// named after the site rather than after the film.
    #[test]
    fn a_lone_film_takes_whatever_subtitles_are_in_the_folder() {
        let root = std::env::temp_dir().join("tp-subs-lone");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let video = root.join("Film (2019).mkv");
        std::fs::write(&video, b"").unwrap();
        std::fs::write(root.join("Film (2019).en.srt"), b"").unwrap();
        std::fs::write(root.join("English.srt"), b"").unwrap();
        std::fs::write(root.join("2_eng.srt"), b"").unwrap();
        // Not a subtitle, and must not be offered as one.
        std::fs::write(root.join("readme.txt"), b"").unwrap();

        let labels: Vec<String> = external(&video)
            .into_iter()
            .map(|s| s.label().to_string())
            .collect();
        // The one named to the convention first, read as its tag; then the
        // loose ones as themselves, sorted.
        assert_eq!(labels, ["en", "2_eng.srt", "English.srt"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The loose half is capped. A folder with more unnamed subtitles in it
    /// than anybody downloads is not a film folder, and the chooser should not
    /// try to be a file browser for it.
    #[test]
    fn loose_subtitles_are_capped() {
        let root = std::env::temp_dir().join("tp-subs-many");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let video = root.join("Film (2019).mkv");
        std::fs::write(&video, b"").unwrap();
        for n in 0..MAX_LOOSE + 5 {
            std::fs::write(root.join(format!("sub{n:02}.srt")), b"").unwrap();
        }

        let found = external(&video);
        assert_eq!(found.len(), MAX_LOOSE);
        // Sorted before it is cut, so the survivors are the same every run
        // rather than whatever the directory answered with first.
        assert_eq!(found[0].label(), "sub00.srt");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A second film in the folder takes the loose rule away, exactly as it
    /// does for soundtracks: `English.srt` names no owner, and guessing one
    /// would put the wrong film's subtitles on screen.
    #[test]
    fn a_second_film_takes_the_loose_subtitles_away() {
        let root = std::env::temp_dir().join("tp-subs-two-films");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let video = root.join("Film (2019).mkv");
        std::fs::write(&video, b"").unwrap();
        std::fs::write(root.join("Other (2020).mkv"), b"").unwrap();
        std::fs::write(root.join("Film (2019).en.srt"), b"").unwrap();
        std::fs::write(root.join("English.srt"), b"").unwrap();

        let labels: Vec<String> = external(&video)
            .into_iter()
            .map(|s| s.label().to_string())
            .collect();
        assert_eq!(labels, ["en"]);

        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Which soundtrack changes a preference cares about.
#[cfg(test)]
mod follows_tests {
    use super::*;

    #[test]
    fn a_named_language_follows_nothing() {
        assert!(!follows_output("ru", false));
        assert!(!follows_output("ru", true));
    }

    /// **Which outputs matter is now a property of the language setting, not
    /// of the kind.** Two tests here used to say that a full mode followed
    /// only its own output while a forced one followed both - true at the
    /// time, because falling back to the other output was hardwired into
    /// forced and unavailable to anything else. Splitting the preference in
    /// two on 2026-08-24 made it a choice, so the same rule now reads off one
    /// setting and applies to every kind.
    #[test]
    fn only_follows_its_own_output_and_fallback_follows_both() {
        assert!(follows_output("first_only", false));
        assert!(!follows_output("first_only", true));
        assert!(!follows_output("second_only", false));
        assert!(follows_output("second_only", true));
        for place in ["first", "second"] {
            assert!(follows_output(place, false), "{place}");
            assert!(follows_output(place, true), "{place}");
        }
    }
}

/// One row of a subtitle list, in the shape audio rows use - see
/// [`crate::label`].
///
/// `naming` decides how a language is written, because that is the caller's
/// business: a chooser on a television names it as itself, and `--list-tracks`
/// keeps the tag so somebody can see what to type.
///
/// **A file beside the video is named the same way**, which it was not until
/// 2026-08-20: its tag was always shown as written, on the reasoning that only
/// the first part of `en.hi` is a language and naming it natively would
/// quietly drop everything after the dot. That stopped being true when the
/// type moved into a segment of its own - `hi` is now read into "SDH" rather
/// than dropped - so a sidecar reads `English - SDH` where the same subtitle
/// inside the file reads `English - SRT - SDH`, and the two are the same fact.
pub fn row(option: &Subtitle, naming: crate::label::Naming) -> String {
    match option {
        // An embedded track states its language and its title separately, so
        // the row is built from those rather than from the label, which is the
        // two already run together and would come out doubled.
        Subtitle::Embedded {
            index,
            language,
            title,
            format,
            kind,
            ..
        } => crate::label::line(
            &crate::label::Parts {
                language,
                technical: format.clone(),
                kind: *kind,
                title,
            },
            naming,
            // What a track with no language of its own is called. Its number,
            // because that is the one thing it always has and is what
            // `--subtitle` takes.
            &tr!("Subtitle {number}").replace("{number}", &(index + 1).to_string()),
        ),
        // Loose in the folder, where the label is the file's own name over
        // again. Told from the one below by exactly that: a file named after
        // nothing is read, and a file named after the film is not.
        Subtitle::External { name, label } if label == name => {
            crate::label::named_after_nothing(name, crate::label::kind_of_subtitle_tag)
        }
        // Named after the film, so everything it says is in the tag - and
        // where the tag is empty it says nothing, and stands as its name.
        Subtitle::External { name, label } => crate::label::named_after_the_film(
            label,
            name,
            crate::label::kind_of_subtitle_tag,
            naming,
        ),
        // Picked by hand from somewhere on disk, named to no convention.
        // Reading a language out of it would be guessing, but what it is still
        // gets read the way a track's title is - `Film.en.forced.srt` says
        // forced whoever chose it.
        Subtitle::File { label, .. } => {
            crate::label::named_after_nothing(label, crate::label::kind_of_subtitle_tag)
        }
        // The library's own subtitle file. It states a language and a title
        // separately, exactly as an embedded track does, and is built from
        // those for the same reason - the label is the two run together.
        //
        // The type comes out of the title rather than from the server, which
        // reports `IsForced=False` on a track it titles "Forced".
        Subtitle::Library {
            language,
            title,
            label,
            ..
        } => crate::label::line(
            &crate::label::Parts {
                language,
                technical: String::new(),
                kind: crate::label::kind_of_subtitle_tag(title),
                title,
            },
            naming,
            // The label, which is the title where there is one and the generic
            // word where the stream said nothing at all.
            label,
        ),
    }
}

/// The same, named as the language names itself - what every on-screen list
/// uses.
pub fn row_native(option: &Subtitle) -> String {
    row(option, crate::label::Naming::Native)
}

#[cfg(test)]
mod row_tests {
    use super::*;

    fn beside(tag: &str, name: &str) -> Subtitle {
        Subtitle::External {
            name: name.to_string(),
            label: tag.to_string(),
        }
    }

    /// What the choosers show. Reported 2026-08-21: every track inside the
    /// film had been brought onto one formatter and the files beside it had
    /// not, so the same fact read two ways in one list.
    #[test]
    fn a_file_beside_the_video_reads_as_a_track_does() {
        assert_eq!(row_native(&beside("en", "Film.en.srt")), "English");
        assert_eq!(
            row_native(&beside("en.hi", "Film.en.hi.srt")),
            "English - SDH"
        );
        assert_eq!(
            row_native(&beside("es.forced", "Film.es.forced.srt")),
            "Español - Forced"
        );
    }

    /// The two files that carry no tag: one named exactly after the film, one
    /// named after nothing. Both stand as their own name, and only the second
    /// is read for what that name says.
    #[test]
    fn a_file_with_no_tag_stands_as_its_name() {
        assert_eq!(row_native(&beside("", "Film.srt")), "Film.srt");
        assert_eq!(
            row_native(&beside("Ver2 (forced).srt", "Ver2 (forced).srt")),
            "Ver2 (forced).srt - Forced"
        );
        // The film's own name is not the file's, so nothing is read out of it:
        // `Forced Entry` is a film, not a forced subtitle.
        assert_eq!(
            row_native(&beside("", "Forced Entry (2021).srt")),
            "Forced Entry (2021).srt"
        );
    }

    /// A library's own subtitle file states its language and its title apart,
    /// as an embedded track does, and is read the same way.
    #[test]
    fn a_librarys_file_reads_as_a_track_does() {
        // Labelled the way `jellyfin::Media::subtitle_options` labels one,
        // since the row falls back to it where the stream stated no language.
        let library = |language: &str, title: &str| Subtitle::Library {
            index: 3,
            label: match (language.is_empty(), title.is_empty()) {
                (false, false) => format!("{language} - {title}"),
                (false, true) => language.to_string(),
                (true, false) => title.to_string(),
                (true, true) => "Subtitles".to_string(),
            },
            language: language.to_string(),
            title: title.to_string(),
        };
        assert_eq!(row_native(&library("eng", "English SDH")), "English - SDH");
        assert_eq!(row_native(&library("eng", "Signs")), "English - Signs");
        // Nothing but a title, which is then the row rather than being said
        // twice.
        assert_eq!(row_native(&library("", "Signs")), "Signs");
    }
}

#[cfg(test)]
mod stated_false_tests {
    use super::*;

    /// The shape a scraped library actually produces, from a report on
    /// 2026-08-24: seventeen subtitle tracks, three of them titled "Forced",
    /// and no forced subtitle offered for any of them.
    ///
    /// **`forced: Some(false)` is the whole point of this fixture.** The
    /// container stated nothing - checked with `matroska::flags`, which
    /// answered `None` for every track - and the `.nfo` written beside it
    /// stated `<forced>False</forced>` on all seventeen, which is what a
    /// scraper writes when it did not look rather than when it checked. So the
    /// tracks arrive here carrying a stated no and a title saying yes.
    fn stated_false() -> Vec<Subtitle> {
        let track = |index: u32, language: &str, title: &str| crate::probe::SubtitleTrack {
            index,
            language: language.to_string(),
            title: title.to_string(),
            format: "SubRip".to_string(),
            // What the sidecar asserted, and what used to end the search.
            forced: Some(false),
            hearing_impaired: None,
            commentary: None,
        };
        let tracks = vec![
            track(0, "ru", "Russian (Forced)"),
            track(1, "ru", "Russian (iTunes)"),
            track(2, "ru", "Russian (Cool Story Blog)"),
            track(3, "en", "English (Forced)"),
            track(4, "en", "SDH"),
            track(5, "uk", "Ukrainian (Forced)"),
        ];
        options(None, &tracks, &[])
    }

    #[test]
    fn the_forced_tracks_are_recognised_as_forced() {
        let o = stated_false();
        for (at, expected) in [(0, true), (1, false), (2, false), (3, true), (5, true)] {
            assert_eq!(
                o[at].is_forced(),
                expected,
                "option {at} ({:?}) read as forced={}",
                o[at].label(),
                o[at].is_forced()
            );
        }
    }

    /// What was reported: English on the first output, Russian on the second,
    /// preference "Forced (Prefer Second Output Language)". It chose nothing.
    #[test]
    fn forced_following_the_second_output_finds_the_russian_one() {
        assert_eq!(
            automatic(
                &Wanted::parse("forced_only"),
                "second",
                &stated_false(),
                Some("en"),
                Some("ru")
            ),
            Some(SubtitleChoice::Embedded(0))
        );
    }

    #[test]
    fn forced_following_the_first_output_finds_the_english_one() {
        assert_eq!(
            automatic(
                &Wanted::parse("forced_only"),
                "first",
                &stated_false(),
                Some("en"),
                Some("ru")
            ),
            Some(SubtitleChoice::Embedded(3))
        );
    }
}

#[cfg(test)]
mod ladder_tests {
    use super::*;
    use crate::label::Kind;

    fn track(index: u32, language: &str, kind: Option<Kind>) -> Subtitle {
        Subtitle::Embedded {
            index,
            label: language.to_string(),
            language: language.to_string(),
            title: String::new(),
            format: String::new(),
            kind,
        }
    }

    /// The rule the whole design rests on: after the kind asked for, take the
    /// nearest on `Forced ⊂ Full ⊂ SDH`.
    #[test]
    fn each_kind_falls_back_to_the_nearest() {
        let ladder = |setting: &str| match Wanted::parse(setting) {
            Wanted::Ladder(rungs) => rungs,
            Wanted::None => Vec::new(),
        };
        assert_eq!(ladder("none"), Vec::new());
        // Forced Only never accepts anything else: everything else adds the
        // dialogue somebody chose to be spared.
        assert_eq!(ladder("forced_only"), vec![Some(Kind::Forced)]);
        assert_eq!(
            ladder("forced"),
            vec![Some(Kind::Forced), None, Some(Kind::Sdh)]
        );
        // From full, SDH and forced are each one step away and SDH wins,
        // because it keeps the dialogue and forced does not.
        assert_eq!(
            ladder("full"),
            vec![None, Some(Kind::Sdh), Some(Kind::Forced)]
        );
        assert_eq!(
            ladder("sdh"),
            vec![Some(Kind::Sdh), None, Some(Kind::Forced)]
        );
    }

    /// Language outside, kind inside: everything acceptable is tried in the
    /// first language before the second is considered at all.
    #[test]
    fn the_first_language_is_exhausted_before_the_second() {
        let options = vec![
            track(0, "en", Some(Kind::Sdh)),
            track(1, "ru", Some(Kind::Forced)),
            track(2, "ru", None),
        ];
        // Wanting full in English: no full English, but SDH English is one
        // step away and in the right language, so it beats anything Russian.
        assert_eq!(
            automatic(
                &Wanted::parse("full"),
                "first",
                &options,
                Some("en"),
                Some("ru")
            ),
            Some(SubtitleChoice::Embedded(0))
        );
    }

    /// The reported case, end to end: no forced track in either language, and
    /// the two settings that differ over what to do about it.
    #[test]
    fn forced_only_shows_nothing_where_prefer_forced_falls_back() {
        let options = vec![track(0, "en", None), track(1, "ru", None)];
        assert_eq!(
            automatic(
                &Wanted::parse("forced_only"),
                "first",
                &options,
                Some("en"),
                Some("ru")
            ),
            None
        );
        assert_eq!(
            automatic(
                &Wanted::parse("forced"),
                "first",
                &options,
                Some("en"),
                Some("ru")
            ),
            Some(SubtitleChoice::Embedded(0))
        );
    }

    /// Commentary is never reached by any ladder. It is one press away in the
    /// chooser, and nobody wants it selected on every film.
    #[test]
    fn commentary_is_never_chosen_automatically() {
        let options = vec![track(0, "en", Some(Kind::Commentary))];
        for setting in KINDS {
            assert_eq!(
                automatic(&Wanted::parse(setting), "first", &options, Some("en"), None),
                None,
                "{setting} chose the commentary track"
            );
        }
    }
}
