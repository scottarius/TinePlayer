//! Choosing subtitles, from inside the file or from alongside it.
//!
//! Kept apart from the audio settings deliberately: the subtitle language is
//! an independent choice, and may well be a third language rather than a copy
//! of either soundtrack.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::probe::SubtitleTrack;

/// Formats GStreamer can parse from a plain file. Blu-ray `.sup` and the
/// VOBSUB `.sub`/`.idx` pair are deliberately absent: both are bitmap
/// formats with no decoder in the shipped GStreamer.
pub const EXTENSIONS: [&str; 4] = ["srt", "ass", "ssa", "vtt"];

/// One entry in the subtitle chooser.
#[derive(Clone, Debug, PartialEq)]
pub enum Subtitle {
    Embedded {
        index: u32,
        label: String,
        /// What a sidecar said, where there was one. See
        /// [`Subtitle::is_forced`] for why the title is still read as well.
        flagged: bool,
    },
    External {
        name: String,
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
}

impl Subtitle {
    pub fn label(&self) -> &str {
        match self {
            Subtitle::Embedded { label, .. }
            | Subtitle::External { label, .. }
            | Subtitle::File { label, .. } => label,
        }
    }

    pub fn choice(&self) -> SubtitleChoice {
        match self {
            Subtitle::Embedded { index, .. } => SubtitleChoice::Embedded(*index),
            Subtitle::External { name, .. } => SubtitleChoice::External(name.clone()),
            Subtitle::File { path, .. } => SubtitleChoice::File(path.clone()),
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
}

/// Everything on offer for a video: what is inside it, then what sits beside
/// it on disk.
///
/// `video` is `None` for a remote source, which offers only what is embedded -
/// there is no folder to look in, and a media server hands its own subtitles
/// over inside the stream anyway.
pub fn options(video: Option<&Path>, embedded: &[SubtitleTrack]) -> Vec<Subtitle> {
    let mut options: Vec<Subtitle> = embedded
        .iter()
        .map(|track| Subtitle::Embedded {
            index: track.index,
            label: if track.title.is_empty() {
                track.language.clone()
            } else {
                format!("{} — {}", track.language, track.title)
            },
            flagged: track.forced,
        })
        .collect();
    if let Some(video) = video {
        options.extend(external(video));
    }
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
fn external(video: &Path) -> Vec<Subtitle> {
    let Some(directory) = video.parent() else {
        return Vec::new();
    };
    let Some(stem) = video.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut found: Vec<Subtitle> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_string_lossy().to_lowercase();
            if !EXTENSIONS.contains(&extension.as_str()) {
                return None;
            }

            // Compared without the extension, so that an upper-case one
            // doesn't defeat the trimming.
            let without_extension = path.file_stem()?.to_string_lossy().to_string();
            // A file named exactly after the video leaves nothing between the
            // two, and falls through to the generic label below.
            let label = without_extension
                .strip_prefix(&stem)?
                .trim_matches('.')
                .to_string();

            Some(Subtitle::External {
                name: path.file_name()?.to_string_lossy().to_string(),
                label: if label.is_empty() {
                    "External".to_string()
                } else {
                    label
                },
            })
        })
        .collect();

    found.sort_by(|a, b| a.label().cmp(b.label()));
    found
}

impl Subtitle {
    /// Whether this is a forced track: one carrying only the lines a viewer
    /// who understands the dialogue still needs, such as alien speech or
    /// signs.
    ///
    /// Read from the name, and from a sidecar when one says so.
    ///
    /// The name is the older signal and still the load-bearing one. Matroska
    /// has a forced flag, but GStreamer does not surface it, and rips
    /// routinely leave it unset while saying "Forced" in the title - the flag
    /// is false on every track of a well-tagged file we tested. The convention
    /// in the title and in subtitle file names is what actually carries the
    /// intent.
    ///
    /// A `.nfo` beside the video is the one place a real flag can be read
    /// from, so it is taken as well. Either saying yes is enough: a library
    /// that recorded the flag and a ripper who wrote it in the title are two
    /// independent ways of being told the same thing, and files exist with
    /// only one of them.
    pub fn is_forced(&self) -> bool {
        let flagged = matches!(self, Subtitle::Embedded { flagged: true, .. });
        flagged || self.label().to_lowercase().contains("forced")
    }
}

/// The automatic choices offered above the language list, as stored and as
/// shown. Following an output is usually better than naming a language: it
/// tracks whatever is actually being heard, file by file.
pub const MODES: [(&str, &str); 5] = [
    ("none", "None"),
    ("primary_forced", "Forced (Prefer First Output Language)"),
    ("secondary_forced", "Forced (Prefer Second Output Language)"),
    ("primary", "First Output Language"),
    ("secondary", "Second Output Language"),
];

/// How the setting reads on screen: one of [`MODES`] or a language name.
///
/// Not `languages::name_for` alone, which hands back whatever it was given
/// when it recognizes nothing - and it recognizes none of the modes, so the
/// settings list showed `primary_forced` rather than what it means.
pub fn describe(setting: Option<&str>) -> String {
    let setting = setting.unwrap_or(DEFAULT_MODE);
    MODES
        .iter()
        .find(|(value, _)| *value == setting)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| crate::languages::name_for(setting))
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
    if MODES
        .iter()
        .any(|(value, _)| value.eq_ignore_ascii_case(spec))
    {
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
        Subtitle::Embedded { .. } => false,
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
                flagged: false,
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
                flagged: false,
            },
            Subtitle::Embedded {
                index: 1,
                label: "ru - Full".to_string(),
                flagged: false,
            },
            Subtitle::Embedded {
                index: 2,
                label: "en - Full".to_string(),
                flagged: false,
            },
            Subtitle::External {
                name: "f.en.forced.srt".to_string(),
                label: "en.forced".to_string(),
            },
        ]
    }

    #[test]
    fn forced_is_read_from_the_name() {
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
            flagged: false,
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
                flagged: false,
            },
            Subtitle::Embedded {
                index: 1,
                label: "ru - Full".to_string(),
                flagged: false,
            },
            Subtitle::Embedded {
                index: 2,
                label: "en - Full".to_string(),
                flagged: false,
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
                flagged: false,
            },
            Subtitle::Embedded {
                index: 1,
                label: "en - Forced".to_string(),
                flagged: false,
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
                flagged: false,
            },
            Subtitle::Embedded {
                index: 1,
                label: "ru - Forced".to_string(),
                flagged: false,
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
            flagged: false,
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
            flagged: false,
        }];
        assert_eq!(
            automatic(&Auto::parse("primary_forced"), &o, Some("ru"), Some("en")),
            None
        );
    }
}
