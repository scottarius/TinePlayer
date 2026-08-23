//! Which soundtrack an output plays, and how that choice is arrived at.
//!
//! The counterpart to [`crate::subtitles`], and deliberately the same shape:
//! one list of what is on offer, one function that chooses from it by
//! preference, and one that resolves what somebody typed. Both the settings
//! and the command line go through these, so a preference and a flag cannot
//! disagree about what "en" means.
//!
//! **It was two implementations until now.** `App::apply_media` picked a track
//! from the language preferences with closures of its own, `probe::resolve_audio`
//! did its own matching for `--primary`, and neither looked at the soundtracks
//! beside the video - which the chooser has offered all along. So a file the
//! menu listed could not be asked for by language, by `ad`, or at all from the
//! command line, while the subtitle beside it could be asked for every way.

use std::path::{Path, PathBuf};

use crate::beside::AudioFile;
use crate::label::Naming;
use crate::probe::AudioTrack;

/// One entry in the soundtrack list: a track inside the video, or a file
/// sitting beside it.
#[derive(Debug, Clone, PartialEq)]
pub enum Audio {
    Track(AudioTrack),
    File(AudioFile),
}

/// What a choice resolved to, in the terms the outputs hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioChoice {
    /// No audio on this output.
    Silent,
    /// A track inside the video, by the stream index the pipeline wants.
    Track(u32),
    /// A separate soundtrack beside the video.
    File(PathBuf),
}

impl Audio {
    /// The language this entry states, or empty where it states none.
    ///
    /// A track carries a tag; a file beside the video carries whatever the
    /// convention left at the front of its name. Both are matched the same way,
    /// by [`crate::languages::matches`], so `en` finds `eng` inside a video and
    /// `Film.en.mka` beside it.
    pub fn language(&self) -> &str {
        match self {
            Audio::Track(track) => &track.language,
            Audio::File(file) => file.language(),
        }
    }

    /// Whether this is narration rather than an ordinary soundtrack.
    pub fn is_described(&self) -> bool {
        match self {
            Audio::Track(track) => track.is_described(),
            Audio::File(file) => file.is_described(),
        }
    }

    pub fn choice(&self) -> AudioChoice {
        match self {
            Audio::Track(track) => AudioChoice::Track(track.index),
            Audio::File(file) => AudioChoice::File(file.path.clone()),
        }
    }

    /// How the row reads. `position` numbers the fallback name a track with no
    /// title of its own gets, counted from one as the printed list counts.
    pub fn label(&self, naming: Naming, position: usize) -> String {
        match self {
            Audio::Track(track) => crate::label::line(
                &crate::label::Parts {
                    language: &track.language,
                    technical: format!("{} {}ch", track.codec, track.channels),
                    kind: track.kind(),
                    title: &track.title,
                },
                naming,
                &format!("Track {position}"),
            ),
            Audio::File(file) => file.named(naming),
        }
    }
}

/// Everything an output could be put onto, in the order every list shows it:
/// the tracks inside the video, then the soundtracks beside it.
///
/// The order is the whole of the numbering. `--list-tracks` prints it, the
/// chooser draws it, and `--primary 9` counts through it - so a number means
/// the same thing wherever it is read.
pub fn options(video: Option<&Path>, tracks: &[AudioTrack]) -> Vec<Audio> {
    let mut options: Vec<Audio> = tracks.iter().cloned().map(Audio::Track).collect();
    options.extend(
        video
            .map(crate::beside::audio)
            .unwrap_or_default()
            .into_iter()
            .map(Audio::File),
    );
    options
}

/// What ordinary selection is allowed to pick from: everything except the
/// described entries, which are only ever chosen by asking for them.
///
/// Without this, a video whose first English track happens to be the described
/// one would hand narration to somebody who never wanted it.
///
/// Unless description is all there is. A video with nothing else would
/// otherwise start silent, which reads as the player being broken rather than
/// as a preference being honored.
pub fn ordinary(options: &[Audio]) -> Vec<&Audio> {
    let plain: Vec<&Audio> = options
        .iter()
        .filter(|entry| !entry.is_described())
        .collect();
    match plain.is_empty() {
        true => options.iter().collect(),
        false => plain,
    }
}

/// The choice the preferences make, or `None` where they make none and the
/// caller's own fallback should decide.
///
/// The single place a preference is read, whether it came from `config.yaml`
/// or from a language code on the command line.
pub fn automatic(
    options: &[Audio],
    preferred: Option<&str>,
    described: bool,
) -> Option<AudioChoice> {
    described_entry(options, described, preferred)
        .or_else(|| by_language(options, preferred))
        .map(Audio::choice)
}

/// The first entry in the preferred language, where one was named.
fn by_language<'a>(options: &'a [Audio], preferred: Option<&str>) -> Option<&'a Audio> {
    let code = preferred?;
    ordinary(options)
        .into_iter()
        .find(|entry| crate::languages::matches(entry.language(), code))
}

/// Narration for an output that asked for it. Not finding any is not a
/// failure - most videos have none - so it falls back to the ordinary choice
/// rather than leaving the output silent.
///
/// A named language is a hard requirement, not a preference to relax:
/// description narrated in a language you do not speak is worse than no
/// description at all, so the fallback is the right language undescribed
/// rather than the wrong language described.
fn described_entry<'a>(
    options: &'a [Audio],
    want: bool,
    preferred: Option<&str>,
) -> Option<&'a Audio> {
    if !want {
        return None;
    }
    let describes = |entry: &&Audio| entry.is_described();
    let Some(code) = preferred else {
        return options.iter().find(describes);
    };
    options
        .iter()
        .find(|entry| describes(entry) && crate::languages::matches(entry.language(), code))
        // Then one whose language is not stated. Unknown is not the same as
        // wrong: an entry tagged for another language is rejected, but plenty
        // of description carries no tag at all - the tool most people use to
        // add one sets a title and no language, and a file called `AD.mp3` is
        // named after nothing - and refusing those would mean finding nothing
        // in the commonest case of all.
        .or_else(|| {
            options
                .iter()
                .find(|entry| describes(entry) && !crate::languages::known(entry.language()))
        })
}

/// Reads what somebody typed at `--primary` or `--secondary`.
///
/// - `3` - the third entry `--list-tracks` prints, inside the video or beside it
/// - `en` - the first entry in that language, wherever it sits
/// - `ad` - the first described entry
/// - `en:ad` - the first described entry in that language
/// - `0` or `none` - no audio on this output
///
/// A plain language code will not select a described entry, matching what the
/// preference does: description is only ever chosen by asking for it.
pub fn resolve(spec: &str, options: &[Audio]) -> Result<AudioChoice, String> {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("none") {
        return Ok(AudioChoice::Silent);
    }

    if let Ok(number) = spec.parse::<usize>() {
        // Zero means no audio on this output, however it is spelled - "0" and
        // "00" are the same request. Checked after parsing rather than against
        // the text, because `number - 1` below underflows otherwise: harmless
        // in a release build, where it wraps and finds nothing, and a panic in
        // a debug one.
        if number == 0 {
            return Ok(AudioChoice::Silent);
        }
        return options.get(number - 1).map(Audio::choice).ok_or_else(|| {
            format!(
                "There is no audio {number}. This video offers {}.",
                options.len()
            )
        });
    }

    let (code, described) = match spec.split_once(':') {
        Some((code, kind)) if kind.eq_ignore_ascii_case("ad") => (Some(code), true),
        Some((_, kind)) => {
            return Err(format!(
                "Don't know what \"{kind}\" means. Use \"ad\" after the colon, as in \"en:ad\"."
            ));
        }
        None if spec.eq_ignore_ascii_case("ad") => (None, true),
        None => (Some(spec), false),
    };

    // The same path the preferences take, so `--primary en` and
    // `primary_language: en` cannot answer differently.
    automatic(options, code, described).ok_or_else(|| {
        let what = match described {
            true => "described audio",
            false => "audio",
        };
        match code {
            Some(code) => format!("No {what} in {code}."),
            None => format!("No {what} for this video."),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(index: u32, language: &str, title: &str) -> Audio {
        Audio::Track(AudioTrack {
            index,
            codec: "AC-3".to_string(),
            channels: 2,
            language: language.to_string(),
            title: title.to_string(),
            described: None,
            commentary: None,
        })
    }

    fn beside(tag: Option<&str>, name: &str) -> Audio {
        Audio::File(AudioFile {
            path: PathBuf::from(format!("D:/films/{name}")),
            tag: tag.map(str::to_string),
            name: name.to_string(),
        })
    }

    /// Four inside the video, two beside it, in the order every list shows.
    fn offered() -> Vec<Audio> {
        vec![
            track(0, "en", "English"),
            track(1, "de", "German"),
            track(2, "en", "Descriptive"),
            track(3, "de", "Audio Description"),
            beside(Some("fr"), "film.fr.m4a"),
            beside(None, "AD.mp3"),
        ]
    }

    #[test]
    fn a_number_counts_through_both_lists() {
        let o = offered();
        assert_eq!(resolve("2", &o), Ok(AudioChoice::Track(1)));
        assert_eq!(
            resolve("5", &o),
            Ok(AudioChoice::File("D:/films/film.fr.m4a".into()))
        );
        assert_eq!(
            resolve("6", &o),
            Ok(AudioChoice::File("D:/films/AD.mp3".into()))
        );
    }

    #[test]
    fn every_spelling_of_zero_means_silence() {
        for spec in ["0", "00", "000", " 0 ", "none", "NONE", "None"] {
            assert_eq!(
                resolve(spec, &offered()),
                Ok(AudioChoice::Silent),
                "{spec:?}"
            );
        }
    }

    /// The whole point of the change: a language reaches a file beside the
    /// video, exactly as it reaches a subtitle file beside it.
    #[test]
    fn a_language_reaches_a_file_beside_the_video() {
        assert_eq!(
            resolve("fr", &offered()),
            Ok(AudioChoice::File("D:/films/film.fr.m4a".into()))
        );
    }

    /// And an embedded track still wins where there is one, because the
    /// tracks inside the video come first in the list.
    #[test]
    fn a_track_inside_the_video_is_preferred_to_a_file_beside_it() {
        let mut o = offered();
        o.push(beside(Some("en"), "film.en.mka"));
        assert_eq!(resolve("en", &o), Ok(AudioChoice::Track(0)));
    }

    #[test]
    fn a_language_alone_never_picks_description() {
        // German track 4 is described and track 2 is not, so the plain code
        // has to reach past the described one.
        assert_eq!(resolve("de", &offered()), Ok(AudioChoice::Track(1)));
    }

    #[test]
    fn ad_picks_description_inside_or_beside() {
        let o = offered();
        assert_eq!(resolve("ad", &o), Ok(AudioChoice::Track(2)));
        assert_eq!(resolve("de:ad", &o), Ok(AudioChoice::Track(3)));
        assert_eq!(resolve("en:ad", &o), Ok(AudioChoice::Track(2)));
        // Nothing described inside the video in French, so the file named
        // after nothing answers: its language is unstated, which is not the
        // same as wrong.
        let beside_only = vec![track(0, "fr", "French"), beside(None, "AD.mp3")];
        assert_eq!(
            resolve("fr:ad", &beside_only),
            Ok(AudioChoice::File("D:/films/AD.mp3".into()))
        );
    }

    /// The preference and the flag are the same call, so they cannot disagree.
    #[test]
    fn the_preference_and_the_flag_agree() {
        let o = offered();
        for (spec, preferred, described) in [
            ("en", Some("en"), false),
            ("de", Some("de"), false),
            ("fr", Some("fr"), false),
            ("ad", None, true),
            ("de:ad", Some("de"), true),
        ] {
            assert_eq!(
                resolve(spec, &o).ok(),
                automatic(&o, preferred, described),
                "for {spec:?}"
            );
        }
    }

    /// Description is only ever chosen by asking, so it is kept out of the
    /// pool ordinary selection draws from - unless it is all there is.
    #[test]
    fn description_is_kept_out_of_ordinary_selection() {
        let o = vec![track(0, "en", "Descriptive"), track(1, "en", "English")];
        assert_eq!(
            automatic(&o, Some("en"), false),
            Some(AudioChoice::Track(1))
        );

        let described_only = vec![track(0, "en", "Descriptive")];
        assert_eq!(
            automatic(&described_only, Some("en"), false),
            Some(AudioChoice::Track(0)),
            "a video with nothing else must not start silent"
        );
    }

    #[test]
    fn says_what_it_could_not_find() {
        let o = offered();
        assert!(resolve("9", &o).is_err());
        assert!(resolve("ja", &o).is_err());
        assert!(resolve("en:hi", &o).is_err());
    }
}
