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

/// What the sidecar beside a video claims, about the film and its streams
/// alike.
///
/// Every field is optional and empty means "not stated" throughout. Nothing
/// here is required for the video to play: a file with no sidecar produces the
/// default, and the media page is written to look deliberate with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sidecar {
    pub audio: Vec<Stream>,
    pub subtitles: Vec<Stream>,
    /// What the film is called, as opposed to what the file is called.
    ///
    /// An episode's sidecar puts the episode's own name here, under a
    /// different root element, and is read without this having to know which
    /// it got. What the show is called is [`Self::show`], kept apart because
    /// an episode is titled by what it is called and not by what it belongs to.
    pub title: String,
    /// What the show is called, from `<showtitle>`, which only an episode's
    /// sidecar carries. Empty for a film.
    ///
    /// Read but never used as the title. It names the series *under* the
    /// episode's own details, where somebody looking at "Ozymandias" may
    /// reasonably want to know which programme that is.
    pub show: String,
    pub year: Option<u32>,
    /// Which episode this is, where the sidecar says so: `<season>` and
    /// `<episode>`, which only an `<episodedetails>` file carries. Both or
    /// neither - a number without the other names nothing.
    pub episode: Option<(u32, u32)>,
    /// The day it first went out, from `<aired>`. Shown in place of the year
    /// for an episode, that being the date anybody would recognise it by.
    pub aired: String,
    /// The summary. `<plot>` where there is one, `<outline>` after it, which
    /// is the shorter form some scrapers write instead.
    pub plot: String,
    /// Minutes, as `<runtime>` states them. Only used when the container
    /// could not report a duration of its own.
    pub runtime_mins: Option<u32>,
    /// The certificate, reduced to the rating itself - see [`certificate`].
    pub mpaa: String,
    /// Out of ten, as every writer of this format scores it.
    pub rating: Option<f64>,
    pub genres: Vec<String>,
    /// Artwork the sidecar names outright. Absolute paths, and often paths on
    /// the machine that did the scraping rather than this one, so they are
    /// taken only when they resolve here - see [`crate::metadata`].
    pub poster: String,
    pub fanart: String,
}

impl Sidecar {
    /// Whether the file said anything worth keeping. A sidecar that parsed to
    /// nothing is the same as one that is not there.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
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
    (!sidecar.is_empty()).then_some(sidecar)
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

    // The stream block, where there is one. Its absence is ordinary: plenty of
    // scrapers write the film's details and never touch the file's.
    let (audio, subtitles) = match between(&text, "<streamdetails>", "</streamdetails>") {
        Some(details) => (
            blocks(details, "audio").iter().map(|b| stream(b)).collect(),
            blocks(details, "subtitle")
                .iter()
                .map(|b| stream(b))
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };

    // Everything below reads the first element of its name anywhere in the
    // document, which is what a schema with no specification allows. The risk
    // is an element of the same name nested somewhere else, and the two that
    // actually collide are handled where they arise: `<thumb>` is not read at
    // all, because an actor has one, and `<name>` is not read for the same
    // reason.
    let plot = {
        let plot = value(&text, "plot").unwrap_or_default();
        if plot.is_empty() {
            value(&text, "outline").unwrap_or_default()
        } else {
            plot
        }
    };

    Sidecar {
        audio,
        subtitles,
        title: value(&text, "title").unwrap_or_default(),
        show: value(&text, "showtitle").unwrap_or_default(),
        // `<year>` is the older field and `<premiered>` the one Jellyfin and
        // Radarr write; a full date reduces to the four digits in front of it.
        year: number(&text, "year")
            .or_else(|| value(&text, "premiered").and_then(|date| date.get(..4)?.parse().ok()))
            .filter(|year| (1870..=2200).contains(year)),
        // Both or neither. A file with one and not the other is telling us
        // something it does not know, and "S01E00" is worse than nothing.
        episode: number(&text, "season").zip(number(&text, "episode")),
        aired: value(&text, "aired").unwrap_or_default(),
        plot,
        runtime_mins: number(&text, "runtime"),
        mpaa: certificate(&value(&text, "mpaa").unwrap_or_default()),
        rating: rating(&text),
        // Repeated, unlike everything else here, and worth keeping in the
        // order the file lists them: a scraper puts the defining genre first.
        genres: blocks(&text, "genre")
            .iter()
            .map(|raw| decode(raw.trim()))
            .filter(|genre| !genre.is_empty())
            .collect(),
        // Only from `<art>`, which is the block that holds local paths. The
        // older `<thumb>` form usually carries a scraper's URL, and this
        // never fetches anything.
        poster: art(&text, "poster"),
        fanart: art(&text, "fanart"),
    }
}

/// A path out of the `<art>` block, which is what Jellyfin, Emby and the
/// *arrs write. Kodi's own `<thumb>` elements are deliberately skipped: they
/// hold remote URLs as often as paths, and an `<actor>` has one too.
fn art(text: &str, name: &str) -> String {
    let Some(block) = between(text, "<art>", "</art>") else {
        return String::new();
    };
    value(block, name).unwrap_or_default()
}

/// The score out of ten, from either shape the format has had.
///
/// The current one nests it - `<ratings><rating><value>7.2</value>` - and the
/// old one is a bare `<rating>7.2</rating>`. Read in that order, because a
/// file carrying the new block often keeps an empty old element beside it.
fn rating(text: &str) -> Option<f64> {
    let nested =
        between(text, "<ratings>", "</ratings>").and_then(|block| number_in(block, "value"));
    nested
        .or_else(|| number_in(text, "rating"))
        // Zero is what a scraper writes when nothing has been voted on, and a
        // film rated 0.0 on screen reads as a verdict rather than as silence.
        .filter(|score| *score > 0.0 && *score <= 10.0)
}

fn number_in(text: &str, name: &str) -> Option<f64> {
    value(text, name)?.parse().ok()
}

fn number(text: &str, name: &str) -> Option<u32> {
    // Written with a unit by some writers - "128 min" - so the digits in
    // front are taken rather than the whole field being rejected.
    let raw = value(text, name)?;
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Reduces a certificate to the part worth showing.
///
/// Writers disagree completely on this field. Kodi's own scrapers produce
/// "Rated PG-13 for sequences of sci-fi violence", the *arrs write a bare
/// "PG-13", and Jellyfin writes a country-prefixed "US:PG-13". All three mean
/// the same thing and the page has room for the short form.
fn certificate(raw: &str) -> String {
    let raw = raw.trim();
    // "US:PG-13" - drop the country, which is not what anyone is reading for.
    let raw = raw.rsplit(':').next().unwrap_or(raw).trim();
    // "Rated PG-13 for ..." - the rating is the token after "Rated", and the
    // justification after it is a sentence the facts line has no room for.
    let raw = raw.strip_prefix("Rated ").unwrap_or(raw).trim();
    raw.split_whitespace().next().unwrap_or("").to_string()
}

/// Neutralizes the parts that may hold anything at all, so that a `<plot>`
/// full of angle brackets cannot be mistaken for markup.
///
/// The two are not handled alike, and the difference matters now that the plot
/// is something the page shows. A comment is thrown away: it is not content,
/// and one mentioning an element must not be read as that element. A CDATA
/// section *is* content - it is how half the writers of this format carry a
/// summary with an ampersand in it - so its text is kept and escaped instead,
/// which leaves it safe to scan and returns it intact through [`decode`].
///
/// Dropping CDATA outright, which is what this did while only the stream block
/// was being read, would silently blank the plot on every file that used one.
fn strip_islands(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let comment = rest.find("<!--");
        let cdata = rest.find("<![CDATA[");
        let (start, end_tag, keep) = match (comment, cdata) {
            (Some(c), Some(d)) if c < d => (c, "-->", false),
            (Some(_), Some(d)) => (d, "]]>", true),
            (Some(c), None) => (c, "-->", false),
            (None, Some(d)) => (d, "]]>", true),
            (None, None) => break,
        };
        out.push_str(&rest[..start]);
        let opener = if keep { "<![CDATA[".len() } else { 0 };
        match rest[start..].find(end_tag) {
            Some(end) => {
                if keep {
                    out.push_str(&escape(&rest[start + opener..start + end]));
                }
                rest = &rest[start + end + end_tag.len()..];
            }
            // Unterminated: everything after it is unusable, so stop here
            // rather than treat the remainder as markup.
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The inverse of [`decode`], for text that was exempt from markup and is
/// about to stop being. The ampersand goes first, or the escapes written after
/// it are escaped a second time.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

    /// An episode's sidecar, which uses a different root element and carries
    /// two numbers no film has.
    #[test]
    fn reads_an_episode() {
        let episode = parse(
            "<episodedetails><title>The Constant</title><season>4</season>             <episode>5</episode><aired>2008-02-28</aired></episodedetails>",
        );
        assert_eq!(episode.title, "The Constant");
        assert_eq!(episode.episode, Some((4, 5)));
        assert_eq!(episode.aired, "2008-02-28");
    }

    /// One number without the other names nothing, so neither is taken.
    #[test]
    fn half_an_episode_number_is_no_episode_number() {
        assert_eq!(
            parse("<episodedetails><season>4</season></episodedetails>").episode,
            None
        );
        assert_eq!(
            parse("<episodedetails><episode>5</episode></episodedetails>").episode,
            None
        );
    }

    /// A film carries neither, and must not be made to look like an episode.
    #[test]
    fn a_film_has_no_episode_number() {
        let film = parse("<movie><title>Supergirl</title><year>2026</year></movie>");
        assert_eq!(film.episode, None);
        assert!(film.aired.is_empty());
    }

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
        // The escaped element names come back as the plot's own words, and
        // no stream is invented from them.
        let posing = parse("<plot>&lt;streamdetails&gt;&lt;audio&gt;</plot>");
        assert_eq!(posing.plot, "<streamdetails><audio>");
        assert!(posing.audio.is_empty());
    }

    #[test]
    fn nothing_at_all_is_not_an_error() {
        assert!(parse("").is_empty());
        assert!(parse("<streamdetails><audio>").is_empty());
        assert!(parse("<movie></movie>").is_empty());
        // Elements that are present and say nothing are the same as absent
        // ones, or every file without a plot would count as having metadata.
        assert!(parse("<movie><title></title><plot>  </plot></movie>").is_empty());
    }

    /// Details without a stream block, which is the common shape: plenty of
    /// scrapers describe the film and never touch the file.
    #[test]
    fn reads_the_film_without_any_streams() {
        let sidecar = parse(
            "<movie><title>Supergirl</title><year>2026</year>\
             <plot>Kara Zor-El.</plot><runtime>108</runtime>\
             <mpaa>PG-13</mpaa><genre>Action</genre><genre>Adventure</genre></movie>",
        );
        assert_eq!(sidecar.title, "Supergirl");
        assert_eq!(sidecar.year, Some(2026));
        assert_eq!(sidecar.plot, "Kara Zor-El.");
        assert_eq!(sidecar.runtime_mins, Some(108));
        assert_eq!(sidecar.mpaa, "PG-13");
        assert_eq!(sidecar.genres, ["Action", "Adventure"]);
        assert!(sidecar.audio.is_empty());
    }

    /// The whole reason CDATA is kept rather than stripped: it is how a
    /// summary with punctuation in it is written, and dropping it blanked
    /// the plot on every file that used one.
    #[test]
    fn a_plot_in_cdata_survives_intact() {
        let sidecar =
            parse("<movie><plot><![CDATA[Kara & Krypto <the dog> travel far.]]></plot></movie>");
        assert_eq!(sidecar.plot, "Kara & Krypto <the dog> travel far.");
    }

    /// Escaped and unescaped forms of the same summary have to arrive the
    /// same way round, or one of the two paths is decoding twice.
    #[test]
    fn both_ways_of_writing_a_plot_agree() {
        let escaped = parse("<movie><plot>Kara &amp; Krypto</plot></movie>");
        let cdata = parse("<movie><plot><![CDATA[Kara & Krypto]]></plot></movie>");
        assert_eq!(escaped.plot, "Kara & Krypto");
        assert_eq!(escaped.plot, cdata.plot);
    }

    /// A comment is still thrown away, unlike CDATA. It is not content, and
    /// one mentioning an element must not be read as that element.
    #[test]
    fn a_comment_is_still_not_content() {
        assert_eq!(
            parse("<movie><!-- <plot>hidden</plot> --></movie>").plot,
            ""
        );
    }

    /// Three writers, three spellings, one certificate.
    #[test]
    fn a_certificate_is_reduced_to_the_rating() {
        assert_eq!(certificate("PG-13"), "PG-13");
        assert_eq!(certificate("US:PG-13"), "PG-13");
        assert_eq!(
            certificate("Rated PG-13 for sequences of sci-fi violence"),
            "PG-13"
        );
        assert_eq!(certificate("  R  "), "R");
        assert_eq!(certificate(""), "");
    }

    /// Both shapes the score has had, and the newer one winning where a file
    /// carries an empty copy of the older one beside it.
    #[test]
    fn reads_the_score_from_either_shape() {
        assert_eq!(
            parse("<movie><rating>7.2</rating></movie>").rating,
            Some(7.2)
        );
        let both = "<movie><rating>0.0</rating><ratings><rating name=\"tmdb\" default=\"true\">\
                    <value>7.2</value><votes>900</votes></rating></ratings></movie>";
        assert_eq!(parse(both).rating, Some(7.2));
        // Nothing voted on yet reads as silence, not as a verdict of zero.
        assert_eq!(parse("<movie><rating>0.0</rating></movie>").rating, None);
    }

    /// `<premiered>` is what the current writers use, and `<year>` what the
    /// older ones did. A date reduces to the year in front of it.
    #[test]
    fn takes_the_year_from_either_field() {
        assert_eq!(parse("<movie><year>1995</year></movie>").year, Some(1995));
        assert_eq!(
            parse("<movie><premiered>2026-07-09</premiered></movie>").year,
            Some(2026)
        );
        // Muxers write the epoch when they were given nothing.
        assert_eq!(parse("<movie><year>1601</year></movie>").year, None);
    }

    /// A unit written into the field must not throw the whole number away.
    #[test]
    fn a_runtime_with_a_unit_still_reads() {
        assert_eq!(
            parse("<movie><runtime>128 min</runtime></movie>").runtime_mins,
            Some(128)
        );
    }

    /// An actor's headshot is a `<thumb>` too, which is why only `<art>` is
    /// read for artwork.
    #[test]
    fn artwork_comes_only_from_the_art_block() {
        let sidecar = parse(
            "<movie><actor><name>Milly Alcock</name><thumb>http://x/face.jpg</thumb></actor>\
             <art><poster>/mnt/hoth/Supergirl/poster.jpg</poster>\
             <fanart>/mnt/hoth/Supergirl/fanart.jpg</fanart></art></movie>",
        );
        assert_eq!(sidecar.poster, "/mnt/hoth/Supergirl/poster.jpg");
        assert_eq!(sidecar.fanart, "/mnt/hoth/Supergirl/fanart.jpg");
        assert_eq!(
            parse("<movie><actor><thumb>http://x/face.jpg</thumb></actor></movie>").poster,
            ""
        );
    }

    /// Trimmed from a real file on the library here, for the traps that only
    /// turn up in one. Every one of these was written by Jellyfin or by
    /// Radarr's Kodi metadata writer and would have gone unnoticed against a
    /// fixture written to suit the parser.
    #[test]
    fn a_file_from_the_wild_reads_correctly() {
        let real = "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>
<movie>
  <plot>After the devastating events of Avengers: Infinity War.</plot>
  <lockdata>false</lockdata>
  <title>Avengers: Endgame</title>
  <originaltitle>Avengers: Endgame</originaltitle>
  <rating>8.235</rating>
  <year>2019</year>
  <mpaa>PG-13</mpaa>
  <premiered>2019-04-24</premiered>
  <runtime>181</runtime>
  <country />
  <country>United States of America</country>
  <genre>Action</genre>
  <genre>Adventure</genre>
  <studio />
  <studio>Marvel Studios</studio>
  <art>
    <poster>/mnt/hoth/Videos/Movies/Avengers - Endgame (2019)/folder.jpg</poster>
    <fanart>/mnt/hoth/Videos/Movies/Avengers - Endgame (2019)/backdrop.jpg</fanart>
  </art>
  <set><name>The Avengers Collection</name></set>
  <fileinfo><streamdetails>
    <video><codec>h264</codec><width>1920</width><height>804</height></video>
    <audio><codec>dts</codec><language>rus</language><channels>6</channels></audio>
    <audio><codec>ac3</codec><language>eng</language><channels>2</channels></audio>
    <subtitle><language>eng</language><forced>False</forced></subtitle>
    <embeddedimage><codec>mjpeg</codec><width>395</width></embeddedimage>
  </streamdetails></fileinfo>
</movie>";
        let s = parse(real);
        // `<originaltitle>` follows `<title>` and must not be reached first,
        // nor its closing tag mistaken for the title's.
        assert_eq!(s.title, "Avengers: Endgame");
        assert_eq!(s.year, Some(2019));
        assert_eq!(s.mpaa, "PG-13");
        assert_eq!(s.rating, Some(8.235));
        assert_eq!(s.runtime_mins, Some(181));
        assert_eq!(s.genres, ["Action", "Adventure"]);
        // `<embeddedimage>` shares the stream block and is not a stream.
        assert_eq!(s.audio.len(), 2);
        assert_eq!(s.audio[1].language, "eng");
        assert_eq!(s.subtitles.len(), 1);
        // A path from the machine that did the scraping. Kept as stated here;
        // whether it resolves is decided in `crate::metadata`.
        assert!(s.poster.ends_with("folder.jpg"));
        assert!(s.fanart.ends_with("backdrop.jpg"));
    }

    /// An episode carries the same fields under a different root, and is read
    /// as one without the page needing to know which it got. Nothing
    /// series-aware is read: the episode's own name and summary are what the
    /// page shows, and the show it belongs to is not named.
    #[test]
    fn an_episode_reads_like_a_film() {
        let sidecar = parse(
            "<episodedetails><title>Pilot</title><showtitle>Supergirl</showtitle>\
             <season>1</season><episode>2</episode>\
             <plot>Kara reveals herself.</plot><aired>2026-01-04</aired></episodedetails>",
        );
        assert_eq!(sidecar.title, "Pilot");
        assert_eq!(sidecar.plot, "Kara reveals herself.");
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
                    described: None,
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
            video: Default::default(),
            tags: Default::default(),
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
