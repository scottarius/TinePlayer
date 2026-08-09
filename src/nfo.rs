//! Reads what a media library already knows about a video, from the `.nfo`
//! file sitting beside it.
//!
//! The format is Kodi's, extended by Jellyfin and Emby, and written by them and
//! by Radarr and Sonarr. Plex is the odd one out: it keeps its metadata in its
//! own database and writes no sidecar at all. There is no specification, only a
//! schema everyone implements approximately, so everything here is optional and
//! a file that cannot be read is the same as a file that is not there.
//!
//! What is worth having is not the plot summary. `<fileinfo><streamdetails>`
//! records a language and a forced flag per stream, and GStreamer exposes no
//! forced flag at all - so for the files that have one, this answers a question
//! the pipeline cannot. See [`crate::subtitles::Subtitle::is_forced`] for what
//! is otherwise the only signal: the words in the track's title.

use std::path::{Path, PathBuf};

/// One audio or subtitle stream, as the sidecar describes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stream {
    /// ISO code as written, lowercased. Empty when the file does not say.
    pub language: String,
    /// Carries only the lines a viewer who understands the dialogue still
    /// needs: signs, and speech in another language.
    pub forced: bool,
    /// The container's default flag, which is not the same as forced and is
    /// not used to choose anything. Read so that it is visible when debugging
    /// a file whose flags disagree with its titles.
    pub default: bool,
}

/// What the sidecar beside a video claims about its streams.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sidecar {
    pub audio: Vec<Stream>,
    pub subtitles: Vec<Stream>,
}

impl Sidecar {
    /// Fills in what the pipeline could not say about a file's streams.
    ///
    /// Only ever adds. A language the container states wins, because it
    /// describes the file in hand rather than whatever the scraper matched,
    /// and a forced flag is turned on but never off - the sidecar knowing
    /// nothing about a track is not the same as it saying no.
    ///
    /// **Guarded on the counts agreeing.** Nothing links a sidecar's streams to
    /// the file's beyond their order, and an `.nfo` can perfectly well describe
    /// a different release from the video it sits beside - a re-encode with
    /// fewer tracks, or a folder where the file was replaced and the sidecar
    /// was not. Different counts mean the two are not about the same thing, and
    /// the sidecar is then dropped rather than lined up hopefully.
    pub fn apply(&self, media: &mut crate::probe::Media) {
        if self.audio.len() == media.audio.len() {
            for (track, known) in media.audio.iter_mut().zip(&self.audio) {
                fill_language(&mut track.language, &known.language);
            }
        }
        if self.subtitles.len() == media.subtitles.len() {
            for (track, known) in media.subtitles.iter_mut().zip(&self.subtitles) {
                fill_language(&mut track.language, &known.language);
                track.forced |= known.forced;
            }
        }
    }
}

/// Whether a language field says anything. Containers spell "no idea" several
/// ways, and an MP4 with no language atom at all comes back empty.
fn unstated(language: &str) -> bool {
    let language = language.trim();
    language.is_empty()
        || language.eq_ignore_ascii_case("und")
        || language.eq_ignore_ascii_case("unknown")
}

fn fill_language(into: &mut String, from: &str) {
    if unstated(into) && !unstated(from) {
        *into = from.to_string();
    }
}

/// Finds the sidecar for a video, in the three layouts that occur in the wild.
///
/// - `<video>.nfo`, which is what a folder holding more than one film needs,
///   and what Sonarr writes beside every episode.
/// - `movie.nfo`, the folder-per-film layout.
/// - `tvshow.nfo` is deliberately *not* read here: it describes the series, and
///   its stream details would belong to no particular episode.
pub fn beside(video: &Path) -> Option<PathBuf> {
    let folder = video.parent()?;
    let named = video.with_extension("nfo");
    if named.is_file() {
        return Some(named);
    }
    let movie = folder.join("movie.nfo");
    movie.is_file().then_some(movie)
}

/// Reads the sidecar for a video, if there is one worth reading.
pub fn read(video: &Path) -> Option<Sidecar> {
    let text = std::fs::read_to_string(beside(video)?).ok()?;
    let sidecar = parse(&text);
    (!sidecar.audio.is_empty() || !sidecar.subtitles.is_empty()).then_some(sidecar)
}

/// Pulls the stream details out of an `.nfo`.
///
/// Deliberately not a general XML parser: the whole of what is wanted is a few
/// leaf elements inside one known block, and the alternative is a dependency
/// for a file that is optional in the first place. What that costs is handled
/// below - comments and CDATA are stripped first, entities are decoded - and
/// anything unrecognised is skipped rather than guessed at.
pub fn parse(text: &str) -> Sidecar {
    let text = strip_islands(text);
    let Some(details) = between(&text, "<streamdetails>", "</streamdetails>") else {
        return Sidecar::default();
    };
    Sidecar {
        audio: blocks(details, "audio").iter().map(|b| stream(b)).collect(),
        subtitles: blocks(details, "subtitle")
            .iter()
            .map(|b| stream(b))
            .collect(),
    }
}

/// Removes the parts that may hold anything at all, so that a `<plot>` full of
/// angle brackets cannot be mistaken for markup.
fn strip_islands(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let comment = rest.find("<!--");
        let cdata = rest.find("<![CDATA[");
        let (start, end_tag) = match (comment, cdata) {
            (Some(c), Some(d)) if c < d => (c, "-->"),
            (Some(_), Some(d)) => (d, "]]>"),
            (Some(c), None) => (c, "-->"),
            (None, Some(d)) => (d, "]]>"),
            (None, None) => break,
        };
        out.push_str(&rest[..start]);
        match rest[start..].find(end_tag) {
            Some(end) => rest = &rest[start + end + end_tag.len()..],
            // Unterminated: everything after it is unusable, so stop here
            // rather than treat the remainder as markup.
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

/// Every `<name>…</name>` block, which is how the streams of one kind are
/// listed. An empty `<name />` is skipped: it says nothing.
fn blocks<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(&after[..end]);
        rest = &after[end + close.len()..];
    }
    found
}

fn stream(block: &str) -> Stream {
    Stream {
        language: value(block, "language").unwrap_or_default().to_lowercase(),
        forced: flag(block, "forced"),
        default: flag(block, "default"),
    }
}

fn value(block: &str, name: &str) -> Option<String> {
    between(block, &format!("<{name}>"), &format!("</{name}>")).map(|raw| decode(raw.trim()))
}

fn flag(block: &str, name: &str) -> bool {
    value(block, name).is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// The five entities XML defines. Numeric ones are left alone: they do not
/// appear in a language code or a boolean, which is all that is read here.
fn decode(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // Last, or an escaped ampersand becomes the start of another entity.
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<movie>
  <plot>A film with <angle> brackets &amp; an ampersand in it.</plot>
  <!-- a comment mentioning <streamdetails> to be ignored -->
  <title>Example</title>
  <fileinfo>
    <streamdetails>
      <video><codec>h264</codec><width>1920</width></video>
      <audio>
        <codec>ac3</codec>
        <language>eng</language>
        <channels>6</channels>
        <default>True</default>
        <forced>False</forced>
      </audio>
      <audio>
        <codec>aac</codec>
        <language>Rus</language>
        <default>False</default>
        <forced>False</forced>
      </audio>
      <subtitle>
        <language>eng</language>
        <default>False</default>
        <forced>True</forced>
      </subtitle>
    </streamdetails>
  </fileinfo>
</movie>"#;

    #[test]
    fn reads_streams_in_order() {
        let sidecar = parse(REAL);
        assert_eq!(sidecar.audio.len(), 2);
        assert_eq!(sidecar.audio[0].language, "eng");
        assert!(sidecar.audio[0].default);
        // Case is not consistent between writers, so it is normalised.
        assert_eq!(sidecar.audio[1].language, "rus");
        assert_eq!(sidecar.subtitles.len(), 1);
        assert!(sidecar.subtitles[0].forced);
        assert!(!sidecar.subtitles[0].default);
    }

    /// The whole point of the file: a forced flag the pipeline cannot see.
    #[test]
    fn forced_survives_either_spelling() {
        assert!(
            parse(&REAL.replace("<forced>True</forced>", "<forced>1</forced>")).subtitles[0].forced
        );
        assert!(
            !parse(&REAL.replace("<forced>True</forced>", "<forced>0</forced>")).subtitles[0]
                .forced
        );
    }

    /// A plot mentioning markup must not be read as markup, and a comment
    /// naming the block must not be mistaken for the block.
    #[test]
    fn text_and_comments_cannot_pose_as_markup() {
        let sidecar = parse(REAL);
        assert_eq!(sidecar.audio.len(), 2);
        assert_eq!(
            parse("<plot>&lt;streamdetails&gt;&lt;audio&gt;</plot>"),
            Sidecar::default()
        );
    }

    #[test]
    fn nothing_at_all_is_not_an_error() {
        assert_eq!(parse(""), Sidecar::default());
        assert_eq!(
            parse("<movie><title>No streams</title></movie>"),
            Sidecar::default()
        );
        assert_eq!(parse("<streamdetails><audio>"), Sidecar::default());
    }

    use crate::probe::{AudioTrack, Media, SubtitleTrack};

    fn media(audio: &[&str], subtitles: &[&str]) -> Media {
        Media {
            audio: audio
                .iter()
                .enumerate()
                .map(|(index, language)| AudioTrack {
                    index: index as u32,
                    codec: "aac".to_string(),
                    channels: 2,
                    language: (*language).to_string(),
                    title: String::new(),
                })
                .collect(),
            subtitles: subtitles
                .iter()
                .enumerate()
                .map(|(index, language)| SubtitleTrack {
                    index: index as u32,
                    language: (*language).to_string(),
                    title: String::new(),
                    forced: false,
                })
                .collect(),
            duration_ns: 0,
        }
    }

    #[test]
    fn fills_only_what_the_file_did_not_say() {
        let mut m = media(&["", "eng"], &["und"]);
        parse(REAL).apply(&mut m);
        // An MP4 that carries no language atom gets one.
        assert_eq!(m.audio[0].language, "eng");
        // A stated language wins: it describes the file in hand.
        assert_eq!(m.audio[1].language, "eng");
        assert_eq!(m.subtitles[0].language, "eng");
        assert!(m.subtitles[0].forced);
    }

    /// The guard that keeps a sidecar for another release from being lined up
    /// against this one.
    #[test]
    fn a_different_shape_is_ignored_entirely() {
        let mut m = media(&[""], &["und", "und"]);
        parse(REAL).apply(&mut m);
        assert_eq!(m.audio[0].language, "");
        assert!(!m.subtitles[0].forced);
        assert!(!m.subtitles[1].forced);
    }

    /// Forced is only ever turned on: a sidecar that says nothing about a
    /// track must not overrule a title that does.
    #[test]
    fn forced_is_never_turned_off() {
        let mut m = media(&["eng", "eng"], &["eng"]);
        m.subtitles[0].forced = true;
        parse(&REAL.replace("<forced>True</forced>", "<forced>False</forced>")).apply(&mut m);
        assert!(m.subtitles[0].forced);
    }

    #[test]
    fn entities_are_decoded_once() {
        assert_eq!(decode("a &amp;lt; b"), "a &lt; b");
        assert_eq!(decode("&lt;tag&gt; &quot;x&quot;"), "<tag> \"x\"");
    }
}
