//! Everything the media page shows about a video, gathered from whatever can
//! answer.
//!
//! Three sources, each less informed than the one before it and each optional:
//!
//! 1. The `.nfo` sidecar a media library left beside the file, which is the
//!    only one that knows what the film is *called* as opposed to what the
//!    file is called. See [`crate::nfo`].
//! 2. The container's own tags, read while probing. Most downloads carry none,
//!    but a recording or a purchased file often names itself, and a title
//!    written into the file beats a filename with a release tag in it.
//! 3. The file itself - its size, and what the pipeline found in it.
//!
//! Every field falls back to the next source and then to nothing, so a file
//! with no sidecar, no tags and no artwork still produces a page. That case is
//! the common one rather than the exception: of 123 movie folders in the
//! library this was written against, 28 have a sidecar. The page is designed
//! up from the empty case, and this is the type that has to make the empty
//! case expressible - which is why nothing here is required and nothing is
//! filled with a placeholder.
//!
//! **Nothing is ever fetched.** No TMDB, no TVDB, no API keys, no terms to
//! honor: what is on disk is the whole of it. That keeps this a page about the
//! file in front of you rather than the first step towards a library browser.

use std::path::{Path, PathBuf};

use crate::probe::{Media, VideoDetails};
use crate::source::Source;

/// What the page draws, once every source has had its say.
#[derive(Clone, Default)]
pub struct Details {
    /// Never empty: the film's name where one is known, and the file's name
    /// otherwise. The fallback is deliberately the filename exactly as it is,
    /// release tags and all - a name half-cleaned by guesswork looks like
    /// metadata and is wrong often enough to be worse than the raw string.
    pub title: String,
    pub year: Option<u32>,
    /// Which episode this is, where a sidecar said so. Only an episode's
    /// sidecar carries it, so this is also how the page tells one from a film.
    pub episode: Option<(u32, u32)>,
    /// The day an episode first went out, ready to read. Empty for a film, and
    /// for an episode whose sidecar did not say.
    pub aired: String,
    pub plot: String,
    /// Certificate, already reduced to the short form - "PG-13".
    pub certificate: String,
    /// Out of ten.
    pub rating: Option<f64>,
    pub genres: Vec<String>,
    /// Seconds. Taken from the container, which measures the file in hand,
    /// and from the sidecar's stated runtime only when the container could
    /// not say - which is the case for some streams and a few bad MP4s.
    pub duration_s: f64,
    /// Bytes, for a local file. A remote source is not stat-able and reports
    /// nothing rather than a guess.
    pub size_bytes: Option<u64>,
    pub video: VideoDetails,
    /// The container's own name, as a viewer would say it: "MKV", "MP4".
    pub container: String,
    /// Where the artwork is, not the artwork itself. Loading it is the slow
    /// part and belongs on a thread - see [`load_image`].
    pub poster: Option<Art>,
    pub backdrop: Option<Art>,
}

/// A picture to draw, and where it came from.
///
/// Kept as a source to read rather than as decoded bytes so that finding
/// artwork stays cheap: discovery is a handful of `is_file` calls, and the
/// megabyte behind whichever one matched is only read when something is
/// actually going to draw it.
#[derive(Clone, PartialEq)]
pub enum Art {
    /// A file beside the video, or one the sidecar named that turned out to
    /// exist on this machine.
    Path(PathBuf),
    /// Cover art carried inside the container itself, already in hand because
    /// the tag list arrived with it.
    Embedded(Vec<u8>),
}

impl Details {
    /// The running time, worded the way the comps do: "1hr 48min".
    ///
    /// Nothing under a minute gets a reading at all. A file the container
    /// could not measure would otherwise announce "0min", which reads as a
    /// broken file rather than as an unmeasured one.
    pub fn runtime(&self) -> Option<String> {
        let total = self.duration_s.round() as u64;
        if total < 60 {
            return None;
        }
        let (hours, minutes) = (total / 3600, (total % 3600) / 60);
        Some(match hours {
            0 => format!("{minutes}min"),
            _ => format!("{hours}hr {minutes}min"),
        })
    }

    /// The size, in the unit that makes it a number a person can hold.
    pub fn filesize(&self) -> Option<String> {
        let bytes = self.size_bytes? as f64;
        // Powers of 1024 labelled with the short forms, which is what every
        // media application shows and what the comps ask for.
        const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
        if bytes < 1024.0 {
            return Some(format!("{bytes:.0} bytes"));
        }
        let mut value = bytes / 1024.0;
        let mut unit = 0;
        while value >= 1024.0 && unit + 1 < UNITS.len() {
            value /= 1024.0;
            unit += 1;
        }
        // A tenth of a gigabyte is a hundred megabytes, which is worth a
        // digit; a tenth of a kilobyte is not worth printing.
        Some(match unit {
            0 => format!("{value:.0} {}", UNITS[unit]),
            _ => format!("{value:.2} {}", UNITS[unit]),
        })
    }

    /// The picture's size, on its own: "1080p".
    pub fn resolution(&self) -> Option<String> {
        self.video.resolution()
    }

    /// What encoded it, on its own: "H.264".
    ///
    /// Cut back to the name. GStreamer describes a stream as fully as it can -
    /// "H.264 (High Profile)", "H.265 (Main 10 Profile)" - and the profile is
    /// a detail for someone debugging a decode rather than for a line under a
    /// poster, where it doubles the length of the reading to say something
    /// almost nobody is asking.
    ///
    /// Reported separately from the resolution, and on its own line, because
    /// the two together were the widest reading in a column that is only as
    /// wide as the poster above it.
    pub fn codec(&self) -> Option<String> {
        let codec = self
            .video
            .codec
            .split(" (")
            .next()
            .unwrap_or_default()
            .trim();
        (!codec.is_empty()).then(|| codec.to_string())
    }

    /// Frames per second, at the precision the number deserves.
    ///
    /// 23.976 and 29.97 are the two that matter and both need three decimals
    /// to be themselves; 24 and 25 would look wrong carrying any. So trailing
    /// zeros are dropped rather than a fixed precision being chosen.
    pub fn framerate(&self) -> Option<String> {
        let fps = self.video.fps;
        if fps <= 0.0 {
            return None;
        }
        let text = format!("{fps:.3}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        Some(format!("{text} fps"))
    }

    /// Overall bitrate, worked out rather than read: the container reports its
    /// streams' rates separately and often not at all, while the size and the
    /// duration are both known and their quotient is what a viewer means by
    /// "how good is this copy".
    ///
    /// Only for a local file, and only when both figures are real.
    pub fn bitrate(&self) -> Option<String> {
        let bytes = self.size_bytes? as f64;
        if self.duration_s < 1.0 {
            return None;
        }
        let mbps = bytes * 8.0 / self.duration_s / 1_000_000.0;
        // Below a megabit the number is small enough that a decimal is noise,
        // and the file is a long way from anything anyone is inspecting.
        Some(match mbps {
            m if m < 1.0 => format!("{:.0} kbps", m * 1000.0),
            m => format!("{m:.1} Mbps"),
        })
    }
}

/// Gathers what is known about a source.
///
/// Cheap by design - a sidecar is a small file and artwork discovery is a
/// handful of `is_file` calls - but it still touches the disk, and over a
/// network share that is not free. Called from wherever the probe already
/// runs, which is a worker thread on the path that matters.
pub fn resolve(source: &Source, media: &Media, beside: Beside, launcher_title: &str) -> Details {
    // Only where the viewer wants what is beside the file read at all. With it
    // off, `path` is `None` here and everything downstream that looks on disk -
    // the sidecar, the poster, the backdrop - finds nothing and says so in the
    // ordinary way, which is the same code path a film with no sidecar takes.
    let path = source.local().filter(|_| beside.metadata);
    let sidecar = path.and_then(crate::nfo::read).unwrap_or_default();

    // The four sources in order, each one only consulted for what the one
    // before it left empty.
    // The film's name if anything knows it, and otherwise the file's own -
    // without its extension. ".mkv" is not part of what the film is called,
    // and as the largest text on the page it read as though it were. The rest
    // The rest of the name is left exactly as it is: release tags and all,
    // because a name half-cleaned by guesswork looks like metadata and is
    // wrong often enough to be worse than the raw string. The one exception is
    // a year in brackets at the very end, which comes off - it is shown in the
    // facts line directly underneath, and a title carrying it too reads as
    // part of what the film is called. The row below still names the file in
    // full.
    // The file's own name, and the year a library may have written on the end
    // of it, taken off. Both come from the one place so they cannot disagree
    // about whether there was a year there at all.
    let (named, named_year) = split_year(&without_extension(&source.label()));
    // The launcher's own title comes first, ahead of anything on disk. Kodi
    // names an item from its library - "Avengers: Endgame" where the file is
    // called `Avengers - Endgame (2019) Bluray-1080p.mkv` - and for an add-on
    // stream it is the only real name there is, since the URI ends in an
    // opaque id. Empty when nothing launched us, which is the ordinary case.
    let title = first_of([
        launcher_title,
        sidecar.title.as_str(),
        media.tags.title.as_str(),
        &named,
    ]);
    let plot = flowed(&first_of([
        sidecar.plot.as_str(),
        media.tags.description.as_str(),
    ]));

    // The container measures the file in hand; the sidecar states what the
    // film runs to, which is the same thing only when the sidecar is about
    // this release. So the container wins wherever it answered at all.
    let duration_s = match media.duration_ns {
        0 => sidecar.runtime_mins.unwrap_or(0) as f64 * 60.0,
        ns => ns as f64 / 1e9,
    };

    Details {
        title,
        // The name is asked last, after anything that was actually written
        // about the film. It is the most reliable of the three when it answers
        // at all - a library put it there on purpose - but it is still a file
        // name, and a sidecar or a container tag is a statement.
        year: sidecar.year.or(media.tags.year).or(named_year),
        episode: sidecar.episode,
        aired: aired(&sidecar.aired),
        plot,
        certificate: sidecar.mpaa.clone(),
        rating: sidecar.rating,
        genres: sidecar.genres.clone(),
        duration_s,
        size_bytes: path
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|m| m.len()),
        video: media.video.clone(),
        container: container(source),
        poster: path.and_then(|path| find_poster(path, &sidecar, media)),
        backdrop: path
            .filter(|_| beside.backdrop)
            .and_then(|path| find_backdrop(path, &sidecar)),
    }
}

/// Collapses a summary's own line breaks so it flows to whatever width it is
/// given.
///
/// Sidecars wrap their prose. A `<plot>` is written as a paragraph in a text
/// file and arrives carrying the newlines of that file, which a label renders
/// as the line breaks they are - so the summary ignored the width available to
/// it and broke wherever the scraper's editor happened to. It reads as a
/// column of ragged text down the middle of the page and looks like a layout
/// fault, which is exactly how this was found.
///
/// Every run of whitespace becomes one space, which is what flowing text means
/// and what the three-line box on the page needs.
fn flowed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A filename without its extension, for use as a title.
///
/// Only a *plausible* extension is removed. A film's name can perfectly well
/// end in a short word after a dot - and one that has been through a scene
/// release ends in things like "1080p.DTS" - so this takes off a final piece
/// only when it is short and entirely letters or digits, which is what a
/// container extension looks like and what "2024" or "Part 2" does not.
/// What the viewer has agreed may be read from disk beside the video.
///
/// Passed in rather than read here, because this module is given a file and
/// answers what is known about it - what a viewer has chosen is the
/// application's business, and threading it through keeps this testable
/// without a config.
#[derive(Clone, Copy)]
pub struct Beside {
    /// The sidecar and the artwork files. Off means the page falls back to the
    /// file name and the container's own tags, which is the same page a film
    /// with no sidecar has always had.
    pub metadata: bool,
    /// The fanart behind the page. Meaningless with `metadata` off, since
    /// there is then nothing found to draw.
    pub backdrop: bool,
}

/// A broadcast date as the page shows it: "September 22, 2004".
///
/// Sidecars write `<aired>` as an ISO date, which is unambiguous and not how
/// anybody says a date out loud. Anything that is not one is handed back
/// untouched rather than dropped - a date this does not recognise is still a
/// date the file is telling us, and showing it as written beats showing
/// nothing.
fn aired(raw: &str) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];

    let raw = raw.trim();
    let mut parts = raw.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return raw.to_string();
    };
    let (Ok(month), Ok(day)) = (month.parse::<usize>(), day.parse::<u32>()) else {
        return raw.to_string();
    };
    match MONTHS.get(month.wrapping_sub(1)) {
        Some(name) if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{name} {day}, {year}")
        }
        _ => raw.to_string(),
    }
}

/// Splits the year a media library wrote onto the end of a file name off the
/// rest of the name.
///
/// Guesswork about file names is avoided everywhere else here - the title
/// keeps its release tags rather than being half-cleaned - so this is
/// deliberately the narrowest rule that covers the convention Kodi, Plex and
/// Jellyfin share: four digits in brackets at the very end, nothing after them.
///
/// Anchored to the end, and that is what makes it safe rather than clever. The
/// film this was written for is called "(500) Days of Summer (2009)": a rule
/// that took the first bracketed number would date it to the year 500, and one
/// that took any bracketed number would find candidates in half the release
/// tags people put in file names. At the end, in brackets, exactly four
/// digits, is a library having written a year.
///
/// The name comes back whole when there is no year to take off it, and also
/// when taking it off would leave nothing - a file actually named "(2009)"
/// keeps that as its title, since something is better to show than nothing.
fn split_year(name: &str) -> (String, Option<u32>) {
    let whole = || (name.to_string(), None);
    let name = name.trim_end();
    let Some((rest, digits)) = name.strip_suffix(')').and_then(|n| n.rsplit_once('(')) else {
        return whole();
    };
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return whole();
    }
    // The same span the container's own dates are held to: early enough for
    // the first films, late enough not to argue with anybody's plans.
    let Some(year) = digits
        .parse()
        .ok()
        .filter(|year| (1870..=2200).contains(year))
    else {
        return whole();
    };
    let rest = rest.trim_end();
    match rest.is_empty() {
        true => whole(),
        false => (rest.to_string(), Some(year)),
    }
}

fn without_extension(name: &str) -> String {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return name.to_string();
    };
    let looks_like_one = !extension.is_empty()
        && extension.len() <= 5
        && extension.chars().all(|c| c.is_ascii_alphanumeric())
        // A trailing number is a part or a year far more often than a format.
        && extension.chars().any(|c| c.is_ascii_alphabetic());
    match looks_like_one && !stem.is_empty() {
        true => stem.to_string(),
        false => name.to_string(),
    }
}

/// The first of several candidates that says anything.
fn first_of<'a>(candidates: impl IntoIterator<Item = &'a str>) -> String {
    candidates
        .into_iter()
        .map(str::trim)
        .find(|text| !text.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// What kind of file it is, from the extension, uppercased.
///
/// The extension rather than what the container reported, because the two
/// disagree in the direction that confuses people: a `.mkv` probes as
/// "Matroska" and a `.m4v` as "ISO MP4/M4A", neither of which is what is
/// written on the file. Anything unreasonably long is dropped rather than
/// printed, since a name with a dot in it and no extension would otherwise
/// produce a line of nonsense.
fn container(source: &Source) -> String {
    let Some(path) = source.local() else {
        return String::new();
    };
    path.extension()
        .map(|ext| ext.to_string_lossy().to_uppercase())
        .filter(|ext| ext.len() <= 5)
        .unwrap_or_default()
}

/// The image formats artwork may be in.
///
/// PNG and JPEG only, and that is a hard limit rather than a shortlist: GDK
/// decodes both itself, while anything else needs a gdk-pixbuf loader, and
/// GStreamer's Windows distribution ships none at all. A WebP poster would
/// load on Linux and be a missing image on Windows, which is worse than not
/// offering it. The same constraint is why the placeholder icon is compiled
/// in as a PNG rather than as the SVG it was drawn from.
const IMAGE_EXTENSIONS: [&str; 3] = ["jpg", "png", "jpeg"];

/// Looks for a poster, in the order the conventions deserve.
///
/// The layouts are Kodi's, Jellyfin's and Emby's, and no two libraries use the
/// same one - all three shapes are present in the library this was written
/// against. The order runs from most specific to least: a name carrying the
/// video's own is unambiguous in a folder holding several films, while
/// `folder.jpg` describes whatever the folder happens to contain.
///
/// The sidecar's own `<art>` comes near the end on purpose. It holds absolute
/// paths from the machine that did the scraping - `/mnt/hoth/...` here, which
/// resolves on the Pi and on nothing else - so it is worth trying and not
/// worth trusting ahead of what is demonstrably beside the file.
fn find_poster(video: &Path, sidecar: &crate::nfo::Sidecar, media: &Media) -> Option<Art> {
    let folder = video.parent()?;
    let stem = video.file_stem()?.to_string_lossy().to_string();

    let named = ["-poster", "-thumb"]
        .iter()
        .map(|suffix| format!("{stem}{suffix}"));
    beside(folder, named.chain(shared_poster_names()))
        .or_else(|| climbed(folder, shared_poster_names))
        .or_else(|| stated(&sidecar.poster))
        // Cover art inside the container, which is the last thing left and
        // the only one that needs no file at all. Real files do carry it -
        // the Matroska rips here hold a 395x500 JPEG apiece.
        .or_else(|| media.tags.image.clone().map(Art::Embedded))
}

/// The backdrop, by the same rules.
///
/// `landscape.jpg` is deliberately not among them. It sits beside the others
/// in this library and is a different picture for a different purpose - a
/// wide crop with the title lettering burned into it - which behind large
/// type would collide with the title drawn over it.
fn find_backdrop(video: &Path, sidecar: &crate::nfo::Sidecar) -> Option<Art> {
    let folder = video.parent()?;
    let stem = video.file_stem()?.to_string_lossy().to_string();

    let named = ["-fanart", "-backdrop"]
        .iter()
        .map(|suffix| format!("{stem}{suffix}"));

    beside(folder, named.chain(shared_art_names()))
        .or_else(|| climbed(folder, shared_art_names))
        .or_else(|| stated(&sidecar.fanart))
}

/// The names a backdrop goes by when it belongs to the folder rather than to
/// one video in it.
fn shared_art_names() -> impl Iterator<Item = String> {
    ["backdrop", "fanart", "background"]
        .iter()
        .map(|name| name.to_string())
}

/// The same, for a poster. `folder.jpg` is what is actually in the library
/// here, written by Jellyfin; `poster.jpg` is Kodi's name for the same thing.
fn shared_poster_names() -> impl Iterator<Item = String> {
    ["poster", "folder", "cover"]
        .iter()
        .map(|name| name.to_string())
}

/// A picture belonging to the series, for an episode that has none of its own.
///
/// Episodes are read generically here - an episode's title, plot and year are
/// its own, and nothing goes looking for a series to describe it by. Artwork
/// is the exception, and only because of where libraries put it: the layout
/// Kodi, Jellyfin and Emby share keeps a per-episode `.nfo` and thumbnail
/// beside each file, and one backdrop for the whole series at its root. An
/// episode therefore has no backdrop of its own to find, and the one meant for
/// it is one or two folders up.
///
/// **It climbs only on evidence, and that is the whole difficulty.** A folder
/// above a video is not a series root just because it is above it: point this
/// at a film in a library whose top folder happens to hold a `fanart.jpg` and
/// every film in it would be drawn against the same picture. So it climbs when
/// the video's folder is named like a season, or when the folder above holds a
/// `tvshow.nfo` - the file that says outright what that folder is.
fn climbed<I>(folder: &Path, names: fn() -> I) -> Option<Art>
where
    I: IntoIterator<Item = String>,
{
    let mut here = folder;
    // Two, which is as deep as the layout goes: series, season, episode.
    for _ in 0..2 {
        let above = here.parent()?;
        if !is_season_folder(here) && !above.join("tvshow.nfo").is_file() {
            return None;
        }
        if let Some(art) = beside(above, names()) {
            return Some(art);
        }
        here = above;
    }
    None
}

/// Whether a folder is named the way libraries name a season.
///
/// Case-insensitive, and "Specials" counts: it is season zero by convention
/// and sits beside the numbered ones.
fn is_season_folder(folder: &Path) -> bool {
    let Some(name) = folder.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    let name = name.trim().to_lowercase();
    name == "specials" || (name.starts_with("season") && name[6..].trim().parse::<u32>().is_ok())
}

/// The first of the candidate names that exists in the folder, tried against
/// each image extension in turn.
fn beside(folder: &Path, names: impl IntoIterator<Item = String>) -> Option<Art> {
    for name in names {
        for extension in IMAGE_EXTENSIONS {
            let candidate = folder.join(format!("{name}.{extension}"));
            if candidate.is_file() {
                return Some(Art::Path(candidate));
            }
        }
    }
    None
}

/// A path the sidecar named, taken only if it resolves on this machine.
fn stated(path: &str) -> Option<Art> {
    if path.is_empty() {
        return None;
    }
    // Not a URL: the older `<thumb>` form carries those and this never
    // fetches anything, so a remote address is simply not artwork here.
    if path.contains("://") {
        return None;
    }
    let path = PathBuf::from(path);
    path.is_file().then_some(Art::Path(path))
}

/// Reads the bytes behind a piece of artwork, ready to be handed to GDK.
///
/// Split from discovery because this is the part that costs: a backdrop is a
/// megabyte or two, and over a network share that is long enough to be felt if
/// it happens while a screen is being built. Callers run it on a thread and
/// fill the picture in when it arrives.
pub fn load_image(art: &Art) -> Option<Vec<u8>> {
    match art {
        Art::Path(path) => std::fs::read(path).ok(),
        Art::Embedded(bytes) => Some(bytes.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::aired;

    /// The shape every sidecar writes, and the one a viewer reads.
    #[test]
    fn an_iso_date_is_read_out_in_words() {
        assert_eq!(aired("2004-09-22"), "September 22, 2004");
        assert_eq!(aired("2010-01-05"), "January 5, 2010");
        assert_eq!(aired(" 1999-12-31 "), "December 31, 1999");
    }

    /// Anything else is handed back as written. A date this does not
    /// understand is still a date the file is telling us, and showing it
    /// as-is beats showing nothing.
    #[test]
    fn anything_else_is_left_alone() {
        assert_eq!(aired("22 September 2004"), "22 September 2004");
        assert_eq!(aired("2004-13-01"), "2004-13-01");
        assert_eq!(aired("2004-09"), "2004-09");
        assert_eq!(aired("2004-09-22-01"), "2004-09-22-01");
        assert_eq!(aired(""), "");
    }

    use super::is_season_folder;
    use std::path::Path;

    /// The names libraries actually use for a season folder, and the one that
    /// is a season without saying so.
    #[test]
    fn season_folders_are_recognized() {
        assert!(is_season_folder(Path::new("/x/Season 01")));
        assert!(is_season_folder(Path::new("/x/season 1")));
        assert!(is_season_folder(Path::new("/x/SEASON 12")));
        assert!(is_season_folder(Path::new("/x/Specials")));
    }

    /// Nothing else climbs. A film's own folder must not be mistaken for a
    /// season, or every film in a library with a picture at its root would be
    /// drawn against that picture.
    #[test]
    fn nothing_else_is_a_season() {
        assert!(!is_season_folder(Path::new("/x/Seasons of Love")));
        assert!(!is_season_folder(Path::new("/x/Season Finale")));
        assert!(!is_season_folder(Path::new(
            "/x/(500) Days of Summer (2009)"
        )));
        assert!(!is_season_folder(Path::new("/x/Movies")));
        assert!(!is_season_folder(Path::new("/")));
    }

    use super::split_year;

    /// The file this rule was written for, and the reason it is anchored to
    /// the end: the title opens with a bracketed number that is not a year.
    #[test]
    fn a_library_year_comes_off_the_end() {
        let (name, year) = split_year("(500) Days of Summer (2009)");
        assert_eq!(name, "(500) Days of Summer");
        assert_eq!(year, Some(2009));
    }

    /// A bracketed number anywhere but the end is left alone, however much it
    /// looks like a year. Release tags are full of them.
    #[test]
    fn only_at_the_very_end() {
        assert_eq!(split_year("Some Film (2009) 1080p BluRay").1, None);
        assert_eq!(split_year("(500) Days of Summer").1, None);
        assert_eq!(split_year("Apollo (13)").1, None);
    }

    /// Four digits exactly, and inside a span a film could have been made in.
    #[test]
    fn implausible_years_are_not_years() {
        assert_eq!(split_year("Film (20099)").1, None);
        assert_eq!(split_year("Film (209)").1, None);
        assert_eq!(split_year("Film (1860)").1, None);
        assert_eq!(split_year("Film (2201)").1, None);
        assert_eq!(split_year("Film (20a9)").1, None);
    }

    /// Taking the year off would leave nothing to call the film, so it stays.
    #[test]
    fn a_name_that_is_only_a_year_keeps_it() {
        let (name, year) = split_year("(2009)");
        assert_eq!(name, "(2009)");
        assert_eq!(year, None);
    }

    use super::*;

    fn details() -> Details {
        Details {
            duration_s: 6480.0,
            size_bytes: Some(2_963_527_925),
            ..Default::default()
        }
    }

    #[test]
    fn a_runtime_reads_the_way_the_comps_word_it() {
        let mut d = details();
        assert_eq!(d.runtime().as_deref(), Some("1hr 48min"));
        d.duration_s = 2700.0;
        assert_eq!(d.runtime().as_deref(), Some("45min"));
        d.duration_s = 7200.0;
        assert_eq!(d.runtime().as_deref(), Some("2hr 0min"));
    }

    /// A file the container could not measure must say nothing rather than
    /// announce "0min", which reads as a broken file.
    #[test]
    fn an_unmeasured_file_gives_no_runtime() {
        let mut d = details();
        d.duration_s = 0.0;
        assert_eq!(d.runtime(), None);
        d.duration_s = 12.0;
        assert_eq!(d.runtime(), None);
    }

    #[test]
    fn a_filesize_reads_in_the_unit_that_suits_it() {
        assert_eq!(details().filesize().as_deref(), Some("2.76 GB"));
        let at = |bytes| {
            Details {
                size_bytes: Some(bytes),
                ..Default::default()
            }
            .filesize()
        };
        assert_eq!(at(512).as_deref(), Some("512 bytes"));
        assert_eq!(at(4096).as_deref(), Some("4 KB"));
        assert_eq!(at(700 * 1024 * 1024).as_deref(), Some("700.00 MB"));
    }

    /// A remote source cannot be measured, and every line that depends on the
    /// size has to go quiet together rather than printing a zero.
    #[test]
    fn nothing_measurable_prints_nothing() {
        let d = Details::default();
        assert_eq!(d.filesize(), None);
        assert_eq!(d.bitrate(), None);
        assert_eq!(d.resolution(), None);
        assert_eq!(d.framerate(), None);
    }

    /// The two rates that matter both need three decimals to be themselves,
    /// and the whole numbers would look wrong carrying any.
    #[test]
    fn a_framerate_keeps_only_the_digits_it_needs() {
        let at = |fps| {
            Details {
                video: VideoDetails {
                    fps,
                    ..Default::default()
                },
                ..Default::default()
            }
            .framerate()
        };
        assert_eq!(at(24000.0 / 1001.0).as_deref(), Some("23.976 fps"));
        assert_eq!(at(30000.0 / 1001.0).as_deref(), Some("29.97 fps"));
        assert_eq!(at(25.0).as_deref(), Some("25 fps"));
        assert_eq!(at(24.0).as_deref(), Some("24 fps"));
        assert_eq!(at(0.0), None);
    }

    #[test]
    fn a_bitrate_is_the_quotient_of_what_is_known() {
        assert_eq!(details().bitrate().as_deref(), Some("3.7 Mbps"));
        let small = Details {
            size_bytes: Some(1_000_000),
            duration_s: 60.0,
            ..Default::default()
        };
        assert_eq!(small.bitrate().as_deref(), Some("133 kbps"));
    }

    #[test]
    fn the_picture_names_its_codec_when_it_has_one() {
        let mut d = Details {
            video: VideoDetails {
                width: 1920,
                height: 804,
                codec: "H.264".to_string(),
                fps: 0.0,
            },
            ..Default::default()
        };
        // Two readings on two lines, never one.
        assert_eq!(d.resolution().as_deref(), Some("1080p"));
        assert_eq!(d.codec().as_deref(), Some("H.264"));
        // The profile is dropped: what the discoverer actually hands back for
        // the Blu-ray rips in the library here is "H.264 (High Profile)",
        // which is twice the length to say something nobody is asking.
        d.video.codec = "H.264 (High Profile)".to_string();
        assert_eq!(d.codec().as_deref(), Some("H.264"));
        d.video.codec = "H.265 (Main 10 Profile)".to_string();
        assert_eq!(d.codec().as_deref(), Some("H.265"));
        // A container that named a size and no codec still gets its size, and
        // simply has no codec line.
        d.video.codec = String::new();
        assert_eq!(d.resolution().as_deref(), Some("1080p"));
        assert_eq!(d.codec(), None);
    }

    /// Sources are consulted in order and an empty one is passed over, which
    /// is the whole of how the fallback works.
    #[test]
    fn the_first_source_with_an_answer_wins() {
        assert_eq!(first_of(["", "  ", "tags", "file"]), "tags");
        assert_eq!(first_of(["sidecar", "tags"]), "sidecar");
        assert_eq!(first_of(["", ""]), "");
    }

    /// The extension is not part of what a film is called, and as the largest
    /// text on the page it read as though it were.
    #[test]
    fn a_filename_loses_its_extension_and_nothing_else() {
        assert_eq!(without_extension("Alien (1979).avi"), "Alien (1979)");
        assert_eq!(
            without_extension("Supergirl (2026) Webdl-1080p.mp4"),
            "Supergirl (2026) Webdl-1080p"
        );
        assert_eq!(
            without_extension("The Quiet Harbor (2024).mkv"),
            "The Quiet Harbor (2024)"
        );
    }

    /// A dot in a name is not automatically an extension, and cutting at the
    /// last one regardless would take words off the end of real titles.
    #[test]
    fn a_name_that_merely_contains_a_dot_is_left_alone() {
        assert_eq!(without_extension("Dr. Strangelove"), "Dr. Strangelove");
        // Ends in digits: a part or a year far more often than a format.
        assert_eq!(without_extension("Blade Runner 2049"), "Blade Runner 2049");
        assert_eq!(
            without_extension("Mission Impossible.2"),
            "Mission Impossible.2"
        );
        // Too long to be a container extension.
        assert_eq!(without_extension("A Film.Directors"), "A Film.Directors");
        assert_eq!(without_extension("noextension"), "noextension");
        assert_eq!(without_extension(""), "");
    }

    /// A scraper's URL is not artwork: this never fetches anything.
    #[test]
    fn a_remote_address_is_not_taken_as_artwork() {
        assert!(stated("https://image.tmdb.org/t/p/original/x.jpg").is_none());
        assert!(stated("").is_none());
        // A path from the machine that did the scraping, absent here.
        assert!(stated("/mnt/hoth/Videos/Movies/Nothing/folder.jpg").is_none());
    }
}
