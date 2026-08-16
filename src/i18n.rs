//! Translating the interface.
//!
//! # How a translation reaches the screen
//!
//! Translators work in `.po` files, which is the format Weblate hosts free for
//! open source and the one people who translate software already know. Those
//! files live in `po/`, are listed in `po/LINGUAS`, and are compiled into the
//! binary by `build.rs` - not installed beside it.
//!
//! **Compiled in rather than installed**, because this project has nowhere to
//! install them to. There is no `share/locale` on Windows or macOS, the
//! portable zip has to work unpacked on a stick, and every other asset here -
//! icons, fonts, the branding, the third-party notices - is already an
//! `include_bytes!`. Adding a per-platform search path for one asset type
//! would be a fourth packaging problem in exchange for nothing.
//!
//! The cost of that is real and is paid for separately: a translator with no
//! Rust toolchain cannot rebuild to see their work. So `TINEPLAYER_PO` loads a
//! `.po` straight off disk at startup, in release builds as much as debug.
//! Hand somebody an ordinary release, and they can point it at the file they
//! are editing and watch the interface change. That is the whole reason polib
//! is a runtime dependency and not only a build one.
//!
//! # Using it
//!
//! ```ignore
//! tr!("Close the player")
//! tr!("Resume at {time}", time = human_time(position))
//! trc!("audio track", "None")          // a context, where English is ambiguous
//! trn!("{n} track", "{n} tracks", count, n = count)
//! ```
//!
//! The macros take literals so that `packaging/extract-strings.py` can find
//! them, and interpolate by name afterwards rather than through `format!` -
//! which cannot be used here, since it needs its template at compile time and
//! the whole point is that the template arrives at run time. Named holes
//! rather than positional ones because a translator has to be free to reorder
//! them, and `{time}` says what it is where `{0}` does not.
//!
//! # What is deliberately not translated
//!
//! **The command line.** Its help lives in clap's `///` doc comments, which no
//! extractor can see and which clap wants as `&'static str` anyway. It is read
//! by people already at a terminal, and `docs/` is English regardless.
//!
//! **The language list in `languages.rs`.** Those fifty entries already carry
//! each language's own name beside the English one, which is what somebody
//! scanning for their language actually reads. Translating them would be a
//! seventh of the whole catalog for almost nothing.
//!
//! **Diagnostics on stderr.** They are for bug reports, and a bug report is
//! more useful in the language the issue tracker is written in.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::RwLock;

/// One compiled-in translation. Built by `build.rs`; see `catalogs.rs` in the
/// build directory for what that comes out as.
pub struct Catalog {
    /// The locale this is for, spelled as `po/LINGUAS` spells it.
    pub code: &'static str,
    /// Ordinary messages, sorted by key so the lookup can binary search. The
    /// key is the msgid, or `context\u{4}msgid` where there is a context.
    pub singular: &'static [(&'static str, &'static str)],
    /// Messages with plural forms, sorted the same way. The key is the
    /// singular msgid, which is what gettext keys a plural message by.
    pub plural: &'static [(&'static str, &'static [&'static str])],
    /// How many forms this language has, from the `Plural-Forms` header.
    pub nplurals: usize,
    /// Which form a given count takes. Rendered into Rust from that same
    /// header - see `src/plural_rule.rs`.
    pub plural_index: fn(u64) -> usize,
}

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// The code that asks for the padded, accented stand-in described under
/// [`pseudo`]. Not a language, and not offered outside a debug build, but
/// honored wherever it is set so that it can be turned on in a release when
/// somebody is checking a layout.
pub const PSEUDO: &str = "x-pseudo";

/// The language the source is written in. Its strings are the msgids, so it
/// needs no catalog and never gets one.
const SOURCE_LANGUAGE: &str = "en";

/// Read instead of the config's setting when it is set, so that a language can
/// be tried without editing a file - which matters most on the machine where
/// the file is hardest to reach, a television.
const LANGUAGE_ENV: &str = "TINEPLAYER_LANG";

/// A `.po` to read at startup in place of anything compiled in.
const CATALOG_ENV: &str = "TINEPLAYER_PO";

/// What is in force. Set at startup and again whenever the setting is changed.
enum Active {
    /// English, or a preference nothing answered to. The msgid shows through,
    /// which is a whole English interface rather than a broken one.
    Untranslated,
    Compiled(&'static Catalog),
    /// Read from disk at startup, for somebody translating.
    Loaded(Loaded),
    Pseudo,
}

/// A catalog read from a `.po` at run time rather than compiled in.
struct Loaded {
    code: String,
    singular: BTreeMap<String, String>,
    plural: BTreeMap<String, Vec<String>>,
    nplurals: usize,
    rule: crate::plural_rule::Expr,
}

/// What is in force, behind a lock so the setting can be changed while the
/// application is running.
///
/// **It holds a `&'static Active`, and every one but the first is leaked.**
/// That is what lets [`translate`] hand back `Cow::Borrowed` pointing straight
/// into a catalog: a borrow can only outlive the lock if what it points at
/// lives forever. The alternative is returning an owned `String` from every
/// lookup, which is an allocation for every label in the interface to spare a
/// leak of a few hundred bytes.
///
/// And it really is a few hundred bytes. What leaks is one `Active` per
/// language *change*, which is a person choosing a row under Settings - not
/// per lookup, per string or per rebuild. A session in which somebody changed
/// their mind fifty times would leak less than one of the icons this binary
/// already carries.
static ACTIVE: RwLock<&'static Active> = RwLock::new(&UNTRANSLATED);

/// Where [`ACTIVE`] points before anything has resolved a language, which is
/// every unit test and any code path that runs before `init`. Static rather
/// than leaked, since it is needed to construct the lock itself.
static UNTRANSLATED: Active = Active::Untranslated;

fn active() -> &'static Active {
    // A poisoned lock is not a reason to stop drawing text. Nothing here can
    // leave the value half-written - it is one pointer - so the worst case of
    // reading through the poison is the language somebody already had.
    *ACTIVE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Puts a resolved language in force, leaking it so its strings can be
/// borrowed for the rest of the run. See [`ACTIVE`].
fn install(resolved: Active) {
    let leaked: &'static Active = Box::leak(Box::new(resolved));
    match ACTIVE.write() {
        Ok(mut active) => *active = leaked,
        Err(poisoned) => *poisoned.into_inner() = leaked,
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// Works out which language the interface is in, at startup.
///
/// `preference` is the config's `language` setting: `None` means "whatever
/// this machine is set to". Call before any window is built, and before GTK is
/// asked for a text direction.
///
/// Never fails. A catalog that cannot be read, a language nothing answers to,
/// or no catalogs at all, all land on English and say so on stderr - an
/// interface in the wrong language is a bad evening, and refusing to start is
/// a worse one.
pub fn init(preference: Option<&str>) {
    install(resolve(preference));
}

/// Changes the language while the application is running, for the row under
/// Settings.
///
/// **Only the screens built after this call are in the new language**, which
/// in practice is all of them: the caller in `app.rs` rebuilds the settings
/// pane immediately, and the playback controls are built fresh for each film.
/// Anything held open across the change keeps the words it was built with.
///
/// `TINEPLAYER_PO` and `TINEPLAYER_LANG` still win, as they do at startup.
/// Somebody who started the application pointed at a particular catalog meant
/// it, and a settings row quietly overriding the thing they are in the middle
/// of testing would be the worse surprise. The choice is still saved.
pub fn set_language(preference: Option<&str>) {
    install(resolve(preference));
}

fn resolve(preference: Option<&str>) -> Active {
    // A file named on the command line beats everything, including the
    // language it declares itself to be for. Somebody who says "show me this
    // file" means this file.
    if let Some(path) = std::env::var_os(CATALOG_ENV) {
        let path = std::path::PathBuf::from(path);
        match load(&path) {
            Ok(loaded) => {
                eprintln!(
                    "Interface language: {}, read from {}",
                    loaded.code,
                    path.display()
                );
                return Active::Loaded(loaded);
            }
            Err(e) => {
                eprintln!("{CATALOG_ENV} could not be used: {e}");
                eprintln!("Carrying on in English.");
                return Active::Untranslated;
            }
        }
    }

    let environment = std::env::var(LANGUAGE_ENV).ok();
    let system = sys_locale::get_locale();
    let wanted = environment
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(preference)
        .or(system.as_deref());

    match choose(wanted, CATALOGS) {
        Choice::Untranslated => Active::Untranslated,
        Choice::Pseudo => Active::Pseudo,
        Choice::Catalog(catalog) => Active::Compiled(catalog),
    }
}

/// Which catalog a wanted locale gets, if any.
///
/// Separated from [`resolve`] so it can be tested without an environment or a
/// machine locale, both of which are exactly the things that make this hard to
/// be sure about by reading.
enum Choice<'a> {
    Untranslated,
    Catalog(&'a Catalog),
    Pseudo,
}

fn choose<'a>(wanted: Option<&str>, catalogs: &'a [Catalog]) -> Choice<'a> {
    let Some(wanted) = wanted.map(normalize).filter(|w| !w.is_empty()) else {
        return Choice::Untranslated;
    };

    if wanted == PSEUDO {
        return Choice::Pseudo;
    }

    // English is the source language, so asking for it means asking for no
    // catalog at all - including `en-GB` and `en-AU`, which differ from the
    // source in spelling this project has already settled on US English.
    if primary(&wanted) == SOURCE_LANGUAGE {
        return Choice::Untranslated;
    }

    // `pt-br` before `pt`: a regional catalog is a deliberate thing to have
    // made, so an exact request for one should not be answered with its parent.
    if let Some(catalog) = catalogs.iter().find(|c| normalize(c.code) == wanted) {
        return Choice::Catalog(catalog);
    }

    // Then anything in the same language. This is what makes `de-AT` land on
    // `de` and `pt-PT` land on `pt-BR` when that is the only Portuguese there
    // is - imperfect, and much better than English.
    let language = primary(&wanted);
    match catalogs
        .iter()
        .find(|c| primary(&normalize(c.code)) == language)
    {
        Some(catalog) => Choice::Catalog(catalog),
        None => Choice::Untranslated,
    }
}

/// A locale tag in one spelling: lowercase, with `-` between the parts.
///
/// Locales arrive spelled three ways - `pt_BR` from a POSIX environment,
/// `pt-BR` from Windows and macOS, `pt_br` from somebody typing into the
/// config - and all three mean the same thing.
fn normalize(tag: &str) -> String {
    tag.trim().to_lowercase().replace('_', "-")
}

/// The language part of a normalized tag: `pt` from `pt-br`.
fn primary(tag: &str) -> &str {
    tag.split('-').next().unwrap_or(tag)
}

/// Reads a `.po` from disk, for `TINEPLAYER_PO`.
fn load(path: &std::path::Path) -> Result<Loaded, String> {
    let catalog = polib::po_file::parse(path)
        .map_err(|e| format!("{} is not readable: {e}", path.display()))?;

    let rules = &catalog.metadata.plural_rules;
    let rule = crate::plural_rule::parse(&rules.expr)
        .map_err(|e| format!("its Plural-Forms rule cannot be used: {e}"))?;

    let mut singular = BTreeMap::new();
    let mut plural = BTreeMap::new();

    for message in catalog.messages() {
        if message.is_fuzzy() || !message.is_translated() {
            continue;
        }
        let key = match message.msgctxt() {
            Some(context) => format!("{context}\u{4}{}", message.msgid()),
            None => message.msgid().to_string(),
        };
        match message.is_plural() {
            // A short catalog is not an error here the way it is in the build:
            // this file is being edited as it is read, and refusing to show
            // somebody their own half-finished work would defeat the point.
            true => {
                if let Ok(forms) = message.msgstr_plural()
                    && forms.len() == rules.nplurals
                {
                    plural.insert(key, forms.clone());
                }
            }
            false => {
                if let Ok(translated) = message.msgstr() {
                    singular.insert(key, translated.to_string());
                }
            }
        }
    }

    let code = match catalog.metadata.language.trim().is_empty() {
        true => path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        false => catalog.metadata.language.clone(),
    };

    Ok(Loaded {
        code,
        singular,
        plural,
        nplurals: rules.nplurals,
        rule,
    })
}

// ---------------------------------------------------------------------------
// Looking a message up
// ---------------------------------------------------------------------------

/// The translation of `msgid`, or `msgid` itself where there is none.
///
/// Behind `tr!`, which is what to call. This is public because the macro
/// expands to it.
pub fn translate(msgid: &'static str) -> Cow<'static, str> {
    match active() {
        Active::Untranslated => Cow::Borrowed(msgid),
        Active::Pseudo => Cow::Owned(pseudo(msgid)),
        Active::Compiled(catalog) => match catalog.singular.binary_search_by_key(&msgid, |e| e.0) {
            Ok(at) => Cow::Borrowed(catalog.singular[at].1),
            Err(_) => Cow::Borrowed(msgid),
        },
        Active::Loaded(loaded) => match loaded.singular.get(msgid) {
            Some(translated) => Cow::Borrowed(translated.as_str()),
            None => Cow::Borrowed(msgid),
        },
    }
}

/// The translation of `msgid` under `context`.
///
/// A context is for the short strings where English is ambiguous and other
/// languages are not: "None" as an audio track and "None" as a subtitle
/// preference are one word here and two in German. Behind `trc!`.
pub fn translate_in_context(context: &'static str, msgid: &'static str) -> Cow<'static, str> {
    match active() {
        Active::Untranslated => Cow::Borrowed(msgid),
        Active::Pseudo => Cow::Owned(pseudo(msgid)),
        Active::Compiled(catalog) => {
            match catalog
                .singular
                .binary_search_by(|entry| compare_key(entry.0, context, msgid))
            {
                Ok(at) => Cow::Borrowed(catalog.singular[at].1),
                Err(_) => Cow::Borrowed(msgid),
            }
        }
        Active::Loaded(loaded) => match loaded.singular.get(&format!("{context}\u{4}{msgid}")) {
            Some(translated) => Cow::Borrowed(translated.as_str()),
            None => Cow::Borrowed(msgid),
        },
    }
}

/// The form of a plural message that `count` takes.
///
/// `one` and `many` are the English singular and plural, which are also the
/// msgid and msgid_plural. Behind `trn!`.
pub fn translate_plural(one: &'static str, many: &'static str, count: u64) -> Cow<'static, str> {
    // What English does when there is nothing better: one of a thing, or some
    // other number of them.
    let untranslated = || match count == 1 {
        true => Cow::Borrowed(one),
        false => Cow::Borrowed(many),
    };

    match active() {
        Active::Untranslated => untranslated(),
        Active::Pseudo => Cow::Owned(pseudo(match count == 1 {
            true => one,
            false => many,
        })),
        Active::Compiled(catalog) => match catalog.plural.binary_search_by_key(&one, |e| e.0) {
            Ok(at) => {
                let forms = catalog.plural[at].1;
                let which = (catalog.plural_index)(count).min(catalog.nplurals - 1);
                match forms.get(which) {
                    Some(form) => Cow::Borrowed(*form),
                    None => untranslated(),
                }
            }
            Err(_) => untranslated(),
        },
        Active::Loaded(loaded) => match loaded.plural.get(one) {
            Some(forms) => {
                let which = (loaded.rule.eval(count) as usize).min(loaded.nplurals - 1);
                match forms.get(which) {
                    Some(form) => Cow::Borrowed(form.as_str()),
                    None => untranslated(),
                }
            }
            None => untranslated(),
        },
    }
}

/// Orders a stored `context\u{4}msgid` key against a context and msgid held
/// apart, so a contextual lookup needs no string built to search with.
fn compare_key(stored: &str, context: &str, msgid: &str) -> std::cmp::Ordering {
    let wanted = context
        .chars()
        .chain(std::iter::once('\u{4}'))
        .chain(msgid.chars());
    stored.chars().cmp(wanted)
}

// ---------------------------------------------------------------------------
// Filling in the holes
// ---------------------------------------------------------------------------

/// Replaces `{name}` in a translated string with what was passed for `name`.
///
/// `format!` cannot do this: it resolves its template at compile time, and the
/// entire point here is that the template arrives at run time from a catalog.
/// So this is a small substitution of its own, with `format!`'s escapes -
/// `{{` and `}}` - because a translator who has seen one Rust string will
/// expect them.
///
/// **A hole nothing was passed for is left standing rather than emptied.**
/// `{tiem}` on screen is a typo somebody can see and report; a gap where the
/// time should be is a mystery. `tests::placeholders_match` catches these
/// before they ship, and this is what happens to the ones it cannot.
pub fn fill(template: Cow<'static, str>, values: &[(&str, String)]) -> Cow<'static, str> {
    if !template.contains('{') {
        return template;
    }

    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template.as_ref();

    while let Some(at) = rest.find(['{', '}']) {
        let (before, from) = rest.split_at(at);
        out.push_str(before);

        let mut characters = from.chars();
        let brace = characters.next().unwrap_or('{');
        let after = characters.as_str();

        // `{{` and `}}` are one literal brace each.
        if after.starts_with(brace) {
            out.push(brace);
            rest = &after[brace.len_utf8()..];
            continue;
        }

        // A lone `}` is not a hole. Pass it through rather than guessing.
        if brace == '}' {
            out.push('}');
            rest = after;
            continue;
        }

        let Some(end) = after.find('}') else {
            // An unclosed `{`, which is a translator's slip. The rest of the
            // string is more useful on screen than an error would be.
            out.push('{');
            rest = after;
            continue;
        };

        let name = &after[..end];
        match values.iter().find(|(known, _)| *known == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[end + 1..];
    }

    out.push_str(rest);
    Cow::Owned(out)
}

// ---------------------------------------------------------------------------
// The pseudo-locale
// ---------------------------------------------------------------------------

/// A stand-in language for finding layout problems before a translator does.
///
/// `Interface Size` becomes `[Ínţéŕƒàçé Šížé ····]`, which answers three
/// questions at a glance:
///
///   - **Was this string extracted?** Anything still in plain English on
///     screen was missed by the extractor or built by hand from pieces.
///   - **Does the layout survive a longer language?** The padding is 40%,
///     which is about what German and Finnish cost against English - the
///     figure the plan has been carrying since this was scheduled.
///   - **Can the bundled fonts draw it?** The accents are Latin-1 and
///     Latin Extended-A, so a box here is a font problem, not a translation
///     one.
///
/// The brackets are the useful part and the reason they are on both ends: a
/// string that is cut off has lost its `]`, and a string built by joining two
/// translated pieces has a `][` in the middle of it.
///
/// Holes are left exactly as they are. Accenting the inside of `{time}` would
/// stop it matching what [`fill`] is looking for, and every value would vanish
/// from the screen at once.
fn pseudo(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    out.push('[');

    let mut letters = 0;
    let mut in_hole = false;
    for character in text.chars() {
        match character {
            '{' => {
                in_hole = true;
                out.push(character);
            }
            '}' => {
                in_hole = false;
                out.push(character);
            }
            _ if in_hole => out.push(character),
            _ => {
                letters += 1;
                out.push(accent(character));
            }
        }
    }

    out.push(' ');
    for _ in 0..(letters * 2 / 5).max(1) {
        out.push('·');
    }
    out.push(']');
    out
}

/// The same letter, wearing a hat. Deliberately still readable: the point is
/// to see that a string went through the catalog, not to make it unreadable.
fn accent(character: char) -> char {
    match character {
        'a' => 'à',
        'b' => 'ƀ',
        'c' => 'ç',
        'd' => 'ð',
        'e' => 'é',
        'f' => 'ƒ',
        'g' => 'ĝ',
        'h' => 'ĥ',
        'i' => 'í',
        'j' => 'ĵ',
        'k' => 'ķ',
        'l' => 'ł',
        'm' => 'ɱ',
        'n' => 'ñ',
        'o' => 'ó',
        'p' => 'ƥ',
        'r' => 'ŕ',
        's' => 'š',
        't' => 'ţ',
        'u' => 'ú',
        'v' => 'ṽ',
        'w' => 'ŵ',
        'y' => 'ý',
        'z' => 'ž',
        'A' => 'Á',
        'B' => 'Ɓ',
        'C' => 'Ç',
        'D' => 'Ð',
        'E' => 'É',
        'F' => 'Ƒ',
        'G' => 'Ĝ',
        'H' => 'Ĥ',
        'I' => 'Í',
        'J' => 'Ĵ',
        'K' => 'Ķ',
        'L' => 'Ł',
        'M' => 'Ṁ',
        'N' => 'Ñ',
        'O' => 'Ó',
        'P' => 'Ƥ',
        'R' => 'Ŕ',
        'S' => 'Š',
        'T' => 'Ţ',
        'U' => 'Ú',
        'V' => 'Ṽ',
        'W' => 'Ŵ',
        'Y' => 'Ý',
        'Z' => 'Ž',
        other => other,
    }
}

// ---------------------------------------------------------------------------
// What the rest of the application asks
// ---------------------------------------------------------------------------

/// The locale in force, for the About page and for bug reports.
pub fn code() -> &'static str {
    match active() {
        Active::Untranslated => SOURCE_LANGUAGE,
        Active::Pseudo => PSEUDO,
        Active::Compiled(catalog) => catalog.code,
        Active::Loaded(loaded) => loaded.code.as_str(),
    }
}

/// Whether the interface should be laid out right to left.
///
/// GTK works this out from the locale itself, but only from the one the
/// *process* was started in - and the language here can come from a setting in
/// the application, which GTK has never heard of. So `main` reads this and
/// tells GTK, rather than letting it guess. See the note there.
///
/// Kept as a list rather than asked of the system: no platform of the three
/// answers this question the same way, and the list of right-to-left languages
/// somebody might translate a media player into is short and does not move.
pub fn is_rtl() -> bool {
    is_rtl_tag(code())
}

/// Whether a given locale tag is right-to-left, by language rather than by
/// region: `ar-EG` is Arabic wherever it is spoken.
fn is_rtl_tag(tag: &str) -> bool {
    const RIGHT_TO_LEFT: &[&str] = &[
        "ar",  // Arabic
        "arc", // Aramaic
        "ckb", // Central Kurdish
        "dv",  // Divehi
        "fa",  // Persian
        "he",  // Hebrew
        "ku",  // Kurdish, where it is written in Arabic script
        "nqo", // N'Ko
        "ps",  // Pashto
        "sd",  // Sindhi
        "ug",  // Uyghur
        "ur",  // Urdu
        "yi",  // Yiddish
    ];
    RIGHT_TO_LEFT.contains(&primary(&normalize(tag)))
}

/// A language the interface can be set to, for the Settings row.
pub struct Offered {
    /// What goes in the config file. `None` is "follow the machine".
    pub code: Option<String>,
    /// What the row reads.
    pub label: String,
}

/// Every language this build can show, for the picker under Settings.
///
/// Named from `languages.rs`, which already carries each language's own name
/// beside its English one - so German reads "German (Deutsch)" rather than
/// "de". That table is not itself translated, deliberately: somebody looking
/// for their language scans for their own word for it, which is the column
/// that is already there.
pub fn offered() -> Vec<Offered> {
    let mut offered = vec![
        Offered {
            code: None,
            label: crate::tr!("Use the system language").into_owned(),
        },
        Offered {
            code: Some(SOURCE_LANGUAGE.to_string()),
            label: crate::languages::menu_name("en", "English", "English"),
        },
    ];

    for catalog in CATALOGS {
        let language = primary(&normalize(catalog.code)).to_string();
        let named = crate::languages::LANGUAGES
            .iter()
            .find(|(code, _, _, _)| *code == language);
        offered.push(Offered {
            code: Some(catalog.code.to_string()),
            label: match named {
                Some((code, name, native, _)) => crate::languages::menu_name(code, name, native),
                // A catalog for something the language table does not carry.
                // The code alone is a poor label and an honest one.
                None => catalog.code.to_string(),
            },
        });
    }

    // A development tool rather than a language, so it is not put in front of
    // anyone who is not looking for it. Still honored from the config or the
    // environment in a release build, which is what makes it usable for
    // checking a layout in the build people actually run.
    if cfg!(debug_assertions) {
        offered.push(Offered {
            code: Some(PSEUDO.to_string()),
            label: "Pseudo-locale (layout testing)".to_string(),
        });
    }

    offered
}

// ---------------------------------------------------------------------------
// The macros
// ---------------------------------------------------------------------------

/// One translated string.
///
/// ```ignore
/// tr!("Close the player")
/// tr!("Resume at {time}", time = human_time(position))
/// ```
///
/// The msgid must be a literal, because `packaging/extract-strings.py` reads
/// these call sites to build `po/tineplayer.pot` and cannot follow a variable.
#[macro_export]
macro_rules! tr {
    ($msgid:literal) => {
        $crate::i18n::translate($msgid)
    };
    ($msgid:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::translate($msgid),
            &[$((stringify!($name), ::std::string::ToString::to_string(&$value))),+],
        )
    };
}

/// A translated string with a context, for where English is ambiguous.
///
/// ```ignore
/// trc!("audio track", "None")
/// ```
///
/// The context is never shown. It reaches the translator as a note beside the
/// string, and it means two identical English words can become two different
/// words elsewhere.
#[macro_export]
macro_rules! trc {
    ($context:literal, $msgid:literal) => {
        $crate::i18n::translate_in_context($context, $msgid)
    };
    ($context:literal, $msgid:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::translate_in_context($context, $msgid),
            &[$((stringify!($name), ::std::string::ToString::to_string(&$value))),+],
        )
    };
}

/// A translated string that counts something.
///
/// ```ignore
/// trn!("{n} track", "{n} tracks", found, n = found)
/// ```
///
/// The count is given twice on purpose: once for the catalog to choose a form
/// with, and once as a hole to fill, because not every language puts the
/// number in the same place - or in at all.
#[macro_export]
macro_rules! trn {
    ($one:literal, $many:literal, $count:expr) => {
        $crate::i18n::translate_plural($one, $many, $count as u64)
    };
    ($one:literal, $many:literal, $count:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::translate_plural($one, $many, $count as u64),
            &[$((stringify!($name), ::std::string::ToString::to_string(&$value))),+],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalog to test the lookup against, built the way `build.rs` builds
    /// one: sorted, with the context separator spelled out.
    fn german() -> Catalog {
        fn index(n: u64) -> usize {
            usize::from(n != 1)
        }
        Catalog {
            code: "de",
            singular: &[
                ("Close the player\u{0}", "unused - sorts first"),
                ("Resume at {time}", "Fortsetzen ab {time}"),
                ("audio track\u{4}None", "Ohne"),
                ("subtitle preference\u{4}None", "Keine"),
            ],
            plural: &[("{n} track", &["{n} Tonspur", "{n} Tonspuren"])],
            nplurals: 2,
            plural_index: index,
        }
    }

    /// The singular table above has to be sorted for the binary search to
    /// work, and a test that quietly relied on an unsorted one would pass for
    /// the wrong reason.
    #[test]
    fn the_fixture_is_sorted_like_a_real_catalog() {
        let catalog = german();
        let mut keys: Vec<&str> = catalog.singular.iter().map(|e| e.0).collect();
        let sorted = {
            let mut copy = keys.clone();
            copy.sort_unstable();
            copy
        };
        keys.dedup();
        assert_eq!(keys.len(), catalog.singular.len(), "keys must be unique");
        assert_eq!(
            catalog.singular.iter().map(|e| e.0).collect::<Vec<_>>(),
            sorted
        );
    }

    // -- negotiation --------------------------------------------------------

    /// What `choose` settled on, named. Comparing `Choice` values directly
    /// would mean `Catalog` had to be comparable, which is a trait on shipped
    /// code added for the benefit of a test.
    fn chose(wanted: Option<&str>, catalogs: &[Catalog]) -> String {
        match choose(wanted, catalogs) {
            Choice::Untranslated => SOURCE_LANGUAGE.to_string(),
            Choice::Pseudo => PSEUDO.to_string(),
            Choice::Catalog(catalog) => catalog.code.to_string(),
        }
    }

    fn stub(code: &'static str) -> Catalog {
        Catalog {
            code,
            singular: &[],
            plural: &[],
            nplurals: 2,
            plural_index: |n| usize::from(n != 1),
        }
    }

    #[test]
    fn an_exact_locale_wins_over_its_language() {
        // A regional catalog is a deliberate thing to have made, so asking for
        // one must not be answered with its parent.
        let catalogs = [stub("pt"), stub("pt-BR")];
        assert_eq!(chose(Some("pt-BR"), &catalogs), "pt-BR");
        assert_eq!(chose(Some("pt"), &catalogs), "pt");
    }

    #[test]
    fn a_region_falls_back_to_its_language() {
        let catalogs = [german()];
        // Austrian German gets German, which is the point of the fallback.
        assert_eq!(chose(Some("de-AT"), &catalogs), "de");
        // A POSIX spelling is the same request differently punctuated.
        assert_eq!(chose(Some("de_AT"), &catalogs), "de");
    }

    #[test]
    fn a_language_falls_back_to_a_region_when_that_is_all_there_is() {
        // Portuguese from Portugal, where Brazilian is the only Portuguese
        // compiled in. Imperfect, and much better than English.
        let catalogs = [stub("pt-BR")];
        assert_eq!(chose(Some("pt-PT"), &catalogs), "pt-BR");
    }

    #[test]
    fn english_asks_for_no_catalog_at_all() {
        let catalogs = [german()];
        for wanted in ["en", "en-US", "en-GB", "EN_gb"] {
            assert_eq!(chose(Some(wanted), &catalogs), "en", "{wanted}");
        }
    }

    #[test]
    fn a_language_with_no_catalog_lands_on_english() {
        let catalogs = [german()];
        assert_eq!(chose(Some("fi"), &catalogs), "en");
        assert_eq!(chose(None, &catalogs), "en");
        assert_eq!(chose(Some("   "), &catalogs), "en");
    }

    #[test]
    fn the_pseudo_locale_is_reachable_by_name() {
        assert_eq!(chose(Some(PSEUDO), &[]), PSEUDO);
    }

    // -- filling in holes ---------------------------------------------------

    #[test]
    fn holes_are_filled_by_name() {
        let filled = fill(
            Cow::Borrowed("Fortsetzen ab {time}"),
            &[("time", "1:04:22".to_string())],
        );
        assert_eq!(filled, "Fortsetzen ab 1:04:22");
    }

    #[test]
    fn holes_may_be_reordered_by_the_translator() {
        // Which is the entire reason they are named rather than positional.
        let english = fill(
            Cow::Borrowed("{track} on {device}"),
            &[
                ("track", "German".to_string()),
                ("device", "Headphones".to_string()),
            ],
        );
        let reordered = fill(
            Cow::Borrowed("{device}: {track}"),
            &[
                ("track", "German".to_string()),
                ("device", "Headphones".to_string()),
            ],
        );
        assert_eq!(english, "German on Headphones");
        assert_eq!(reordered, "Headphones: German");
    }

    #[test]
    fn a_hole_may_be_used_twice() {
        let filled = fill(
            Cow::Borrowed("{name} is {name}"),
            &[("name", "TinePlayer".to_string())],
        );
        assert_eq!(filled, "TinePlayer is TinePlayer");
    }

    #[test]
    fn a_misspelled_hole_is_left_where_it_can_be_seen() {
        let filled = fill(
            Cow::Borrowed("Fortsetzen ab {tiem}"),
            &[("time", "1:04:22".to_string())],
        );
        assert_eq!(filled, "Fortsetzen ab {tiem}");
    }

    #[test]
    fn braces_can_be_escaped_the_way_rust_escapes_them() {
        let filled = fill(
            Cow::Borrowed("{{literal}} and {real}"),
            &[("real", "filled".to_string())],
        );
        assert_eq!(filled, "{literal} and filled");
    }

    #[test]
    fn a_string_with_no_holes_is_not_copied() {
        let filled = fill(Cow::Borrowed("Close the player"), &[]);
        assert!(matches!(filled, Cow::Borrowed(_)));
    }

    #[test]
    fn an_unclosed_hole_does_not_eat_the_rest_of_the_string() {
        let filled = fill(Cow::Borrowed("Resume at {time"), &[]);
        assert_eq!(filled, "Resume at {time");
    }

    // -- the pseudo-locale --------------------------------------------------

    #[test]
    fn the_pseudo_locale_pads_and_brackets() {
        let padded = pseudo("Interface Size");
        assert!(padded.starts_with('['), "{padded}");
        assert!(padded.ends_with(']'), "{padded}");
        assert!(
            padded.chars().count() > "Interface Size".chars().count() * 5 / 4,
            "{padded} is not long enough to catch a tight layout"
        );
    }

    #[test]
    fn the_pseudo_locale_leaves_holes_alone() {
        // If it accented the inside of a hole, `fill` would stop matching it
        // and every value in the interface would disappear at once.
        let padded = pseudo("Resume at {time}");
        assert!(padded.contains("{time}"), "{padded}");
        let filled = fill(Cow::Owned(padded), &[("time", "1:04".to_string())]);
        assert!(filled.contains("1:04"), "{filled}");
    }

    // -- looking things up --------------------------------------------------

    #[test]
    fn a_context_picks_between_two_spellings_of_one_english_word() {
        let catalog = german();
        let none_audio = catalog
            .singular
            .binary_search_by(|e| compare_key(e.0, "audio track", "None"));
        let none_subtitle = catalog
            .singular
            .binary_search_by(|e| compare_key(e.0, "subtitle preference", "None"));
        assert_eq!(catalog.singular[none_audio.expect("present")].1, "Ohne");
        assert_eq!(catalog.singular[none_subtitle.expect("present")].1, "Keine");
    }

    #[test]
    fn a_context_that_is_not_there_is_not_confused_with_one_that_is() {
        let catalog = german();
        let missing = catalog
            .singular
            .binary_search_by(|e| compare_key(e.0, "chapter", "None"));
        assert!(missing.is_err());
    }

    /// `compare_key` orders against a key it never builds, so it has to agree
    /// with the ordering `build.rs` sorted the table by.
    #[test]
    fn comparing_a_split_key_matches_comparing_a_joined_one() {
        let cases = [
            ("audio track", "None"),
            ("audio track", "Nine"),
            ("subtitle preference", "None"),
            ("a", "b"),
            ("", "None"),
        ];
        for (context, msgid) in cases {
            let joined = format!("{context}\u{4}{msgid}");
            for (other_context, other_msgid) in cases {
                let other = format!("{other_context}\u{4}{other_msgid}");
                assert_eq!(
                    compare_key(&other, context, msgid),
                    other.cmp(&joined),
                    "{other} against {joined}"
                );
            }
        }
    }

    // -- what ships ---------------------------------------------------------

    /// Every catalog compiled into this binary, checked for the mistake a
    /// translator is most likely to make and least likely to notice: dropping
    /// or renaming a `{hole}`. The string looks fine in Weblate and comes out
    /// on screen missing the one piece of information it existed to carry.
    ///
    /// Deliberately a test rather than a build failure. A catalog arrives from
    /// somebody outside this project, and refusing to build is a bad way to
    /// tell them - but this has to fail before a release, so CI is where it
    /// belongs.
    #[test]
    fn placeholders_match() {
        fn holes(text: &str) -> Vec<&str> {
            let mut found = Vec::new();
            let mut rest = text;
            while let Some(at) = rest.find('{') {
                rest = &rest[at + 1..];
                // `{{` is an escaped brace rather than the start of a hole.
                if rest.starts_with('{') {
                    rest = &rest[1..];
                    continue;
                }
                let Some(end) = rest.find('}') else { break };
                found.push(&rest[..end]);
                rest = &rest[end + 1..];
            }
            found.sort_unstable();
            found.dedup();
            found
        }

        for catalog in CATALOGS {
            for (key, translated) in catalog.singular {
                // The msgid is the key with any context stripped back off.
                let msgid = key.split('\u{4}').next_back().unwrap_or(key);
                assert_eq!(
                    holes(msgid),
                    holes(translated),
                    "po/{}.po: \"{msgid}\" is translated as \"{translated}\", which does not \
                     carry the same {{holes}}",
                    catalog.code
                );
            }
            for (key, forms) in catalog.plural {
                let msgid = key.split('\u{4}').next_back().unwrap_or(key);
                for form in *forms {
                    // A plural form may legitimately drop `{n}` - "eine
                    // Tonspur" is a fine translation of "1 track" - so what is
                    // checked is that nothing *new* was invented.
                    for hole in holes(form) {
                        assert!(
                            holes(msgid).contains(&hole),
                            "po/{}.po: \"{msgid}\" has a plural form \"{form}\" using {{{hole}}}, \
                             which the English does not have",
                            catalog.code
                        );
                    }
                }
            }
        }
    }

    /// Every compiled catalog has to name a language the picker can label and
    /// the direction check can read.
    #[test]
    fn every_catalog_is_named_sensibly() {
        for catalog in CATALOGS {
            let code = normalize(catalog.code);
            assert!(!code.is_empty());
            assert_ne!(
                primary(&code),
                SOURCE_LANGUAGE,
                "po/{}.po translates the language the source is written in",
                catalog.code
            );
            assert_eq!(
                catalog
                    .plural
                    .iter()
                    .find(|(_, forms)| forms.len() != catalog.nplurals),
                None,
                "po/{}.po has a plural message with the wrong number of forms",
                catalog.code
            );
        }
    }

    /// A catalog with Windows line endings, which is not a hypothetical: this
    /// repository is checked out with `core.autocrlf` on Windows, and
    /// `.gitattributes` pins only `*.sh` to LF. So a contributor cloning here
    /// gets CRLF in every `.po`, and a translator editing one in Notepad saves
    /// CRLF whatever the file had.
    ///
    /// The endings are pinned in `.gitattributes` now, which is the real fix
    /// and covers Weblate and Poedit as well as this. This is the guard for
    /// the file that arrives with them anyway.
    #[test]
    fn a_catalog_with_windows_line_endings_still_reads() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("po")
            .join("de.po");
        let text = std::fs::read_to_string(&source).expect("po/de.po is readable");
        let crlf = text.replace('\n', "\r\n");

        let path = std::env::temp_dir().join("tineplayer-crlf-catalog.po");
        std::fs::write(&path, &crlf).expect("the temporary directory is writable");
        let loaded = load(&path);
        let _ = std::fs::remove_file(&path);

        let loaded = loaded.expect("a catalog with CRLF endings is still a catalog");
        assert_eq!(loaded.code, "de");
        assert_eq!(
            loaded
                .singular
                .get("Interface Language")
                .map(String::as_str),
            Some("Sprache der Oberfläche"),
            "a translation came back with a stray carriage return in it"
        );
    }

    /// The `TINEPLAYER_PO` path, against a real catalog in this repository.
    ///
    /// Worth a test of its own because it is the one route through this module
    /// that no build touches: everything else is compiled in and would fail
    /// loudly, while this parses a file at startup on somebody else's machine.
    /// It is also the route a translator uses, so it breaking is the failure
    /// they would hit and nobody here would.
    #[test]
    fn a_catalog_can_be_read_from_disk() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("po")
            .join("de.po");
        let loaded = load(&path).expect("po/de.po is a catalog this can read");

        assert_eq!(loaded.code, "de");
        assert_eq!(loaded.nplurals, 2);
        assert_eq!(
            loaded
                .singular
                .get("Interface Language")
                .map(String::as_str),
            Some("Sprache der Oberfläche")
        );
        // Its context separator survived the round trip through the file.
        assert_eq!(
            loaded
                .singular
                .get("audio output device\u{4}None")
                .map(String::as_str),
            Some("Keines")
        );
        // And its plural rule evaluates, rather than merely having parsed.
        assert_eq!(loaded.rule.eval(1), 0);
        assert_eq!(loaded.rule.eval(7), 1);
    }

    #[test]
    fn a_catalog_that_is_not_there_is_refused_rather_than_ignored() {
        let missing = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("po")
            .join("no-such-language.po");
        assert!(load(&missing).is_err());
    }

    /// `is_rtl` reads whatever `init` resolved, which a test cannot set: it is
    /// once per process and the whole test binary shares one. `is_rtl_tag` is
    /// the decision itself, and is what this checks.
    #[test]
    fn right_to_left_is_recognized_by_language_not_by_region() {
        for tag in ["ar", "ar-EG", "he_IL", "fa", "UR"] {
            assert!(is_rtl_tag(tag), "{tag} is written right to left");
        }
        for tag in ["de", "en-US", "fi", "ja"] {
            assert!(!is_rtl_tag(tag), "{tag} is written left to right");
        }
    }

    /// Every language the picker offers has to be one the direction check can
    /// read, or a right-to-left catalog would ship laid out left to right.
    #[test]
    fn every_offered_language_has_a_direction() {
        for offered in offered() {
            let Some(code) = offered.code else { continue };
            if code == PSEUDO {
                continue;
            }
            assert!(
                !primary(&normalize(&code)).is_empty(),
                "{code} has no language part"
            );
        }
    }
}
