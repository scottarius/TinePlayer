use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use crate::source::Source;
use std::rc::Rc;

use gstreamer::prelude::DeviceExt;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::appearance;
use crate::config::Config;
use crate::controls::Controls;
use crate::devices::list_audio_output_devices;
use crate::pipeline::Playing;
use crate::player::Playback;
use crate::probe::AudioTrack;
use crate::sound::Sounds;
use crate::subtitles::{Subtitle, SubtitleChoice};
// Exported at the crate root by `macro_export`, which is where a macro lands
// however deep in the tree it was written. See src/i18n.rs.
use crate::{tr, trc, trn};

mod about;
mod alignment;
mod browsing;
mod choosers;
mod files;
mod input;
mod jellyfin;
mod keys;
mod kodi_screens;
mod levels;
mod media_page;
mod menu;
mod outputs;
mod pairing;
mod playback;
mod settings;
mod startup;
mod style;
mod teardown;
mod tracks;
mod widgets;

use style::{install_styles, style_css};
pub(crate) use widgets::*;

/// Marks the overlay a modal is stacked in, so that opening one over another
/// can tell it apart from a page that happens to be built out of an overlay
/// too - which the media page is.
const MODAL_STACK: &str = "tp-modal-stack";

/// How many languages either summary line names before it counts the rest.
///
/// The line has one line's worth of room and must not wrap: the rows below it
/// sit at a fixed height, so anything that grows here would push them down.
/// Six is past the point where a list is being read rather than scanned, and a
/// file with more than six subtitle languages is a disc rip whose exact
/// inventory is a chooser away.
///
/// Whatever is left over is said rather than dropped - "+5 more". Stopping at
/// six in silence reads as a complete list, which on a file carrying eleven is
/// not merely crowded but wrong.
const MOST_LANGUAGES: usize = 6;

/// What a track that never stated its language is called on the page.
///
/// "Unknown" rather than the container's own word for it: `und` is what the
/// file says and "Undetermined" is what the specification calls that, and
/// neither is what a viewer would say about a soundtrack they can plainly
/// hear. It says the same thing in the word already being used everywhere
/// else something is missing.
///
/// A function rather than a `const` for the reason every other piece of
/// interface text here became one: it is translated, and a translation is
/// built when it is asked for.
fn unknown_language() -> Cow<'static, str> {
    trc!("a track's language", "Unknown")
}

/// One summary line's markup: the label, the languages that fit, and a count
/// of the ones that did not.
///
/// Both lines are built from this, which is the point of its being a function:
/// audio and subtitles differ only in what they are handed, and a rule applied
/// in one place cannot drift out of step with the other. In practice the audio
/// line rarely reaches the limit and the subtitle line often does, so the
/// truncation would otherwise go untested on the side that shows it least.
///
/// What was left off is counted and said outright. Stopping at the limit in
/// silence reads as a complete list, so a file with eleven subtitle languages
/// appeared to have six - worse than a long line, because it is wrong rather
/// than merely crowded. The count is dimmed like the label, so it reads as a
/// note about the list rather than as another language in it.
/// **The markup is built here rather than translated.** It used to be inside
/// the string - `", <span alpha='60%'>+{extra} more</span>"` - which asks
/// every translator to carry a `<span>` through into their own language
/// intact. One dropped angle bracket and Pango refuses the whole label, so a
/// slip in one catalog costs the row rather than the word. What is translated
/// is the words; the dimming is wrapped around them afterwards.
fn summary_markup(name: &str, languages: &[String]) -> String {
    let shown = match languages.is_empty() {
        true => trc!("audio or subtitle languages", "None").into_owned(),
        false => languages
            .iter()
            .take(MOST_LANGUAGES)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
    };
    let more = match languages.len().saturating_sub(MOST_LANGUAGES) {
        0 => String::new(),
        extra => {
            // Both English forms are the same, and it is still a plural: a
            // language with a dual or a case ending on the number needs to say
            // so, and cannot if it is handed one fixed string.
            let counted = trn!("+{n} more", "+{n} more", extra as u64, n = extra);
            format!(
                ", <span alpha='60%'>{}</span>",
                glib::markup_escape_text(&counted)
            )
        }
    };
    format!(
        "<span alpha='60%'>{}:</span> {}{more}",
        glib::markup_escape_text(name),
        glib::markup_escape_text(&shown),
    )
}

/// What Kodi handing a video over should do, in the order the chooser offers
/// them, indexed by whether `--play` is written into Kodi's arguments.
///
/// One list rather than two, because the row states what is in force and the
/// chooser offers the alternatives: written out twice they would eventually
/// disagree, and a row reading one thing while its own chooser marks another
/// as current is the kind of fault nobody reports.
///
/// The menu is first, and is what no flag means. Choosing the two audio tracks
/// is the reason this application exists, so landing there is the answer an
/// integration should have to be talked out of rather than into.
fn handover() -> [Cow<'static, str>; 2] {
    [
        tr!("Show Track Selection Menu"),
        tr!("Play Video Immediately"),
    ]
}

/// What a summary line says when the file carries no such track at all, in
/// English. The text itself now lives in the catalog under the context
/// "audio or subtitle languages" - see `summary_markup` - and this is what the
/// tests read, since they check the untranslated interface.
///
/// Distinct from `Unknown`, and the difference is worth keeping: one means
/// there is a track and nobody said what language it is in, the other means
/// there is nothing there to choose. That distinction is exactly why the
/// string carries a context: two languages that spell those differently
/// cannot say so if both arrive as the bare word "None".
#[cfg(test)]
const NO_TRACKS: &str = "None";

/// Which setting a chooser screen is editing. The menu drills into one of
/// these and returns once a choice is made.
#[derive(Clone, Copy, PartialEq)]
enum Setting {
    PrimaryDevice,
    PrimaryTrack,
    SecondaryDevice,
    SecondaryTrack,
    Subtitles,
    PrimaryLanguage,
    SecondaryLanguage,
    SubtitleKind,
    SubtitleLanguage,
    SubtitleFont,
    /// What language the interface itself is in, which is a different question
    /// from what language the *audio* should be in and sits in a different
    /// category for that reason.
    InterfaceLanguage,
    /// What one Kodi does with TinePlayer, and what happens when it hands a
    /// video over. Both carry that installation's place in the list the
    /// Kodi pane was built from, since there may be several.
    KodiType(usize),
    KodiHandover(usize),
}

/// What a slider on the settings screen is setting.
///
/// Most are percentages, which is what lets one set of arithmetic serve them.
/// The delay is the exception: it is milliseconds, so it carries its own step,
/// range and reading rather than borrowing the percentage ones.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slider {
    /// The level for one output, by role.
    Volume(&'static str),
    /// How far one output is held back, by role, in milliseconds.
    Offset(&'static str),
    /// How big the interface is, in steps either side of its normal size.
    Scale,
    /// Subtitle size, in points against the video's own resolution.
    SubtitleSize,
    ResumeThreshold,
    WatchedThreshold,
}

impl Slider {
    /// How far one press moves it. Levels move in fives, being a rough
    /// setting anyone can hear; the thresholds move by one, since the useful
    /// range of each is narrow enough that fives would be three choices.
    /// The delay moves in tens, which is about the smallest step that can be
    /// told apart against a picture and still crosses its range in a few
    /// seconds of holding.
    fn step(self) -> f64 {
        match self {
            Slider::Volume(_) => 5.0,
            Slider::Offset(_) => 10.0,
            // A tenth of a step, which is about a nine per cent change in
            // size - small enough to settle on a size, large enough to cross
            // the range in a few seconds of holding.
            Slider::Scale => 0.1,
            // A point at a time. The range is small enough that anything
            // coarser would be six choices in a row of buttons.
            Slider::SubtitleSize => 1.0,
            _ => 1.0,
        }
    }

    fn range(self) -> std::ops::RangeInclusive<f64> {
        match self {
            Slider::Volume(_) => 0.0..=100.0,
            // Both directions. Holding a sink back is unbounded; pulling one
            // forward is limited by how much audio the pipeline has already
            // buffered, which measured comfortably past half a second.
            Slider::Offset(_) => -crate::config::MAX_OFFSET_MS..=crate::config::MAX_OFFSET_MS,
            // Below one per cent is indistinguishable from starting over, and
            // past a quarter of a film nothing would ever be resumable.
            Slider::ResumeThreshold => 1.0..=25.0,
            // Anything under half is not watching it, and a hundred means
            // sitting through the credits to be counted.
            Slider::WatchedThreshold => 50.0..=100.0,
            // Steps rather than the multiplier itself, so the middle is the
            // normal size and the two halves are the same length. Three steps
            // either way, which is a third at one end and three times at the
            // other.
            Slider::Scale => -3.0..=3.0,
            // Against the video's own height rather than the screen's, so
            // these hold whatever it is played back on. Below eight is
            // unreadable at any size; past twenty-four covers the picture.
            Slider::SubtitleSize => 8.0..=24.0,
        }
    }
}

/// A size chosen by hand, held to what the slider could have produced.
///
/// The file is editable, so it can hold anything at all; the interface has to
/// stay usable enough to change it back from inside.
fn chosen_scale(config: &crate::config::Config) -> Option<f64> {
    config
        .ui_scale
        .map(|scale| scale.clamp(appearance::MIN_CHOSEN_SCALE, appearance::MAX_CHOSEN_SCALE))
}

/// A size in steps either side of normal, as the multiplier it means.
///
/// Geometric rather than added: a step down is the same change as a step up,
/// so three steps down is exactly a third where three up is exactly three
/// times. Adding a fixed amount instead would make the lower half of the
/// slider cover almost nothing and the upper half everything.
fn scale_from_steps(steps: f64) -> f64 {
    let scale = 3f64.powf(steps / 3.0);
    // To the hundredth, so the file holds a number somebody could have typed
    // and the reading beside the bar is what was stored.
    (scale * 100.0).round() / 100.0
}

/// The same, backwards, for putting the bar where a stored size says.
fn steps_from_scale(scale: f64) -> f64 {
    3.0 * scale.max(0.01).log(3.0)
}

/// How a size reads beside its bar: the multiplier, without trailing noughts.
fn scale_label(scale: f64) -> String {
    let text = format!("{scale:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}x")
}

/// How far an output is shifted, wherever that is shown.
///
/// One function for the settings screen and the panel during playback,
/// because two of them drifted into two different styles for the same number
/// and the same feature read as two.
///
/// Signed and short. It is watched while it moves against a picture, where
/// what matters is seeing it change; words for the direction read better at
/// rest but turn every step into something to be re-read.
pub fn offset_label(ms: f64) -> String {
    let ms = ms.round();
    if ms == 0.0 {
        // Round can give -0, which formats with a sign that says the output
        // is shifted when it is not.
        "0ms".to_string()
    } else {
        format!("{ms:+}ms")
    }
}

/// Rows of the settings screen, in the order they appear.
/// Longer than a keyboard leaves between repeats, and short enough not to read
/// as a delay on an ordinary press. Windows repeats at up to thirty a second,
/// which is a gap of about thirty-three milliseconds.
const REPEAT_GAP: std::time::Duration = std::time::Duration::from_millis(90);

/// Rows a page jump covers, roughly a screenful at the default size. What
/// makes a folder of a hundred films navigable without a hundred presses.
const PAGE_ROWS: i32 = 8;

/// Space kept for the reading beside a bar, in characters. Shared by the
/// settings sliders and the volume panel, so a row of one lines up with a row
/// of the other.
///
/// Sized to the longest any of them shows - "-1000ms" - because the width is a
/// floor and not a ceiling: a longer reading widens the label, which moves the
/// bar, which moves under the pointer that is dragging it. Anything added that
/// reads longer than this has to raise it.
pub const READING_CHARS: i32 = 7;

/// How wide the alignment panel is, measured in characters of its own body
/// text.
///
/// Both the floor and the ceiling, so the three steps are one panel changing
/// what it says rather than three differently sized windows. It has to sit on
/// the text rather than on the container, because GTK offers no maximum width
/// on a box - and the text and the track names are the only things in the
/// panel that could push it wider anyway. Around 74 characters is also about
/// as long a line as is comfortable to read.
const ALIGN_PANEL_CHARS: i32 = 74;

/// A floor in unscaled pixels as well, for the case the character measure
/// cannot cover: a narrow font would otherwise draw a panel too cramped to
/// read from across a room, which is the distance this is built for.
const ALIGN_PANEL_MIN: f64 = 520.0;

/// Font families offered in the menu. Generic names Pango always resolves
/// rather than an enumeration of everything installed, which would run to
/// hundreds of rows. `subtitle_font` in the config takes any description.
const SUBTITLE_FONTS: [&str; 5] = ["Sans Bold", "Sans", "Serif Bold", "Serif", "Monospace Bold"];

/// How long scrubbing must be still before the seek is actually performed.
/// Short enough to feel like it happens on release, long enough to bridge the
/// gap between auto-repeat steps and the release events X11 interleaves
/// between them.
/// Scrub redraw interval. The movement is driven from here rather than from
/// input repeats, so it stays smooth at every speed.
const SCRUB_TICK: std::time::Duration = std::time::Duration::from_millis(33);

/// Safety net: if a release is somehow missed, scrubbing still ends rather
/// than running away.
const SCRUB_ABANDON: std::time::Duration = std::time::Duration::from_millis(700);

/// Tracked so Escape can mean "go back one level" rather than one fixed
/// action: out of playback, out of a chooser, or out of the application.
#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Menu,
    Settings,
    Browser,
    PasteUri,
    VideoSource,
    Opening,
    /// The three steps of aligning an audio file, in the one panel that
    /// carries them: which track to measure against, the measuring, and what
    /// it found. Separate screens because backing out means different things
    /// at each - nothing has been decided at the first, a thread is running at
    /// the second, and the third is only a report.
    AlignChoose,
    AlignProgress,
    AlignResult,
    Confirm,
    Notices,
    /// The key and button list, when there is no picture to lay it over. In
    /// playback it is an overlay on the controls instead - see
    /// `Controls::toggle_shortcuts` for why it cannot be a page there.
    Shortcuts,
    /// The panels the Kodi pane can open over itself: the folder
    /// browser for naming a Kodi by hand, the confirmation asked before the
    /// first change to a file and before a removal, the sandbox instructions
    /// for a Flatpak, and a failure to write.
    ///
    /// These were nine, and were a wizard. The five that collected answers are
    /// rows on the pane now.
    KodiFolder,
    KodiConfirm,
    KodiPermission,
    KodiError,
    /// The Quick Connect code, while it waits to be approved. Its own screen
    /// rather than one of the panels below, because something is running
    /// behind it: the polling stops when this stops being what is on screen.
    JellyfinConnect,
    /// Everything else the Jellyfin pane opens over itself - the server
    /// address, the question asked before disconnecting, and anything that
    /// went wrong. One variant rather than three, since what they have in
    /// common is the whole of what is asked of them: Escape returns to the
    /// pane, exactly as pressing Cancel does.
    JellyfinPanel,
    ConfirmQuit,
    Error,
    Playing,
}

/// Which output a choice is about, where the two are otherwise handled alike.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Primary,
    Secondary,
}

impl Role {
    /// How the config file names this output. Two spellings of one thing, and
    /// this is where they meet.
    fn key(self) -> &'static str {
        match self {
            Role::Primary => "primary",
            Role::Secondary => "secondary",
        }
    }
}

/// What choosing a row on the main menu does.
///
/// Carried beside each row rather than worked out from its position: the
/// alignment rows come and go with the audio files chosen, so a fixed index
/// would name a different row depending on what is set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Device(Role),
    Track(Role),
    Align(Role),
    Subtitles,
}

/// One line of a chooser: what it says, and which choice it stands for.
/// `None` is the "None" entry, which most of these lists begin with.
type Choice = (String, Option<usize>);

/// Everything a chooser needs to draw itself.
struct Choices {
    entries: Vec<Choice>,
    /// The choice already in force, so the list opens on it rather than at the
    /// top. `None` when nothing is set, which lands on the "None" row every
    /// list that has one begins with.
    current: Option<usize>,
    /// Entries that begin a group, by index, and what that group is called
    /// where it is called anything.
    ///
    /// Most are a plain rule: the subtitle preference offers three unlike
    /// things in one list - nothing, four ways of following an output, and two
    /// hundred languages - and without the rules they read as one long
    /// undifferentiated run. A rule says "these are a different kind of
    /// answer", which is all most of them have to say.
    ///
    /// A caption says *which* kind, for the one group where the rows cannot:
    /// the separate audio files, which look like the track rows above them.
    /// Drawn as a heading in place of the rule rather than beside it, the way
    /// the settings screen's groups are - and a heading sits outside the
    /// selection model and the focus chain, so it cannot be landed on.
    dividers: Vec<(usize, Option<String>)>,
}

/// Puts a selector's rows in, and can be run again when what they should say
/// has changed - which for a device list is a moment after it opens.
type Fill = dyn Fn(&Rc<App>);

/// One row on the settings screen, named rather than numbered.
///
/// **These were twenty-three `const ROW_*: i32` values, and the numbering was
/// the bug.** Every list of switches and sliders was keyed by position, so
/// inserting a row moved everything below it and the widgets went on being
/// built against the old numbers - a comment in the old code records exactly
/// that happening, a switch landing on the wrong row and leaving another with
/// none. Categories would have made it worse, each pane starting its own count
/// from zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    InterfaceScale,
    InterfaceLanguage,
    Sounds,
    StartFullscreen,
    ReadMetadata,
    ShowBackdrop,
    RememberPositions,
    ResumeThreshold,
    WatchedThreshold,
    Updates,
    UpdateStatus,
    ClearData,
    /// The five rows each output has, told apart by which output they are for
    /// rather than by five more names apiece.
    Device(Role),
    Language(Role),
    Description(Role),
    Volume(Role),
    Sync(Role),
    SubtitlePreference,
    SubtitleLanguagePreference,
    SubtitleSize,
    SubtitleFont,
    /// The rows one Kodi installation has, by its place in the list the pane
    /// was built from. Unlike every other row here there may be none of these,
    /// or several sets of them - which is why the category is told what was
    /// found before it can say what it holds.
    ///
    /// Which of the three an installation gets depends on how it was
    /// installed: a Snap has only the first, and says on it why there is
    /// nothing else, and only a Flatpak has the last.
    KodiType(usize),
    KodiHandover(usize),
    KodiPermission(usize),
    /// Stands in for the groups when there are none, so the pane says why it
    /// is empty rather than only offering to add something.
    KodiNone,
    KodiAdd,
    /// The one row the Jellyfin pane has, in whichever of its two states it
    /// is in. Never both: a Connect that is really a Disconnect, or a
    /// Disconnect on a pane with nothing to disconnect from, would each be a
    /// row that means the opposite of what it says.
    JellyfinConnect,
    JellyfinDisconnect,
    Notices,
}

impl Item {
    /// The bar this row carries, if it carries one.
    fn slider(self) -> Option<Slider> {
        Some(match self {
            Item::InterfaceScale => Slider::Scale,
            Item::SubtitleSize => Slider::SubtitleSize,
            Item::Volume(role) => Slider::Volume(role.key()),
            Item::Sync(role) => Slider::Offset(role.key()),
            Item::ResumeThreshold => Slider::ResumeThreshold,
            Item::WatchedThreshold => Slider::WatchedThreshold,
            _ => return None,
        })
    }

    /// The chooser this row opens, if it opens one.
    fn setting(self) -> Option<Setting> {
        Some(match self {
            Item::Device(Role::Primary) => Setting::PrimaryDevice,
            Item::Device(Role::Secondary) => Setting::SecondaryDevice,
            Item::Language(Role::Primary) => Setting::PrimaryLanguage,
            Item::Language(Role::Secondary) => Setting::SecondaryLanguage,
            Item::SubtitlePreference => Setting::SubtitleKind,
            Item::SubtitleLanguagePreference => Setting::SubtitleLanguage,
            Item::SubtitleFont => Setting::SubtitleFont,
            Item::InterfaceLanguage => Setting::InterfaceLanguage,
            Item::KodiType(index) => Setting::KodiType(index),
            Item::KodiHandover(index) => Setting::KodiHandover(index),
            _ => return None,
        })
    }

    /// Whether a switch sits on this row, which decides two things: that a
    /// click on the row itself must not work it, and that activating the row
    /// from the keyboard must.
    fn has_switch(self) -> bool {
        matches!(
            self,
            Item::InterfaceScale
                | Item::Sounds
                | Item::StartFullscreen
                | Item::ReadMetadata
                | Item::ShowBackdrop
                | Item::RememberPositions
                | Item::Description(_)
                | Item::Volume(_)
                | Item::Sync(_)
                | Item::Updates
        )
    }
}

/// What the Kodi pane needs to know about one Kodi to say which rows
/// it has and what heads them.
///
/// A descriptor rather than the `Setup` itself, so `Category::items` stays a
/// plain function of its inputs: a test can ask what the pane holds for three
/// imagined installations without a disk to find any of them on.
/// Where the connection flow was started from, and so where it goes back to.
///
/// It is reachable from two screens that have nothing to do with each other -
/// the settings pane, and the page shown when no video is loaded - and a flow
/// that always returned to Settings would strand somebody who never opened it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConnectFrom {
    Settings,
    Menu,
}

/// How far the pairing with a Jellyfin server has got, which is all the pane
/// needs to know to say what is on it.
///
/// Two states rather than three. A server that has been named but never
/// approved reads exactly like one that has not been named at all - there is
/// an address to set and a code to ask for - and the difference between them
/// is whether Connect can be pressed, which is a fact about one row rather
/// than about the pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum JellyfinPane {
    NotConnected,
    Connected,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct KodiPane {
    /// The group heading, which is the installation's name: "KODI 21.1
    /// (STANDARD)". Held rather than derived because working it out means
    /// asking the system what version it installed.
    heading: String,
    confinement: crate::kodi_setup::Confinement,
}

/// The left column of the settings screen, and what each of its entries holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    General,
    Outputs,
    Subtitles,
    /// The server TinePlayer can be cast from. Named for the one thing in it,
    /// on the rule the Kodi category set below.
    Jellyfin,
    /// Named for the one thing in it rather than for the kind of thing it is.
    /// It was "Integrations", which reads as a place to put the next one - and
    /// everything in here is a group per Kodi installation, so a second kind
    /// of integration would land among them with nothing to say where Kodi
    /// ends and it begins. Whatever comes next gets a category of its own,
    /// which is what Jellyfin above is.
    Kodi,
    About,
}

impl Category {
    /// The order the column shows them in, which is the only thing the order
    /// of these decides.
    const ALL: [Category; 6] = [
        Category::General,
        Category::Outputs,
        Category::Subtitles,
        Category::Jellyfin,
        Category::Kodi,
        Category::About,
    ];

    fn title(self) -> Cow<'static, str> {
        match self {
            Category::General => tr!("General"),
            Category::Outputs => tr!("Outputs"),
            Category::Subtitles => trc!("settings category", "Subtitles"),
            // Product names, the same in every language.
            Category::Jellyfin => Cow::Borrowed("Jellyfin"),
            Category::Kodi => Cow::Borrowed("Kodi"),
            Category::About => tr!("About"),
        }
    }

    /// What the right-hand pane shows, and the heading each group opens with.
    ///
    /// `kodis` is every Kodi installation found, and `jellyfin` how far the
    /// pairing with a server has got. Both are passed in rather than looked up
    /// here so this stays a plain function of its inputs, and so a test can ask
    /// what a category holds without an application to ask it of - one walks
    /// the disk and the other reads a credentials file.
    ///
    /// The headings are what make Outputs readable: it holds two rows called
    /// Volume and two called Audio Sync, and until now they were told apart
    /// only by which half of the list they were in. The Kodi category now works the
    /// same way, one heading per installation.
    fn items(
        self,
        kodis: &[KodiPane],
        jellyfin: JellyfinPane,
    ) -> Vec<(Option<Cow<'static, str>>, Item)> {
        match self {
            Category::General => vec![
                (Some(tr!("INTERFACE")), Item::InterfaceScale),
                (None, Item::InterfaceLanguage),
                (None, Item::Sounds),
                (None, Item::StartFullscreen),
                (Some(tr!("LIBRARY")), Item::ReadMetadata),
                (None, Item::ShowBackdrop),
                // Above the two thresholds rather than below them: it decides
                // whether they apply at all, and a switch that governs the
                // rows under it reads better before them than after.
                (Some(tr!("RESUMING")), Item::RememberPositions),
                (None, Item::ResumeThreshold),
                (None, Item::WatchedThreshold),
                // With the settings that decide what gets saved, rather than
                // alone under a heading of its own at the end. It is still the
                // one thing here that destroys something, but it destroys
                // exactly what the three rows above it govern, and a row that
                // says "clear this" reads better under them than a screen
                // away.
                (None, Item::ClearData),
                (Some(tr!("UPDATES")), Item::Updates),
                (None, Item::UpdateStatus),
            ],
            Category::Outputs => vec![
                (Some(tr!("FIRST OUTPUT")), Item::Device(Role::Primary)),
                (None, Item::Language(Role::Primary)),
                (None, Item::Description(Role::Primary)),
                (None, Item::Volume(Role::Primary)),
                (None, Item::Sync(Role::Primary)),
                (Some(tr!("SECOND OUTPUT")), Item::Device(Role::Secondary)),
                (None, Item::Language(Role::Secondary)),
                (None, Item::Description(Role::Secondary)),
                (None, Item::Volume(Role::Secondary)),
                (None, Item::Sync(Role::Secondary)),
            ],
            Category::Subtitles => vec![
                (None, Item::SubtitlePreference),
                (None, Item::SubtitleLanguagePreference),
                (None, Item::SubtitleSize),
                (None, Item::SubtitleFont),
            ],
            Category::Kodi => {
                // Nothing found: one heading rather than two, because the row
                // saying so and the row that does something about it are the
                // same subject. A pane offering only "Add a Kodi Folder" would
                // leave somebody wondering whether it had looked.
                if kodis.is_empty() {
                    return vec![(Some(tr!("KODI")), Item::KodiNone), (None, Item::KodiAdd)];
                }

                let mut rows: Vec<(Option<Cow<'static, str>>, Item)> = Vec::new();
                for (index, kodi) in kodis.iter().enumerate() {
                    rows.push((Some(kodi.heading.clone().into()), Item::KodiType(index)));
                    // A Snap gets the one row and no others. It cannot start
                    // an external player at all, so a handover question below
                    // it would be a setting for something that will not
                    // happen - and the row itself carries the reason.
                    if !kodi.confinement.supported() {
                        continue;
                    }
                    rows.push((None, Item::KodiHandover(index)));
                    if kodi.confinement == crate::kodi_setup::Confinement::Flatpak {
                        rows.push((None, Item::KodiPermission(index)));
                    }
                }
                // Under a heading of its own: it belongs to no installation,
                // and without one it reads as another row of the last group.
                rows.push((Some(tr!("OTHER")), Item::KodiAdd));
                rows
            }
            // One heading over the lot. Unlike Kodi there is only ever one
            // server, so a group per anything would be a group of one.
            // One row, which is the whole of what there is to do: connect,
            // or stop being connected. The server and the account are facts
            // rather than settings, so they are stated in the note under the
            // heading instead of taking a row each and inviting a press.
            Category::Jellyfin => match jellyfin {
                JellyfinPane::NotConnected => {
                    vec![(Some(tr!("JELLYFIN")), Item::JellyfinConnect)]
                }
                JellyfinPane::Connected => {
                    vec![(Some(tr!("JELLYFIN")), Item::JellyfinDisconnect)]
                }
            },
            // The text itself is not a row - see `about_body`, which the
            // pane draws above these.
            Category::About => vec![(None, Item::Notices)],
        }
    }
}

/// What the file browser was opened to find.
///
/// Held on the application rather than passed down, because stepping into a
/// folder re-enters the browser and would otherwise forget the errand.
#[derive(Clone, Copy, PartialEq, Default)]
enum Errand {
    #[default]
    Video,
    /// A separate soundtrack for one of the two outputs.
    Audio(Role),
    /// A subtitle file from somewhere other than beside the video.
    Subtitle,
}

/// A screen's navigation, held while a popover borrows the keyboard.
struct NavState {
    list: Option<gtk::ListBox>,
    header: Vec<gtk::Button>,
    footer: Vec<gtk::Button>,
    header_entry: Option<gtk::Button>,
    stops: Vec<gtk::Widget>,
    copy_root: Option<gtk::Widget>,
}

/// What the alignment thread has to say for itself, on its way back to the
/// main thread.
enum Step {
    /// How many of the three windows have finished.
    Window(usize),
    Done(crate::align::Verdict),
}

/// How far the Quick Connect thread has got, on its way back to the main
/// thread.
///
/// One channel for the whole pairing rather than one per stage: asking for a
/// code and waiting for it to be approved are two halves of one errand, and
/// the panel shows them in the same place.
enum QuickConnect {
    /// What the server calls itself, asked before anything else because it is
    /// unauthenticated and cheap, and because a panel that can say which
    /// server it is talking to should say so before asking for a code.
    Named(String),
    /// The six characters to show, once the server has issued them.
    Code(String),
    /// Approved, with the account it granted. Boxed because it is much the
    /// largest of the three, and every message would otherwise be its size.
    Done(Box<crate::jellyfin::Account>),
    /// Refused, expired, or a server that could not be reached. All of them
    /// end the same way: say so, and let another code be asked for.
    Failed(String),
}

/// Choices given on the command line, which skip the menu entirely.
#[derive(Clone)]
pub struct Preset {
    /// A track number as `--list-tracks` prints them, a language code, `ad`,
    /// or `en:ad`. See [`crate::probe::resolve_audio`].
    pub primary: Option<String>,
    pub secondary: Option<String>,
    /// One particular subtitle - a number, a language code, a file name - or
    /// a kind to choose automatically, optionally with a language after a
    /// colon. See [`crate::subtitles::resolve`].
    pub subtitle: Option<String>,
}

/// How this run was started, as against what it should play.
///
/// Grouped because they arrive together from the command line and are read
/// together here, and because the list was going to keep growing.
#[derive(Clone, Copy)]
pub struct Launch {
    /// Ignore any saved position and start from the beginning.
    pub restart: bool,
    pub fullscreen: bool,
    /// Fullscreen is not the viewer's to change: a launcher asked for it and
    /// is waiting for this playback, so the controls for it are gone rather
    /// than present and refusing.
    pub locked_fullscreen: bool,
    /// Something else chose the video and is waiting for this playback.
    pub external: bool,
    /// That something else is Kodi, which can also be talked to.
    pub kodi: bool,
    /// Start playing rather than opening the menu.
    pub play: bool,
}

/// Everything the menu can act on. Devices persist to the config file;
/// the file and track choices last for the session.
pub struct App {
    window: gtk::ApplicationWindow,
    /// Holds the display awake while a film is playing. See [`crate::awake`].
    awake: crate::awake::KeepAwake,
    config: RefCell<Config>,
    /// What the version check has found, and what has been seen of it.
    /// Held here so the settings screen and the badge on the button that
    /// opens it read the same answer.
    updates: RefCell<crate::updates::State>,
    /// The buttons currently on screen that should carry the mark when
    /// a new version is waiting to be seen.
    update_badges: RefCell<Vec<gtk::Button>>,
    file: RefCell<Option<Source>>,
    /// What is known about the file on screen: its name, its artwork, and
    /// whatever a sidecar or the container itself had to say. Default when
    /// there is no file, and default is a perfectly good page - most files
    /// have no sidecar and the layout is designed up from that case.
    details: RefCell<crate::metadata::Details>,
    /// Whether a now-playing update is already queued behind the main loop.
    ///
    /// Set while one is pending so that a scrub, which commits a seek on every
    /// release, does not queue a copy of the poster for each of them.
    now_playing_queued: Cell<bool>,
    /// Artwork already decoded for the file on screen, keyed by nothing more
    /// than being the current file: it is dropped whenever one is loaded.
    ///
    /// Held so that returning from a chooser redraws the page instantly. The
    /// menu is rebuilt on every trip in and out of one, and re-reading a
    /// backdrop from a network share each time is both slow and visible.
    /// Whether the page being built already has artwork that has only just
    /// arrived, and so should fade rather than appear.
    ///
    /// A page opened with its pictures already in hand draws them; it does not
    /// perform.
    fade_art: Cell<bool>,
    /// The two places artwork is drawn, kept so that a picture arriving late
    /// can be put into the page that is already on screen.
    ///
    /// Rebuilding the page instead would be simpler and is what this used to
    /// do. It is wrong: artwork can take seconds - a backdrop over a network
    /// especially - and by then somebody may be part-way down the track lists
    /// choosing what to watch. Rebuilding under them moves their focus and
    /// undoes what they were doing, to deliver a picture they did not ask for
    /// yet.
    backdrop_widget: RefCell<Option<crate::artwork::Artwork>>,
    poster_frame: RefCell<Option<gtk::Box>>,
    /// The small frame under the file details that holds the series' poster,
    /// kept for the same reason `poster_frame` is: the picture is fetched on a
    /// thread and arrives after the page it belongs on.
    series_frame: RefCell<Option<gtk::Box>>,
    poster_art: RefCell<Option<gdk::Texture>>,
    backdrop_art: RefCell<Option<gdk::Texture>>,
    /// The series' poster, for an episode. Kept apart from `poster_art`
    /// because they are two different pictures of two different things: that
    /// one is a still from this episode, this one is the show.
    series_art: RefCell<Option<gdk::Texture>>,
    /// Bumped whenever a different file is loaded, so artwork still arriving
    /// from a thread for the previous one is dropped rather than drawn.
    art_generation: Cell<u64>,
    /// The output devices as last enumerated, and whether an enumeration has
    /// ever finished.
    ///
    /// Held because finding them is not cheap: it starts a GStreamer device
    /// monitor, which probes every audio backend on the machine and takes long
    /// enough on the main thread to be seen as lag when a menu opens. The list
    /// changes only when hardware is plugged in or unplugged, so it is worth
    /// keeping between openings rather than asking again each time.
    device_names: RefCell<Vec<String>>,
    device_scan: Cell<bool>,
    /// The rebuild waiting for a drag-resize to stop, and the poster height
    /// the page on screen was built at. See [`App::rebuild_when_resize_ends`].
    resize_settle: RefCell<Option<glib::SourceId>>,
    built_poster: Cell<f64>,
    tracks: RefCell<Vec<AudioTrack>>,
    primary_track: RefCell<Option<u32>>,
    secondary_track: RefCell<Option<u32>>,
    /// A separate audio file feeding an output, in place of any track inside
    /// the video. Takes precedence over that output's track, which is left
    /// alone so it comes back if the file is cleared.
    primary_file: RefCell<Option<Source>>,
    secondary_file: RefCell<Option<Source>>,
    /// Separate soundtracks sitting beside the video and named after it, found
    /// the way subtitle files beside it are found. Offered in both outputs'
    /// track lists, so a described or dubbed track downloaded next to a film
    /// is simply there rather than something to go looking on disk for.
    ///
    /// What is *found* rather than what is chosen: an output's own choice is
    /// `primary_file`/`secondary_file` above, which may be one of these or a
    /// file from anywhere else on disk.
    audio_files: RefCell<Vec<crate::beside::AudioFile>>,
    /// Which output the browser is picking a soundtrack for, or `None` when it
    /// is picking a video. Held here because stepping into a folder re-enters
    /// the browser and would otherwise lose the errand it was opened on.
    /// What the browser is open for. One value rather than a flag per errand:
    /// the browser, the system dialog and the row handler all ask this, and
    /// two flags could answer differently.
    errand: Cell<Errand>,
    /// What alignment worked out for each output, in milliseconds, ready to
    /// add to whatever the viewer has set. Already negated: alignment reports
    /// how late the audio runs, and a sink is held back by a negative offset.
    ///
    /// Zero when nothing has been measured, which is most of the time - so the
    /// arithmetic below is the same whether there is a baseline or not.
    primary_baseline: Cell<f64>,
    secondary_baseline: Cell<f64>,
    /// The video's running time in seconds, which alignment needs to place its
    /// three windows across it. Zero when the source could not say, which some
    /// live streams cannot.
    duration_s: Cell<f64>,
    /// Everything on offer for the current file: streams inside it, then
    /// subtitle files sitting beside it.
    subtitle_options: RefCell<Vec<Subtitle>>,
    subtitle: RefCell<Option<SubtitleChoice>>,
    /// Whether the subtitle showing was chosen by a person rather than worked
    /// out from the preference.
    ///
    /// The difference only matters when a soundtrack changes: the preference
    /// is written in terms of the outputs, so its answer can go stale, and
    /// [`App::follow_audio_with_subtitle`] brings it up to date. A choice
    /// somebody made is not stale and is never revisited. Cleared when a video
    /// opens, because it is a fact about this sitting rather than this file.
    subtitle_by_hand: Cell<bool>,
    playback: RefCell<Option<Rc<Playback>>>,
    screen: RefCell<Screen>,
    /// Restored when returning from a chooser, so the menu comes back with
    /// the row you left from still highlighted.
    menu_row: RefCell<i32>,
    settings_row: RefCell<i32>,
    sounds: RefCell<Sounds>,
    restart: bool,
    /// The list the current screen is built around, and the button below it.
    ///
    /// The keyboard reaches these through GTK's own focus handling, but the
    /// gamepad has no events to hand to GTK, so it needs to move the
    /// selection itself and therefore needs to know what it is moving.
    nav_list: RefCell<Option<gtk::ListBox>>,
    /// A second list beside the main one, waiting to be put into the tab
    /// order.
    ///
    /// Held rather than added directly because a screen builds its column
    /// before it wires its navigation, and `set_nav` rebuilds the order from
    /// scratch - so anything added ahead of it was thrown away again.
    nav_side_list: RefCell<Option<gtk::ListBox>>,
    /// What Tab moves between on this screen, in order: the header buttons,
    /// the lists, then the footer buttons.
    ///
    /// Kept because GTK will not do it. A GtkListBox implements focus
    /// traversal by moving between its rows, so once no row can take focus it
    /// reports that it cannot be focused at all and Tab steps straight over
    /// it - even though focusing the list directly works perfectly well.
    nav_stops: RefCell<Vec<gtk::Widget>>,
    /// The sliders on the settings screen, by the row each one sits in, so
    /// left and right can find the one that is selected. Emptied whenever a
    /// screen without them is built.
    settings_sliders: RefCell<Vec<(Item, Slider, gtk::Scale, gtk::Label)>>,
    /// Which category the settings screen is showing, kept so leaving and
    /// coming back lands where it was left rather than at the top.
    settings_category: Cell<Category>,
    /// Whether the keyboard is in the settings themselves rather than in the
    /// column of categories.
    ///
    /// The screen is entered a step at a time: the categories take the keys
    /// first, Enter hands them to the settings beside them, and Escape hands
    /// them back before it leaves the screen. Left and right cannot do that
    /// job here - they belong to the bars on half these rows, and a row
    /// without one would have moved the focus off the pane instead.
    in_settings_pane: Cell<bool>,
    /// What the right-hand pane is showing, by row. The one place a row's
    /// position is turned back into what it is.
    pane_items: RefCell<Vec<Item>>,
    /// The About page's scroll position, so up and down can move a page that
    /// has nothing on it to select.
    about_scroll: RefCell<Option<gtk::Adjustment>>,
    /// Where selectable text lives on the screen being shown, so Ctrl+C can
    /// find it. Set by the screens that have any, cleared by every other.
    copy_root: RefCell<Option<gtk::Widget>>,
    /// The switches on the settings screen, by row, so a toggle can move the
    /// one it belongs to instead of rebuilding the screen under the viewer.
    settings_switches: RefCell<Vec<(Item, gtk::Switch)>>,
    /// The settings list itself, so a row can be redrawn without rebuilding
    /// the screen around it.
    settings_list: RefCell<Option<gtk::ListBox>>,
    /// The column of categories beside it, so the keyboard can be handed back
    /// to it from outside the function that built it.
    settings_categories: RefCell<Option<gtk::ListBox>>,
    /// What a category says above its rows, where it says anything. Only About
    /// does: its text used to be a screen of its own, two steps away from the
    /// row that named it.
    settings_body: RefCell<Option<gtk::Box>>,
    /// The Kodi installations the Kodi pane was last built from, so a
    /// row can say what it is and act on it without scanning the disk again
    /// for every label it draws.
    kodi_setups: RefCell<Vec<crate::kodi_setup::Setup>>,
    /// Whether the settings row about to be activated was clicked rather than
    /// chosen with a key or a gamepad. A switch row responds to a press on
    /// the switch itself, not to a click anywhere along the row - but Enter
    /// on the selected row must still work it, and both arrive here as the
    /// same activation.
    clicked_row: Cell<bool>,
    /// Set while a switch is being moved to match what it already reports, so
    /// its own handler knows not to act on it.
    settling_switch: Cell<bool>,
    /// Whether the key that works the highlighted control is still down, so
    /// that holding it acts once rather than on every repeat the keyboard
    /// sends.
    key_held: Cell<bool>,
    /// Whether the press now in progress started the volume button's hold,
    /// and so still has the ordinary press to do when the key comes up.
    hold_started: Cell<bool>,
    /// Counts releases, so one waiting to be believed can be dropped when a
    /// repeat arrives behind it.
    releases: Cell<u64>,
    /// The size a drag has reached, kept until the bar is let go. Nothing
    /// while the size is not being dragged.
    wanted_scale: Cell<Option<f64>>,
    nav_footer: RefCell<Vec<gtk::Button>>,
    /// Buttons above the list: the browser's path trail, and the media page's
    /// play and settings row. Up from the first row reaches them, the way Down
    /// reaches the footer.
    nav_header: RefCell<Vec<gtk::Button>>,
    /// A row of buttons between the header and the footer, for the one screen
    /// that has three: the empty page, where choosing a video and connecting a
    /// server are not the same errand and do not sit on the same line.
    ///
    /// Set after `set_nav`, which clears it, so it belongs to one screen only -
    /// the same way `nav_header_entry` does.
    nav_middle: RefCell<Vec<gtk::Button>>,
    /// Which header button Up from the list should land on, where the last
    /// one is not the right answer.
    ///
    /// The default is the last, which is what a path trail wants: the crumb
    /// nearest the list is the folder you are in. A row of actions wants the
    /// opposite - the first is the one the page is for, and arriving on
    /// Settings when you meant Play is a button's width of travel every time.
    nav_header_entry: RefCell<Option<gtk::Button>>,
    controls: RefCell<Option<Rc<Controls>>>,
    /// Whether the window was already maximized when fullscreen was entered,
    /// so that leaving fullscreen can put back the state it found rather than
    /// the one fullscreen implies. See [`App::toggle_fullscreen`].
    maximized_before_fullscreen: Cell<bool>,
    /// The size the window last had while it was an ordinary window, kept so
    /// it can be written down on the way out.
    ///
    /// Tracked rather than read at the end, because by then it may not be one:
    /// a window closed while maximized or fullscreen reports the screen, and
    /// saving that would mean opening at screen size for ever after with
    /// nothing to go back to.
    windowed_size: Cell<(i32, i32)>,
    /// Kept so the interface can be re-scaled after the fact.
    styles: gtk::CssProvider,
    /// The scale in force, which the settings screen reports and the
    /// monitor check below may revise.
    scale: Cell<f64>,
    /// Bumped whenever a scrub ends, retiring the ticker that was driving it.
    scrub_generation: Cell<u64>,
    /// Last time a scrub key or button was seen held.
    scrub_seen: Cell<Option<std::time::Instant>>,
    /// Drives the controls readout while a file is playing.
    tick: RefCell<Option<glib::SourceId>>,
    /// Something else chose the video and is waiting for this playback of it:
    /// no browser, no drag and drop, no confirmation on the way out. Set by
    /// `--external`, and by `--kodi`, which implies it.
    external: bool,
    /// Whether fullscreen is fixed for this run. See [`Launch`].
    locked_fullscreen: bool,
    /// Whether the error on screen ended the session: a video named on the
    /// command line that could not be opened leaves nothing to go back to, so
    /// its button closes the player. Every other error returns to the menu.
    error_is_fatal: Cell<bool>,
    /// What Kodi says it is playing through us: its title, database id, resume
    /// point, and the path to report progress against. Fetched once at startup,
    /// because it cannot change while we are the player. `None` when Kodi was
    /// not involved or did not answer, which is not an error.
    kodi_item: RefCell<Option<crate::kodi::Item>>,
    /// The server this installation is paired with, once it has been reached.
    ///
    /// Absent when Jellyfin was never set up, when the pairing was revoked,
    /// and while the server is unreachable - all of which are ordinary, and
    /// none of which stop anything else working.
    jellyfin: RefCell<Option<crate::jellyfin::Client>>,
    /// What the pairing file says, as the settings pane last read it.
    ///
    /// Held rather than read per row, for the same reason the Kodi
    /// installations are: every label, value and enabled state on that pane
    /// comes out of this, and a file read apiece would be a dozen for one
    /// screen. Re-read whenever the pane is built, so a token revoked from
    /// elsewhere shows up on the next visit.
    jellyfin_pairing: RefCell<Option<crate::jellyfin::Pairing>>,
    /// Bumped whenever a Quick Connect is started or abandoned, so the polling
    /// left over from one attempt cannot outlive it and approve another.
    jellyfin_attempt: Cell<u64>,
    /// Which screen the connection flow was opened from, and so where
    /// finishing or cancelling it returns to.
    connect_from: Cell<ConnectFrom>,
    /// What Jellyfin knows about the video on screen, when it was cast from
    /// there. The counterpart to `kodi_item`, and read by the same three
    /// accessors: a launcher's library knows the title and where the viewer
    /// stopped better than the file does.
    jellyfin_item: RefCell<Option<crate::jellyfin::Item>>,
    /// The open connection. Dropping it closes the socket, which is how
    /// disconnecting works - and while it is closed TinePlayer is not on
    /// anybody's phone, so it is held for the life of the application.
    jellyfin_session: RefCell<Option<crate::jellyfin::Session>>,
    /// Ticks since Jellyfin was last told where playback had reached.
    jellyfin_reported: Cell<u32>,
    /// What Jellyfin calls this viewing. One string for the whole of it, from
    /// started to stopped, because that is how the server ties the reports
    /// together into a single session rather than three unrelated events.
    jellyfin_play_session: RefCell<String>,
    /// Where playback had reached when it was last left, and the video it
    /// belongs to. Offered as a resume point regardless of how far in it was,
    /// unlike a position read back from disk.
    ///
    /// The saved-position rules exist to answer "were you part way through
    /// this, days ago" - a minute into a long film is a false start rather
    /// than progress. Within one session that question is already answered:
    /// you were watching it a moment ago. Backing out to change a setting and
    /// losing your place is the exact annoyance those rules guard against.
    session_resume: RefCell<Option<(String, u64)>>,
    /// Whether subtitles were switched off during this video, so leaving
    /// playback and coming back does not turn them on again. Cleared when a
    /// different video is loaded, or when a different subtitle is chosen -
    /// picking one is asking to see it.
    subtitles_hidden: Cell<bool>,
    /// Whether a volume change is waiting to be written out. Dragging a slider
    /// produces a change per pixel, and each one would otherwise be a write to
    /// disk.
    volume_save_pending: Cell<bool>,
    /// Whether everything is silenced at once. Held here rather than in the
    /// configuration, and separately from each output's own mute, because it is
    /// a layer over them: the outputs go on being set to whatever they were set
    /// to underneath it, and a film that started silent because of a door
    /// knocked on last week would be a bug rather than a memory.
    hushed: Cell<bool>,
    /// Whether a report of what the sound is doing is already on its way to
    /// Jellyfin. Dragging a bar produces a change per pixel, and each one would
    /// otherwise be a request across the house.
    sound_report_pending: Cell<bool>,
    /// The state of a hold on the gamepad's left face button, which silences
    /// everything rather than changing the subtitles. The same button for
    /// both because they are the same question - whether you are being given
    /// the sound or the words. Kept here rather than in the controls: it is
    /// about what a button meant, not about the strip, and it works whether
    /// or not the strip is on screen.
    subtitles_hold: Cell<u64>,
    subtitles_holding: Cell<bool>,
    subtitles_held: Cell<bool>,
    /// The screen a modal was opened from, so backing out of one returns
    /// there. Reached by shortcut as well as by row, so it cannot be assumed
    /// to be the step that offers them.
    origin: Cell<Screen>,
}

#[cfg(test)]
mod summary_lines {
    use super::{MOST_LANGUAGES, summary_markup};

    fn languages(count: usize) -> Vec<String> {
        (0..count).map(|i| format!("Lang{i}")).collect()
    }

    /// The rule applies to whichever line is handed too many, not to the one
    /// that happens to show it most. The subtitle line is usually the long
    /// one, so an audio line built by different code would go untested.
    #[test]
    fn either_line_counts_what_it_left_out() {
        for name in ["Audio", "Subtitles"] {
            let line = summary_markup(name, &languages(MOST_LANGUAGES + 5));
            assert!(line.contains("+5 more"), "{name} did not count the rest");
            assert!(line.starts_with(&format!("<span alpha='60%'>{name}:</span>")));
        }
    }

    /// Exactly at the limit is a complete list, and saying "+0 more" about it
    /// would be both noise and a lie.
    #[test]
    fn nothing_left_over_is_said_about() {
        let line = summary_markup("Audio", &languages(MOST_LANGUAGES));
        assert!(!line.contains("more"));
        assert!(line.contains("Lang0"));
        assert!(line.contains(&format!("Lang{}", MOST_LANGUAGES - 1)));
    }

    /// Only as many as fit are named, whatever it was given.
    #[test]
    fn never_names_more_than_the_limit() {
        let line = summary_markup("Subtitles", &languages(40));
        assert_eq!(line.matches("Lang").count(), MOST_LANGUAGES);
        assert!(line.contains(&format!("+{} more", 40 - MOST_LANGUAGES)));
    }

    /// A file with no such track says so, rather than showing an empty line
    /// where a list should be.
    #[test]
    fn nothing_at_all_says_so() {
        let line = summary_markup("Subtitles", &[]);
        assert!(line.contains(super::NO_TRACKS));
        assert!(!line.contains("more"));
    }

    /// Track titles come from files and can hold anything. An unescaped
    /// ampersand is not a stray character here - it makes the markup invalid,
    /// and GTK draws nothing at all for the whole line.
    #[test]
    fn a_language_named_with_markup_cannot_break_the_line() {
        let line = summary_markup("Audio", &["Ol' <b>Bill</b> & Ben".to_string()]);
        assert!(line.contains("&amp;"));
        assert!(line.contains("&lt;b&gt;"));
    }
}

#[cfg(test)]
mod readings {
    use super::{offset_label, volume_label};

    /// The sign is the whole reading: it says which way the sound moves, and
    /// it is the only thing separating the two directions now that the words
    /// are gone.
    #[test]
    fn a_shifted_output_reads_with_its_direction() {
        assert_eq!(offset_label(150.0), "+150ms");
        assert_eq!(offset_label(-150.0), "-150ms");
        assert_eq!(offset_label(crate::config::MAX_OFFSET_MS), "+1000ms");
        assert_eq!(offset_label(-crate::config::MAX_OFFSET_MS), "-1000ms");
    }

    /// Unshifted is a plain zero and never a signed one. Rounding a small
    /// negative gives -0, which formats as "-0ms" and claims a shift that is
    /// not there.
    #[test]
    fn an_unshifted_output_reads_without_a_sign() {
        assert_eq!(offset_label(0.0), "0ms");
        assert_eq!(offset_label(-0.0), "0ms");
        assert_eq!(offset_label(-0.4), "0ms");
        assert_eq!(offset_label(0.4), "0ms");
    }

    /// Sliders move in tens but a stored value can be anything, including
    /// something written into the config file by hand.
    #[test]
    fn a_reading_is_rounded_to_the_millisecond() {
        assert_eq!(offset_label(149.6), "+150ms");
        assert_eq!(offset_label(-149.6), "-150ms");
    }

    /// Every reading has to fit the space kept for it, or it widens the label
    /// and moves the bar beside it.
    #[test]
    fn every_reading_fits_the_space_kept_for_it() {
        let longest = [
            offset_label(-crate::config::MAX_OFFSET_MS),
            offset_label(crate::config::MAX_OFFSET_MS),
            volume_label(1.0, false),
            volume_label(0.0, true),
        ];
        for reading in longest {
            assert!(
                reading.chars().count() <= super::READING_CHARS as usize,
                "{reading:?} is wider than the space kept for it"
            );
        }
    }

    /// A silenced output says so rather than showing the level it will come
    /// back to, which would read as though it were playing.
    #[test]
    fn a_silenced_output_says_so_whatever_its_level() {
        assert_eq!(volume_label(0.75, true), "Muted");
        assert_eq!(volume_label(0.0, true), "Muted");
        assert_eq!(volume_label(0.75, false), "75%");
        assert_eq!(volume_label(0.0, false), "0%");
        assert_eq!(volume_label(1.0, false), "100%");
    }
}

#[cfg(test)]
mod settings_rows {
    use super::*;

    use crate::kodi_setup::Confinement;

    /// The Kodi installations these tests pretend were found. Two, so that the
    /// repeated rows are actually repeated - with one there is no difference
    /// between "a group per installation" and "a group" - and both ordinary,
    /// so the count below is not also counting a sandbox's extra row.
    fn kodis() -> Vec<KodiPane> {
        vec![
            KodiPane {
                heading: "KODI 21.1 (STANDARD)".to_string(),
                confinement: Confinement::None,
            },
            KodiPane {
                heading: "KODI 20.5 (CUSTOM)".to_string(),
                confinement: Confinement::None,
            },
        ]
    }

    /// How many rows an ordinary installation contributes: what type of player
    /// it is, and what it does when it hands a video over.
    const ROWS_PER_KODI: usize = 2;

    /// Every row the whole screen holds, for one state of the Jellyfin
    /// pairing.
    fn every_row(jellyfin: JellyfinPane) -> Vec<Item> {
        Category::ALL
            .iter()
            .flat_map(|category| category.items(&kodis(), jellyfin))
            .map(|(_, item)| item)
            .collect()
    }

    /// Every setting is somewhere, and nowhere twice.
    ///
    /// This is what the old numbering could not promise. Rows were positions
    /// in one list, so a stale number silently built a control onto the wrong
    /// row and left another as a plain line of text - which is what happened
    /// to Preferred Language, and it read as a missing setting rather than a
    /// bug. Categories make losing one easy in a new way: an item can simply
    /// be left out of every list and never appear at all.
    #[test]
    fn every_item_appears_in_exactly_one_category() {
        // Both states of the pairing, because the Jellyfin pane shows
        // different rows in each - and a row placed in neither would be a row
        // nobody can ever reach.
        for jellyfin in [JellyfinPane::NotConnected, JellyfinPane::Connected] {
            let all = every_row(jellyfin);
            for item in &all {
                let count = all.iter().filter(|other| *other == item).count();
                assert_eq!(count, 1, "an item appears {count} times");
            }
            // Written out rather than derived, so adding a setting and
            // forgetting to place it fails here instead of at a glance. It is
            // not the number of `Item` variants: the five an output has are
            // placed once for each output, and the Kodi category holds a group
            // of rows per Kodi found, plus the one row that belongs to no
            // installation and names another by hand.
            // 27 since 2026-08-24: the subtitle preference became two rows,
            // one for the kind and one for the language.
            let elsewhere = 27;
            let kodi = ROWS_PER_KODI * kodis().len() + 1;
            // One row either way: the way in, or the way out.
            let paired = 1;
            assert_eq!(all.len(), elsewhere + kodi + paired);
        }

        // And between the two states, every Jellyfin row is reachable.
        let both: Vec<Item> = every_row(JellyfinPane::NotConnected)
            .into_iter()
            .chain(every_row(JellyfinPane::Connected))
            .collect();
        for item in [Item::JellyfinConnect, Item::JellyfinDisconnect] {
            assert!(both.contains(&item), "{item:?} is on no pane at all");
        }
    }

    /// The pane says one thing or the other, and never both: a Connect on a
    /// pane that is already connected, or a Disconnect on one with nothing to
    /// disconnect from, would each mean the opposite of what it says.
    #[test]
    fn the_jellyfin_pane_takes_two_shapes() {
        let rows = |state| -> Vec<Item> {
            Category::Jellyfin
                .items(&[], state)
                .into_iter()
                .map(|(_, item)| item)
                .collect()
        };
        assert_eq!(
            rows(JellyfinPane::NotConnected),
            vec![Item::JellyfinConnect]
        );
        assert_eq!(
            rows(JellyfinPane::Connected),
            vec![Item::JellyfinDisconnect]
        );
        // One row under one heading, in both.
        for state in [JellyfinPane::NotConnected, JellyfinPane::Connected] {
            let rows = Category::Jellyfin.items(&[], state);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0.as_deref(), Some("JELLYFIN"));
        }
    }

    /// Every installation heads its own group, and the row that adds one by
    /// hand belongs to none of them.
    ///
    /// This is the shape the wizard's five screens came down to. The three
    /// things it asked - which Kodi, what type of player, what to do on
    /// handover - are the heading and the two rows under it.
    #[test]
    fn each_installation_heads_its_own_group() {
        let rows = Category::Kodi.items(&kodis(), JellyfinPane::NotConnected);
        let headed: Vec<(String, Item)> = rows
            .iter()
            .filter_map(|(heading, item)| heading.as_ref().map(|text| (text.to_string(), *item)))
            .collect();
        assert_eq!(
            headed,
            vec![
                ("KODI 21.1 (STANDARD)".to_string(), Item::KodiType(0)),
                ("KODI 20.5 (CUSTOM)".to_string(), Item::KodiType(1)),
                ("OTHER".to_string(), Item::KodiAdd),
            ]
        );
        // Each installation's rows carry its own index, or a change made on
        // one group would land on another.
        let items: Vec<Item> = rows.iter().map(|(_, item)| *item).collect();
        assert_eq!(
            items,
            vec![
                Item::KodiType(0),
                Item::KodiHandover(0),
                Item::KodiType(1),
                Item::KodiHandover(1),
                Item::KodiAdd,
            ]
        );
    }

    /// How an installation was made decides which rows it gets. A Snap cannot
    /// start an external player at all, so it has nothing to set; a Flatpak
    /// can, once it is given permission, so it has somewhere to say so.
    #[test]
    fn a_sandbox_changes_which_rows_an_installation_has() {
        let sandboxed = |confinement| {
            Category::Kodi
                .items(
                    &[KodiPane {
                        heading: "KODI".to_string(),
                        confinement,
                    }],
                    JellyfinPane::NotConnected,
                )
                .into_iter()
                .map(|(_, item)| item)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sandboxed(Confinement::Snap),
            vec![Item::KodiType(0), Item::KodiAdd]
        );
        assert_eq!(
            sandboxed(Confinement::Flatpak),
            vec![
                Item::KodiType(0),
                Item::KodiHandover(0),
                Item::KodiPermission(0),
                Item::KodiAdd,
            ]
        );
    }

    /// With nothing found the pane says so, rather than offering only a way to
    /// add something and leaving open whether it ever looked.
    #[test]
    fn an_empty_pane_says_why_it_is_empty() {
        let rows = Category::Kodi.items(&[], JellyfinPane::NotConnected);
        let items: Vec<Item> = rows.iter().map(|(_, item)| *item).collect();
        assert_eq!(items, vec![Item::KodiNone, Item::KodiAdd]);
        // One heading over both, since the row saying nothing was found and
        // the row that does something about it are the same subject.
        assert_eq!(rows[0].0.as_deref(), Some("KODI"));
        assert_eq!(rows[1].0, None);
    }

    /// The version sits under the switch that decides whether anything is
    /// said about newer ones. Read the other way round it is a status with no
    /// stated relationship to the control above it.
    #[test]
    fn the_version_follows_the_update_switch() {
        let general: Vec<Item> = Category::General
            .items(&kodis(), JellyfinPane::NotConnected)
            .into_iter()
            .map(|(_, item)| item)
            .collect();
        let switch = general.iter().position(|item| *item == Item::Updates);
        let status = general.iter().position(|item| *item == Item::UpdateStatus);
        assert_eq!(status, switch.map(|at| at + 1));
    }

    /// Clear Data destroys something, and belongs with the settings that
    /// decide what there is to destroy: it closes the resuming group rather
    /// than sitting alone at the end of the screen, which is where it was
    /// until the switch above it existed.
    #[test]
    fn clearing_data_closes_the_resuming_group() {
        let general: Vec<Item> = Category::General
            .items(&kodis(), JellyfinPane::NotConnected)
            .into_iter()
            .map(|(_, item)| item)
            .collect();
        let at = |item: Item| general.iter().position(|other| *other == item);
        assert_eq!(
            at(Item::ClearData),
            at(Item::WatchedThreshold).map(|n| n + 1)
        );
        // And the group it closes is the one the switch opens.
        assert!(at(Item::RememberPositions) < at(Item::ClearData));
    }

    /// A row carries a switch or a bar or neither, and the two that carry
    /// both - the pair whose bar can be turned off - are deliberate. What must
    /// not happen is a row claiming a switch it was never built with, since
    /// activating it would then do nothing at all.
    #[test]
    fn every_switch_row_has_something_to_switch() {
        for (_, item) in Category::ALL
            .iter()
            .flat_map(|category| category.items(&kodis(), JellyfinPane::Connected))
        {
            if item.has_switch() {
                assert!(
                    item.setting().is_none(),
                    "a row cannot both open a chooser and hold a switch"
                );
            }
        }
    }
}

/// Which of Jellyfin's three reporting endpoints a moment belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum JellyfinMoment {
    Started,
    Progress,
    Stopped,
}
