//! Track flags read out of a Matroska file's own header.
//!
//! Matroska records, per track, whether it is forced, whether it is for the
//! hard of hearing, whether it carries a description for blind viewers,
//! whether it is a commentary, and whether it is the container's default.
//! GStreamer reads several of these and hands none of them on: `matroskademux`
//! logs `TrackForced: 1` while parsing and then sends tags carrying only the
//! language, the title and the track id, because `GstStreamFlags` has nowhere
//! to put the rest. Verified 2026-08-19 against a file with the flag set.
//!
//! So the information is in the file, is read, and is thrown away before it
//! reaches us. This reads it again rather than guessing from track titles,
//! which is what the rest of the application had to do.
//!
//! **Matroska only, and deliberately.** MP4 and QuickTime have no equivalent
//! flags to read, so those files keep falling back to what their track names
//! say. That is not much of a loss: the files that carry these flags are rips,
//! and rips are `.mkv`.
//!
//! **It reads the header and stops.** Every track's description sits before
//! the first frame of video - byte 83 in the sample film, where the media
//! starts at 3937 - so this costs one short read at the front of the file and
//! never touches the rest of it.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// How much of the file to look at. The track descriptions live at the front,
/// ahead of the media, and a megabyte is generous for that: the sample film's
/// twenty tracks are described inside four kilobytes.
///
/// Bounded rather than trusting, because this runs against whatever a person
/// points at, and a file that claims to be Matroska and is not should cost one
/// read rather than as much memory as it likes.
const HEADER_BYTES: usize = 1 << 20;

/// The element ids worth stopping on. Everything else is skipped by the size
/// it declares, which is what makes this a header reader rather than a parser.
const SEGMENT: u64 = 0x1853_8067;
const TRACKS: u64 = 0x1654_AE6B;
const TRACK_ENTRY: u64 = 0xAE;
const TRACK_NUMBER: u64 = 0xD7;
const TRACK_TYPE: u64 = 0x83;
const FLAG_DEFAULT: u64 = 0x88;
const FLAG_FORCED: u64 = 0x55AA;
const FLAG_HEARING_IMPAIRED: u64 = 0x55AB;
const FLAG_VISUAL_IMPAIRED: u64 = 0x55AC;
const FLAG_ORIGINAL: u64 = 0x55AE;
const FLAG_COMMENTARY: u64 = 0x55AF;

/// What the container says about one track, and only what it says.
///
/// Every field is an `Option` on purpose, and the three states are different
/// answers rather than degrees of the same one: `Some(true)` and `Some(false)`
/// are the file saying so, and `None` is the file not saying, which must fall
/// through to the `.nfo` and then to the track's name rather than being read
/// as "no".
///
/// That distinction is not decoration. Matroska gives `FlagDefault` a default
/// value of *one*, so a writer omits it on the track that is default and
/// writes an explicit zero on the others - measured on the sample film, where
/// the default English soundtrack is the one track with no `FlagDefault`
/// element at all. Read absence as false and the answer inverts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub default: Option<bool>,
    pub forced: Option<bool>,
    pub hearing_impaired: Option<bool>,
    pub visual_impaired: Option<bool>,
    pub original: Option<bool>,
    pub commentary: Option<bool>,
}

/// Every track's flags, keyed by Matroska track number.
///
/// The key is the track number rather than a position in a list, because that
/// is what `matroskademux` puts in the `container-specific-track-id` tag - so a
/// caller can join these to GStreamer's streams by asking each stream its id
/// rather than by trusting two orderings to agree. Ordering is exactly how
/// this would go quietly wrong.
///
/// Empty for anything that is not a readable Matroska file: a different
/// container, a URL, an unreadable path, a truncated header. Nothing here
/// reports a failure, because there is nothing a caller could usefully do
/// about one - the flags are an improvement on reading track titles, not a
/// requirement, and the titles are still there.
pub fn flags(video: &Path) -> HashMap<u64, Flags> {
    let Some(header) = head(video) else {
        return HashMap::new();
    };
    let mut tracks = HashMap::new();
    walk(
        &header,
        0,
        header.len(),
        &mut tracks,
        &mut Flags::default(),
        0,
    );
    tracks
}

/// The first [`HEADER_BYTES`] of the file, or fewer if it is shorter.
fn head(video: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(video).ok()?;
    let mut buffer = Vec::new();
    file.take(HEADER_BYTES as u64)
        .read_to_end(&mut buffer)
        .ok()?;
    // Every Matroska file opens with the EBML header's own id. Checked so that
    // pointing this at an MP4 costs one read and no walking.
    if buffer.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some(buffer)
    } else {
        None
    }
}

/// One EBML variable-length integer, and where it ended.
///
/// The width is written into the first byte as the position of its highest set
/// bit, the same trick UTF-8 uses. An id keeps those marker bits, because the
/// marker is part of the id; a size strips them, because there the marker is
/// only saying how long the number is.
///
/// A size whose value bits are all set means "unknown", which is legal on a
/// master element and means it runs until its parent does - `Segment` is
/// routinely written that way in a file meant to be streamed. Returning `None`
/// for it rather than a very large number is the difference between reading
/// such a file and running off the end of the buffer.
fn number(buffer: &[u8], at: usize, keep_marker: bool) -> Option<(Option<u64>, usize)> {
    let first = *buffer.get(at)?;
    if first == 0 {
        // No marker bit in the first byte means a width of more than eight,
        // which EBML does not have. A corrupt file, or not a file at all.
        return None;
    }
    let width = 8 - (7 - first.leading_zeros() as usize);
    if at + width > buffer.len() {
        return None;
    }
    let mut value = if keep_marker {
        u64::from(first)
    } else {
        // Shifting a `u8` by eight is not zero, it is a panic - and width is
        // eight for the widest legal number, which is exactly how `Segment`
        // writes an unknown size.
        let mask = if width >= 8 { 0 } else { 0xFFu8 >> width };
        u64::from(first & mask)
    };
    for byte in &buffer[at + 1..at + width] {
        value = (value << 8) | u64::from(*byte);
    }
    let unknown = !keep_marker && value == (1u64 << (7 * width)) - 1;
    Some((if unknown { None } else { Some(value) }, at + width))
}

/// An unsigned integer of `len` bytes, which is how EBML stores every flag
/// here. A zero-length one is legal and means zero.
fn unsigned(buffer: &[u8], from: usize, to: usize) -> u64 {
    buffer[from..to]
        .iter()
        .fold(0u64, |v, b| (v << 8) | u64::from(*b))
}

/// Walks the header, descending only into the elements on the way to a track's
/// flags and stepping over everything else by its declared size.
///
/// `depth` is a guard rather than a feature: a corrupt file can describe a
/// master element that contains itself, and this is reading whatever somebody
/// pointed at.
fn walk(
    buffer: &[u8],
    mut at: usize,
    end: usize,
    tracks: &mut HashMap<u64, Flags>,
    track: &mut Flags,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let mut number_of_this_track: Option<u64> = None;
    while at < end {
        let Some((Some(id), after_id)) = number(buffer, at, true) else {
            return;
        };
        let Some((size, after_size)) = number(buffer, after_id, false) else {
            return;
        };
        // An unknown size runs to the end of the parent, which is what a
        // streamed `Segment` declares.
        let stop = match size {
            Some(size) => match after_size.checked_add(size as usize) {
                Some(stop) => stop.min(end),
                None => return,
            },
            None => end,
        };
        if stop > buffer.len() || stop < after_size {
            return;
        }

        match id {
            SEGMENT | TRACKS => walk(buffer, after_size, stop, tracks, track, depth + 1),
            TRACK_ENTRY => {
                let mut entry = Flags::default();
                walk(buffer, after_size, stop, tracks, &mut entry, depth + 1);
            }
            TRACK_NUMBER => number_of_this_track = Some(unsigned(buffer, after_size, stop)),
            TRACK_TYPE => {}
            FLAG_DEFAULT => track.default = Some(unsigned(buffer, after_size, stop) != 0),
            FLAG_FORCED => track.forced = Some(unsigned(buffer, after_size, stop) != 0),
            FLAG_HEARING_IMPAIRED => {
                track.hearing_impaired = Some(unsigned(buffer, after_size, stop) != 0)
            }
            FLAG_VISUAL_IMPAIRED => {
                track.visual_impaired = Some(unsigned(buffer, after_size, stop) != 0)
            }
            FLAG_ORIGINAL => track.original = Some(unsigned(buffer, after_size, stop) != 0),
            FLAG_COMMENTARY => track.commentary = Some(unsigned(buffer, after_size, stop) != 0),
            _ => {}
        }
        at = stop;
    }
    if let Some(number) = number_of_this_track {
        tracks.insert(number, *track);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One element: its id bytes as written, then a one-byte size, then the
    /// payload. A size below 128 fits in one byte with the marker bit set,
    /// which every element here is well inside.
    fn element(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.push(0x80 | payload.len() as u8);
        out.extend_from_slice(payload);
        out
    }

    fn file(tracks: &[u8]) -> Vec<u8> {
        let mut out = element(&[0x1A, 0x45, 0xDF, 0xA3], &[]);
        out.extend_from_slice(&[0x18, 0x53, 0x80, 0x67]);
        // The unknown size a streamed Segment declares, which is the shape
        // that ran a first attempt at this straight off the end of the buffer.
        out.push(0xFF);
        out.extend_from_slice(&element(&[0x16, 0x54, 0xAE, 0x6B], tracks));
        out
    }

    #[test]
    fn reads_the_flags_a_track_states() {
        let entry = [
            element(&[0xD7], &[3]),
            element(&[0x55, 0xAA], &[1]),
            element(&[0x88], &[0]),
            element(&[0x55, 0xAC], &[1]),
        ]
        .concat();
        let bytes = file(&element(&[0xAE], &entry));

        let mut tracks = HashMap::new();
        walk(
            &bytes,
            0,
            bytes.len(),
            &mut tracks,
            &mut Flags::default(),
            0,
        );

        let track = tracks.get(&3).expect("track 3");
        assert_eq!(track.forced, Some(true));
        assert_eq!(track.default, Some(false));
        assert_eq!(track.visual_impaired, Some(true));
        // Never stated, and so unknown rather than false: the caller has to be
        // free to fall through to the sidecar and then to the track name.
        assert_eq!(track.hearing_impaired, None);
        assert_eq!(track.commentary, None);
    }

    /// Matroska gives `FlagDefault` a default value of one, so a writer omits
    /// it on the default track and writes an explicit zero on the rest. Absent
    /// therefore cannot be read as false, and this is the case that proves the
    /// three states are worth carrying.
    #[test]
    fn an_absent_flag_is_unknown_and_not_false() {
        let stated = element(
            &[0xAE],
            &[element(&[0xD7], &[1]), element(&[0x88], &[0])].concat(),
        );
        let silent = element(&[0xAE], &element(&[0xD7], &[2]));
        let bytes = file(&[stated, silent].concat());

        let mut tracks = HashMap::new();
        walk(
            &bytes,
            0,
            bytes.len(),
            &mut tracks,
            &mut Flags::default(),
            0,
        );

        assert_eq!(tracks.get(&1).unwrap().default, Some(false));
        assert_eq!(tracks.get(&2).unwrap().default, None);
    }

    /// Anything that is not Matroska costs one read and no walking, rather
    /// than an answer invented from whatever the bytes happened to be.
    #[test]
    fn a_file_that_is_not_matroska_says_nothing() {
        let root = std::env::temp_dir().join("tp-mkv-not");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("clip.mp4");
        std::fs::write(&path, b"   ftypmp42").unwrap();

        assert!(flags(&path).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A truncated header stops rather than reading past what is there.
    #[test]
    fn a_header_that_stops_early_stops_here_too() {
        let full = file(&element(&[0xAE], &element(&[0xD7], &[7])));
        for cut in 1..full.len() {
            let mut tracks = HashMap::new();
            walk(&full[..cut], 0, cut, &mut tracks, &mut Flags::default(), 0);
        }
    }
}
