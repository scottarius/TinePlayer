//! Whether a newer TinePlayer has been released.
//!
//! Notify only. Nothing is downloaded and nothing is installed; the most this
//! does is say a version exists and offer to open the page. A player that
//! replaced itself while somebody was watching a film would be worse than one
//! that never checked.
//!
//! Every failure is silent. No route out, a proxy in the way, GitHub having a
//! bad afternoon: none of that is the viewer's problem, and a player that
//! complained about connectivity while playing a local file would be worse
//! than one that said nothing.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// `/latest` rather than the releases list, deliberately: it skips drafts and
/// prereleases, so the `-test` tags used to exercise the release pipeline can
/// never be offered to anybody.
const LATEST: &str = "https://api.github.com/repos/scottarius/TinePlayer/releases/latest";

/// GitHub answers 403 to a request without one.
const AGENT: &str = concat!(
    "TinePlayer/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/scottarius/TinePlayer)"
);

/// At most one check a day. The limit is 60 requests an hour per address for
/// an unauthenticated caller, which is far more than this needs - the reason
/// to be sparing is that nobody asked to talk to GitHub every time they watch
/// something.
const BETWEEN_CHECKS: u64 = 60 * 60 * 24;

/// Short enough that a machine with no route out is not kept waiting on one.
const TIMEOUT: u64 = 10;

/// What is remembered between runs.
///
/// State rather than settings, so it lives beside the resume positions rather
/// than in `config.yaml`: nobody edits this by hand, and a version somebody
/// has already been told about is not a preference.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// When the last check finished, in seconds since the epoch. Zero means
    /// never, which is what makes a first run check straight away.
    #[serde(default)]
    pub checked: u64,
    /// The newest version the last check found, whether or not it is newer
    /// than this build.
    #[serde(default)]
    pub latest: Option<String>,
    /// Where to send somebody who wants it.
    #[serde(default)]
    pub url: Option<String>,
    /// The version whose badge has been seen. Kept so that the mark on the
    /// settings button appears once per release rather than every launch.
    #[serde(default)]
    pub acknowledged: Option<String>,
}

pub fn load() -> State {
    std::fs::read_to_string(crate::config::updates_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(state: &State) {
    if let Ok(text) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(crate::config::updates_path(), text);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Whether enough time has passed to ask again.
pub fn due(state: &State) -> bool {
    now().saturating_sub(state.checked) >= BETWEEN_CHECKS
}

/// Asks GitHub what the newest release is. Blocking, so it belongs on a
/// thread of its own.
pub fn look() -> Option<(String, String)> {
    look_at(LATEST)
}

/// The part that talks, with the address given rather than assumed, so it can
/// be pointed at a repository that answers while ours is still private.
fn look_at(url: &str) -> Option<(String, String)> {
    let response = minreq::get(url)
        .with_header("User-Agent", AGENT)
        .with_header("Accept", "application/vnd.github+json")
        .with_timeout(TIMEOUT)
        .send()
        .ok()?;
    // 404 while the repository is private, and that is not a failure worth
    // reporting: it reads the same as there being nothing newer, and the day
    // the repository opens this starts working with no change here.
    if response.status_code != 200 {
        return None;
    }
    let body: serde_json::Value = response.json().ok()?;
    let tag = body.get("tag_name")?.as_str()?.to_string();
    let url = body
        .get("html_url")
        .and_then(|url| url.as_str())
        .unwrap_or("https://github.com/scottarius/TinePlayer/releases")
        .to_string();
    Some((tag, url))
}

/// A check, start to finish, for a thread to run. Returns the state to save.
pub fn check(previous: &State) -> State {
    let mut state = previous.clone();
    state.checked = now();
    if let Some((tag, url)) = look() {
        // A version that is no longer the newest clears an acknowledgement
        // made against it, so the next release is announced again.
        if state.latest.as_deref() != Some(tag.as_str()) {
            state.acknowledged = None;
        }
        state.latest = Some(tag);
        state.url = Some(url);
    }
    state
}

/// The three numbers in a version, ignoring anything after them.
///
/// Compared as numbers and never as text, or 0.10.0 sorts below 0.9.0.
fn parts(version: &str) -> (u32, u32, u32) {
    let mut fields = version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+', ' ']);
    let mut next = || {
        fields
            .next()
            .map(|field| field.trim_matches(|c: char| !c.is_ascii_digit()))
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(0)
    };
    (next(), next(), next())
}

/// The newer version, if the one found is ahead of the one running.
pub fn newer(state: &State) -> Option<(&str, &str)> {
    let latest = state.latest.as_deref()?;
    if parts(latest) <= parts(env!("CARGO_PKG_VERSION")) {
        return None;
    }
    let url = state
        .url
        .as_deref()
        .unwrap_or("https://github.com/scottarius/TinePlayer/releases");
    Some((latest, url))
}

/// Whether the settings button should carry a mark: a newer version exists
/// and nobody has looked at it yet.
pub fn unseen(state: &State) -> bool {
    match newer(state) {
        Some((latest, _)) => state.acknowledged.as_deref() != Some(latest),
        None => false,
    }
}

/// Records that the row naming the new version has been reached, which is
/// what takes the mark off the settings button. The mark stays on the row
/// itself, because the version is still there to be had.
pub fn acknowledge(state: &mut State) {
    if let Some((latest, _)) = newer(state) {
        let latest = latest.to_string();
        if state.acknowledged.as_deref() != Some(latest.as_str()) {
            state.acknowledged = Some(latest);
            save(state);
        }
    }
}

/// How the row reads when there is nothing newer.
pub const NOTHING_NEW: &str = "No new version";

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of parsing rather than comparing text.
    #[test]
    fn versions_compare_as_numbers() {
        assert!(parts("0.10.0") > parts("0.9.0"));
        assert!(parts("1.0.0") > parts("0.99.99"));
        assert_eq!(parts("v0.6.0"), (0, 6, 0));
        assert_eq!(parts("0.6"), (0, 6, 0));
    }

    /// Tags arrive with a `v`, and sometimes with something after the
    /// numbers. Neither should change which is newer.
    #[test]
    fn decoration_is_ignored() {
        assert_eq!(parts("v1.2.3"), parts("1.2.3"));
        assert_eq!(parts("1.2.3-beta"), (1, 2, 3));
        assert_eq!(parts("1.2.3+build7"), (1, 2, 3));
    }

    /// Nothing is offered unless it is actually ahead of what is running.
    #[test]
    fn only_newer_counts() {
        let same = State {
            latest: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..Default::default()
        };
        assert!(newer(&same).is_none());

        let ahead = State {
            latest: Some("99.0.0".to_string()),
            ..Default::default()
        };
        assert!(newer(&ahead).is_some());
        assert!(unseen(&ahead));
    }

    /// Acknowledging one version must not silence the next.
    #[test]
    fn a_newer_release_is_announced_again() {
        let mut state = State {
            latest: Some("99.0.0".to_string()),
            acknowledged: Some("99.0.0".to_string()),
            ..Default::default()
        };
        assert!(!unseen(&state));
        state.latest = Some("99.1.0".to_string());
        assert!(unseen(&state));
    }

    /// The request and the parsing, against a repository that actually
    /// answers.
    ///
    /// Ignored by default, so neither CI nor an ordinary `cargo test` reaches
    /// the network. Run it by hand after touching anything in `look_at`:
    ///
    ///     cargo test -- --ignored --nocapture
    ///
    /// Our own repository is private until release, and the API answers 404
    /// for those without a token, so there is nothing else to check this
    /// against until then. GitHub's own CLI is used because it is public,
    /// tags releases the same way, and will not stop existing.
    #[test]
    #[ignore = "reaches the network"]
    fn a_real_release_can_be_read() {
        let found = look_at("https://api.github.com/repos/cli/cli/releases/latest");
        let Some((tag, url)) = found else {
            panic!("no answer from the GitHub API - offline, or the shape changed");
        };
        println!("tag = {tag}\nurl = {url}");
        assert!(parts(&tag) > (0, 0, 0), "tag {tag:?} parsed as nothing");
        assert!(url.starts_with("https://github.com/"), "url was {url:?}");
    }

    /// A first run has never checked, so it checks.
    #[test]
    fn never_checked_is_due() {
        assert!(due(&State::default()));
        let just_now = State {
            checked: now(),
            ..Default::default()
        };
        assert!(!due(&just_now));
    }
}
