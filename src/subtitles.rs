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
        eprintln!(
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
    pub fn is_forced(&self) -> bool {
        if let Subtitle::Embedded { kind, .. } = self {
            // Already answered, on the full ladder, where the track was in
            // hand: see `SubtitleTrack::kind`.
            return *kind == Some(crate::label::Kind::Forced);
        }
        self.label().to_lowercase().contains("forced")
    }
}

/// The automatic choices offered above the language list, as stored and as
/// shown. Following an output is usually better than naming a language: it
/// tracks whatever is actually being heard, file by file.
/// **Stored values only.** These go into `config.yaml` and are matched by
/// `--subtitle`, so they are not language and never change. What each one
/// reads as on screen is [`mode_label`], which is - the two used to be one
/// table of pairs, and a translated label in it would have been written to
/// the config file as the setting.
pub const MODES: [&str; 5] = [
    "none",
    "primary_forced",
    "secondary_forced",
    "primary",
    "secondary",
];

/// How one of [`MODES`] reads on screen, or `None` if that is not a mode.
pub fn mode_label(value: &str) -> Option<Cow<'static, str>> {
    Some(match value {
        // A third sense of "None" in this interface, after an output device
        // and a list of languages. English spells all three the same way and
        // several languages do not, which is what the context is for.
        "none" => trc!("subtitle preference", "None"),
        "primary_forced" => tr!("Forced (Prefer First Output Language)"),
        "secondary_forced" => tr!("Forced (Prefer Second Output Language)"),
        "primary" => tr!("First Output Language"),
        "secondary" => tr!("Second Output Language"),
        _ => return None,
    })
}

/// How the setting reads on screen: one of [`MODES`] or a language name.
///
/// Not `languages::display_name` alone, which hands back whatever it was given
/// when it recognizes nothing - and it recognizes none of the modes, so the
/// settings list showed `primary_forced` rather than what it means.
pub fn describe(setting: Option<&str>) -> String {
    let setting = setting.unwrap_or(DEFAULT_MODE);
    mode_label(setting)
        .map(Cow::into_owned)
        .unwrap_or_else(|| crate::languages::display_name(setting))
}

/// Forced subtitles for whatever the room is hearing. A dub usually speaks
/// every sign and foreign line aloud, so the only gap worth filling is in the
/// original language - which the primary output is most likely to be carrying.
pub const DEFAULT_MODE: &str = "primary_forced";

/// How a subtitle is chosen for a video with no remembered choice.
#[derive(Clone, Debug, PartialEq)]
pub enum Auto {
    /// Show none.
    None,
    /// Follow one of the outputs: the language being heard there.
    Output { secondary: bool, forced: bool },
    /// A language of its own, whatever is being heard.
    Language(String),
}

impl Auto {
    /// Reads the setting. Anything unrecognized is treated as a language code,
    /// which is what the rest of the list holds.
    pub fn parse(setting: &str) -> Self {
        match setting.trim().to_lowercase().as_str() {
            "" | "none" => Self::None,
            "primary_forced" => Self::Output {
                secondary: false,
                forced: true,
            },
            "primary" => Self::Output {
                secondary: false,
                forced: false,
            },
            "secondary_forced" => Self::Output {
                secondary: true,
                forced: true,
            },
            "secondary" => Self::Output {
                secondary: true,
                forced: false,
            },
            _ => Self::Language(setting.trim().to_string()),
        }
    }
}

/// The subtitle to show for a video nobody has chosen one for.
///
/// Forced and unforced are kept strictly apart. Asking for forced subtitles and
/// getting a full translation would bury a film someone is listening to in
/// their own language; asking for a full translation and getting only the signs
/// would look like the subtitles were broken. Neither substitutes for the
/// other, so no match means none.
///
/// The forced modes prefer one output but will take the other: forced
/// subtitles translate signs and foreign lines, which are worth having in
/// either language on offer. The full modes do not fall back, because a full
/// translation in the wrong language is a worse answer than none.
/// Whether changing one output's soundtrack can change what this preference
/// answers, and so whether it is worth asking again.
///
/// `secondary_output` names which output moved. The three cases:
///
/// - **None, and a fixed language**: not about the outputs at all. A
///   preference for Russian subtitles says the same thing whatever anybody is
///   listening to.
/// - **The full modes** name one output and never fall back, because a whole
///   translation in the wrong language is worse than none - so only the output
///   they name can change the answer.
/// - **The forced modes** prefer one output but will take the other, since
///   forced subtitles translate signs and are worth having in either language
///   on offer. Either output moving can therefore change the answer, which is
///   why this cannot be decided from the role alone.
pub fn follows_output(mode: &Auto, secondary_output: bool) -> bool {
    match mode {
        Auto::None | Auto::Language(_) => false,
        Auto::Output { secondary, forced } => *forced || *secondary == secondary_output,
    }
}

pub fn automatic(
    mode: &Auto,
    options: &[Subtitle],
    primary_language: Option<&str>,
    secondary_language: Option<&str>,
) -> Option<SubtitleChoice> {
    let (languages, forced): (Vec<&str>, bool) = match mode {
        Auto::None => return None,
        Auto::Language(code) => (vec![code.as_str()], false),
        Auto::Output { secondary, forced } => {
            let (first, second) = if *secondary {
                (secondary_language, primary_language)
            } else {
                (primary_language, secondary_language)
            };
            let mut order = Vec::new();
            order.extend(first);
            if *forced {
                order.extend(second);
            }
            (order, *forced)
        }
    };

    languages.into_iter().find_map(|language| {
        options
            .iter()
            .find(|option| {
                option.is_forced() == forced && crate::languages::matches(option.label(), language)
            })
            .map(Subtitle::choice)
    })
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

    // Anything the setting accepts is accepted here too, and means the same:
    // one run set that way, rather than changing the setting itself.
    if MODES.iter().any(|value| value.eq_ignore_ascii_case(spec)) {
        return Ok(automatic(
            &Auto::parse(spec),
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
        let forced = Auto::parse("primary_forced");
        assert_eq!(
            automatic(&forced, &o, Some("ru"), Some("en")),
            Some(SubtitleChoice::Embedded(0))
        );
        let full = Auto::parse("secondary");
        assert_eq!(
            automatic(&full, &o, Some("ru"), Some("en")),
            Some(SubtitleChoice::Embedded(2))
        );
    }

    #[test]
    fn forced_and_full_never_substitute_for_each_other() {
        let o = options();
        // German is not present at all.
        assert_eq!(
            automatic(&Auto::parse("primary"), &o, Some("de"), None),
            None
        );
        // Only a full English track exists besides the forced file, so asking
        // for forced English gets the file, and asking for full gets the track.
        assert_eq!(
            automatic(&Auto::parse("primary_forced"), &o, Some("en"), None),
            Some(SubtitleChoice::External("f.en.forced.srt".to_string()))
        );
        assert_eq!(
            automatic(&Auto::parse("primary"), &o, Some("en"), None),
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
            automatic(&Auto::parse("primary_forced"), &only_full, Some("ru"), None),
            None
        );
    }

    #[test]
    fn none_and_a_named_language() {
        let o = options();
        assert_eq!(automatic(&Auto::parse("none"), &o, Some("ru"), None), None);
        assert_eq!(
            automatic(&Auto::parse("en"), &o, Some("ru"), None),
            Some(SubtitleChoice::Embedded(2))
        );
    }

    #[test]
    fn the_default_follows_the_primary_output_forced() {
        assert_eq!(
            Auto::parse(DEFAULT_MODE),
            Auto::Output {
                secondary: false,
                forced: true
            }
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
    fn the_argument_accepts_what_the_setting_accepts() {
        let o = options();
        assert_eq!(
            resolve("primary_forced", &o, Some("ru"), Some("en")),
            Ok(Some(SubtitleChoice::Embedded(0)))
        );
        assert_eq!(
            resolve("secondary", &o, Some("ru"), Some("en")),
            Ok(Some(SubtitleChoice::Embedded(2)))
        );
        // English has no forced track here, so preferring the secondary
        // output falls back to the forced Russian one.
        assert_eq!(
            resolve("secondary_forced", &o, Some("ru"), Some("en")),
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
            automatic(&Auto::parse("primary_forced"), &o, Some("ru"), Some("en")),
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
            automatic(&Auto::parse("primary_forced"), &o, Some("ru"), Some("en")),
            Some(SubtitleChoice::Embedded(1))
        );
        assert_eq!(
            automatic(&Auto::parse("secondary_forced"), &o, Some("ru"), Some("en")),
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
            automatic(&Auto::parse("primary"), &o, Some("ru"), Some("en")),
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
            automatic(&Auto::parse("primary_forced"), &o, Some("ru"), Some("en")),
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
    fn a_preference_about_nothing_follows_nothing() {
        for mode in [Auto::parse("none"), Auto::parse("ru")] {
            assert!(!follows_output(&mode, false));
            assert!(!follows_output(&mode, true));
        }
    }

    /// A whole translation never falls back to the other output, so only the
    /// output it names can change its answer.
    #[test]
    fn a_full_mode_follows_only_its_own_output() {
        let primary = Auto::parse("primary");
        assert!(follows_output(&primary, false));
        assert!(!follows_output(&primary, true));

        let secondary = Auto::parse("secondary");
        assert!(!follows_output(&secondary, false));
        assert!(follows_output(&secondary, true));
    }

    /// Forced subtitles are worth having in either language on offer, so the
    /// forced modes take the other output when the preferred one has nothing -
    /// which means either output moving can change the answer.
    #[test]
    fn a_forced_mode_follows_both_outputs() {
        for mode in [
            Auto::parse("primary_forced"),
            Auto::parse("secondary_forced"),
        ] {
            assert!(follows_output(&mode, false));
            assert!(follows_output(&mode, true));
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
