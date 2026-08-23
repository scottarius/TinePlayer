//! Where a diagnostic goes so that somebody other than a developer can find
//! it.
//!
//! Every `log!` in this application writes twice: to standard error, which is
//! what a developer running from a shell has always read, and to a file in the
//! user's data directory, which is what everybody else can be asked for. The
//! second half is the reason this module exists. A Windows GUI build only
//! attaches to a console when it was started from one, a macOS `.app` has no
//! terminal behind it at all, and a Linux desktop entry sends the output to a
//! journal nobody reads - so before this, the eighty-odd diagnostics in this
//! source reached exactly one person, on one machine, and every report from
//! anyone else was "it froze" with nothing to attach.
//!
//! **The token is why this is a boundary and not a tee.** A Jellyfin stream
//! URI carries `?api_key=` and that is a bearer credential: anything holding
//! it can read and stream the library as that viewer. Three call sites print a
//! source URI when it will not open, which was harmless while the destination
//! was a console nobody reads, and is not harmless at all once the
//! destination is the file people attach to bug reports - which is precisely
//! the failure `crate::jellyfin` opens by explaining it avoided for
//! `config.yaml`. So redaction happens here, on the formatted line, rather
//! than at each call site: a site added later cannot forget a rule it never
//! has to know about.
//!
//! **Three runs are kept**, rotated on startup rather than by size or age. A
//! fault worth reporting is usually noticed after the fact - the film froze,
//! it was closed, and only then did anyone think to look - and one file per
//! run means the interesting one has not already been overwritten by the two
//! launches it took to ask.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

/// How much one run may write before it stops.
///
/// A ceiling rather than an estimate. Every diagnostic here is written when
/// something unusual happens, so an ordinary run produces a few lines and a
/// megabyte is unreachable - but a loop that fails once per frame is not
/// hypothetical, and this runs on a Raspberry Pi whose disk is a memory card
/// somebody else has to replace. Past this the file says so and stops.
const MAX_BYTES: u64 = 1 << 20;

/// How many runs are kept, this one included.
const KEEP: usize = 3;

/// The open file for this run, and what has been written to it.
///
/// A `Mutex` because diagnostics come from every thread in the application -
/// the Jellyfin socket, the Kodi reporter, the probe and alignment workers -
/// and interleaved half-lines would be worse than none. `None` once the file
/// could not be opened or the ceiling above was reached, which is not an
/// error: standard error still gets everything either way.
static FILE: OnceLock<Mutex<Sink>> = OnceLock::new();

#[derive(Default)]
struct Sink {
    file: Option<std::fs::File>,
    written: u64,
}

/// `tineplayer.log` for this run, `tineplayer.1.log` for the one before it.
fn path(back: usize) -> Option<PathBuf> {
    let dir = crate::config::log_dir()?;
    Some(match back {
        0 => dir.join("tineplayer.log"),
        n => dir.join(format!("tineplayer.{n}.log")),
    })
}

/// Shuffles the previous runs down one and drops the oldest.
///
/// Failures are ignored throughout. A log that cannot be rotated is a log that
/// keeps the previous run's name, which is untidy and costs nothing; refusing
/// to start over it would be absurd.
fn rotate() {
    let Some(oldest) = path(KEEP - 1) else { return };
    let _ = std::fs::remove_file(&oldest);
    for back in (0..KEEP - 1).rev() {
        let (Some(from), Some(to)) = (path(back), path(back + 1)) else {
            return;
        };
        let _ = std::fs::rename(from, to);
    }
}

/// Opens this run's file and takes over reporting panics.
///
/// Called once, as early in `main` as the data directory can be found -
/// before anything that might have something to say. Everything here fails
/// quietly: a machine where the file cannot be opened still logs to standard
/// error, and a player that refused to start because it could not write a
/// diagnostic would be a worse fault than the one it was trying to record.
pub fn start() {
    rotate();

    let file = path(0).and_then(|path| {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .ok()
    });
    let _ = FILE.set(Mutex::new(Sink { file, written: 0 }));

    write(&format!(
        "TinePlayer {} on {} starting",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
    ));

    take_panics();
}

/// Sends a panic to the log as well as to standard error.
///
/// This is most of the point of having a file at all. The interface holds its
/// state in a great many `RefCell`s, and the failure that class of design
/// actually produces - a `borrow_mut` while a `borrow` is live - is a panic
/// with a location in it, from a callback, on somebody else's machine. Without
/// this it is an application that "just closed".
///
/// The default hook is called as well rather than replaced, so a developer
/// watching a terminal sees exactly what they always did.
fn take_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // `force_capture` rather than `capture`, which respects RUST_BACKTRACE
        // and is therefore empty for every user this is written for.
        let trace = std::backtrace::Backtrace::force_capture();
        to_file(&format!("\npanic: {info}\n{trace}"));
        previous(info);
    }));
}

/// One diagnostic, to standard error and to the file.
///
/// Behind `log!`, which is what to call.
pub fn write(line: &str) {
    let line = redact(line);
    eprintln!("{line}");
    to_file(&line);
}

/// The file half, which the panic hook uses directly: its own line has already
/// been through `redact` and must not be printed to standard error twice.
fn to_file(line: &str) {
    let Some(sink) = FILE.get() else { return };
    let Ok(mut sink) = sink.lock() else { return };
    if sink.written >= MAX_BYTES {
        return;
    }
    // Both worked out before the file is borrowed: they read the same struct.
    let bytes = line.len() as u64 + 1;
    let full = sink.written + bytes > MAX_BYTES;

    let Some(file) = sink.file.as_mut() else {
        return;
    };

    // The write is its own step so the borrow of `file` ends before the
    // counter below is touched - they live in the same struct.
    let written = match full {
        true => {
            let _ = writeln!(file, "\n[log full - nothing further from this run]");
            None
        }
        false => writeln!(file, "{line}").ok().map(|()| bytes),
    };

    match written {
        Some(bytes) => sink.written += bytes,
        None if full => sink.written = MAX_BYTES,
        // The disk filled or the card was pulled. Standard error still works,
        // and there is nothing else to try.
        None => sink.file = None,
    }
}

/// Cuts the query string off any URL in a line.
///
/// A Jellyfin stream, subtitle and image URL all carry `?api_key=<token>`, and
/// the WebSocket address carries it too. Everything after the `?` goes rather
/// than the token alone: `mediaSourceId` and the item id are no use in a
/// diagnostic, and a rule that removes one named parameter is a rule that
/// misses the next one somebody adds.
///
/// What is kept is the part that answers the question a diagnostic is asking -
/// which host, which path, which item would not open. `file://` URIs have no
/// query string and are untouched, which matters because they are most of what
/// this ever prints.
fn redact(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while let Some(at) = rest.find("://") {
        // Back to the start of the scheme, which is the run of letters
        // immediately before the separator. Without this, "see http://x?y"
        // would keep "see htt" and lose the rest of the word.
        let scheme_len = rest[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        let start = at - scheme_len;

        // A URL ends at whitespace, or at the end of the line.
        let mut end = rest[at..]
            .find(char::is_whitespace)
            .map(|offset| at + offset)
            .unwrap_or(rest.len());

        // Then back off the punctuation that ended the clause rather than the
        // address. Every site that prints a URI writes "...{uri}: {error}", so
        // without this the colon sits inside the span and is swallowed along
        // with the query string it happens to follow - which reads as a
        // mangled sentence rather than a redacted one.
        while end > at
            && matches!(
                rest.as_bytes()[end - 1],
                b':' | b',' | b'.' | b';' | b')' | b'"' | b'\''
            )
        {
            end -= 1;
        }

        out.push_str(&rest[..start]);
        let url = &rest[start..end];
        // `file://` carries no credential and never has a query string, so a
        // `?` inside one is a character in somebody's filename - legal on
        // Linux - and cutting there would mangle a local path to no purpose.
        // Local files are also most of what this ever prints.
        let local = url.len() >= 7 && url[..7].eq_ignore_ascii_case("file://");
        match url.find('?') {
            Some(query) if !local => {
                out.push_str(&url[..query]);
                out.push_str("?<redacted>");
            }
            _ => out.push_str(url),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Writes a diagnostic to standard error and to this run's log file.
///
/// Takes the same arguments as `eprintln!` and replaces it everywhere a
/// message is meant for a person reading a report later - which is every
/// diagnostic in this application. `println!` is left alone: that is the
/// output of `--list-devices` and its neighbours, which is an answer to a
/// question rather than a report of something going wrong, and belongs on
/// standard output where it can be piped.
///
/// **Called as `crate::log!` rather than imported, which `tr!` is not.** A
/// `#[macro_export]` puts the name at the crate root, where this one would
/// collide with the module of the same name: `use crate::log;` is ambiguous
/// between the two, and renaming either to avoid it would be worse than
/// spelling out four extra characters at each call site.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(&::std::format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case this module exists for.
    #[test]
    fn a_stream_url_loses_its_token() {
        let line = redact(
            "Couldn't open http://hoth:8096/Videos/abc/stream.mkv?static=true&api_key=SECRET: no",
        );
        assert!(!line.contains("SECRET"), "{line}");
        assert!(
            line.contains("http://hoth:8096/Videos/abc/stream.mkv?<redacted>"),
            "{line}"
        );
        // The part that says which server and which item is what makes the
        // diagnostic worth keeping, so it has to survive.
        assert!(line.contains("hoth:8096"), "{line}");
        assert!(line.ends_with(": no"), "{line}");
    }

    #[test]
    fn the_socket_address_loses_its_token() {
        let line =
            redact("Jellyfin connection lost: ws://hoth:8096/socket?api_key=SECRET&deviceId=1");
        assert!(!line.contains("SECRET"), "{line}");
        assert!(line.contains("ws://hoth:8096/socket?<redacted>"), "{line}");
    }

    /// Most of what is ever printed, and it must come through untouched.
    #[test]
    fn a_local_file_is_untouched() {
        let line = "Couldn't read file:///home/scott/Films/Film (2019).mkv: no such file";
        assert_eq!(redact(line), line);
    }

    #[test]
    fn plain_text_is_untouched() {
        let line = "Missing GStreamer element \"tee\". Check the install.";
        assert_eq!(redact(line), line);
    }

    /// The scheme has to survive with the word in front of it, which a naive
    /// search for "://" gets wrong.
    #[test]
    fn the_word_before_a_url_survives() {
        assert_eq!(
            redact("asking http://a?b then https://c?d done"),
            "asking http://a?<redacted> then https://c?<redacted> done"
        );
    }

    /// A question mark that is not in a URL is not a query string.
    #[test]
    fn a_question_mark_outside_a_url_is_kept() {
        let line = "Could not form a URI for C:\\Films\\What Is It? (2005).mkv: bad path";
        assert_eq!(redact(line), line);
    }

    /// The shape every call site actually writes: `"...{uri}: {error}"`. The
    /// colon belongs to the sentence and has to survive the redaction of the
    /// query string it sits behind.
    #[test]
    fn the_punctuation_after_a_url_survives() {
        assert_eq!(
            redact("Couldn't open http://h/x?api_key=SECRET: no such file"),
            "Couldn't open http://h/x?<redacted>: no such file"
        );
    }

    /// A `?` is legal in a Linux filename and means nothing in a `file://`
    /// URI, so cutting there would mangle a local path for no gain.
    #[test]
    fn a_question_mark_in_a_local_path_is_kept() {
        let line = "Couldn't read file:///home/scott/WhatIsIt?.mkv: no such file";
        assert_eq!(redact(line), line);
    }

    #[test]
    fn a_url_at_the_end_of_a_line_is_cut() {
        assert_eq!(
            redact("stream: http://hoth:8096/x?api_key=SECRET"),
            "stream: http://hoth:8096/x?<redacted>"
        );
    }
}
