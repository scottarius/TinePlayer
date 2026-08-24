//! Where a diagnostic goes so that somebody other than a developer can find
//! it.
//!
//! Every `log::info!` and `log::error!` writes twice: to standard error,
//! which is what a developer running from a shell has always read, and to a
//! file in the user's data directory, which is what everybody else can be
//! asked for. The second half is the reason this module exists. A Windows GUI
//! build only attaches to a console when it was started from one, a macOS
//! `.app` has no terminal behind it at all, and a Linux desktop entry sends
//! the output to a journal nobody reads - so before this, the eighty-odd
//! diagnostics in this source reached exactly one person, on one machine, and
//! every report from anyone else was "it froze" with nothing to attach.
//!
//! **The `log` facade rather than macros of our own, and that is not
//! bookkeeping.** `gilrs`, `rustls` and `tungstenite` all write through it
//! already. With no backend installed those calls compile to nothing at
//! runtime, so every word any of them has ever said has gone nowhere -
//! including a TLS handshake failing, which is the one thing that would
//! explain a Jellyfin connection refusing to come up. Installing [`Logger`]
//! behind the facade is what turns that back on, and it costs no crate that
//! was not already being compiled. Their lines are labelled with which crate
//! said them; ours are not, so an unlabelled line is always TinePlayer's.
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

    // The facade, so that this is the one place anything writes a diagnostic -
    // ours and our dependencies' alike. See the module comment.
    log::set_logger(&SINK).ok();
    log::set_max_level(level());

    // The date is here rather than on every line: one file is one run, so
    // repeating it eighty times would be noise. The lines carry the time.
    let today = glib::DateTime::now_local()
        .and_then(|now| now.format("%Y-%m-%d"))
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "unknown date".to_string());
    log::info!(
        "TinePlayer {} starting on {} {}, {today}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );

    take_panics();
}

/// How much to keep, from `TINEPLAYER_LOG`.
///
/// `info` by default, which is this application's own account of itself plus
/// anything a dependency thinks is worth a warning. Above that the volume
/// stops being useful in a report: `rustls` at `debug` describes every
/// handshake, and a log nobody can read is the same as no log.
///
/// `TINEPLAYER_LOG=debug` raises it, which is what to ask for when a report
/// needs the TLS or gamepad detail - the same shape as `TINEPLAYER_TRACE_AUDIO`
/// for the routing, and off for the same reason.
fn level() -> log::LevelFilter {
    match std::env::var("TINEPLAYER_LOG").as_deref() {
        Ok("trace") => log::LevelFilter::Trace,
        Ok("debug") => log::LevelFilter::Debug,
        Ok("warn") => log::LevelFilter::Warn,
        Ok("error") => log::LevelFilter::Error,
        Ok("off") => log::LevelFilter::Off,
        _ => log::LevelFilter::Info,
    }
}

/// The backend behind the facade.
///
/// Ours rather than one of the ready-made ones, for three reasons that all
/// come back to this not being an ordinary application log. The redaction
/// below has to happen in the write path and no backend does it. Rotating per
/// *run* and keeping three is not what any of them offer - they rotate by size
/// or by day, neither of which lines up with "the launch where it went wrong".
/// And every dependency in this project is justified by the paragraph above
/// it; a crate earning its place by doing something already written and tested
/// in eighty lines would not survive that.
struct Logger;
static SINK: Logger = Logger;

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        // The facade has already applied `set_max_level`, which is the only
        // filter there is.
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        // `%H:%M:%S` and the milliseconds appended by hand. GLib's formatter
        // has no `%.3f` - it answers with an error rather than ignoring it,
        // which is a blank timestamp on every line and exactly the kind of
        // thing that is only visible by reading the file afterwards.
        let at = glib::DateTime::now_local()
            .and_then(|now| {
                let millis = now.microsecond() / 1000;
                now.format("%H:%M:%S")
                    .map(|text| format!("{text}.{millis:03}"))
            })
            .unwrap_or_else(|_| "--:--:--.---".to_string());

        // A dependency's messages are labelled with which one, because
        // "rustls" against a failed connection is most of the diagnosis. Our
        // own are not: every unlabelled line is TinePlayer's.
        let target = record.target();
        let from = match target.starts_with(env!("CARGO_CRATE_NAME")) {
            true => String::new(),
            false => format!("{} ", target.split("::").next().unwrap_or(target)),
        };

        write(&format!(
            "{at} [{}] {from}{}",
            match record.level() {
                log::Level::Error => "error",
                log::Level::Warn => "warn",
                log::Level::Info => "info",
                log::Level::Debug => "debug",
                log::Level::Trace => "trace",
            },
            record.args()
        ));
    }

    fn flush(&self) {}
}

/// What this copy is running against, once there is something to ask.
///
/// Separate from [`start`] because GStreamer has to be initialised before it
/// will say what version it is, and that happens well after the first line is
/// written - deliberately, since a failure during initialisation is exactly
/// the kind that has to be logged before it happens.
///
/// This is most of what a report needs and none of it can be guessed from the
/// outside. "Which GStreamer" in particular decides which plugins exist, and
/// almost every question about a file that will not play is really a question
/// about that.
pub fn environment() {
    log::info!("{}", gstreamer::version_string());
    log::info!(
        "GTK {}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version()
    );
    if let Some(dir) = crate::config::log_dir() {
        log::info!("Data folder {}", dir.display());
    }
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
fn write(line: &str) {
    let line = hide_public_hosts(&anonymize(&redact(line)));
    eprintln!("{line}");
    to_file(&line);
}

/// Replaces the host of any address that is not on a private network.
///
/// **A LAN address gives nothing away and answers real questions.**
/// `http://192.168.3.2:8096` says the scheme, the port and that the server is
/// on the same network - which is most of diagnosing a connection - while
/// telling a reader nothing they could not guess, since every home network
/// uses the same handful of ranges.
///
/// A public one is different in kind. Somebody running Jellyfin on a domain or
/// a routable address, pasted into a public issue that is indexed for ever,
/// has advertised a server and named it. That is worth more to whoever finds
/// it than it is to whoever is reading the report.
///
/// So the split is by reachability rather than by caution applied evenly. The
/// scheme and port survive either way, because "it was https on 8920" is the
/// part that matters and the host is the part that does not.
///
/// A bare name - `hoth`, `jellyfin.local` - is replaced too. It cannot be
/// resolved from outside and is therefore not an invitation, but it is chosen
/// by a person and often after themselves, and knowing it was a name rather
/// than an address is all the diagnosis needs.
fn hide_public_hosts(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while let Some(at) = rest.find("://") {
        let scheme_len = rest[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        let start = at - scheme_len;
        let after = at + 3;

        // The authority runs to the next `/`, or to whatever ends the address.
        let end = rest[after..]
            .find(|c: char| c == '/' || c.is_whitespace())
            .map(|offset| after + offset)
            .unwrap_or(rest.len());
        let authority = &rest[after..end];
        let (host, port) = match authority.rsplit_once(':') {
            // Only a port if it is one - an IPv6 literal is full of colons.
            Some((host, tail)) if tail.chars().all(|c| c.is_ascii_digit()) => (host, Some(tail)),
            _ => (authority, None),
        };

        out.push_str(&rest[..start]);
        out.push_str(&rest[start..after]);
        match is_private(host) {
            true => out.push_str(authority),
            false => {
                out.push_str("<server>");
                if let Some(port) = port {
                    out.push(':');
                    out.push_str(port);
                }
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Whether an address is one that cannot be reached from outside the network
/// it is on. Anything that is not plainly one of those is treated as public,
/// which is the safe direction to be wrong in.
fn is_private(host: &str) -> bool {
    // An empty authority is `file:///path`, which names no host at all - and
    // local files are most of what this ever prints, so getting this wrong
    // rewrote every one of them to `file://<server>/path`. Caught by the test
    // below rather than by reading, which is the whole reason it is there.
    if host.is_empty() {
        return true;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        // No stable `is_unique_local` on the version this builds against, so
        // the prefixes are read directly: `fc00::/7` and `fe80::/10`.
        Ok(std::net::IpAddr::V6(v6)) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
        // A name, not an address. `localhost` is the one that is certainly
        // this machine; everything else is replaced - see the note above.
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

/// The home directory, in both spellings a path can reach the log in.
///
/// Worked out once: it cannot change while the process runs, and this is
/// consulted on every line.
fn home_forms() -> &'static [String] {
    static FORMS: OnceLock<Vec<String>> = OnceLock::new();
    FORMS.get_or_init(|| {
        let native = glib::home_dir().to_string_lossy().to_string();
        if native.is_empty() {
            return Vec::new();
        }
        // A URI spells it with forward slashes - `file:///C:/Users/Ada/...` -
        // and `Path::display` with the platform's own. Both reach here.
        let slashed = native.replace('\\', "/");
        match slashed == native {
            true => vec![native],
            false => vec![native, slashed],
        }
    })
}

/// Replaces the home directory with `~` wherever it appears.
///
/// **The account name is in it, and it is usually a real one.** Nearly every
/// path this logs sits underneath it - the data folder, a video, a subtitle
/// beside it, Kodi's userdata - so without this a log pasted into a public
/// issue says who wrote it, repeatedly, in a tracker that is indexed for ever.
///
/// Here rather than at each call site, for the reason `redact` is: a site
/// added later cannot forget a rule it never has to know about. It is also the
/// only place that catches a path arriving inside somebody else's message,
/// which is where most of them come from - a `std::io::Error` names the file
/// it failed on, and nothing in this crate formats that.
///
/// Case-sensitively, even on Windows where paths are not. Matching loosely
/// would mean lowercasing the line to compare and then rebuilding it, and the
/// forms that occur are built from `glib::home_dir` and from URIs made out of
/// it, which agree. A path somebody typed in another case keeps its directory,
/// which is a miss rather than a leak of anything else on the line.
fn anonymize(line: &str) -> String {
    let mut out = line.to_string();
    for form in home_forms() {
        if out.contains(form.as_str()) {
            out = out.replace(form.as_str(), "~");
        }
    }
    out
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

    /// The account name is in the home directory, and the home directory is
    /// in nearly every path this logs.
    #[test]
    fn the_home_directory_becomes_a_tilde() {
        let home = glib::home_dir().to_string_lossy().to_string();
        assert!(!home.is_empty(), "the test needs a home directory to hide");

        let line = anonymize(&format!("Couldn't read {home}/Films/Film.mkv: no"));
        assert!(!line.contains(&home), "{line}");
        assert!(line.contains("~/Films/Film.mkv"), "{line}");
    }

    /// A URI spells the same directory with forward slashes, and that form
    /// has to be caught too or every `file://` line keeps the account name.
    #[test]
    fn the_uri_spelling_of_home_is_caught_too() {
        let home = glib::home_dir().to_string_lossy().to_string();
        let slashed = home.replace('\\', "/");
        let line = anonymize(&format!("Opened file:///{slashed}/Films/Film.mkv"));
        assert!(!line.contains(&slashed), "{line}");
    }

    /// A LAN address stays: it answers real questions and gives nothing away.
    #[test]
    fn a_private_address_is_kept_whole() {
        let line = "Jellyfin: connecting to ws://192.168.3.2:8096/socket";
        assert_eq!(hide_public_hosts(line), line);
        assert_eq!(
            hide_public_hosts("http://10.0.0.5:8096"),
            "http://10.0.0.5:8096"
        );
        assert_eq!(
            hide_public_hosts("http://172.16.4.1:8096"),
            "http://172.16.4.1:8096"
        );
        assert_eq!(
            hide_public_hosts("http://localhost:8096"),
            "http://localhost:8096"
        );
    }

    /// A server on a domain, pasted into a public issue, is an advertisement.
    /// The scheme and port are what the diagnosis needed anyway.
    #[test]
    fn a_public_host_is_replaced_but_the_port_survives() {
        assert_eq!(
            hide_public_hosts("Jellyfin: connecting to wss://jellyfin.example.com:8920/socket"),
            "Jellyfin: connecting to wss://<server>:8920/socket"
        );
        assert_eq!(
            hide_public_hosts("http://203.0.113.7:8096"),
            "http://<server>:8096"
        );
    }

    /// 172.32 is outside the private range, and looks enough like 172.16 that
    /// getting it wrong would be easy and invisible.
    #[test]
    fn an_address_just_outside_the_private_range_is_public() {
        assert_eq!(
            hide_public_hosts("http://172.32.0.1:8096"),
            "http://<server>:8096"
        );
    }

    /// A name cannot be resolved from outside, but it is chosen by a person
    /// and often after one.
    #[test]
    fn a_bare_name_is_replaced() {
        assert_eq!(
            hide_public_hosts("http://hoth:8096"),
            "http://<server>:8096"
        );
    }

    /// The local files this mostly prints have no host to judge.
    #[test]
    fn a_file_uri_is_left_alone() {
        let line = "Couldn't read file:///srv/media/Film.mkv: no such file";
        assert_eq!(hide_public_hosts(line), line);
    }

    #[test]
    fn a_line_with_no_home_in_it_is_untouched() {
        let line = "Missing GStreamer element \"tee\". Check the install.";
        assert_eq!(anonymize(line), line);
    }

    #[test]
    fn a_url_at_the_end_of_a_line_is_cut() {
        assert_eq!(
            redact("stream: http://hoth:8096/x?api_key=SECRET"),
            "stream: http://hoth:8096/x?<redacted>"
        );
    }
}
