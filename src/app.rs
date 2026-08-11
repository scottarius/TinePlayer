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
use crate::player::Playback;
use crate::probe::AudioTrack;
use crate::sound::Sounds;
use crate::subtitles::{Subtitle, SubtitleChoice};

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
const UNKNOWN_LANGUAGE: &str = "Unknown";

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
fn summary_markup(name: &str, languages: &[String]) -> String {
    let shown = match languages.is_empty() {
        true => NO_TRACKS.to_string(),
        false => languages
            .iter()
            .take(MOST_LANGUAGES)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
    };
    let more = match languages.len().saturating_sub(MOST_LANGUAGES) {
        0 => String::new(),
        extra => format!(", <span alpha='60%'>+{extra} more</span>"),
    };
    format!(
        "<span alpha='60%'>{name}:</span> {}{more}",
        glib::markup_escape_text(&shown),
    )
}

/// What a summary line says when the file carries no such track at all.
///
/// Distinct from `Unknown`, and the difference is worth keeping: one means
/// there is a track and nobody said what language it is in, the other means
/// there is nothing there to choose.
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
    SubtitleLanguage,
    SubtitleFont,
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
    About,
    Notices,
    Kodi,
    /// The screens of the Kodi wizard. None of them writes anything: only
    /// Configure on the summary does.
    KodiChoose,
    KodiFolder,
    KodiHow,
    KodiHandover,
    KodiManual,
    KodiSummary,
    KodiConfirm,
    KodiError,
    KodiDone,
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
    /// Entries with a rule drawn above them, by index. Only the subtitle
    /// preference has any: it offers three unlike things in one list - nothing,
    /// four ways of following an output, and two hundred languages - and
    /// without the rules they read as one long undifferentiated run.
    dividers: Vec<usize>,
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
    Sounds,
    StartFullscreen,
    ReadMetadata,
    ShowBackdrop,
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
    SubtitleSize,
    SubtitleFont,
    Kodi,
    About,
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
            Item::SubtitlePreference => Setting::SubtitleLanguage,
            Item::SubtitleFont => Setting::SubtitleFont,
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
                | Item::Description(_)
                | Item::Volume(_)
                | Item::Sync(_)
                | Item::Updates
        )
    }
}

/// The left column of the settings screen, and what each of its entries holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    General,
    Outputs,
    Subtitles,
    Integrations,
    About,
}

impl Category {
    const ALL: [Category; 5] = [
        Category::General,
        Category::Outputs,
        Category::Subtitles,
        Category::Integrations,
        Category::About,
    ];

    fn title(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Outputs => "Outputs",
            Category::Subtitles => "Subtitles",
            Category::Integrations => "Integrations",
            Category::About => "About",
        }
    }

    /// What the right-hand pane shows, and the heading each group opens with.
    ///
    /// The headings are what make Outputs readable: it holds two rows called
    /// Volume and two called Audio Sync, and until now they were told apart
    /// only by which half of the list they were in.
    fn items(self) -> Vec<(Option<&'static str>, Item)> {
        match self {
            Category::General => vec![
                (None, Item::InterfaceScale),
                (None, Item::Sounds),
                (None, Item::StartFullscreen),
                (Some("LIBRARY"), Item::ReadMetadata),
                (None, Item::ShowBackdrop),
                (None, Item::ResumeThreshold),
                (None, Item::WatchedThreshold),
                (Some("UPDATES"), Item::Updates),
                (None, Item::UpdateStatus),
                // Last, and alone under its own heading: it is the one thing
                // on this screen that destroys something.
                (Some("DATA"), Item::ClearData),
            ],
            Category::Outputs => vec![
                (Some("FIRST OUTPUT"), Item::Device(Role::Primary)),
                (None, Item::Language(Role::Primary)),
                (None, Item::Description(Role::Primary)),
                (None, Item::Volume(Role::Primary)),
                (None, Item::Sync(Role::Primary)),
                (Some("SECOND OUTPUT"), Item::Device(Role::Secondary)),
                (None, Item::Language(Role::Secondary)),
                (None, Item::Description(Role::Secondary)),
                (None, Item::Volume(Role::Secondary)),
                (None, Item::Sync(Role::Secondary)),
            ],
            Category::Subtitles => vec![
                (None, Item::SubtitlePreference),
                (None, Item::SubtitleSize),
                (None, Item::SubtitleFont),
            ],
            Category::Integrations => vec![(None, Item::Kodi)],
            Category::About => vec![(None, Item::About), (None, Item::Notices)],
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

/// Choices given on the command line, which skip the menu entirely.
#[derive(Clone)]
pub struct Preset {
    /// A track number as `--list-tracks` prints them, a language code, `ad`,
    /// or `en:ad`. See [`crate::probe::resolve_audio`].
    pub primary: Option<String>,
    pub secondary: Option<String>,
    /// A number, a language code, or a subtitle file name beside the video.
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
    /// Artwork already decoded for the file on screen, keyed by nothing more
    /// than being the current file: it is dropped whenever one is loaded.
    ///
    /// Held so that returning from a chooser redraws the page instantly. The
    /// menu is rebuilt on every trip in and out of one, and re-reading a
    /// backdrop from a network share each time is both slow and visible.
    poster_art: RefCell<Option<gdk::Texture>>,
    backdrop_art: RefCell<Option<gdk::Texture>>,
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
    /// What the right-hand pane is showing, by row. The one place a row's
    /// position is turned back into what it is.
    pane_items: RefCell<Vec<Item>>,
    /// The About page's scroll position, so up and down can move a page that
    /// has nothing on it to select.
    about_scroll: RefCell<Option<gtk::Adjustment>>,
    /// Where selectable text lives on the screen being shown, so Ctrl+C can
    /// find it. Set by the screens that have any, cleared by every other.
    copy_root: RefCell<Option<gtk::Widget>>,
    /// What the Kodi wizard has been told so far. `None` outside the wizard.
    kodi_draft: RefCell<Option<KodiDraft>>,
    /// The switches on the settings screen, by row, so a toggle can move the
    /// one it belongs to instead of rebuilding the screen under the viewer.
    settings_switches: RefCell<Vec<(Item, gtk::Switch)>>,
    /// The settings list itself, so a row can be redrawn without rebuilding
    /// the screen around it.
    settings_list: RefCell<Option<gtk::ListBox>>,
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

impl App {
    pub fn build(
        gtk_app: &gtk::Application,
        config: Config,
        file: Option<Source>,
        preset: Option<Preset>,
        launch: Launch,
        config_problem: Option<String>,
    ) {
        let Launch {
            restart,
            fullscreen,
            locked_fullscreen,
            external,
            kodi,
            play,
        } = launch;
        appearance::force_dark();
        suppress_error_bell();

        // Sized from the tallest monitor to begin with, since no window exists
        // yet to ask which one it is on. Corrected below once there is.
        let styles = install_styles();
        let monitor = appearance::tallest_monitor();
        let scale = appearance::resolve_scale(config.ui_scale, monitor.as_ref());
        styles.load_from_data(&style_css(scale));
        if config.ui_scale.is_none()
            && scale != 1.0
            && let Some(monitor) = monitor.as_ref()
        {
            eprintln!(
                "Interface scaled {scale}x for a {}px-tall display. \
                 Set ui_scale in the config file to override.",
                monitor.geometry().height()
            );
        }

        let sounds = Sounds::new(config.sounds, config.primary_sink.clone());

        let (width, height) = default_window_size(
            scale,
            monitor.as_ref(),
            (config.window_width, config.window_height),
        );
        let window = gtk::ApplicationWindow::builder()
            .application(gtk_app)
            .title("TinePlayer")
            .default_width(width)
            .default_height(height)
            .build();

        // Which monitor the window landed on is only knowable once it has
        // been realized, and on a mixed setup (a television beside a desk
        // monitor) that is the difference between a readable menu and a tiny
        // one. Skipped entirely when the size was set by hand.

        let app = Rc::new(App {
            window: window.clone(),
            awake: crate::awake::KeepAwake::new(gtk_app),
            config: RefCell::new(config),
            updates: RefCell::new(crate::updates::load()),
            update_badges: RefCell::new(Vec::new()),
            file: RefCell::new(None),
            details: RefCell::new(Default::default()),
            poster_art: RefCell::new(None),
            backdrop_art: RefCell::new(None),
            art_generation: Cell::new(0),
            device_names: RefCell::new(Vec::new()),
            device_scan: Cell::new(false),
            maximized_before_fullscreen: Cell::new(false),
            windowed_size: Cell::new((0, 0)),
            resize_settle: RefCell::new(None),
            built_poster: Cell::new(0.0),
            tracks: RefCell::new(Vec::new()),
            primary_file: RefCell::new(None),
            secondary_file: RefCell::new(None),
            errand: Cell::new(Errand::Video),
            primary_baseline: Cell::new(0.0),
            secondary_baseline: Cell::new(0.0),
            duration_s: Cell::new(0.0),
            primary_track: RefCell::new(None),
            secondary_track: RefCell::new(None),
            subtitle_options: RefCell::new(Vec::new()),
            subtitle: RefCell::new(None),
            playback: RefCell::new(None),
            screen: RefCell::new(Screen::Menu),
            menu_row: RefCell::new(0),
            settings_row: RefCell::new(0),
            sounds: RefCell::new(sounds),
            restart,
            nav_list: RefCell::new(None),
            nav_side_list: RefCell::new(None),
            nav_stops: RefCell::new(Vec::new()),
            settings_sliders: RefCell::new(Vec::new()),
            settings_category: Cell::new(Category::General),
            pane_items: RefCell::new(Vec::new()),
            about_scroll: RefCell::new(None),
            copy_root: RefCell::new(None),
            kodi_draft: RefCell::new(None),
            settings_switches: RefCell::new(Vec::new()),
            settings_list: RefCell::new(None),
            clicked_row: Cell::new(false),
            settling_switch: Cell::new(false),
            key_held: Cell::new(false),
            hold_started: Cell::new(false),
            releases: Cell::new(0),
            wanted_scale: Cell::new(None),
            nav_footer: RefCell::new(Vec::new()),
            nav_header: RefCell::new(Vec::new()),
            nav_header_entry: RefCell::new(None),
            controls: RefCell::new(None),
            styles: styles.clone(),
            scale: Cell::new(scale),
            scrub_generation: Cell::new(0),
            scrub_seen: Cell::new(None),
            tick: RefCell::new(None),
            external,
            locked_fullscreen,
            error_is_fatal: Cell::new(false),
            kodi_item: RefCell::new(None),
            session_resume: RefCell::new(None),
            subtitles_hidden: Cell::new(false),
            volume_save_pending: Cell::new(false),
            subtitles_hold: Cell::new(0),
            subtitles_holding: Cell::new(false),
            subtitles_held: Cell::new(false),
            origin: Cell::new(Screen::Menu),
        });

        // Weak, so the polling closure doesn't keep the application alive
        // after its window has gone.
        {
            let weak = Rc::downgrade(&app);
            crate::gamepad::install(move |action| {
                if let Some(app) = weak.upgrade() {
                    app.handle_action(action);
                }
            });
        }

        // Playback has to be torn down before the window goes away, so the
        // resume position is written and the audio devices are released.
        {
            let app = app.clone();
            window.connect_close_request(move |_| {
                app.stop_playback();
                glib::Propagation::Proceed
            });
        }

        // Which monitor the window landed on is only knowable once it has
        // been realized, and on a mixed setup (a television beside a desk
        // monitor) that is the difference between a readable menu and a tiny
        // one. Skipped entirely when the size was set by hand.
        // Watched whatever the size is set to now: a size set by hand can be
        // handed back to the screen while running, and nothing would be
        // listening if these were attached only when it started out
        // automatic. `follow_automatic_scale` decides whether to act.
        let weak = Rc::downgrade(&app);
        window.connect_realize(move |window| {
            let Some(app) = weak.upgrade() else { return };
            app.follow_automatic_scale(window);
            // The surface is what reports the window's size as it is dragged,
            // and it does not exist until here. Connected to rather than the
            // window's own properties because it survives every rebuild of the
            // page, so this handler is attached exactly once.
            if let Some(surface) = window.surface() {
                let weak = Rc::downgrade(&app);
                surface.connect_layout(move |_, _, _| {
                    let Some(app) = weak.upgrade() else { return };
                    app.note_windowed_size();
                    app.rebuild_when_resize_ends();
                });
            }
        });
        // And again whenever the window fills the screen or stops doing so,
        // since that is what the automatic size depends on.
        let weak = Rc::downgrade(&app);
        window.connect_fullscreened_notify(move |window| {
            let Some(app) = weak.upgrade() else { return };
            app.follow_automatic_scale(window);
        });

        // The media page draws its poster as a proportion of the page's
        // height, which is read when the page is built. Filling the screen
        // changes that height by a long way in one step, so the page is
        // rebuilt to match rather than left with a poster sized for a window
        // half the height.
        //
        // Connected here, once, rather than by the page that wants it: a
        // handler attached while building the menu would be attached again by
        // every rebuild, and each rebuild would then trigger the next.
        // Deferred to an idle so the rebuild does not tear down the widgets
        // whose own handlers are still running.
        for maximize in [true, false] {
            let weak = Rc::downgrade(&app);
            let watch = move |window: &gtk::ApplicationWindow| {
                let _ = window;
                let Some(app) = weak.upgrade() else { return };
                if *app.screen.borrow() != Screen::Menu {
                    return;
                }
                glib::idle_add_local_once(move || {
                    if *app.screen.borrow() == Screen::Menu {
                        app.show_menu();
                    }
                });
            };
            match maximize {
                true => window.connect_maximized_notify(watch),
                false => window.connect_fullscreened_notify(watch),
            };
        }

        {
            let weak = Rc::downgrade(&app);
            window.connect_close_request(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.remember_window_size();
                }
                glib::Propagation::Proceed
            });
        }

        app.install_key_handling();
        app.install_accelerators(gtk_app);

        // Find the outputs now, in the background, so the first menu that
        // lists them opens with them already in it rather than with
        // "Searching for outputs..." and a pause. Startup is where there is
        // time to spare for this: nothing is waiting on the answer, and the
        // probe takes long enough to be seen if it is left until a menu wants
        // it. Every opening still looks again - see `show_selector` - so this
        // is a head start rather than the only look.
        app.scan_devices_soon(|_| {});

        // Applied to the window itself rather than at playback, so the
        // menus are fullscreen too.
        if fullscreen {
            window.fullscreen();
        }

        // Asked before the file is loaded, because the answer supplies the
        // title shown for it and the resume position it starts from. Kodi is
        // the only thing that knows either, and only it can say which library
        // item this playback is.
        if kodi {
            *app.kodi_item.borrow_mut() = crate::kodi::current_item();
        }

        let unopenable = match &file {
            Some(source) => app.set_file(source).err().map(|e| (source.clone(), e)),
            None => None,
        };

        // Track choices from the command line are applied whether or not
        // playback is starting. Without --play they simply arrive already
        // made, so the menu opens on them and they can be checked before
        // pressing Play.
        //
        // Each output is only touched when its own flag was given. Assigning
        // both meant `--primary` alone silenced the secondary, because an
        // absent flag resolved to no track - so naming one output threw away
        // what the language preference had already chosen for the other.
        if let Some(preset) = preset.as_ref()
            && app.file.borrow().is_some()
        {
            let resolve = |spec: &str| -> Option<u32> {
                match crate::probe::resolve_audio(spec, &app.tracks.borrow()) {
                    Ok(choice) => choice,
                    // Reported rather than obeyed silently, the same way a
                    // subtitle that cannot be resolved is: playing the
                    // wrong track is not what was asked for either.
                    Err(e) => {
                        eprintln!("{e}");
                        None
                    }
                }
            };
            // A spec naming a file that exists is an audio file to play on
            // that output, rather than anything to look for inside the video.
            // Checked before the track specs because none of them can be a
            // path: a number, a language code and `ad` are all short words,
            // and a file has to be there on disk to be taken for one.
            let as_file = |spec: &str| {
                let source = Source::parse(spec);
                source.is_available().then_some(source)
            };
            if let Some(spec) = preset.primary.as_deref() {
                match as_file(spec) {
                    Some(file) => *app.primary_file.borrow_mut() = Some(file),
                    None => *app.primary_track.borrow_mut() = resolve(spec),
                }
            }
            if let Some(spec) = preset.secondary.as_deref() {
                match as_file(spec) {
                    Some(file) => *app.secondary_file.borrow_mut() = Some(file),
                    None => *app.secondary_track.borrow_mut() = resolve(spec),
                }
            }

            // Only touched when asked for, so a video's remembered
            // subtitle survives being launched with audio flags alone.
            if let Some(spec) = preset.subtitle.as_deref() {
                // The languages actually going to the outputs, so a mode
                // like "primary_forced" means the same on the command line
                // as it does in the settings.
                let language_of = |index: Option<u32>| {
                    index.and_then(|index| {
                        app.tracks
                            .borrow()
                            .iter()
                            .find(|track| track.index == index)
                            .map(|track| track.language.clone())
                    })
                };
                let primary = language_of(*app.primary_track.borrow());
                let secondary = language_of(*app.secondary_track.borrow());
                match crate::subtitles::resolve(
                    spec,
                    &app.subtitle_options.borrow(),
                    primary.as_deref(),
                    secondary.as_deref(),
                ) {
                    Ok(choice) => *app.subtitle.borrow_mut() = choice,
                    // Reported rather than obeyed silently: playing with
                    // the wrong subtitles, or none, is not what was asked
                    // for either way.
                    Err(e) => eprintln!("{e}"),
                }
            }
            // An audio file named on the command line arrives after the media
            // was applied, so whatever alignment was measured for that pairing
            // has to be read again now that the pairing is known.
            app.load_baselines();
        }

        match (&unopenable, &config_problem) {
            // Nothing to choose from if the video could not be read, so the
            // reason is shown instead of an empty menu.
            //
            // The video comes first when both went wrong: it is what someone
            // asked for, and settings that failed to load can be seen for
            // themselves in the menu behind.
            (Some((source, error)), _) => app.show_source_error(source, error, true),
            // Not fatal: Back lands in the menu, which is where the settings
            // would be put right.
            (None, Some(problem)) => app.show_error(problem, false),
            // Asked for outright rather than inferred. Refused out loud when
            // there is nowhere to play to, since silently showing the menu
            // instead would leave a launcher waiting on a film that never
            // started, with nothing said about why.
            (None, None) if play => {
                if app.config.borrow().primary_sink.is_some() {
                    app.start_playback(app.restart);
                } else {
                    app.show_error(
                        "No audio output has been chosen yet, so there is nowhere to play.

                         Choose one under Settings, or run with --list-devices and set                          primary_sink in config.yaml.",
                        false,
                    );
                }
            }
            (None, None) => app.show_menu(),
        }

        // Never when something else is driving. A film handed over by Kodi is
        // not a session anyone chose to start, and a launcher waiting on
        // playback has no use for news about a release.
        if !external {
            app.check_for_updates(false);
        }

        window.present();

        // After the window is on screen, which is when it first has anything
        // for Windows to attach to. Windows sends the media keys as window
        // messages rather than as keys, so there they arrive through here
        // instead of the key handler; everywhere else this installs nothing,
        // because Linux reports them as ordinary keysyms. Both routes end at
        // `handle_media`, so the two can never disagree.
        {
            let weak = Rc::downgrade(&app);
            crate::media_keys::install(&window, move |command| {
                weak.upgrade().is_some_and(|app| app.handle_media(command))
            });
        }
    }

    /// The commands that belong to the application rather than to a screen,
    /// bound as actions with accelerators instead of keys matched by hand.
    ///
    /// `<Primary>` does not do what it is reputed to. It resolves to Control
    /// on macOS exactly as it does on Windows and Linux, so binding it alone
    /// left Command-Q dead - which is the bug this was meant to fix. Measured
    /// 2026-08-08 on GTK 4.22.4 by printing what `gtk::accelerator_parse`
    /// returns: `<Primary>q` came back `CONTROL_MASK`, and Command-Q did
    /// nothing on a Mac until Command was named outright.
    ///
    /// So macOS names Command outright, as `<Meta>`. That is measured too:
    /// a synthesised Command-G arrives at the key handler as `META_MASK` and
    /// a Control-G as `CONTROL_MASK`, on the same build seconds apart.
    /// Command raises no key event of its own, so there is nothing to learn
    /// from watching the modifier alone - the letter beside it is what
    /// carries the answer.
    ///
    /// Control stays bound on macOS as well: it is what the other two
    /// platforms use, someone who presses it means the same thing by it, and
    /// nothing else on a Mac wants Control-Q.
    ///
    /// Only commands with nothing focused behind them live here. An
    /// accelerator claims its key ahead of the widget that has focus, so copy
    /// and the rest stay in the key controller below, where a text field can
    /// still take the key first. See `primary_mask`.
    fn install_accelerators(self: &Rc<Self>, gtk_app: &gtk::Application) {
        // Command-Q on a Mac, Control-Q elsewhere, with W beside it: there is
        // one window, so closing it and quitting are the same act, and both
        // keys get reached for.
        //
        // Straight out, without the "Close the Player?" question Escape asks
        // from the top of the menu. That question guards against a keypress
        // nobody meant, which Escape can be; this is not a combination anyone
        // presses by accident. The resume position is still written, through
        // the window's close handler.
        let quit = gtk::gio::SimpleAction::new("quit", None);
        {
            let app = self.clone();
            quit.connect_activate(move |_, _| {
                // Waited on, unlike the window's own close handler: the
                // process is about to end, and the last progress report to
                // Kodi goes out on a detached thread that exiting would take
                // with it. The stop button under a launcher waits for the same
                // reason.
                app.finish_playback(true);
                app.window.close();
            });
        }
        gtk_app.add_action(&quit);
        bind_accels(gtk_app, "app.quit", &["q", "w"]);

        // Where every desktop platform keeps its preferences.
        //
        // Gated to the same screens as Ctrl+O and Ctrl+L, and for a sharper
        // reason: reaching Settings from playback means stopping the film,
        // which is what the control bar's settings button does deliberately
        // and what a shortcut must never do quietly. Leaving it off the wizard
        // screens keeps it from jumping out of a half-finished Kodi setup.
        let settings = gtk::gio::SimpleAction::new("settings", None);
        {
            let app = self.clone();
            settings.connect_activate(move |_, _| {
                // Copied out before `show_settings`, which takes the same cell
                // mutably.
                let screen = *app.screen.borrow();
                if matches!(screen, Screen::Menu | Screen::VideoSource) {
                    app.show_settings();
                }
            });
        }
        gtk_app.add_action(&settings);
        bind_accels(gtk_app, "app.settings", &["comma"]);
    }

    fn install_key_handling(self: &Rc<Self>) {
        let controller = gtk::EventControllerKey::new();
        let primary = primary_mask();
        let app = self.clone();
        controller.connect_key_pressed(move |_, key, _, state| {
            let playing = app.playback.borrow().is_some();
            match key {
                // Only claimed during playback - the menus need Space for
                // activating whatever row has focus.
                //
                // The transport keys on a keyboard, a headset or a remote
                // arrive as ordinary key events, so they cost nothing but a
                // name here. Most hardware sends one key for play and pause
                // together, which is what Space already is.
                //
                // Windows delivers none of them. Measured 2026-08-08 by
                // synthesising the VK_MEDIA_* keys and tracing this handler:
                // the events arrive, four for four, with a keyval of
                // 0xffffff - `VoidSymbol`. GDK's Windows backend has no
                // mapping from Windows' media keys to the XF86Audio keysyms,
                // so there is nothing to match on and no way to match it from
                // here. Matched by name anyway for the platforms whose keysyms
                // are real, and worth knowing before anyone tries to debug the
                // Windows half of it.
                gdk::Key::space if playing => {
                    app.toggle_pause();
                    app.wake_controls();
                    glib::Propagation::Stop
                }
                gdk::Key::AudioPlay | gdk::Key::AudioPause if playing => {
                    app.handle_media(crate::media_keys::Command::PlayPause);
                    glib::Propagation::Stop
                }
                // Deliberately what Escape does rather than what the stop
                // button does, which under a launcher closes the application
                // instead of returning to the menu. Two meanings for "stop"
                // are enough without a third.
                gdk::Key::AudioStop if playing => {
                    app.handle_media(crate::media_keys::Command::Stop);
                    glib::Propagation::Stop
                }
                // The skip keys move by the same ten seconds the arrows and
                // the control bar's own buttons do, through the same path.
                //
                // Not "next track", which is what they mean on a music player:
                // there is no playlist here to step through, and a key marked
                // with a bar and a triangle is exactly the shape of the two
                // buttons sitting either side of pause on the control bar.
                // Rewind and fast-forward, which some keyboards have instead,
                // land on the same thing.
                gdk::Key::AudioNext
                | gdk::Key::AudioForward
                | gdk::Key::AudioPrev
                | gdk::Key::AudioRewind
                    if playing =>
                {
                    app.handle_media(
                        if matches!(key, gdk::Key::AudioNext | gdk::Key::AudioForward) {
                            crate::media_keys::Command::Next
                        } else {
                            crate::media_keys::Command::Previous
                        },
                    );
                    glib::Propagation::Stop
                }
                // Only during playback: elsewhere the arrows belong to the
                // menus, where left and right mean nothing.
                gdk::Key::Left if playing => {
                    app.controls_left_right(-1);
                    glib::Propagation::Stop
                }
                gdk::Key::Right if playing => {
                    app.controls_left_right(1);
                    glib::Propagation::Stop
                }
                // In the menus they belong to a slider if one is selected,
                // and to nothing otherwise.
                gdk::Key::Left if app.settings_slider(-1) => glib::Propagation::Stop,
                gdk::Key::Right if app.settings_slider(1) => glib::Propagation::Stop,
                // Always goes back one level, so it never quits by surprise
                // from somewhere the user was only browsing.
                // Only while the button row is held: elsewhere in playback
                // there is nothing highlighted to press, and Enter should not
                // quietly become a second play/pause.
                gdk::Key::Up if playing => {
                    app.enter_controls();
                    glib::Propagation::Stop
                }
                gdk::Key::Down if playing => {
                    app.leave_controls();
                    glib::Propagation::Stop
                }
                // Straight out, whatever the strip happens to be doing. A
                // keyboard already has Down for putting the strip away, so
                // spending Escape on it as well made leaving a film two
                // presses when it reads as one.
                //
                // The gamepad's B still steps out of the strip first, which is
                // not an inconsistency: it has no second button to spare for
                // it, and that is what its own comment above Action::Back is
                // about.
                gdk::Key::Escape => {
                    app.go_back();
                    glib::Propagation::Stop
                }
                // Ours rather than GTK's, which cannot see the lists at all.
                // Shift+Tab arrives as ISO_Left_Tab on X11 and Wayland both,
                // so the modifier is not enough to tell them apart.
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                    let backwards = key == gdk::Key::ISO_Left_Tab
                        || state.contains(gdk::ModifierType::SHIFT_MASK);
                    if app.move_focus_stop(if backwards { -1 } else { 1 }) {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                // Between the two panes of the browser, and along a slider
                // where the selected row carries one. Same order the gamepad
                // uses, so the two cannot disagree.
                gdk::Key::Left | gdk::Key::Right if !playing => {
                    let delta = if key == gdk::Key::Left { -1 } else { 1 };
                    if app.settings_slider(delta) || app.move_between_lists(delta) {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                // Rows cannot take focus, so GTK no longer activates one for
                // us: pressing a row is now this. A button keeps its own
                // behaviour, and a text field consumes the key before this
                // sees it.
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    app.activate_focused();
                    glib::Propagation::Stop
                }
                // Available on every screen, not just during playback: on a
                // television the menus want the whole display too.
                gdk::Key::Page_Up => {
                    app.move_selection(-PAGE_ROWS);
                    glib::Propagation::Stop
                }
                gdk::Key::Page_Down => {
                    app.move_selection(PAGE_ROWS);
                    glib::Propagation::Stop
                }
                // Home and End only ever reach this on the About page.
                //
                // GtkListBox binds them itself and lands exactly where a
                // jump-to-first and jump-to-last should, so every screen with
                // rows was already served and nothing here is needed for one.
                // Measured 2026-08-08 rather than assumed: with a trace at the
                // top of this handler, pressing either key on a list printed
                // nothing at all, because the list consumes it in the focus
                // chain long before a bubble-phase controller on the window
                // sees it. The About page is the one screen with nothing to
                // select, and its text scrolls once the interface is scaled up.
                //
                // Left unclaimed during playback: seeking to the start is
                // plausible, seeking to the end is not, and the pair is worth
                // less there than the decisions it would need.
                gdk::Key::Home | gdk::Key::End
                    if !playing && app.scroll_about_edge(key == gdk::Key::End) =>
                {
                    glib::Propagation::Stop
                }
                // F11 alongside F: it is what a browser, a file manager and
                // every other video player use, and costs one name.
                gdk::Key::f | gdk::Key::F | gdk::Key::F11 => {
                    app.toggle_fullscreen();
                    glib::Propagation::Stop
                }
                // Only during playback: there is nothing to turn off from a
                // menu, and the choosers want the letter for type-ahead.
                gdk::Key::c | gdk::Key::C if playing => {
                    app.toggle_subtitles();
                    glib::Propagation::Stop
                }
                // The same silence the volume button is held for, without
                // having to reach the button first.
                gdk::Key::m | gdk::Key::M if playing => {
                    app.toggle_mute();
                    glib::Propagation::Stop
                }
                gdk::Key::t | gdk::Key::T if playing => {
                    app.toggle_time_readout();
                    glib::Propagation::Stop
                }
                // The shortcut GTK's own file chooser and every web browser use
                // to reach an address bar, worth having from the menu which is
                // otherwise two steps away from the panel.
                //
                // Not from inside a modal, which already is one of the two
                // ways of choosing a video, and never when something else
                // chose the video: the menu's row for it is disabled then, and
                // a shortcut past that would let a keypress replace what a
                // launcher is waiting on.
                gdk::Key::l | gdk::Key::L
                    if state.intersects(primary)
                        && !app.external
                        && matches!(*app.screen.borrow(), Screen::Menu | Screen::VideoSource) =>
                {
                    app.show_paste_uri();
                    glib::Propagation::Stop
                }
                // The shortcut for copying, which GTK would otherwise only
                // deliver to whichever widget has focus - and the text on the
                // About page deliberately never takes it.
                //
                // Matched here rather than bound as an accelerator for exactly
                // that reason in reverse: an accelerator would claim the key
                // ahead of the focused widget, so a text field would lose its
                // own copy. `copy_selection` saying no is what hands the key
                // back.
                gdk::Key::c | gdk::Key::C if state.intersects(primary) && app.copy_selection() => {
                    glib::Propagation::Stop
                }
                // The other half of the pair, and the shortcut every desktop
                // application uses for opening a file.
                gdk::Key::o | gdk::Key::O
                    if state.intersects(primary)
                        && !app.external
                        && matches!(*app.screen.borrow(), Screen::Menu | Screen::VideoSource) =>
                {
                    app.browse_for_file();
                    glib::Propagation::Stop
                }
                // Last, so it can't shadow the keys above: anything else
                // during playback summons the timeline without claiming the
                // key.
                _ if playing => {
                    app.wake_controls();
                    glib::Propagation::Proceed
                }
                _ => glib::Propagation::Proceed,
            }
        });
        {
            let app = self.clone();
            controller.connect_key_released(move |_, key, _, _| match key {
                gdk::Key::Left | gdk::Key::Right => app.end_scrub(),
                _ => {}
            });
        }
        // Dropping a file on the window loads it, from any screen including
        // mid-playback. Quicker than any picker when the file is already in
        // front of you in a file manager.
        //
        // Left out for the same reason the browser is: something else chose
        // the video and is waiting for this playback of it to end.
        if !self.external {
            let app = self.clone();
            let drop = gtk::DropTarget::new(gtk::gio::File::static_type(), gdk::DragAction::COPY);
            drop.connect_drop(move |_, value, _, _| {
                let Ok(file) = value.get::<gtk::gio::File>() else {
                    return false;
                };
                // Only local files have a path; a remote URI has nothing for
                // filesrc to open.
                let Some(path) = file.path() else {
                    return false;
                };
                app.stop_playback();
                let source = Source::File(path);
                match app.set_file(&source) {
                    Ok(()) => app.show_menu(),
                    Err(e) => app.show_source_error(&source, &e, false),
                }
                true
            });
            self.window.add_controller(drop);
        }

        self.window.add_controller(controller);

        // Enter, taken before the focused widget can have it.
        //
        // A transport button is a real button, and GTK activates a focused
        // one on Enter - so the key never reached the handler above at all.
        // Holding it opened and shut the panel on every repeat while nothing
        // here saw a single press. Claimed in the capture phase, which runs
        // from the window down, and only while the strip has something
        // highlighted: everywhere else Enter still belongs to whatever has
        // the focus.
        let capture = gtk::EventControllerKey::new();
        capture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let app = self.clone();
        capture.connect_key_pressed(move |_, key, _, _| {
            if !matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) || !app.strip_takes_enter() {
                return glib::Propagation::Proceed;
            }
            app.press_activate();
            glib::Propagation::Stop
        });
        let app = self.clone();
        capture.connect_key_released(move |_, key, _, _| {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && app.strip_takes_enter() {
                app.release_activate();
            }
        });
        self.window.add_controller(capture);
    }

    /// Pause or resume, keeping the display-awake hold in step with it.
    fn toggle_pause(&self) {
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.toggle_pause();
            self.awake.set(playback.is_playing());
        }
    }

    /// What a media key means, wherever the platform reported it from: a
    /// keysym on Linux, a `WM_APPCOMMAND` on Windows. Says whether it was
    /// used, which Windows needs in order to decide whether to pass the key
    /// on to whatever else would have played.
    ///
    /// Nothing here means anything without a film, and there is no menu
    /// action a transport key would obviously map to, so the menus leave
    /// these alone entirely.
    fn handle_media(self: &Rc<Self>, command: crate::media_keys::Command) -> bool {
        use crate::media_keys::Command;

        // Read and released before anything below can borrow it again.
        let Some(is_playing) = self
            .playback
            .borrow()
            .as_ref()
            .map(|playback| playback.is_playing())
        else {
            return false;
        };

        let flip = || {
            self.toggle_pause();
            self.wake_controls();
        };

        match command {
            Command::PlayPause => flip(),
            // A keyboard with separate play and pause keys means them
            // literally, so neither flips what it asked for. Both are claimed
            // even when there is nothing to do: what was asked for is already
            // true, which is not the same as the key going unused.
            Command::Play if !is_playing => flip(),
            Command::Pause if is_playing => flip(),
            Command::Play | Command::Pause => {}
            Command::Stop => self.go_back(),
            Command::Next | Command::Previous => {
                self.scrub(if command == Command::Next {
                    crate::player::STEP_SECONDS
                } else {
                    -crate::player::STEP_SECONDS
                });
                self.end_scrub();
                self.wake_controls();
            }
        }
        true
    }

    /// One level up: out of playback, out of a chooser, or out of the
    /// application. Shared by Escape and the gamepad's back button so the two
    /// can never disagree about what "back" means.
    fn go_back(self: &Rc<Self>) {
        // Copied out first: the handlers below take the same cell mutably,
        // and holding the read borrow across them panics.
        let screen = *self.screen.borrow();
        match screen {
            Screen::Playing => self.leave_playback(),
            Screen::Confirm | Screen::About | Screen::Notices | Screen::Kodi => {
                self.show_settings()
            }
            // Every wizard screen leaves the wizard rather than stepping back
            // through it. Nothing has been written until Configure, so this
            // is the same as pressing Cancel, which is what Escape should
            // mean on a screen whose other button says Cancel.
            Screen::KodiChoose | Screen::KodiConfirm | Screen::KodiDone => self.show_kodi(),
            Screen::KodiFolder => self.show_kodi_choose(),
            Screen::KodiHow => self.show_kodi_choose(),
            Screen::KodiHandover => self.show_kodi_how(),
            Screen::KodiManual | Screen::KodiSummary => self.show_kodi_handover(),
            Screen::KodiError => self.show_kodi_summary(),
            // Nothing to go back to when the video we were started for could
            // not be opened.
            Screen::Error if self.error_is_fatal.get() => self.window.close(),
            Screen::Opening => self.show_paste_uri(),
            // Leaving the middle step abandons the measurement rather than
            // stepping back into the track list: the thread cannot be stopped,
            // but its answer is dropped, and nothing has been written.
            Screen::PasteUri
            | Screen::Browser
            | Screen::AlignChoose
            | Screen::AlignProgress
            | Screen::AlignResult => self.return_to_origin(),
            Screen::VideoSource | Screen::Settings | Screen::Error | Screen::ConfirmQuit => {
                self.show_menu()
            }
            Screen::Menu => self.show_confirm_quit(),
        }
    }

    /// Refreshes the controls readout ten times a second.
    ///
    /// Fast enough that the playhead slides rather than stepping: at twice a
    /// second the jumps were plainly visible against a timeline the width of
    /// the screen. It costs two pipeline queries a tick, which is nothing
    /// next to decoding video.
    fn start_tick(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        // Ticks since Kodi was last told where playback had reached. Counted
        // here rather than given a timer of its own so that it stops when
        // playback does, without anything extra to tear down.
        let mut since_report = 0u32;
        let source = glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            // Cloned out before touching the controls, which can rebuild the
            // screen if playback has ended underneath us.
            let playback = app.playback.borrow().clone();
            let controls = app.controls.borrow().clone();
            match (playback, controls) {
                (Some(playback), Some(controls)) => {
                    controls.update(&playback);
                    since_report += 1;
                    // Every 30 seconds, so that a player killed outright still
                    // leaves Kodi's library close to where you actually got to.
                    if since_report >= 300 {
                        since_report = 0;
                        playback.report_to_kodi();
                    }
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
        *self.tick.borrow_mut() = Some(source);
    }

    /// Shows where playback has reached, and nothing else. What a seek wants:
    /// the buttons appearing over the picture on every skip is more than was
    /// asked for.
    fn peek_controls(&self) {
        let playback = self.playback.borrow().clone();
        let controls = self.controls.borrow().clone();
        if let (Some(playback), Some(controls)) = (playback, controls) {
            controls.update(&playback);
            controls.peek();
        }
    }

    /// Brings the controls up on any input during playback, so the timeline
    /// is there whenever someone reaches for a control.
    fn wake_controls(&self) {
        let playback = self.playback.borrow().clone();
        let controls = self.controls.borrow().clone();
        if let (Some(playback), Some(controls)) = (playback, controls) {
            controls.update(&playback);
            controls.flash(!playback.is_playing());
        }
    }

    /// Up: reveal the strip and take hold of it, then climb from the buttons
    /// to the timeline.
    ///
    /// The first press lands on the buttons rather than the timeline, because
    /// the buttons are what cannot be reached any other way - left and right
    /// already seek without any of this.
    fn enter_controls(self: &Rc<Self>) {
        use crate::controls::Row;
        let Some(controls) = self.controls.borrow().clone() else {
            return;
        };
        match controls.row() {
            Row::None => controls.set_row(Row::Buttons),
            Row::Buttons => controls.set_row(Row::Timeline),
            Row::Timeline => {}
            // The panel opens upward out of its button, so up climbs the list
            // of outputs and stops at the top of it.
            Row::Volume => controls.move_output(-1),
        }
    }

    /// Down: back to the buttons from the timeline, then let the strip go.
    fn leave_controls(self: &Rc<Self>) {
        use crate::controls::Row;
        let Some(controls) = self.controls.borrow().clone() else {
            return;
        };
        match controls.row() {
            Row::Timeline => controls.set_row(Row::Buttons),
            Row::Buttons => controls.set_row(Row::None),
            // Down the list of outputs, and off the bottom of it back to the
            // button the panel came out of.
            Row::Volume => {
                if controls.at_last_output() {
                    controls.set_row(Row::Buttons);
                } else {
                    controls.move_output(1);
                }
            }
            // Nothing is held, so there is nothing to put down - but the strip
            // may still be on screen from a seek or a moved mouse, and down
            // should be rid of that too.
            Row::None => {
                if controls.is_showing() {
                    controls.hide();
                }
            }
        }
    }

    /// A press on whatever the strip has highlighted. The volume button is
    /// held rather than pressed, so it waits for the release; everything else
    /// acts at once, as it always has.
    ///
    /// Cloned out of the cell before anything is pressed: stop and settings
    /// both tear playback down, which takes this same cell mutably, and doing
    /// that while a read borrow is alive panics.
    /// Whether Enter belongs to the control strip rather than to whatever
    /// happens to have the focus.
    fn strip_takes_enter(&self) -> bool {
        self.playback.borrow().is_some()
            && self
                .controls
                .borrow()
                .as_ref()
                .is_some_and(|controls| controls.takes_activation())
    }

    fn press_activate(self: &Rc<Self>) {
        let controls = self.controls.borrow().clone();
        let Some(controls) = controls else { return };
        // Any release waiting to be believed is not one: the key is still
        // down. See `release_activate`.
        self.releases.set(self.releases.get() + 1);
        // Once per press, however long it is held. A key down sends presses
        // over and over, and acting on each one turned holding Enter into a
        // control worked dozens of times a second - a delay running away, or
        // an output muted and unmuted until the key came up.
        if self.key_held.replace(true) {
            return;
        }
        // Decided here rather than again on the way up: acting on a press can
        // move the strip somewhere else, and a release that asks a second
        // time gets an answer about wherever it has just moved to. Closing
        // the panel this way put the highlight back on the button, so the
        // release read as a fresh press on it and opened the panel again.
        let holds = controls.holds_press();
        self.hold_started.set(holds);
        if holds {
            controls.press_volume();
        } else {
            controls.activate_focused();
        }
    }

    /// Letting go of a held button. Does the ordinary thing unless the hold
    /// already did something else.
    fn release_activate(self: &Rc<Self>) {
        // Held back rather than acted on, because a key held down does not
        // simply repeat: it sends a release before each repeat, and taking
        // those at face value ended the hold before it could ever reach its
        // six hundred milliseconds - so holding Enter on the volume button
        // opened and shut the panel over and over instead of silencing
        // everything. A release followed closely by a press was never one.
        let mark = self.releases.get() + 1;
        self.releases.set(mark);
        let app = self.clone();
        glib::timeout_add_local_once(REPEAT_GAP, move || {
            if app.releases.get() != mark {
                return;
            }
            app.finish_release();
        });
    }

    /// A release that outlived the gap between repeats, and so is real.
    fn finish_release(self: &Rc<Self>) {
        self.key_held.set(false);
        let controls = self.controls.borrow().clone();
        let Some(controls) = controls else { return };
        // Only a press that started a hold has anything left to do here.
        // Everything else acted on the way down.
        if self.hold_started.replace(false) && controls.release_volume() {
            controls.activate_focused();
        }
    }

    /// Writes the configuration out a second after the last volume change,
    /// rather than on each one. The level itself takes effect immediately;
    /// this is only about remembering it.
    fn save_volume_soon(self: &Rc<Self>) {
        if self.volume_save_pending.replace(true) {
            return;
        }
        let app = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
            app.volume_save_pending.set(false);
            if let Err(e) = app.config.borrow().save() {
                eprintln!("Could not save volume: {e}");
            }
        });
    }

    /// Left and right: between the buttons while the button row is held,
    /// through the video everywhere else.
    fn controls_left_right(self: &Rc<Self>, direction: isize) {
        use crate::controls::Row;
        let row = self
            .controls
            .borrow()
            .as_ref()
            .map(|controls| controls.row())
            .unwrap_or(Row::None);
        if row == Row::Buttons || row == Row::Volume {
            if let Some(controls) = self.controls.borrow().as_ref() {
                if row == Row::Volume {
                    controls.adjust_level(direction);
                } else {
                    controls.move_focus(direction);
                }
            }
            return;
        }
        self.scrub(direction as f64 * crate::player::STEP_SECONDS);
    }

    /// Begins or continues a scrub. Nothing moves until the ticker decides
    /// this is a hold; a tap resolves to a single step when released.
    fn scrub(self: &Rc<Self>, seconds: f64) {
        let playback = self.playback.borrow().clone();
        let Some(playback) = playback else { return };

        let already = playback.is_scrubbing();
        playback.scrub_input(seconds);
        self.scrub_seen.set(Some(std::time::Instant::now()));
        self.peek_controls();
        if already {
            return;
        }

        let generation = self.scrub_generation.get();
        let weak = Rc::downgrade(self);
        let mut last = std::time::Instant::now();
        glib::timeout_add_local(SCRUB_TICK, move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if app.scrub_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let playback = app.playback.borrow().clone();
            let Some(playback) = playback else {
                return glib::ControlFlow::Break;
            };

            // Auto-repeat is what keeps this alive; long enough without it
            // and the release must have gone missing.
            let stale = app
                .scrub_seen
                .get()
                .is_none_or(|seen| seen.elapsed() > SCRUB_ABANDON);
            if stale {
                app.end_scrub();
                return glib::ControlFlow::Break;
            }

            let now = std::time::Instant::now();
            playback.scrub_tick(now - last);
            last = now;
            app.peek_controls();
            glib::ControlFlow::Continue
        });
    }

    /// The direction was let go: perform the one seek the gesture asked for.
    fn end_scrub(&self) {
        let playback = self.playback.borrow().clone();
        let Some(playback) = playback else { return };
        if !playback.is_scrubbing() {
            return;
        }
        self.scrub_generation
            .set(self.scrub_generation.get().wrapping_add(1));
        self.scrub_seen.set(None);
        playback.commit_scrub();
        // A peek, matching the press that began this. Waking the whole strip
        // here brought the buttons in on every release, so a tap of the arrow
        // keys made the bar duck and pop back.
        self.peek_controls();
    }

    fn toggle_fullscreen(&self) {
        if self.locked_fullscreen {
            return;
        }
        let wanted = !self.window.is_fullscreen();
        if wanted {
            // Read before the change, because a fullscreen window reports
            // itself maximized whether or not anybody maximized it.
            self.maximized_before_fullscreen
                .set(self.window.is_maximized());
            self.window.fullscreen();
        } else {
            self.window.unfullscreen();
            // Put back the state fullscreen was entered from, said outright
            // in both directions rather than left to GTK.
            //
            // Leaving fullscreen does not restore it: a window that was never
            // maximized comes back maximized, because fullscreen implies
            // maximized and that is the state handed back - and one launched
            // fullscreen comes back maximized having never been drawn at its
            // own size at all. Asking only for the un-maximize fixed those two
            // and broke the third: a window maximized on purpose came back
            // windowed, because GTK restores the size it had and not the fact
            // that it was maximized. So both halves are asked for.
            //
            // The flag is only ever set on the way in, so a window launched
            // fullscreen has never set it and takes the default - which is the
            // right answer for exactly that case.
            match self.maximized_before_fullscreen.get() {
                true => self.window.maximize(),
                false => self.window.unmaximize(),
            }
            // The pointer only hides in fullscreen, and leaving takes the
            // countdown that would have brought it back with it.
            if let Some(controls) = self.controls.borrow().as_ref() {
                controls.reveal_pointer();
            }
        }

        // Deliberately not written down. Whether to *open* fullscreen is a
        // setting somebody sets on purpose, and it used to be whatever the
        // window happened to be at the moment they quit - so pressing F11 once
        // on the way out changed how the application started for ever after.
    }

    /// Records what the gamepad should be moving through. Screens built from
    /// buttons alone pass `None`, and fall back to GTK's directional focus.
    fn set_nav(&self, list: Option<&gtk::ListBox>, header: &[gtk::Button], footer: &[gtk::Button]) {
        // Every screen goes through here, which makes it the one place that
        // can be sure a screen with selectable text is no longer the one on
        // display. A screen that has some sets it again afterwards.
        *self.copy_root.borrow_mut() = None;
        *self.nav_list.borrow_mut() = list.cloned();
        *self.nav_header.borrow_mut() = header.to_vec();
        // Cleared here so it belongs to one screen only: a page that wants Up
        // to land somewhere particular says so after wiring its navigation.
        *self.nav_header_entry.borrow_mut() = None;
        *self.nav_footer.borrow_mut() = footer.to_vec();

        let mut stops: Vec<gtk::Widget> = header.iter().map(|b| b.clone().upcast()).collect();
        // A column beside the list comes first, being to its left.  Taken
        // rather than read, so it belongs to this screen only.
        if let Some(side) = self.nav_side_list.borrow_mut().take() {
            stops.push(side.upcast());
        }
        if let Some(list) = list {
            stops.push(list.clone().upcast());
        }
        stops.extend(footer.iter().map(|b| b.clone().upcast()));
        *self.nav_stops.borrow_mut() = stops;
    }

    /// The button Down from the list should land on.
    ///
    /// The first *usable* one rather than simply the first: with no video
    /// chosen the play button is insensitive, and stopping there would leave
    /// the gear beside it unreachable without a pointer.
    fn first_footer(footer: &[gtk::Button]) -> Option<&gtk::Button> {
        footer.iter().find(|button| button.is_sensitive())
    }

    /// The header button Up from the list should land on.
    fn last_header(header: &[gtk::Button]) -> Option<&gtk::Button> {
        header.iter().rev().find(|button| button.is_sensitive())
    }

    fn handle_action(self: &Rc<Self>, action: crate::gamepad::Action) {
        use crate::gamepad::Action;
        match action {
            Action::Up if self.playback.borrow().is_some() => self.enter_controls(),
            Action::Down if self.playback.borrow().is_some() => self.leave_controls(),
            Action::Up => self.move_selection(-1),
            Action::Down => self.move_selection(1),
            Action::Left if self.playback.borrow().is_some() => self.controls_left_right(-1),
            Action::Right if self.playback.borrow().is_some() => self.controls_left_right(1),
            // The same three in the same order the arrow keys use: a slider on
            // the selected row, then the panes of the browser, then whatever
            // GTK can find. move_between_lists has to be in here explicitly -
            // child_focus cannot reach a list, because the rows are not
            // focusable and the list being a focus stop is our arrangement
            // rather than something GTK's directional search knows about.
            Action::Left => {
                if !self.settings_slider(-1) && !self.move_between_lists(-1) {
                    self.window.child_focus(gtk::DirectionType::Left);
                }
            }
            Action::Right => {
                if !self.settings_slider(1) && !self.move_between_lists(1) {
                    self.window.child_focus(gtk::DirectionType::Right);
                }
            }
            // During playback the lower face button is the obvious place for
            // play/pause, and there is nothing else on screen to activate.
            // On the button row the lower face button presses whatever is
            // highlighted. Everywhere else in playback it is play/pause, which
            // is what it should be when nothing is being driven.
            Action::Activate | Action::PlayPause if self.playback.borrow().is_some() => {
                let on_buttons = self
                    .controls
                    .borrow()
                    .as_ref()
                    .is_some_and(|controls| controls.takes_activation());
                if on_buttons && action == Action::Activate {
                    self.press_activate();
                    return;
                }
                if let Some(playback) = self.playback.borrow().as_ref() {
                    playback.toggle_pause();
                    self.awake.set(playback.is_playing());
                }
                self.wake_controls();
            }
            Action::Activate => self.activate_focused(),
            Action::PlayPause => {}
            Action::ActivateReleased if self.playback.borrow().is_some() => self.release_activate(),
            Action::ActivateReleased => {}
            Action::DirectionReleased => self.end_scrub(),
            Action::PageUp => self.move_selection(-PAGE_ROWS),
            Action::PageDown => self.move_selection(PAGE_ROWS),
            // Harmless during playback, where there are no stops to move
            // between and this does nothing.
            Action::FocusNext => {
                self.move_focus_stop(1);
            }
            Action::FocusPrevious => {
                self.move_focus_stop(-1);
            }
            // Whatever is on screen goes away first, whether it is being
            // driven or simply lingering: backing out of the film while the
            // strip is up would be a surprise either way.
            Action::Back => {
                let showing = self
                    .controls
                    .borrow()
                    .as_ref()
                    .is_some_and(|controls| controls.is_showing());
                if showing {
                    if let Some(controls) = self.controls.borrow().as_ref() {
                        controls.hide();
                    }
                } else {
                    self.go_back();
                }
            }
            Action::Fullscreen => self.toggle_fullscreen(),
            // Ignored outside playback, matching the keyboard: there is
            // nothing to turn off from a menu.
            // During playback this button is held for silence and tapped for
            // subtitles, so the tap waits for the release to know which it
            // was. Everywhere else there is nothing to silence and nothing to
            // subtitle, so it does neither.
            Action::Subtitles if self.playback.borrow().is_some() => self.press_subtitles(),
            Action::Subtitles => {}
            Action::SubtitlesReleased if self.playback.borrow().is_some() => {
                self.release_subtitles()
            }
            Action::SubtitlesReleased => {}
            Action::TimeReadout => self.toggle_time_readout(),
        }
    }

    /// Moves the selection one row, obeying the same boundary rules the
    /// keyboard does: the footer button sits below the last row, and the top
    /// of the list is a hard stop rather than wrapping.
    fn move_selection(self: &Rc<Self>, delta: i32) {
        if self.scroll_about(delta) {
            return;
        }
        // Cloned out before anything can rebuild the screen underneath us.
        let list = self.nav_list.borrow().clone();
        let footer = self.nav_footer.borrow().clone();
        let header = self.nav_header.borrow().clone();

        let Some(list) = list else {
            let direction = if delta < 0 {
                gtk::DirectionType::Up
            } else {
                gtk::DirectionType::Down
            };
            self.window.child_focus(direction);
            return;
        };

        let last = last_row_index(&list);
        let select = |index: i32| {
            if let Some(row) = list.row_at_index(index) {
                self.sounds.borrow().click();
                list.select_row(Some(&row));
                settle_on(&row);
            }
        };

        if header.iter().any(|button| button.has_focus()) {
            if delta > 0 {
                select(0);
            }
            return;
        }
        if footer.iter().any(|button| button.has_focus()) {
            if delta < 0 {
                select(last);
            }
            return;
        }

        let position = list.selected_row().map(|row| row.index()).unwrap_or(0);
        let next = position + delta;
        // A page that runs off the end stops at the end, rather than doing
        // nothing: only a single step from the very edge should be ignored.
        if next < 0 {
            if position > 0 {
                select(0);
            } else if let Some(button) = self
                .nav_header_entry
                .borrow()
                .clone()
                .or_else(|| App::last_header(&header).cloned())
            {
                self.sounds.borrow().click();
                button.grab_focus();
            }
            return;
        }
        if next > last && position < last {
            select(last);
            return;
        }
        if next > last {
            if let Some(button) = App::first_footer(&footer) {
                self.sounds.borrow().click();
                button.grab_focus();
            }
            return;
        }
        select(next);
    }

    /// Activates whatever holds focus. Rows go through the list's
    /// `row-activated` signal, which is what the screens connect to; anything
    /// else (the footer, the confirm screen's buttons) activates directly.
    fn activate_focused(self: &Rc<Self>) {
        // Disambiguated: GtkWindowExt and RootExt both define `focus`.
        let Some(widget) = gtk::prelude::GtkWindowExt::focus(&self.window) else {
            return;
        };
        let list = self.nav_list.borrow().clone();

        // The focus is on a row again, so that a screen reader has something
        // to announce. Both shapes are still accepted: the row directly, and
        // the list for the moment after Tab has landed on one but before a
        // row has been settled on.
        let focused_list = widget
            .downcast_ref::<gtk::ListBoxRow>()
            .and_then(|row| row.parent())
            .and_downcast::<gtk::ListBox>()
            .or_else(|| widget.downcast_ref::<gtk::ListBox>().cloned())
            .or_else(|| list.filter(|list| list.has_focus()));
        if let Some(list) = focused_list
            && let Some(row) = list.selected_row()
        {
            self.sounds.borrow().click();
            list.emit_by_name::<()>("row-activated", &[&row]);
            return;
        }
        widget.activate();
    }

    /// Turns subtitles on or off for the playback in progress, and brings the
    /// strip up so the change is visible: the letters dim or light, which is
    /// the only confirmation when the moment has no subtitle to draw anyway.
    fn toggle_subtitles(&self) {
        let Some(playback) = self.playback.borrow().clone() else {
            return;
        };
        let showing = playback.toggle_subtitles();
        self.subtitles_hidden.set(!showing);
        if let Some(controls) = self.controls.borrow().as_ref() {
            controls.set_subtitles(playback.has_subtitles(), showing);
        }
        self.wake_controls();
    }

    /// Starts a hold on the left face button. Nothing happens yet: what the
    /// press meant is only known when it is let go, or when it has been down
    /// long enough to have meant the other thing.
    fn press_subtitles(self: &Rc<Self>) {
        if self.subtitles_holding.replace(true) {
            return;
        }
        self.subtitles_held.set(false);
        let mark = self.subtitles_hold.get() + 1;
        self.subtitles_hold.set(mark);
        let app = self.clone();
        glib::timeout_add_local_once(crate::controls::HOLD, move || {
            if app.subtitles_hold.get() != mark {
                return;
            }
            app.subtitles_held.set(true);
            app.toggle_mute();
        });
    }

    /// Changes the subtitles, unless the hold already silenced everything.
    fn release_subtitles(self: &Rc<Self>) {
        self.subtitles_holding.set(false);
        self.subtitles_hold.set(self.subtitles_hold.get() + 1);
        if !self.subtitles_held.replace(false) {
            self.toggle_subtitles();
        }
    }

    /// Moves the level on the settings row that is selected, and says whether
    /// there was one. Left and right do nothing else on this screen, so they
    /// are free to mean this where a slider is sitting.
    fn settings_slider(self: &Rc<Self>, direction: isize) -> bool {
        // On that screen and no other. The sliders are held on the application
        // rather than on the page they belong to, and they outlive it: leaving
        // settings does not empty the list, so this went on matching by row
        // number against whatever screen came next. Backing out to the media
        // page and pressing Left moved the interface size, because the row
        // selected there had the same number as the row the size sits on.
        if *self.screen.borrow() != Screen::Settings {
            return false;
        }
        let Some(index) = self
            .nav_list
            .borrow()
            .as_ref()
            .and_then(|list| list.selected_row())
            .map(|row| row.index())
        else {
            return false;
        };
        let Some(item) = self.item_at(index) else {
            return false;
        };
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((kind, scale, value)) = found else {
            return false;
        };
        // Snapped to the step rather than added to: a value set finely with a
        // pointer, or from the panel during playback, otherwise carries its
        // odd remainder through every press that follows.
        let step = kind.step();
        let now = scale.value();
        // Nudged by a step from where it is, snapped onto the step grid. The
        // nudge is what the epsilon protects: a value already sitting exactly
        // on a step would otherwise floor to itself and go nowhere, which is
        // what stopped the interface size after one press - its steps are a
        // tenth, and rounding to a whole number made every press compute the
        // same target.
        let ratio = now / step;
        let moved = if direction > 0 {
            ((ratio + 1e-6).floor() + 1.0) * step
        } else {
            ((ratio - 1e-6).ceil() - 1.0) * step
        };
        let range = kind.range();
        let moved = moved.clamp(*range.start(), *range.end());
        scale.set_value(moved);
        self.set_slider(kind, moved, &value);
        // Safe here: nothing is holding the bar, so redrawing cannot be read
        // as another movement.
        if kind == Slider::Scale {
            self.apply_scale(moved);
        }
        true
    }

    /// Silences the output the selected row belongs to, or lets it go. What
    /// activating a level row does, since there is nothing to open.
    fn toggle_settings_mute(self: &Rc<Self>, item: Item) {
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((Slider::Volume(role), scale, value)) = found else {
            return;
        };
        let muted = !self.config.borrow().muted(role);
        {
            let mut config = self.config.borrow_mut();
            config.set_volume(role, scale.value() / 100.0);
            config.set_muted(role, muted);
        }
        value.set_text(&volume_label(scale.value() / 100.0, muted));
        // On is unmuted, so the switch reads as the output being heard rather
        // than as the mute being applied. A silenced output's bar is dimmed
        // with it: the level it will come back to is worth still showing, and
        // moving it while nothing can be heard is not.
        scale.set_sensitive(!muted);
        value.set_sensitive(!muted);
        self.set_settings_switch(item, !muted);
        self.save_volume_soon();
    }

    /// Turns an output's delay on or off, keeping whatever it is set to.
    ///
    /// Off is how somebody checks whether a delay is helping: winding it to
    /// zero would answer the same question and lose the value they spent time
    /// finding.
    fn toggle_settings_offset(self: &Rc<Self>, item: Item) {
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((Slider::Offset(role), scale, value)) = found else {
            return;
        };
        let on = !self.config.borrow().offset_on(role);
        {
            let mut config = self.config.borrow_mut();
            config.set_offset_on(role, on);
            let _ = config.save();
        }
        // Heard straight away, like the delay itself: the point of the switch
        // is comparing with and without while something is playing.
        self.push_offset_live(role);
        scale.set_sensitive(on);
        value.set_text(&offset_label(self.config.borrow().applied_offset_ms(role)));
        value.set_sensitive(on);
        self.set_settings_switch(item, on);
    }

    /// Where a slider stands now, and how that reads beside it.
    fn slider_state(&self, kind: Slider) -> (f64, String) {
        let config = self.config.borrow();
        match kind {
            Slider::Volume(role) => {
                let level = config.volume(role);
                (level * 100.0, volume_label(level, config.muted(role)))
            }
            Slider::Offset(role) => {
                // The bar keeps the stored delay, so turning it back on shows
                // what it will be; the reading says what is actually being
                // applied, which while it is off is nothing.
                (
                    config.offset_ms(role),
                    offset_label(config.applied_offset_ms(role)),
                )
            }
            Slider::ResumeThreshold => {
                let percent = config.resume_min_percent().round();
                (percent, format!("{percent}%"))
            }
            Slider::WatchedThreshold => {
                let percent = config.watched_percent().round();
                (percent, format!("{percent}%"))
            }
            Slider::Scale => {
                // The bar sits at whatever size is in force either way, so
                // turning the switch off starts from what is on screen. The
                // reading says Auto rather than the number, since while the
                // switch is on that number is a consequence and not a
                // setting.
                let chosen = chosen_scale(&config);
                let scale = chosen.unwrap_or_else(|| self.scale.get());
                let reading = match chosen {
                    Some(scale) => scale_label(scale),
                    None => "Auto".to_string(),
                };
                (steps_from_scale(scale), reading)
            }
            Slider::SubtitleSize => {
                let size = config
                    .subtitle_size
                    .unwrap_or(crate::pipeline::DEFAULT_SUBTITLE_SIZE);
                (size as f64, size.to_string())
            }
        }
    }

    /// Writes a slider through to the configuration and puts the reading
    /// beside it in step. Turning an output up unmutes it, as the panel
    /// during playback does.
    fn set_slider(self: &Rc<Self>, kind: Slider, moved: f64, value: &gtk::Label) {
        let range = kind.range();
        let moved = moved.clamp(*range.start(), *range.end());
        {
            let mut config = self.config.borrow_mut();
            match kind {
                Slider::Volume(role) => {
                    config.set_volume(role, moved / 100.0);
                    config.set_muted(role, false);
                }
                Slider::Offset(role) => config.set_offset_ms(role, moved),
                Slider::ResumeThreshold => config.resume_min_percent = Some(moved),
                Slider::WatchedThreshold => config.watched_percent = Some(moved),
                Slider::Scale => config.ui_scale = Some(scale_from_steps(moved)),
                Slider::SubtitleSize => config.subtitle_size = Some(moved.round() as u32),
            }
        }
        // Nothing redrawn here. Restyling moves the bar under whatever is
        // moving it, which GTK reads as another movement, which restyles
        // again - a loop that ran the size to its limit as soon as it was
        // dragged. Who calls this decides when it is safe: a key press
        // applies at once, a drag waits to be let go.
        // Heard straight away when a film is playing, so a delay can be placed
        // against the picture rather than guessed at and checked later.
        // The configuration above already holds `moved`, so this reads the
        // same number back rather than adding the baseline to it by hand.
        if let Slider::Offset(role) = kind {
            self.push_offset_live(role);
        }
        value.set_text(&match kind {
            Slider::Volume(_) => volume_label(moved / 100.0, false),
            Slider::Offset(_) => offset_label(moved),
            Slider::Scale => scale_label(scale_from_steps(moved)),
            Slider::SubtitleSize => format!("{}", moved.round()),
            _ => format!("{}%", moved.round()),
        });
        self.save_volume_soon();
    }

    /// Swaps the right-hand readout between the length and what is left.
    fn toggle_time_readout(&self) {
        let controls = self.controls.borrow().clone();
        if let Some(controls) = controls {
            controls.toggle_remaining();
        }
    }

    /// Silences every output at once, or puts back what each was doing. The
    /// same thing holding the volume button does, reached directly.
    fn toggle_mute(&self) {
        let controls = self.controls.borrow().clone();
        if let Some(controls) = controls {
            controls.toggle_hush();
        }
    }

    fn stop_playback(&self) {
        self.finish_playback(false);
    }

    /// Leaves playback for the menu, remembering where it had reached.
    ///
    /// What Escape, the stop button and the settings button all do, so that
    /// stepping out to change something and coming back is one motion however
    /// it was asked for.
    fn leave_playback(self: &Rc<Self>) {
        let position = self
            .playback
            .borrow()
            .as_ref()
            .and_then(|playback| playback.position())
            .map(|position| position.nseconds())
            .filter(|position| *position > 0);
        if let Some((key, position)) = self.storage_key().zip(position) {
            *self.session_resume.borrow_mut() = Some((key, position));
        }
        self.stop_playback();
        self.show_menu();
    }

    /// Tears playback down, saving or clearing the resume position as it goes.
    ///
    /// `wait_for_kodi` holds on until the last progress report has actually
    /// reached Kodi. That only matters when the process is about to end, since
    /// the report goes out on a detached thread and exiting would take it
    /// along; everywhere else it would be a stall for nothing.
    fn finish_playback(&self, wait_for_kodi: bool) {
        // Whatever else happens below, stop holding the display awake: this
        // is reached from the window closing as well as from playback ending.
        self.awake.set(false);
        if let Some(tick) = self.tick.borrow_mut().take() {
            tick.remove();
        }
        if let Some(controls) = self.controls.borrow_mut().take() {
            controls.cancel();
            // Playback ending with the pointer hidden would leave the menus
            // behind it without one.
            controls.reveal_pointer();
        }
        if let Some(playback) = self.playback.borrow_mut().take() {
            playback.stop();
            if wait_for_kodi {
                playback.finish_reporting();
            }
        }
        self.window.set_title(Some("TinePlayer"));
    }

    /// Where playback should pick up, and the title to show for the file.
    ///
    /// Under Kodi its library is the authority, so playback starts from the
    /// position Kodi's own interface was just showing and the two never
    /// visibly disagree. Its answer stands even when it holds no resume point:
    /// a film Kodi considers unwatched starts at the beginning rather than
    /// wherever our own file happens to remember. Only a Kodi that does not
    /// answer at all falls back to `positions.json`.
    ///
    /// The title comes from the same call, so it is refreshed here rather
    /// than costing a second round trip.
    fn resume_position(&self) -> Option<u64> {
        let key = self.storage_key()?;
        // Ahead of everything, including Kodi's library: this is where the
        // viewer actually was, seconds ago, and no stored answer is better
        // informed than that.
        if let Some((remembered, position)) = self.session_resume.borrow().as_ref()
            && *remembered == key
        {
            return Some(*position);
        }
        if let Some(item) = self.kodi_item.borrow().as_ref() {
            return item.resume_ns;
        }
        crate::config::load_resume(&key)
            .and_then(|resume| resume.resume_position(self.config.borrow().resume_min_percent()))
    }

    /// How this video's position and track choices are filed.
    ///
    /// Kodi's own id when it launched us, which survives an add-on stream URL
    /// changing and is the same whichever form of the path is in play.
    /// Otherwise the source names itself.
    fn storage_key(&self) -> Option<String> {
        if let Some(item) = self.kodi_item.borrow().as_ref() {
            return Some(item.key());
        }
        self.file.borrow().as_ref().map(Source::key)
    }

    /// The same key for a video that is not the current one yet.
    ///
    /// `apply_media` needs this: it reads what was remembered about the file it
    /// is loading, and `self.file` does not become that file until the end of
    /// it. Asking `storage_key` there returns the *previous* video's key - or
    /// none at all on the first file of a session, which is why remembered
    /// choices were quietly ignored at startup.
    fn storage_key_for(&self, source: &Source) -> String {
        match self.kodi_item.borrow().as_ref() {
            Some(item) => item.key(),
            None => source.key(),
        }
    }

    /// What to call the current file on screen: Kodi's library title when it
    /// has one, otherwise the file name.
    fn file_label(&self) -> Option<String> {
        if let Some(item) = self.kodi_item.borrow().as_ref()
            && !item.title.is_empty()
        {
            return Some(item.title.clone());
        }
        self.file.borrow().as_ref().map(Source::label)
    }

    // --- Menu ----------------------------------------------------------

    /// Builds the screen the application sits on, without installing it.
    ///
    /// Two shapes behind one entry point, because everything that shows the
    /// menu wants whichever is right rather than having to ask first. With no
    /// video there is nothing to configure and nothing to play, so the page is
    /// an invitation to choose one. With a video it is a page about that
    /// video, and the choices sit under what they are choices about.
    ///
    /// Split out so the browser can raise the same page behind itself as a
    /// backdrop, which is what makes it read as a window opening over the
    /// menu rather than as another screen replacing it.
    fn build_menu_page(self: &Rc<Self>) -> (gtk::Widget, Option<gtk::ListBox>) {
        // What a resize compares against to decide whether rebuilding this
        // page would change anything - recorded here, for every menu page,
        // rather than where the poster is built.
        //
        // Only the media page has a poster, so recording it there left the
        // empty page's figure at whatever it happened to be, which never
        // matched and so always answered "yes, rebuild". The page was then
        // rebuilt every quarter second for as long as it was on screen, and
        // the surface layout that followed each rebuild scheduled the next.
        //
        // It was close to invisible, because the page it kept rebuilding looks
        // the same each time - but the pointer's idea of what is under it does
        // not survive the widget being destroyed and made again, so hovering a
        // button only lit it while the mouse was moving, and a click only
        // landed if it happened to arrive between two rebuilds.
        self.built_poster.set(self.poster_height(self.scale.get()));
        if self.file.borrow().is_none() {
            return (self.build_empty_page().upcast(), None);
        }
        let (page, list) = self.build_media_page();
        (page.upcast(), Some(list))
    }

    /// The page about the video that is loaded: what it is, above how it is
    /// about to be played.
    fn build_media_page(self: &Rc<Self>) -> (gtk::Overlay, gtk::ListBox) {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;

        // Everything sits in one column, held to 16:9 by `hold_safe_area` so
        // that a wide window widens the artwork behind rather than the text on
        // top. A plot line three thousand pixels across is not a page anyone
        // reads, and a row whose value drifts that far from its label stops
        // reading as one row.
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(16.0))
            .margin_top(px(30.0))
            // Matched to the sides rather than to the top. The panel now runs
            // to the bottom of the page, so this margin is a visible edge
            // along it, and at 26 it read as a thinner border than the 34 down
            // either side.
            .margin_bottom(px(34.0))
            .margin_start(px(34.0))
            .margin_end(px(34.0))
            // Filled, not centered. The centering is `Column`'s job, and a
            // box that also centers itself shrinks to its natural width
            // inside the column it was just given - which is what truncated
            // every row value on a file with a short plot.
            .css_classes(["tp-media"])
            .build();

        // The poster keeps to the left for the height of the page; everything
        // else runs down the column beside it.
        let columns = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(32.0))
            .vexpand(true)
            .build();
        columns.append(&self.poster_column(scale));

        let main = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(6.0))
            .hexpand(true)
            .build();
        columns.append(&main);
        content.append(&columns);

        let (scroller, list) = scrolling_list();
        name_it(&list, "Playback Options");

        // The film's details sit still. Only the rows scroll, so the poster,
        // the title and the buttons stay where they are however long the list
        // gets - and the list scrolls under them rather than the page moving
        // as a whole.
        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(6.0))
            .valign(gtk::Align::Start)
            .build();
        for widget in self.heading_block(scale) {
            info.append(&widget);
        }
        main.append(&info);

        let file = self.file.borrow().clone();
        let config = self.config.borrow();
        let tracks = self.tracks.borrow();

        // Asked before the rows are built, not after: this is what fetches
        // Kodi's title as well as its resume point, and a row built ahead of
        // it would show the file name until something rebuilt the screen.
        let resume_at = self.resume_position();

        let has_file = file.is_some();
        let has_secondary = config.secondary_sink.is_some();

        // The rows, and the group each one opens - `None` for a row that
        // continues the group above it.
        //
        // Kept as a second list rather than a fifth element on the tuple so
        // that `alignment_row` can go on returning a row and nothing else. The
        // two are pushed together every time, which is what keeps them in step.
        let mut rows: Vec<(String, String, bool, MenuAction)> = Vec::new();
        let mut groups: Vec<Option<&str>> = Vec::new();
        let mut push = |group: Option<&'static str>,
                        row: Option<(String, String, bool, MenuAction)>| {
            if let Some(row) = row {
                groups.push(group);
                rows.push(row);
            }
        };

        // Which output, said once at the top of the group, rather than on the
        // front of all three rows under it. "First Output" and "Second Output"
        // rather than primary and secondary: the ordinal is the whole of what
        // distinguishes them to anyone watching, and Primary/Secondary is the
        // vocabulary of the code and the config file.
        push(
            Some("FIRST OUTPUT"),
            Some((
                "Output Device".to_string(),
                config
                    .primary_sink
                    .clone()
                    .unwrap_or_else(|| "Not set".to_string()),
                true,
                MenuAction::Device(Role::Primary),
            )),
        );
        push(
            None,
            Some((
                "Audio Track".to_string(),
                if has_file {
                    self.describe_audio(Role::Primary)
                } else {
                    "—".to_string()
                },
                has_file,
                MenuAction::Track(Role::Primary),
            )),
        );
        push(None, self.alignment_row(Role::Primary));

        push(
            Some("SECOND OUTPUT"),
            Some((
                "Output Device".to_string(),
                config
                    .secondary_sink
                    .clone()
                    .unwrap_or_else(|| "None".to_string()),
                true,
                MenuAction::Device(Role::Secondary),
            )),
        );
        push(
            None,
            Some((
                "Audio Track".to_string(),
                if has_file && has_secondary {
                    self.describe_audio(Role::Secondary)
                } else {
                    "—".to_string()
                },
                has_file && has_secondary,
                MenuAction::Track(Role::Secondary),
            )),
        );
        if has_secondary {
            push(None, self.alignment_row(Role::Secondary));
        }

        // Its own group rather than sitting with the audio pair: the subtitle
        // language is an independent choice, and may be a third language again
        // or a repeat of either soundtrack.
        push(
            Some("SUBTITLES"),
            Some((
                "Language".to_string(),
                self.describe_subtitle(),
                has_file,
                MenuAction::Subtitles,
            )),
        );

        let can_play = has_file && config.primary_sink.is_some();
        drop(tracks);
        drop(config);

        // What each row is called to anyone who cannot see the list. The group
        // heading is read once at the top of a group and does not survive into
        // a row announced on its own, so the name carries it: "Audio Track" is
        // two rows on this page and "First output, Audio Track" is one.
        //
        // Worked out here, where both lists are still in hand, and in title
        // case rather than the heading's capitals - a screen reader given
        // "FIRST OUTPUT" may spell it.
        let mut heading = String::new();
        let names: Vec<String> = rows
            .iter()
            .zip(&groups)
            .map(|((label, value, _, _), group)| {
                if let Some(group) = group {
                    heading = title_case(group);
                }
                row_name(&format!("{heading}, {label}"), value)
            })
            .collect();

        for ((label, value, enabled, _), name) in rows.iter().zip(&names) {
            append_named(&list, &menu_row(label, value, *enabled), name);
        }

        // A heading above the row that opens a group, and nothing above the
        // rest. Headings are not rows: they sit outside the selection model
        // and outside the focus chain, so they are unselectable and skipped by
        // the arrow keys without anything having to arrange it.
        //
        // That is also why the indent under them is gone. It said "this
        // belongs to the output above"; the heading says it for all three rows
        // at once, and says which output.
        //
        // It has to be done through this function rather than by setting the
        // header on each row directly, which is the obvious way and does
        // nothing: `set_header` only stores the widget on the row, and the
        // list parents and draws it from inside its header function - which
        // returns immediately when none is set. The headings were built, held
        // and never mounted.
        list.set_header_func(move |row, _before| {
            let index = row.index();
            match groups.get(index as usize).copied().flatten() {
                Some(group) => row.set_header(Some(&group_heading(group, scale, index == 0))),
                None => row.set_header(None::<&gtk::Widget>),
            }
        });

        let resumable = resume_at.is_some();

        // Between the film and the choices rather than under both. Playing is
        // what the page is for, so it sits where the eye arrives after
        // reading what the film is - and the rows below become what they
        // actually are, the settings you may want to change first rather than
        // a list to get past. Generous room above and below, so it reads as a
        // division of the page rather than as another row.
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .margin_top(px(34.0))
            .margin_bottom(px(34.0))
            .build();
        // Everything in this row packs to the left, over the rows it acts on:
        // playing, starting over, and then the two marks. Nothing expands, so
        // there is no gap pushing the marks to the far end - they read as the
        // rest of one row of controls rather than as a separate corner.
        let plays = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::Start)
            .build();
        let mut play_buttons: Vec<gtk::Button> = Vec::new();

        // Resuming is the common case for a part-watched film, so it takes
        // the first position and the focus. Starting over is deliberate
        // enough to be worth its own button rather than a hidden modifier -
        // but not enough to be worth a word beside it, so once there are two
        // the second keeps only its mark. It is the same button either way;
        // what changes is how much room it argues for.
        let play = gtk::Button::new();
        play.set_child(Some(&marked_face(
            play_image(scale),
            &match resume_at {
                Some(position) => format!(
                    "  Resume ({})",
                    crate::controls::format_time(gstreamer::ClockTime::from_nseconds(position))
                ),
                None => "  Play".to_string(),
            },
        )));
        // The face is two labels, so the button has no text of its own for a
        // screen reader to read off. Named outright instead.
        name_it(
            &play,
            &match resume_at {
                Some(position) => format!(
                    "Resume at {}",
                    crate::controls::format_time(gstreamer::ClockTime::from_nseconds(position))
                ),
                None => "Play".to_string(),
            },
        );
        play.add_css_class("tp-button");
        play.add_css_class("tp-action");
        play.add_css_class("tp-tall");
        play.set_sensitive(can_play);
        plays.append(&play);
        play_buttons.push(play);

        if resume_at.is_some() {
            let restart = gtk::Button::new();
            restart.set_child(Some(&marked_face(restart_image(scale), "")));
            restart.add_css_class("tp-button");
            restart.add_css_class("tp-action");
            restart.add_css_class("tp-action-icon");
            restart.add_css_class("tp-tall");
            restart.set_sensitive(can_play);
            // The word is gone from the face, so it has to be somewhere: a
            // tooltip for a pointer, and a name for a screen reader, which
            // would otherwise announce the glyph or nothing at all.
            restart.set_tooltip_text(Some("Start from the beginning"));
            name_it(&restart, "Restart");
            plays.append(&restart);
            play_buttons.push(restart);
        }
        buttons.append(&plays);

        let (fullscreen, gear) = self.corner_buttons();
        let open = self.browse_button();
        // Square, and as tall as the play button beside them. The marks are
        // built the same way on the empty page, where there is no tall button
        // to match, so this is asked for here rather than where they are made.
        for mark in [Some(&open), Some(&gear), fullscreen.as_ref()]
            .into_iter()
            .flatten()
        {
            mark.add_css_class("tp-tall");
        }
        // A little clear air between the pair that plays the film and the
        // marks that do not, so the row reads as two groups rather than a run
        // of equal buttons.
        open.set_margin_start(px(16.0));
        // Left out under a launcher: something else chose the film and is
        // waiting for this playback of it, so there is nothing to choose here.
        if !self.external {
            buttons.append(&open);
        }
        buttons.append(&gear);
        if let Some(fullscreen) = fullscreen.as_ref() {
            buttons.append(fullscreen);
        }
        let close = self.close_button();
        close.add_css_class("tp-tall");
        buttons.append(&close);

        // The page in order: what the film is, what to do about it, and then
        // the choices - which are the only part that scrolls.
        main.append(&buttons);
        // The rows sit in a panel of their own rather than loose on the page.
        // It runs to the bottom because the scroller inside it expands, which
        // is also what turns the space left below the last row into part of
        // the panel instead of a band of nothing.
        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .css_classes(["tp-menu-panel"])
            .build();
        panel.append(&scroller);
        main.append(&panel);

        // A header now rather than a footer, because that is where they sit:
        // Up from the first row reaches them, and Down from them returns.
        // Ordered as they appear, so left and right walk along the row.
        let mut header = play_buttons.clone();
        if !self.external {
            header.push(open);
        }
        header.push(gear);
        header.extend(fullscreen);
        header.push(close);

        {
            let app = self.clone();
            let actions: Vec<MenuAction> = rows.iter().map(|(_, _, _, action)| *action).collect();
            list.connect_row_activated(move |_, row| {
                // A row drawn insensitive is stating something rather than
                // offering it - the video row under Kodi, or a track row with
                // no file yet. Only the row's contents carry that; the
                // ListBoxRow that GTK wraps them in stays sensitive, and would
                // otherwise still take a click or Enter.
                //
                // Left focusable deliberately: the gamepad moves the selection
                // by grabbing focus, which fails on an insensitive widget and
                // would strand it here.
                if row.child().is_some_and(|child| !child.is_sensitive()) {
                    return;
                }
                app.sounds.borrow().click();
                *app.menu_row.borrow_mut() = row.index();
                match actions.get(row.index() as usize) {
                    Some(MenuAction::Device(Role::Primary)) => {
                        app.show_selector(Setting::PrimaryDevice, row)
                    }
                    Some(MenuAction::Track(Role::Primary)) => {
                        app.show_selector(Setting::PrimaryTrack, row)
                    }
                    Some(MenuAction::Device(Role::Secondary)) => {
                        app.show_selector(Setting::SecondaryDevice, row)
                    }
                    Some(MenuAction::Track(Role::Secondary)) => {
                        app.show_selector(Setting::SecondaryTrack, row)
                    }
                    Some(MenuAction::Align(role)) => app.show_align(*role),
                    Some(MenuAction::Subtitles) => app.show_selector(Setting::Subtitles, row),
                    None => {}
                }
            });
        }
        for (index, button) in play_buttons.iter().enumerate() {
            // With two buttons the second one restarts; with one it plays
            // from wherever it left off, which for a fresh file is the start.
            let restart = resumable && index == 1;
            let app = self.clone();
            button.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.start_playback(restart);
            });
        }

        self.wire_navigation(&list, &header, &[]);
        // Up from the top row lands on Play rather than on the far end of the
        // row, which is Settings. Playing is what the page is for, and it is
        // also what someone arrowing upwards off the list is reaching for.
        *self.nav_header_entry.borrow_mut() = header.first().cloned();
        (self.behind_artwork(&content), list)
    }

    /// Puts a page in front of the backdrop, and holds it to its column.
    ///
    /// Both screens go through here, so a page with no artwork still gets the
    /// same ground and the same width as one with it - which is what keeps the
    /// two from being two designs.
    fn behind_artwork(self: &Rc<Self>, content: &gtk::Box) -> gtk::Overlay {
        let backdrop = crate::artwork::Artwork::backdrop();
        backdrop.set_texture(self.backdrop_art.borrow().clone());

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&backdrop));
        // The backdrop fills the window; the page inside stops widening once
        // lines get too long to read. See src/column.rs for why that is a
        // widget rather than something set on this box.
        let most = (PAGE_MAX_UNITS * self.scale.get()).round() as i32;
        overlay.add_overlay(&crate::column::Column::around(content, most));
        overlay
    }

    fn show_menu(self: &Rc<Self>) {
        let (page, list) = self.build_menu_page();

        *self.screen.borrow_mut() = Screen::Menu;
        self.window.set_child(Some(&page));

        // The empty page has no rows to land on: its two buttons are the
        // whole of it, and `build_empty_page` has already focused one.
        let Some(list) = list else { return };
        // Selected as well as focused: focus alone doesn't mark a row
        // selected, which left the list opening with nothing highlighted
        // until the first arrow key.
        let remembered = (*self.menu_row.borrow()).min(last_row_index(&list));
        if let Some(row) = list.row_at_index(remembered) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    // --- The media page ------------------------------------------------

    /// How tall the page is, or is about to be.
    ///
    /// Before the window is mapped it has no size, but it does already know
    /// the size it is going to open at - and that is not simply the interface
    /// scale times a constant, because the opening size is capped to the
    /// monitor. Guessing it as `700 * scale` put a 1050px poster in a 1325px
    /// window at 3x, which pushed the rows and the whole footer off the bottom
    /// of the screen.
    fn page_height(&self, scale: f64) -> f64 {
        match (self.window.height(), self.window.default_height()) {
            (0, 0) => 700.0 * scale,
            (0, planned) => planned as f64,
            (height, _) => height as f64,
        }
    }

    /// How tall the poster should be for the window as it stands: a share of
    /// the page, within hard bounds at both ends.
    ///
    /// The ceiling matters for more than composition. This is a size
    /// *request*, which is a minimum its window must honor, so a poster sized
    /// from the window's own height is a loop: the taller the window, the more
    /// height its contents insist on. Capping it breaks that - past this size
    /// the poster stops following the window, and the window stays free to be
    /// made smaller again.
    ///
    /// The floor is absolute rather than scaled for the opposite reason:
    /// scaled, it grows with the interface exactly when there is least room
    /// for it.
    fn poster_height(&self, scale: f64) -> f64 {
        (self.page_height(scale) * POSTER_SHARE).clamp(120.0, 620.0 * scale)
    }

    /// Remembers the window's size while it is an ordinary window.
    ///
    /// Neither maximized nor fullscreen: both report the screen's dimensions,
    /// and a size taken then is not a size the window can be restored to.
    fn note_windowed_size(&self) {
        if self.window.is_maximized() || self.window.is_fullscreen() {
            return;
        }
        let (width, height) = (self.window.width(), self.window.height());
        if width > 0 && height > 0 {
            self.windowed_size.set((width, height));
        }
    }

    /// Writes down where the window was left, on the way out.
    ///
    /// Every way of leaving goes through `window.close()`, which is what makes
    /// this one handler enough - the close button, Ctrl+Q, the confirmation,
    /// and a fatal error all end here.
    fn remember_window_size(&self) {
        let (width, height) = self.windowed_size.get();
        if width <= 0 || height <= 0 {
            return;
        }
        let mut config = self.config.borrow_mut();
        if config.window_width == Some(width) && config.window_height == Some(height) {
            return;
        }
        config.window_width = Some(width);
        config.window_height = Some(height);
        if let Err(e) = config.save() {
            eprintln!("Could not save the window size: {e}");
        }
    }

    /// Rebuilds the media page once a drag-resize has stopped moving.
    ///
    /// GTK has no "the resize finished" signal - `layout` arrives on every
    /// frame of a drag, and rebuilding the page on each one would be both slow
    /// and unpleasant to watch, the poster jumping under the pointer. So the
    /// rebuild is put on a short timer that each new size cancels and restarts,
    /// and only the last one in a drag survives to fire.
    ///
    /// Without this the poster only resized on maximize and restore, which
    /// have their own handler and change the height in one step. Dragging a
    /// window smaller left the page built for the size it used to be, which is
    /// the sort of thing that looks like a bug rather than a decision.
    ///
    /// The guard is the poster's own height rather than the window's: past the
    /// ceiling in [`App::poster_height`] the window can grow as much as it
    /// likes without the page looking any different, and rebuilding then would
    /// throw away the viewer's place in the list for nothing.
    fn rebuild_when_resize_ends(self: &Rc<Self>) {
        /// Long enough to sit out a drag, short enough that letting go and
        /// seeing the page settle reads as one action rather than two.
        const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

        if *self.screen.borrow() != Screen::Menu {
            return;
        }
        if let Some(pending) = self.resize_settle.borrow_mut().take() {
            pending.remove();
        }
        let app = self.clone();
        let source = glib::timeout_add_local_once(SETTLE, move || {
            *app.resize_settle.borrow_mut() = None;
            if *app.screen.borrow() != Screen::Menu {
                return;
            }
            if app.poster_height(app.scale.get()) == app.built_poster.get() {
                return;
            }
            app.show_menu();
        });
        *self.resize_settle.borrow_mut() = Some(source);
    }

    /// The poster, and the facts about the file under it.
    ///
    /// The two belong together and to nothing else on the page: one is what
    /// the film looks like and the other is what this copy of it is, and
    /// neither is a choice anybody makes. Keeping them in their own column
    /// leaves the whole of the space beside it for the choices.
    fn poster_column(self: &Rc<Self>, scale: f64) -> gtk::Box {
        let px = |base: f64| (base * scale).round() as i32;

        // Half the page's height, which is the proportion the comps are drawn
        // to - 550px of 1080 - and the reason this is not simply a size in
        // interface units. On a maximized ultrawide the page is held to a
        // 16:9 column far taller than the default window, and a poster fixed
        // in scaled pixels sits in the corner of it looking like a thumbnail
        // of itself. Bounded at both ends so a very short window still gets
        // something poster-shaped and a very tall one does not get a
        // billboard.
        //
        // Read when the page is built rather than tracked, so a window
        // resized while the menu is up keeps the size it was built at until
        // something rebuilds the page - which every trip into a chooser does.
        // The alternative is another custom widget, and this is a proportion
        // rather than a constraint: being a little out until the next rebuild
        // costs nothing that anyone can see.
        let height = self.poster_height(scale);
        // Two by three, which every poster in every library is drawn to.
        let width = height * 2.0 / 3.0;

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(12.0))
            .valign(gtk::Align::Start)
            .build();
        // Exactly as wide as the poster and no wider. Without this the column
        // is as wide as its widest *fact*, so a long codec name pushed the
        // whole page to the right and left a gap beside the poster that
        // belonged to nothing.
        column.set_size_request(width.round() as i32, -1);
        // Explicitly not expanding, and this is load-bearing. GTK propagates
        // `hexpand` up from children, so the poster picture asking to fill its
        // own frame quietly made this whole column an expanding one - and a
        // box then splits the spare width between it and the page beside it.
        // Measured: a column asking for 291px was being handed 567, which is
        // the gap that appeared to sit between the poster and the rows.
        column.set_hexpand(false);

        let frame = gtk::Box::builder()
            .css_classes(["tp-poster"])
            .halign(gtk::Align::Start)
            // Clipped, so a poster that is not exactly two by three is
            // cropped by the frame rather than allowed to reshape it.
            .overflow(gtk::Overflow::Hidden)
            .build();
        frame.set_size_request(width.round() as i32, height.round() as i32);

        match self.poster_art.borrow().clone() {
            Some(texture) => {
                // Fills the frame and keeps its shape, which is the same rule
                // the backdrop follows and the reason both are cropped rather
                // than letterboxed: a poster with bars down its sides reads
                // as a mistake. Real posters are two by three and are not
                // cropped at all; what this rescues is an episode thumbnail
                // or a scan that is a few pixels out.
                // Expanding is how it fills the frame: the widget draws a
                // texture and measures as nothing, so without this the frame
                // allocates it no width at all and the poster disappears.
                // The request stops at the column, which sets its own
                // `hexpand` explicitly - see there.
                let picture = crate::artwork::Artwork::poster();
                picture.set_texture(Some(texture));
                picture.set_hexpand(true);
                picture.set_vexpand(true);
                frame.append(&picture);
            }
            // Nothing found, which is the common case: of the 123 film folders
            // in the library this was written against, 28 carry artwork. The
            // mark is sized from the frame rather than from the interface, so
            // it keeps its place inside it at every window size.
            None => frame.append(&video_file_image(width * 0.42)),
        }
        column.append(&frame);

        let facts = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(1.0))
            // A few pixels in from the poster's edges, and the same on both:
            // the readings are ranged right, so without this they sat hard
            // against the frame above on one side while the names were inset
            // on the other.
            .margin_start(px(4.0))
            .margin_end(px(4.0))
            .build();
        // Two columns: what it is on the left, what it says on the right,
        // ranged against the poster's own right edge. As one run of text the
        // readings started at a different place on every line and there was
        // nothing to read down; against an edge they line up as a table, which
        // is what a column of measurements wants to be.
        for (name, value) in self.file_facts() {
            let line = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(px(8.0))
                .build();

            // Ellipsizing decides how a label *shrinks*; it does nothing to
            // what one asks for in the first place, which stays the full width
            // of its text. So a long reading here would widen the column past
            // the poster and push the whole page right. Both halves are capped
            // in what they may ask for; the width they actually get comes from
            // the poster above, and anything longer is cut with an ellipsis.
            let key = gtk::Label::new(Some(&format!("{name}:")));
            key.add_css_class("tp-fact");
            key.add_css_class("tp-fact-name");
            key.set_xalign(0.0);
            key.set_ellipsize(gtk::pango::EllipsizeMode::End);
            // Enough for the longest of them, "Resolution:". Capped at six it
            // cut every name to "Resol...", which is a label that has stopped
            // labelling anything. The pair still comes to well under the
            // poster's width, which is what the cap is protecting.
            key.set_max_width_chars(12);
            line.append(&key);

            let reading = gtk::Label::new(Some(&value));
            reading.add_css_class("tp-fact");
            reading.set_xalign(1.0);
            reading.set_ellipsize(gtk::pango::EllipsizeMode::End);
            reading.set_max_width_chars(12);
            // Pushes itself to the far edge. Safe only because the column
            // sets `hexpand` false outright - otherwise this request would
            // travel up and widen the whole left column, which is the fault
            // the poster picture caused before it.
            reading.set_hexpand(true);
            line.append(&reading);

            facts.append(&line);
        }
        column.append(&facts);
        column
    }

    /// What this copy of the film is, as opposed to what the film is.
    ///
    /// Only what is actually known: a remote source can be measured for none
    /// of it, and a line reading "Unknown" is worse than no line, so anything
    /// unanswered is simply absent. The order runs from what a viewer checks
    /// first to what they check last.
    fn file_facts(&self) -> Vec<(String, String)> {
        let details = self.details.borrow();
        [
            // Two lines rather than "1080p (H.264)". Together they are the
            // longest reading in the column, and the column is only as wide
            // as the poster - so as one line they were the thing that decided
            // how much room the picture got.
            ("Resolution", details.resolution()),
            ("Codec", details.codec()),
            ("Framerate", details.framerate()),
            ("Bitrate", details.bitrate()),
            (
                "Container",
                Some(details.container.clone()).filter(|c| !c.is_empty()),
            ),
            // Last, under the readings that describe the picture. It is the
            // one line here that says nothing about how the film will look.
            ("File size", details.filesize()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name.to_string(), value)))
        .collect()
    }

    /// The title, the facts line, the summary, and what languages the file
    /// holds - everything above the choices.
    ///
    /// Everything except the summary keeps its natural height; the summary is
    /// held to three lines whether it has them or not, which is what stops the
    /// rows underneath moving between one film and the next.
    fn heading_block(self: &Rc<Self>, scale: f64) -> Vec<gtk::Widget> {
        let px = |base: f64| (base * scale).round() as i32;
        let details = self.details.borrow();
        let mut block: Vec<gtk::Widget> = Vec::new();

        let title = gtk::Label::new(Some(&details.title));
        title.add_css_class("tp-film-title");
        title.set_xalign(0.0);
        // One line, cut with an ellipsis. A filename with a release tag on it
        // is long and would happily take two - but the rows below sit at a
        // fixed distance from the top, and a title that is sometimes one line
        // and sometimes two is exactly the thing that moves them.
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        block.push(title.upcast());

        // Year, running time, certificate, score, genres - whichever of them
        // anything answered. Spaced rather than punctuated between, which is
        // what the comps do and what keeps a line of three facts from reading
        // as a sentence.
        let mut facts: Vec<String> = Vec::new();
        // An episode says when it went out, in place of the year a film shows:
        // a date is what anybody would recognise an episode by, where a year
        // barely distinguishes it from the twenty others made alongside it.
        // Only where the sidecar gave one - an episode without a date falls
        // back to the year like anything else.
        match (&details.aired, details.year) {
            (aired, _) if !aired.is_empty() => facts.push(aired.clone()),
            (_, Some(year)) => facts.push(year.to_string()),
            _ => {}
        }
        // Beside the date rather than near the title: which episode this is
        // belongs with the facts about it, and the title is the episode's own
        // name. Two digits each, which is how everything else writes it and
        // what makes a column of them line up.
        facts.extend(
            details
                .episode
                .map(|(season, episode)| format!("S{season:02}E{episode:02}")),
        );
        facts.extend(details.runtime());
        if !details.certificate.is_empty() {
            facts.push(details.certificate.clone());
        }
        // A star, so a bare number is not left to be guessed at. Out of ten is
        // what every writer of this format stores and what the star implies,
        // and the sidecar is the only place it comes from - nothing is ever
        // fetched to produce it.
        //
        // The star is in a font TinePlayer ships, which the other marks in the
        // interface are not: see `INTERFACE_SYMBOLS` in
        // packaging/fonts/build-fonts.py before using any new symbol here.
        //
        // One decimal: the scrapers store three, and "8.235" is a precision
        // nobody asked for about an opinion.
        facts.extend(details.rating.map(|score| format!("★ {score:.1}")));
        if !details.genres.is_empty() {
            // Three at most. A scraper will happily list six, and the line has
            // the width of one line.
            facts.push(
                details
                    .genres
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !facts.is_empty() {
            let line = gtk::Label::new(Some(&facts.join("     ")));
            line.add_css_class("tp-film-facts");
            line.set_xalign(0.0);
            line.set_ellipsize(gtk::pango::EllipsizeMode::End);
            line.set_margin_top(px(4.0));
            block.push(line.upcast());
        }

        // The summary, in a space of its own that is the same height whether
        // there is one or not. This is the only thing on the page held to a
        // fixed height, and it is the only one that needs to be: a plot runs
        // from nothing to a paragraph, and everything else here is one line or
        // absent. Reserving three lines for it is what keeps the rows below
        // from walking up and down the page as you step through a folder.
        let plot = gtk::Label::new(Some(&details.plot));
        plot.add_css_class("tp-film-plot");
        plot.set_xalign(0.0);
        plot.set_yalign(0.0);
        plot.set_wrap(true);
        plot.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        plot.set_lines(3);
        plot.set_ellipsize(gtk::pango::EllipsizeMode::End);
        plot.set_margin_top(px(12.0));
        // Filling the width it is given rather than a fraction of it. A
        // wrapping label asks for its whole text on one line, so it used to be
        // capped at twenty characters to stop it stretching the page - which
        // capped where it *wrapped* too, and left it running down the middle
        // of the column at about half width. Nothing needs to cap it now that
        // the poster column no longer expands and `Column` decides the page's
        // width outright.
        // -1, which is the value that means "no cap". Zero is a cap of zero
        // characters, and left it wrapping down the middle of the column.
        plot.set_max_width_chars(-1);
        plot.set_size_request(-1, px(PLOT_UNITS));
        block.push(plot.upcast());
        drop(details);

        // What is in the file, in languages rather than in track numbers.
        // The rows below say which track is going where; this says what there
        // was to choose from, which is the question someone asks before they
        // start opening choosers.
        //
        // Both lines are always drawn, even when there is nothing to put on
        // them. They are the two facts this application exists to act on, and
        // a line that comes and goes with the file moves everything under it.
        let spoken = (self.audio_languages(), self.subtitle_languages());
        let summary = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(1.0))
            .margin_top(px(14.0))
            .build();
        for (name, languages) in [("Audio", spoken.0), ("Subtitles", spoken.1)] {
            let line = gtk::Label::new(None);
            line.add_css_class("tp-fact");
            line.set_xalign(0.0);
            // Cut rather than wrapped: a second line here would push the rows
            // down on exactly the files that carry the most languages.
            line.set_ellipsize(gtk::pango::EllipsizeMode::End);

            line.set_markup(&summary_markup(name, &languages));
            summary.append(&line);
        }
        block.push(summary.upcast());
        block
    }

    /// Every language the file offers sound in, in the order the tracks are
    /// listed, with description called out.
    ///
    /// Deduplicated, because a file with four English tracks is offering one
    /// language four ways and a line reading "English, English, English,
    /// English" says less than one reading "English". A described track is a
    /// separate entry rather than a duplicate: it is a genuinely different
    /// thing to listen to, and for this application the most important entry
    /// on the line.
    fn audio_languages(&self) -> Vec<String> {
        let mut named: Vec<String> = Vec::new();
        for track in self.tracks.borrow().iter() {
            // A track that never said what it is still counts. Plenty of files
            // tag nothing at all - an AVI usually does not - and a line that
            // quietly left those out would claim a file had no soundtrack.
            let name = crate::languages::name_of_tag(&track.language).unwrap_or(UNKNOWN_LANGUAGE);
            let entry = match crate::probe::is_audio_description(&track.title) {
                true => format!("{name} (Described)"),
                false => name.to_string(),
            };
            if !named.contains(&entry) {
                named.push(entry);
            }
        }
        named
    }

    /// The same for subtitles, over everything on offer - streams inside the
    /// file and files sitting beside it alike.
    ///
    /// Both are things the viewer can pick, so a line that counted only the
    /// embedded ones would understate a folder full of `.srt` files, which is
    /// exactly the shape most of this library is in.
    fn subtitle_languages(&self) -> Vec<String> {
        let mut named: Vec<String> = Vec::new();
        for option in self.subtitle_options.borrow().iter() {
            // Labels arrive as a tag and possibly a title after it - "eng",
            // "eng — Forced", "en.hi" - and the language is the first word of
            // whichever shape it is.
            let tag = option
                .label()
                .split(" — ")
                .next()
                .unwrap_or_default()
                .split('.')
                .next()
                .unwrap_or_default();
            let name = crate::languages::name_of_tag(tag).unwrap_or(UNKNOWN_LANGUAGE);
            if !named.iter().any(|held| held == name) {
                named.push(name.to_string());
            }
        }
        named
    }

    /// The panel that offers the two ways to choose a video: the prompt, and
    /// a button for each.
    ///
    /// Shared by the screen shown when nothing is loaded and by the panel the
    /// browse button opens over a film, because they say the same thing and
    /// should not drift apart. `cancel` adds a third button and is what tells
    /// them apart: the empty screen has nowhere to go back to, while the panel
    /// is floating over a film that is still loaded.
    ///
    /// Returns the panel and its buttons, since what each one does depends on
    /// which screen asked for it.
    fn choose_source_panel(
        self: &Rc<Self>,
        scale: f64,
        cancel: bool,
    ) -> (gtk::Box, gtk::Button, gtk::Button, Option<gtk::Button>) {
        let px = |base: f64| (base * scale).round() as i32;

        let middle = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(24.0))
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .vexpand(true)
            .build();
        // The mark only where the screen is otherwise empty. Over a film it
        // would be the application introducing itself in the middle of being
        // used.
        if !cancel {
            middle.append(&logo_image(scale * 2.2));
        }

        let prompt = gtk::Label::new(Some(
            "Drop a video file here, browse for a local file, or enter a URL",
        ));
        prompt.add_css_class("tp-empty-prompt");
        prompt.set_wrap(true);
        prompt.set_justify(gtk::Justification::Center);
        middle.append(&prompt);

        const BROWSE_ICON: &[u8] = include_bytes!("../data/ui/browse.png");
        const LINK_ICON: &[u8] = include_bytes!("../data/ui/link.png");

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::Center)
            .build();
        // Straight to the thing itself rather than to a menu row that opens
        // it: with one file to choose and two ways to choose it, a step in
        // between is a step for nothing.
        //
        // Each carries the mark of what it opens, and Browse carries the same
        // one the media page's button does - so the button on the page and the
        // button in the panel it opens are visibly the same errand.
        let browse = gtk::Button::new();
        browse.set_child(Some(&marked_face(
            marked_image(BROWSE_ICON, PLAY_MARK_PX * scale),
            "  Browse...",
        )));
        browse.add_css_class("tp-button");
        browse.add_css_class("tp-action");
        name_it(&browse, "Browse");

        let address = gtk::Button::new();
        address.set_child(Some(&marked_face(
            marked_image(LINK_ICON, PLAY_MARK_PX * scale),
            "  Enter URL",
        )));
        address.add_css_class("tp-button");
        address.add_css_class("tp-action");
        name_it(&address, "Enter URL");

        buttons.append(&browse);
        buttons.append(&address);
        middle.append(&buttons);

        // On a row of its own beneath them rather than beside them: it is not
        // a third way to choose a video, and standing in line with two that
        // are made it look like one.
        let back = cancel.then(|| {
            let back = gtk::Button::with_label("Cancel");
            back.add_css_class("tp-button");
            back.set_halign(gtk::Align::Center);
            middle.append(&back);
            back
        });
        (middle, browse, address, back)
    }

    /// The screen with no video on it: an invitation, and the two ways to
    /// accept it.
    ///
    /// Deliberately not the menu with everything greyed out. There is nothing
    /// to choose until there is a film to choose it for, and a page of dashes
    /// asks to be read before it can be dismissed. The gear stays, because
    /// this is where somebody who has just installed the application arrives
    /// and every setting they might need is behind it.
    fn build_empty_page(self: &Rc<Self>) -> gtk::Overlay {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(px(30.0))
            .margin_bottom(px(26.0))
            .margin_start(px(34.0))
            .margin_end(px(34.0))
            // Filled for the reason the media page is: `Column` does the
            // centering, and a box that centers itself as well collapses to
            // its contents and takes the footer's corner with it.
            .build();

        let (middle, browse, address, _) = self.choose_source_panel(scale, false);
        content.append(&middle);

        {
            let app = self.clone();
            browse.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.browse_for_file();
            });
        }
        {
            let app = self.clone();
            address.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_paste_uri();
            });
        }

        // The same pair as the media page carries, in the same corner, so
        // they do not appear to move when a film is chosen.
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::End)
            .build();
        let (fullscreen, gear) = self.corner_buttons();
        footer.append(&gear);
        if let Some(fullscreen) = fullscreen.as_ref() {
            footer.append(fullscreen);
        }
        content.append(&footer);

        let mut stops: Vec<gtk::Button> = vec![browse.clone(), address];
        stops.push(gear);
        stops.extend(fullscreen);
        self.set_nav(None, &[], &stops);
        // Deferred until the page is actually in the window. This is built
        // before `show_menu` installs it, and focus cannot be taken by a
        // widget that is not on screen yet - the same reason `settle_on`
        // waits for the map on the first screen of a session.
        match browse.is_mapped() {
            true => browse.grab_focus(),
            false => {
                browse.connect_map(|browse| {
                    browse.grab_focus();
                });
                true
            }
        };

        self.behind_artwork(&content)
    }

    /// The mark that closes the player, at the far end of the row.
    ///
    /// Where a window's own close button would be, and worth having because
    /// on a television there is no window: TinePlayer opens fullscreen with no
    /// titlebar, and quitting otherwise means knowing that Escape asks. It
    /// asks the same question Escape does rather than quitting outright.
    fn close_button(self: &Rc<Self>) -> gtk::Button {
        const ICON: &[u8] = include_bytes!("../data/ui/close.png");

        let close = gtk::Button::new();
        close.set_child(Some(&marked_image(ICON, CORNER_MARK_PX * self.scale.get())));
        close.add_css_class("tp-gear");
        close.set_focus_on_click(false);
        close.set_tooltip_text(Some("Close the player"));
        name_it(&close, "Close the player");
        {
            let app = self.clone();
            close.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_confirm_quit();
            });
        }
        close
    }

    /// The mark that opens the panel for choosing a different video.
    ///
    /// Drawn and placed like the settings and fullscreen marks rather than
    /// like the play button, because it is the same kind of thing: something
    /// the page can do, rather than the thing the page is for.
    fn browse_button(self: &Rc<Self>) -> gtk::Button {
        const ICON: &[u8] = include_bytes!("../data/ui/browse.png");

        let open = gtk::Button::new();
        open.set_child(Some(&marked_image(ICON, CORNER_MARK_PX * self.scale.get())));
        open.add_css_class("tp-gear");
        open.set_focus_on_click(false);
        open.set_tooltip_text(Some("Choose a video"));
        name_it(&open, "Choose a video");
        {
            let app = self.clone();
            open.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.choose_video();
            });
        }
        open
    }

    /// The fullscreen mark and the gear, which sit together at the end of
    /// every footer on these two screens.
    ///
    /// Built here rather than twice, because the pair has three details worth
    /// not getting differently right in two places: the mark follows the
    /// window's own state, the gear carries the update badge, and neither
    /// takes focus from a click.
    fn corner_buttons(self: &Rc<Self>) -> (Option<gtk::Button>, gtk::Button) {
        // Maximize and restore rather than the usual fullscreen pair, which
        // is absent from the icon theme on both platforms and would draw the
        // missing-image glyph.
        let fullscreen = gtk::Button::new();
        fullscreen.set_child(Some(&fullscreen_image(
            self.window.is_fullscreen(),
            self.scale.get(),
        )));
        fullscreen.add_css_class("tp-gear");
        fullscreen.set_focus_on_click(false);
        fullscreen.set_tooltip_text(Some("Toggle fullscreen"));
        name_it(&fullscreen, "Toggle fullscreen");
        {
            let app = self.clone();
            fullscreen.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.toggle_fullscreen();
            });
        }
        {
            let weak = fullscreen.downgrade();
            let scale = self.scale.get();
            self.window.connect_fullscreened_notify(move |window| {
                if let Some(button) = weak.upgrade() {
                    button.set_child(Some(&fullscreen_image(window.is_fullscreen(), scale)));
                }
            });
        }

        let gear = gtk::Button::new();
        gear.set_child(Some(&settings_image(CORNER_MARK_PX * self.scale.get())));
        gear.add_css_class("tp-gear");
        gear.set_focus_on_click(false);
        gear.set_tooltip_text(Some("Settings"));
        name_it(&gear, "Settings");
        {
            let app = self.clone();
            gear.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }
        *self.update_badges.borrow_mut() = vec![gear.clone()];
        self.draw_update_badge();

        // Left out entirely when fullscreen is not this viewer's to change: a
        // button that declines to do the one thing it offers is worse than no
        // button.
        match self.locked_fullscreen {
            true => (None, gear),
            false => (Some(fullscreen), gear),
        }
    }

    /// Reads the artwork for the file just loaded, and redraws the page when
    /// it arrives.
    ///
    /// On a thread, because this is the part with a megabyte in it. A backdrop
    /// over a network share is long enough to be felt, and the page has to be
    /// on screen before it - a film's details held back until its wallpaper
    /// loads is the wrong thing to wait for.
    fn start_art_load(self: &Rc<Self>) {
        let (poster, backdrop) = {
            let details = self.details.borrow();
            (details.poster.clone(), details.backdrop.clone())
        };
        if poster.is_none() && backdrop.is_none() {
            return;
        }

        // What the artwork being read belongs to. A viewer who opens one film
        // and immediately opens another gets the second one's backdrop, not
        // whichever thread happened to finish last.
        let generation = self.art_generation.get();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let read = |art: Option<crate::metadata::Art>| {
                art.as_ref().and_then(crate::metadata::load_image)
            };
            let _ = sender.send((read(poster), read(backdrop)));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
            let (poster, backdrop) = match receiver.try_recv() {
                Ok(art) => art,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            if app.art_generation.get() != generation {
                return glib::ControlFlow::Break;
            }

            // Decoding happens here rather than on the thread: a GdkTexture
            // belongs to the main thread, and this is the only place that can
            // make one.
            let decode = |bytes: Option<Vec<u8>>| {
                let bytes = bytes?;
                match gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
                    Ok(texture) => Some(texture),
                    // Said out loud, because a poster that silently fails to
                    // appear looks like one that was never found - and the two
                    // want completely different things done about them.
                    Err(e) => {
                        eprintln!("Couldn't decode artwork: {e}");
                        None
                    }
                }
            };
            *app.poster_art.borrow_mut() = decode(poster);
            *app.backdrop_art.borrow_mut() = decode(backdrop);

            // Only worth redrawing the page that shows it, and only while it
            // is still the page on screen.
            if *app.screen.borrow() == Screen::Menu {
                app.show_menu();
            }
            glib::ControlFlow::Break
        });
    }

    // --- Choosers ------------------------------------------------------

    /// Enumerates the output devices on a thread, and calls `then` on the main
    /// thread if the answer differs from what the cache already held.
    ///
    /// For the popover, which opens immediately against whatever the cache
    /// already has and fills itself in when this lands. The probe is the one
    /// slow thing either menu does, and it is slow because it starts a device
    /// monitor - which asks every audio backend on the machine what it has.
    ///
    /// Polled rather than pushed, in the manner of the other threads here:
    /// nothing in this application may be touched from another thread, so the
    /// answer comes back through a channel and is picked up on this one.
    fn scan_devices_soon(self: &Rc<Self>, then: impl Fn(&Rc<Self>) + 'static) {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let names: Vec<String> = list_audio_output_devices()
                .map(|devices| {
                    devices
                        .iter()
                        .map(|device| device.display_name().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let _ = sender.send(names);
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
            let names = match receiver.try_recv() {
                Ok(names) => names,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                // Gone without an answer, which leaves nothing to show and no
                // reason to keep looking.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            app.device_scan.set(true);
            // Only when the answer is different. A refill re-selects the entry
            // in force, so running it against an unchanged list would throw
            // away wherever the viewer had arrowed to, a moment after they
            // got there.
            if *app.device_names.borrow() == names {
                return glib::ControlFlow::Break;
            }
            *app.device_names.borrow_mut() = names;
            then(&app);
            glib::ControlFlow::Break
        });
    }

    /// What a chooser offers, and which of it is already in force.
    ///
    /// Split out from the screen that shows it so a popover and a full page
    /// can offer exactly the same list. They differ in how they are put on
    /// screen and in nothing else, and two copies of this match is the way
    /// that stops being true.
    fn chooser_entries(self: &Rc<Self>, setting: Setting) -> Choices {
        // Entries are (display text, choice). `None` means the "None"
        // option, which every list offers except the primary device - an
        // output has to exist for anything to play.
        let mut entries: Vec<Choice> = Vec::new();
        let mut current: Option<usize> = None;
        let mut dividers: Vec<usize> = Vec::new();
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                if setting == Setting::SecondaryDevice {
                    entries.push(("None".to_string(), None));
                    // A rule under it. "None" here means "play nothing on a
                    // second output", which is a different kind of answer to
                    // the hardware listed below it - and the only list where
                    // this one is offered at all.
                    dividers.push(1);
                }
                let configured = {
                    let config = self.config.borrow();
                    if setting == Setting::PrimaryDevice {
                        config.primary_sink.clone()
                    } else {
                        config.secondary_sink.clone()
                    }
                };
                let devices = self.device_names.borrow();
                // Nothing found and nothing looked for yet: the caller is
                // showing this while the probe runs, so say so rather than
                // offering an empty list, which reads as "no outputs".
                if devices.is_empty() && !self.device_scan.get() {
                    entries.push(("Searching for outputs...".to_string(), None));
                }
                for (position, name) in devices.iter().enumerate() {
                    if configured.as_deref() == Some(name.as_str()) {
                        current = Some(position);
                    }
                    entries.push((name.clone(), Some(position)));
                }
            }
            Setting::Subtitles => {
                entries.push(("None".to_string(), None));
                let chosen = self.subtitle.borrow().clone();
                for (position, option) in self.subtitle_options.borrow().iter().enumerate() {
                    if chosen.as_ref() == Some(&option.choice()) {
                        current = Some(position);
                    }
                    entries.push((subtitle_label(option), Some(position)));
                }
                // Last, after everything the video came with, the same way the
                // track lists offer one: a subtitle file from somewhere else
                // is the answer when what is wanted is not beside the film.
                entries.push((
                    "Browse...".to_string(),
                    Some(self.subtitle_options.borrow().len()),
                ));
            }
            Setting::PrimaryTrack | Setting::SecondaryTrack => {
                entries.push(("None".to_string(), None));
                let role = if setting == Setting::PrimaryTrack {
                    Role::Primary
                } else {
                    Role::Secondary
                };
                let chosen = *self.track_for(role).borrow();
                let file = self.file_for(role).borrow().clone();
                for (position, track) in self.tracks.borrow().iter().enumerate() {
                    if file.is_none() && chosen == Some(track.index) {
                        current = Some(position);
                    }
                    entries.push((describe_audio_track(track), Some(position)));
                }
                // Last, after everything inside the video: a separate file is
                // the answer when what you want is not in there at all, which
                // is most films with one soundtrack and a description track
                // downloaded beside them.
                let audio_file = entries.len() - 1;
                if let Some(file) = file.as_ref() {
                    current = Some(audio_file);
                    entries.push((format!("Audio File: {}", file.label()), Some(audio_file)));
                } else {
                    entries.push(("Browse...".to_string(), Some(audio_file)));
                }
            }
            Setting::PrimaryLanguage | Setting::SecondaryLanguage => {
                let configured = {
                    let config = self.config.borrow();
                    if setting == Setting::PrimaryLanguage {
                        config.primary_language.clone()
                    } else {
                        config.secondary_language.clone()
                    }
                };
                current = language_position(configured.as_deref());
                // A rule under it, before the languages. The entry above is
                // not a language at all - it is the absence of a preference,
                // which leaves the choice to whatever the file offers first -
                // and run flush against Afrikaans it reads as one.
                dividers.push(1);
                // Worded exactly as the settings row shows it when unset, so
                // the list and the value it came from agree.
                entries.push((
                    if setting == Setting::PrimaryLanguage {
                        "First track".to_string()
                    } else {
                        "Second track".to_string()
                    },
                    None,
                ));
                for (position, (code, name, native, _)) in
                    crate::languages::LANGUAGES.iter().enumerate()
                {
                    entries.push((
                        crate::languages::menu_name(code, name, native),
                        Some(position),
                    ));
                }
            }
            Setting::SubtitleLanguage => {
                // The automatic choices first, then the languages, in one
                // list: they answer the same question, and following an
                // output is the answer most people want.
                let modes = crate::subtitles::MODES.len();
                let setting = self
                    .config
                    .borrow()
                    .subtitle_language
                    .clone()
                    .unwrap_or_else(|| crate::subtitles::DEFAULT_MODE.to_string());
                current = crate::subtitles::MODES
                    .iter()
                    .position(|(value, _)| *value == setting)
                    .or_else(|| {
                        crate::languages::LANGUAGES
                            .iter()
                            .position(|(code, _, _, _)| *code == setting)
                            .map(|position| modes + position)
                    });
                // Below "None", and again above the languages. What sits
                // between is the part worth choosing: following an output
                // tracks whatever is actually being heard, file by file, where
                // naming a language is a guess that holds until it does not.
                dividers.push(1);
                dividers.push(modes);
                for (position, (_, label)) in crate::subtitles::MODES.iter().enumerate() {
                    entries.push((label.to_string(), Some(position)));
                }
                for (position, (code, name, native, _)) in
                    crate::languages::LANGUAGES.iter().enumerate()
                {
                    entries.push((
                        crate::languages::menu_name(code, name, native),
                        Some(modes + position),
                    ));
                }
            }
            Setting::SubtitleFont => {
                let chosen = self
                    .config
                    .borrow()
                    .subtitle_font
                    .clone()
                    .unwrap_or_else(|| crate::pipeline::DEFAULT_SUBTITLE_FONT.to_string());
                current = SUBTITLE_FONTS.iter().position(|font| *font == chosen);
                for (position, font) in SUBTITLE_FONTS.iter().enumerate() {
                    entries.push((font.to_string(), Some(position)));
                }
            }
        }

        Choices {
            entries,
            current,
            dividers,
        }
    }

    /// Puts the current screen's navigation aside, so something on top of it
    /// can have the keyboard for a while.
    ///
    /// The application keeps one navigation model for the screen on display -
    /// which list the arrows drive, which buttons sit above and below it. A
    /// popover is the first thing that is neither a screen nor part of one: it
    /// needs the arrows while it is open and has to give them back exactly as
    /// it found them, because the page underneath is still there and still
    /// where the viewer will be returned to.
    fn take_nav(&self) -> NavState {
        NavState {
            list: self.nav_list.borrow().clone(),
            header: self.nav_header.borrow().clone(),
            footer: self.nav_footer.borrow().clone(),
            header_entry: self.nav_header_entry.borrow().clone(),
            stops: self.nav_stops.borrow().clone(),
            copy_root: self.copy_root.borrow().clone(),
        }
    }

    /// Gives the screen underneath its navigation back.
    fn put_nav(&self, state: NavState) {
        *self.nav_list.borrow_mut() = state.list;
        *self.nav_header.borrow_mut() = state.header;
        *self.nav_footer.borrow_mut() = state.footer;
        *self.nav_header_entry.borrow_mut() = state.header_entry;
        *self.nav_stops.borrow_mut() = state.stops;
        *self.copy_root.borrow_mut() = state.copy_root;
    }

    /// A selector over the row that opened it, rather than a page that
    /// replaces everything.
    ///
    /// The same entries a full chooser would list, from `chooser_entries`, in
    /// a popover anchored to the row. The page stays visible behind it, which
    /// is the point: what you are choosing for is still on screen, and the
    /// same widget will work over a playing film when these are wanted during
    /// playback.
    fn show_selector(self: &Rc<Self>, setting: Setting, anchor: &gtk::ListBoxRow) {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;
        // A device list is not ready when the popover opens - it is being
        // probed on a thread - so the entries are held rather than captured,
        // and the rows are filled by something that can be run twice.
        let entries: Rc<RefCell<Vec<Choice>>> = Rc::new(RefCell::new(Vec::new()));
        let (scroller, list) = scrolling_list();
        let fill: Rc<Fill> = {
            let entries = entries.clone();
            let list = list.clone();
            Rc::new(move |app: &Rc<Self>| {
                let Choices {
                    entries: fresh,
                    current,
                    dividers,
                } = app.chooser_entries(setting);
                while let Some(row) = list.row_at_index(0) {
                    list.remove(&row);
                }
                for (text, _) in &fresh {
                    let entry = chooser_row(text);
                    // Right-aligned, unlike the same row on a full chooser
                    // page. The popover opens against a row whose value sits
                    // on the right, and the choices are that value's
                    // alternatives - so they read as a column under it rather
                    // than as a list that starts somewhere else.
                    entry.set_xalign(1.0);
                    append_named(&list, &entry, text);
                }
                // Opened on whatever is already in force. Grabbing focus
                // scrolls it into view, which is what a long list needs.
                let opening = fresh
                    .iter()
                    .position(|(_, choice)| *choice == current)
                    .unwrap_or(0) as i32;
                *entries.borrow_mut() = fresh;
                // A rule above the entries that begin a group. A header rather
                // than a row of its own, for the reason the media page's group
                // headings give: headers sit outside the selection model and
                // the focus chain, so a rule cannot be landed on. Set on every
                // fill, since the rows it describes are rebuilt each time.
                list.set_header_func(move |row, _| {
                    match dividers.contains(&(row.index() as usize)) {
                        true => {
                            row.set_header(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)))
                        }
                        false => row.set_header(None::<&gtk::Widget>),
                    }
                });
                if let Some(row) = list.row_at_index(opening) {
                    row.add_css_class("tp-current");
                    list.select_row(Some(&row));
                    settle_on(&row);
                } else {
                    // Nothing to settle on, but the claim is still worth
                    // making: it supersedes any settling left pending by the
                    // row this popover opened over, which would otherwise come
                    // due and pull the focus back out to the page.
                    claim_settling();
                    list.grab_focus();
                }
            })
        };
        fill(self);
        // As wide as its longest entry, between a floor and a ceiling.
        //
        // `propagate_natural_width` is the part that does the work, and its
        // absence is what made the first attempt at this a narrow column of
        // "...": without it a scrolled window's natural width *is* its
        // `min-content-width`, so the popover opened at the floor no matter
        // what was in it. Ellipsizing entries make that failure look like a
        // sizing bug rather than a missing property, because ellipsizing is
        // what lets a label shrink that far in the first place - it lowers the
        // minimum width and leaves the natural width alone, which is exactly
        // the number wanted here.
        // Fixed for a device list, which opens holding a placeholder and is
        // filled in a moment later: sized to its contents it would open narrow
        // and jump wider under the pointer. The row's own width is a stable
        // number and a generous one, and device names are long.
        let devices = matches!(setting, Setting::PrimaryDevice | Setting::SecondaryDevice);
        // Two different questions. Every opening of a device list goes and
        // looks again, because hardware is plugged in and unplugged between
        // openings and a cache that is never refreshed is only a stale list.
        // Only the first opening has nothing to show while that happens.
        let waiting = devices && !self.device_scan.get();
        if waiting {
            scroller.set_size_request(anchor.width().max(px(SELECTOR_MIN_WIDTH)), -1);
        }
        scroller.set_propagate_natural_width(true);
        scroller.set_min_content_width(px(SELECTOR_MIN_WIDTH));
        // A ceiling as well, for the one entry that has no natural length: an
        // audio file is named by its path, and some of those are a page wide.
        scroller.set_max_content_width(px(SELECTOR_MAX_WIDTH));
        // Tall lists scroll rather than growing past the window - the language
        // list is two hundred entries. Short ones stay short.
        scroller.set_max_content_height(px(SELECTOR_HEIGHT));
        scroller.set_propagate_natural_height(true);

        let popover = gtk::Popover::builder()
            .child(&scroller)
            .position(gtk::PositionType::Bottom)
            // No arrow: this is a panel of choices, not a speech bubble, and
            // the anchor is already obvious from where it opens.
            .has_arrow(false)
            .build();
        popover.add_css_class("tp-selector");
        popover.set_parent(anchor);
        // What the popover will be: its contents, plus the padding
        // `.tp-selector > contents` puts around them. Measured on the child
        // for the reason `aim` gives - the popover itself measures zero.
        let (_, content_width, _, _) = scroller.measure(gtk::Orientation::Horizontal, -1);
        aim_right(&popover, anchor, content_width + px(SELECTOR_PAD) * 2);

        // The arrows belong to the popover while it is up, and to the page
        // again the moment it is not.
        let saved = self.take_nav();
        self.wire_navigation(&list, &[], &[]);
        {
            let app = self.clone();
            let saved = std::cell::RefCell::new(Some(saved));
            popover.connect_closed(move |popover| {
                if let Some(saved) = saved.borrow_mut().take() {
                    app.put_nav(saved);
                }
                // A popover parented by hand has to be unparented by hand, or
                // it outlives the row and GTK complains when that row goes.
                if popover.parent().is_some() {
                    popover.unparent();
                }
            });
        }

        {
            let app = self.clone();
            let entries = entries.clone();
            let popover = popover.clone();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let choice = match entries.borrow().get(row.index() as usize) {
                    Some((_, choice)) => *choice,
                    None => return,
                };
                popover.popdown();
                // After the popover has gone, not during. Applying a choice
                // rebuilds the page underneath, which destroys the row this is
                // anchored to - and doing that while it is still up is how a
                // widget ends up parented to something that no longer exists.
                //
                // Rebuilt rather than patched because a choice can change more
                // than the row it was made on: picking a second output fills
                // in the rows below it, and clearing one empties them.
                let app = app.clone();
                let over = *app.screen.borrow();
                glib::idle_add_local_once(move || {
                    if app.apply_choice(setting, choice) {
                        return;
                    }
                    match over {
                        Screen::Settings => app.show_settings(),
                        _ => app.show_menu(),
                    }
                });
            });
        }

        popover.popup();
        // Deliberately not re-aimed once it is open. Correcting against
        // `popover.width()` after the fact was tried and is wrong twice over:
        // an allocated popover measures wider than its contents, because the
        // allocation carries the margin the shadow is drawn into, so the
        // correction moved a popover that had opened in the right place about
        // fifty pixels to the left - and it did it a frame late, in full view.
        //
        // Selecting the current entry is `fill`'s job, and it is run again
        // here so that it happens with the list allocated: scrolling a row
        // into view needs a size, and inside a popover there is none until it
        // has been shown.
        fill(self);

        // The outputs, once something has gone and found them. The popover is
        // already up with "Searching for outputs..." in it, and fills in when
        // this lands - which is the whole point of doing it this way, since
        // the probe is slow enough on the main thread to read as the menu
        // being stuck.
        if devices {
            let fill = fill.clone();
            // Only if it is still open. Refilling a popover that has been
            // dismissed would be pointless, and worse than pointless: it ends
            // by focusing the entry in force, which would take focus off the
            // page the viewer went back to.
            let popover = popover.downgrade();
            self.scan_devices_soon(move |app| {
                if popover
                    .upgrade()
                    .is_some_and(|popover| popover.is_visible())
                {
                    fill(app);
                }
            });
        }
    }

    fn wire_navigation(
        self: &Rc<Self>,
        list: &gtk::ListBox,
        header: &[gtk::Button],
        footer: &[gtk::Button],
    ) {
        self.set_nav(Some(list), header, footer);
        announce_selection(list);

        // Every arrow key goes through move_selection, which already knows
        // where the focus is and what should happen at each boundary - it is
        // what the gamepad and the page keys have always used.
        //
        // It has to, now that rows are not focusable: GtkListBox moves the
        // cursor by moving focus between rows, and with nothing in the list
        // able to take focus that does nothing at all. Capture phase so this
        // runs before the list's own bindings rather than after they have
        // swallowed the key.
        self.wire_arrows(list.upcast_ref());
        for button in header.iter().chain(footer.iter()) {
            self.wire_arrows(button.upcast_ref());
        }

        // Tabbing into a list has to land somewhere. GTK selects nothing on
        // its own now that no row takes focus, which left the list holding
        // focus with nothing highlighted and the arrow keys apparently dead.
        {
            let list_weak = list.downgrade();
            let controller = gtk::EventControllerFocus::new();
            controller.connect_enter(move |_| {
                let Some(list) = list_weak.upgrade() else {
                    return;
                };
                if list.selected_row().is_some() {
                    return;
                }
                let first = (0..).find(|index| {
                    list.row_at_index(*index)
                        .is_none_or(|row| row.is_sensitive())
                });
                if let Some(row) = first.and_then(|index| list.row_at_index(index)) {
                    list.select_row(Some(&row));
                }
            });
            list.add_controller(controller);
        }
    }

    /// Writes the current track pair against the current file, so a choice
    /// survives even if the file is never played.
    fn remember_tracks(&self) {
        let Some(key) = self.storage_key() else {
            return;
        };
        crate::config::save_tracks(
            &key,
            *self.primary_track.borrow(),
            *self.secondary_track.borrow(),
            self.subtitle.borrow().clone(),
            self.saved_path(Role::Primary),
            self.saved_path(Role::Secondary),
        );
    }

    /// The audio file chosen for an output, as something worth writing down.
    ///
    /// Only a local path: a file reached by URL is not ours to promise will
    /// still be there, and rebuilding one from a saved string is a different
    /// question from finding a file again.
    fn saved_path(&self, role: Role) -> Option<std::path::PathBuf> {
        self.file_for(role)
            .borrow()
            .as_ref()
            .and_then(|file| file.local().map(|path| path.to_path_buf()))
    }

    /// What the menu shows against the Subtitles row.
    fn describe_subtitle(&self) -> String {
        let Some(chosen) = self.subtitle.borrow().clone() else {
            return "None".to_string();
        };
        self.subtitle_options
            .borrow()
            .iter()
            .find(|option| option.choice() == chosen)
            .map(subtitle_label)
            .unwrap_or_else(|| "None".to_string())
    }

    /// Returns whether it has already moved to another screen, in which case
    /// the caller must not navigate on top of it.
    fn apply_choice(self: &Rc<Self>, setting: Setting, choice: Option<usize>) -> bool {
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                // From the cache the list was built from, not a fresh probe.
                // This used to enumerate the hardware all over again just to
                // turn the row that was pressed back into a name, which put a
                // second pause between the press and anything happening.
                let picked = {
                    let names = self.device_names.borrow();
                    choice.and_then(|index| names.get(index).cloned())
                };

                let mut cleared_secondary = false;
                {
                    let mut config = self.config.borrow_mut();
                    if setting == Setting::PrimaryDevice {
                        // The primary output cannot be cleared: without one
                        // there is nothing to play through.
                        if picked.is_none() {
                            return false;
                        }
                        config.primary_sink = picked;
                    } else {
                        config.secondary_sink = picked;
                        // A secondary track without a device to play it on is
                        // meaningless, so clear it alongside - and a separate
                        // audio file the same way, which was missed. Left set,
                        // it is still a choice the menu shows and the pipeline
                        // tries to honor, against an output that no longer
                        // exists.
                        if config.secondary_sink.is_none() {
                            *self.secondary_track.borrow_mut() = None;
                            *self.secondary_file.borrow_mut() = None;
                            cleared_secondary = true;
                        }
                    }
                    config.capture_display_session();
                    if let Err(e) = config.save() {
                        eprintln!("Failed to save config: {e}");
                    }
                }

                // Interface sounds follow the primary output, so they play
                // where the user is listening. Rebuilt on change rather
                // than only at startup, which previously meant a restart
                // before a newly chosen device took effect.
                if cleared_secondary {
                    self.remember_tracks();
                    // The file went with the device, so its alignment goes too.
                    self.load_baselines();
                }

                if setting == Setting::PrimaryDevice {
                    let (enabled, device) = {
                        let config = self.config.borrow();
                        (config.sounds, config.primary_sink.clone())
                    };
                    *self.sounds.borrow_mut() = Sounds::new(enabled, device);
                }
            }
            Setting::PrimaryLanguage | Setting::SecondaryLanguage => {
                let picked = choice
                    .and_then(|index| crate::languages::LANGUAGES.get(index))
                    .map(|(code, _, _, _)| code.to_string());
                let mut config = self.config.borrow_mut();
                if setting == Setting::PrimaryLanguage {
                    config.primary_language = picked;
                } else {
                    config.secondary_language = picked;
                }
                let _ = config.save();
            }
            Setting::SubtitleLanguage => {
                let modes = crate::subtitles::MODES.len();
                let picked = choice.map(|index| match index.checked_sub(modes) {
                    Some(language) => crate::languages::LANGUAGES[language].0.to_string(),
                    None => crate::subtitles::MODES[index].0.to_string(),
                });
                let mut config = self.config.borrow_mut();
                config.subtitle_language = picked;
                let _ = config.save();
            }
            Setting::SubtitleFont => {
                let mut config = self.config.borrow_mut();
                config.subtitle_font = choice
                    .and_then(|index| SUBTITLE_FONTS.get(index))
                    .map(|font| font.to_string());
                let _ = config.save();
            }
            Setting::Subtitles => {
                let options = self.subtitle_options.borrow();
                // The row after the last option is the browse one, which opens
                // a screen instead of settling anything here.
                if choice == Some(options.len()) {
                    drop(options);
                    self.browse_for_subtitle();
                    return true;
                }
                let picked = choice
                    .and_then(|index| options.get(index))
                    .map(|o| o.choice());
                drop(options);
                *self.subtitle.borrow_mut() = picked;
                // Choosing a subtitle is asking to see it, whatever the
                // toggle was doing for the last one.
                self.subtitles_hidden.set(false);
                self.remember_tracks();
            }
            Setting::PrimaryTrack | Setting::SecondaryTrack => {
                let role = if setting == Setting::PrimaryTrack {
                    Role::Primary
                } else {
                    Role::Secondary
                };
                let count = self.tracks.borrow().len();
                // The row after the last track is the audio file one, which
                // opens the browser instead of settling anything here.
                if choice == Some(count) {
                    self.browse_for_audio(role);
                    return true;
                }

                let tracks = self.tracks.borrow();
                let picked = choice.and_then(|index| tracks.get(index)).map(|t| t.index);
                drop(tracks);
                *self.track_for(role).borrow_mut() = picked;
                // Choosing anything inside the video, including None, is
                // choosing not to use a separate file on that output.
                *self.file_for(role).borrow_mut() = None;
                self.remember_tracks();
                // The pairing is gone, so the alignment measured for it has to
                // go with it. A baseline left behind is applied to a track
                // inside the video, which shares the video's timeline and needs
                // no correction - and a large one silences that output
                // outright. Measured on the Pi 2026-08-10: -830ms against an
                // embedded track produced no audio at all, while -300ms and
                // +830ms both played, so it is pulling the audio further
                // forward than the pipeline can deliver.
                self.load_baselines();
            }
        }
        false
    }

    // --- File selection ------------------------------------------------

    fn open_file_chooser(self: &Rc<Self>, start: &std::path::Path) {
        // FileChooserNative rather than FileDialog: the latter needs GTK
        // 4.10, above this project's 4.6 baseline. It also gives the real
        // system file dialog on each platform.
        // Which errand this is on, decided the same way the built-in browser
        // decides it, so the two always agree about what is being chosen.
        let errand = self.errand.get();
        let chooser = gtk::FileChooserNative::new(
            Some(match errand {
                Errand::Audio(_) => "Choose an audio file",
                Errand::Subtitle => "Choose a subtitle file",
                _ => "Choose a video",
            }),
            Some(&self.window),
            gtk::FileChooserAction::Open,
            Some("Open"),
            Some("Cancel"),
        );

        // The pipeline typefinds rather than assuming a container, so this
        // list is about not cluttering the dialog with non-video files, not
        // about what will actually play. Anything GStreamer can demux works,
        // which is why "All files" stays available below.
        let filter = gtk::FileFilter::new();
        let (name, extensions) = if errand == Errand::Subtitle {
            ("Subtitle files", &crate::subtitles::EXTENSIONS[..])
        } else if matches!(errand, Errand::Audio(_)) {
            ("Audio files", crate::browser::AUDIO_EXTENSIONS)
        } else {
            ("Video files", &crate::browser::VIDEO_EXTENSIONS[..])
        };
        filter.set_name(Some(name));
        for extension in extensions {
            // Case-insensitive by hand: GTK's pattern matching is not, and
            // ".MKV" off a camera or an old disc is common enough to matter.
            filter.add_pattern(&format!("*.{extension}"));
            filter.add_pattern(&format!("*.{}", extension.to_uppercase()));
        }
        chooser.add_filter(&filter);
        open_at(&chooser, start);

        let all = gtk::FileFilter::new();
        all.set_name(Some("All files"));
        all.add_pattern("*");
        chooser.add_filter(&all);

        let app = self.clone();
        // Where this was opened from, so canceling returns there rather than
        // dropping to the menu. Reached from the browser, canceling should
        // leave you in the folder you were looking at.
        let from_browser = *self.screen.borrow() == Screen::Browser;
        let folder = self.config.borrow().last_folder.clone();

        // Held by the closure so the dialog outlives this function; a
        // dropped FileChooserNative closes before the user can answer.
        let held = RefCell::new(Some(chooser.clone()));
        chooser.connect_response(move |chooser, response| {
            let chosen = (response == gtk::ResponseType::Accept)
                .then(|| chooser.file().and_then(|f| f.path()))
                .flatten();
            held.borrow_mut().take();

            match chosen {
                // A subtitle or a soundtrack for the video already loaded,
                // rather than a video to load.
                Some(path) if errand == Errand::Subtitle => {
                    app.set_subtitle_file(&path);
                    app.show_menu();
                }
                Some(path) if matches!(errand, Errand::Audio(_)) => {
                    app.set_audio_file(&path);
                    app.show_menu();
                }
                // A file was picked, so the menu is where to go next either
                // way.
                Some(path) => {
                    let source = Source::File(path);
                    match app.set_file(&source) {
                        Ok(()) => app.show_menu(),
                        Err(e) => app.show_source_error(&source, &e, false),
                    }
                }
                None => match folder.as_deref().filter(|_| from_browser) {
                    Some(folder) => app.show_browser(folder, None),
                    None => app.show_menu(),
                },
            }
        });
        chooser.show();
    }

    /// Probes the file and chooses tracks for it.
    ///
    /// A file played before comes back with the tracks it was played with;
    /// otherwise the first track goes to the primary output and a different
    /// one to the secondary, which is the whole point of the application.
    fn set_file(self: &Rc<Self>, source: &Source) -> Result<(), String> {
        match crate::probe::probe_media(source) {
            Ok(media) => self.apply_media(source, media),
            Err(e) => {
                eprintln!("Couldn't read {}: {e}", source.uri());
                self.forget_file();
                Err(e)
            }
        }
    }

    /// Drops everything that described the file that was loaded.
    fn forget_file(&self) {
        *self.details.borrow_mut() = Default::default();
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        // Anything still being read for the file being forgotten is now for
        // the wrong one, and this is what tells it so.
        self.art_generation.set(self.art_generation.get() + 1);
        *self.tracks.borrow_mut() = Vec::new();
        *self.subtitle_options.borrow_mut() = Vec::new();
        *self.primary_track.borrow_mut() = None;
        *self.secondary_track.borrow_mut() = None;
        *self.subtitle.borrow_mut() = None;
        *self.file.borrow_mut() = None;
        self.duration_s.set(0.0);
    }

    /// Takes up a probed source: which tracks to start on, which subtitle,
    /// and what to show in the menu.
    ///
    /// Separate from the probing so that a caller which probed on a thread,
    /// rather than making the interface wait for it, has somewhere to hand
    /// the result back on the main thread.
    fn apply_media(
        self: &Rc<Self>,
        source: &Source,
        media: crate::probe::Media,
    ) -> Result<(), String> {
        // A different video starts with its subtitles showing, whatever the
        // last one was left doing.
        self.subtitles_hidden.set(false);
        // Kodi's one video player slot is necessarily this playback while it
        // waits for us, but a session started by hand with --kodi could attach
        // to a *different* external player's item. Lengths agreeing is a cheap
        // guard against that, and against writing progress onto the wrong film.
        if let Some(runtime) = self
            .kodi_item
            .borrow()
            .as_ref()
            .map(|item| item.runtime_s)
            .filter(|runtime| *runtime > 0)
            && media.duration_ns > 0
        {
            let ours = media.duration_ns / 1_000_000_000;
            if ours.abs_diff(runtime) > 5 {
                eprintln!(
                    "Kodi reports a {runtime}s item but this source is {ours}s;                      ignoring what it said and keeping local positions."
                );
                *self.kodi_item.borrow_mut() = None;
            }
        }

        // What the page shows about the file, from the sidecar beside it and
        // the container's own tags. Cheap - a small file and a few `is_file`
        // calls - and the artwork behind whatever it found is read separately,
        // on a thread, because that is the part with a megabyte in it.
        //
        // Taken here rather than further down because the lists below are
        // moved out of `media`, and this reads the whole of it.
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        self.art_generation.set(self.art_generation.get() + 1);
        let beside = {
            let config = self.config.borrow();
            crate::metadata::Beside {
                metadata: config.read_metadata,
                backdrop: config.show_backdrop,
            }
        };
        *self.details.borrow_mut() = crate::metadata::resolve(source, &media, beside);

        let duration_ns = media.duration_ns;
        let tracks = media.audio;
        let mut options = crate::subtitles::options(source.local(), &media.subtitles);

        let (primary_language, secondary_language, subtitle_language, described) = {
            let config = self.config.borrow();
            (
                config.primary_language.clone(),
                config.secondary_language.clone(),
                config.subtitle_language.clone(),
                (
                    config.primary_audio_description,
                    config.secondary_audio_description,
                ),
            )
        };
        let describes =
            |track: &crate::probe::AudioTrack| crate::probe::is_audio_description(&track.title);

        // What ordinary selection is allowed to pick from: everything except
        // the described tracks, which are only ever chosen by asking for them.
        // Without this, a file whose first English track happens to be the
        // described one would hand narration to someone who never wanted it.
        //
        // Unless description is all there is. A file with nothing else would
        // otherwise start silent, which reads as the player being broken
        // rather than as a preference being honored.
        let pool: Vec<&crate::probe::AudioTrack> = {
            let plain: Vec<_> = tracks.iter().filter(|track| !describes(track)).collect();
            if plain.is_empty() {
                tracks.iter().collect()
            } else {
                plain
            }
        };

        // First track in the preferred language, if one was named.
        let by_language = |preferred: &Option<String>| -> Option<u32> {
            let code = preferred.as_deref()?;
            pool.iter()
                .find(|track| crate::languages::matches(&track.language, code))
                .map(|track| track.index)
        };
        // A described track for an output that asked for one. Not finding one
        // is not a failure - most files have none - so it falls back to the
        // ordinary choice rather than leaving the output silent.
        //
        // A named language is a hard requirement, not a preference to relax:
        // description narrated in a language you do not speak is worse than no
        // description at all, so the fallback is the right language undescribed
        // rather than the wrong language described.
        let described_track = |want: bool, preferred: &Option<String>| -> Option<u32> {
            if !want {
                return None;
            }
            let Some(code) = preferred.as_deref() else {
                return tracks
                    .iter()
                    .find(|track| describes(track))
                    .map(|track| track.index);
            };
            tracks
                .iter()
                .find(|track| describes(track) && crate::languages::matches(&track.language, code))
                // Then one whose language is not stated. Unknown is not the
                // same as wrong: a track tagged for another language is
                // rejected, but plenty of description carries no tag at all -
                // the tool most people use to add one sets a title and no
                // language - and refusing those would mean finding nothing in
                // the commonest case of all.
                .or_else(|| {
                    tracks
                        .iter()
                        .find(|track| describes(track) && !crate::languages::known(&track.language))
                })
                .map(|track| track.index)
        };

        // Keyed on the video being loaded rather than the one still current,
        // which is not this one until the end of this function.
        let saved = crate::config::load_resume(&self.storage_key_for(source))
            .and_then(|resume| resume.tracks);
        let (primary, secondary) = match saved.clone() {
            // A saved None is a real choice ("no audio on that output"), so a
            // saved pair is taken as it stands rather than filled in.
            Some(choice) => (choice.primary, choice.secondary),
            // Otherwise the preferred languages decide, falling back to the
            // old behavior of the first track and a different one.
            None => (
                described_track(described.0, &primary_language)
                    .or_else(|| by_language(&primary_language))
                    .or_else(|| pool.first().map(|t| t.index)),
                described_track(described.1, &secondary_language)
                    .or_else(|| by_language(&secondary_language))
                    .or_else(|| pool.get(1).map(|t| t.index)),
            ),
        };
        // The file may have been re-encoded since it was last played.
        let known = |choice: Option<u32>| choice.filter(|i| tracks.iter().any(|t| t.index == *i));

        *self.primary_track.borrow_mut() = known(primary);
        *self.secondary_track.borrow_mut() = if self.config.borrow().secondary_sink.is_some() {
            known(secondary)
        } else {
            // Without a device to play it on, holding a secondary track only
            // produces a pipeline that fails to build.
            None
        };
        // A separate audio file, kept only if it is still where it was. One
        // that has been deleted, renamed, or is on a drive not mounted today
        // falls back to the track underneath it rather than failing when play
        // is pressed - the same rule the subtitle below follows.
        let still_there = |path: Option<&std::path::PathBuf>| {
            path.filter(|path| path.exists())
                .map(|path| Source::File(path.clone()))
        };
        *self.primary_file.borrow_mut() = still_there(
            saved
                .as_ref()
                .and_then(|choice| choice.primary_file.as_ref()),
        );
        *self.secondary_file.borrow_mut() = if self.config.borrow().secondary_sink.is_some() {
            still_there(
                saved
                    .as_ref()
                    .and_then(|choice| choice.secondary_file.as_ref()),
            )
        } else {
            None
        };

        // Only kept if it still resolves: an embedded stream the file no
        // longer has, or a subtitle file since deleted, quietly reverts to
        // none rather than failing when play is pressed.
        let subtitle = match saved {
            Some(choice) => choice.subtitle,
            // Follows whichever audio is actually going to each output, not
            // the language preference: the preference may have found nothing,
            // and what is being heard is what subtitles have to match.
            None => {
                let language_of = |index: Option<u32>| {
                    index.and_then(|index| {
                        tracks
                            .iter()
                            .find(|track| track.index == index)
                            .map(|track| track.language.as_str())
                    })
                };
                crate::subtitles::automatic(
                    &crate::subtitles::Auto::parse(
                        subtitle_language
                            .as_deref()
                            .unwrap_or(crate::subtitles::DEFAULT_MODE),
                    ),
                    &options,
                    language_of(known(primary)),
                    language_of(known(secondary)),
                )
            }
        };
        // A file chosen by hand is not beside the video, so nothing above
        // found it. Put it back before the check below, or the choice would be
        // dropped as unrecognised every time the file was loaded again - and
        // only if it is still on disk, since a remembered path can outlive the
        // file it names.
        if let Some(crate::subtitles::SubtitleChoice::File(path)) = subtitle.as_ref()
            && path.is_file()
        {
            options.push(crate::subtitles::chosen_file(path));
        }
        *self.subtitle.borrow_mut() =
            subtitle.filter(|choice| options.iter().any(|option| option.choice() == *choice));
        *self.subtitle_options.borrow_mut() = options;
        *self.tracks.borrow_mut() = tracks;
        *self.file.borrow_mut() = Some(source.clone());
        self.duration_s.set(duration_ns as f64 / 1e9);
        // Now that the video and its audio files are both settled, whatever
        // was measured about that pairing applies again.
        self.load_baselines();
        // The page can be drawn without artwork and filled in when it lands,
        // so this is started rather than waited for.
        self.start_art_load();

        // Only a local file is worth reopening: a remote URL can carry an
        // access token that expires, and whatever launched us will hand it over
        // again anyway.
        if let Some(path) = source.local() {
            let mut config = self.config.borrow_mut();
            config.last_video = Some(path.to_path_buf());
            let _ = config.save();
        }
        Ok(())
    }

    /// Says, on screen, why a video could not be opened.
    ///
    /// Worth a screen rather than a line on stderr: when something else
    /// launched the player there is no terminal to read, and the window
    /// closing again immediately is all anyone sees. That is exactly the case
    /// most likely to fail, because a media center can hand over a path or a
    /// URL that means nothing on this machine.
    ///
    /// The message GStreamer gave is shown as it stands. It is more specific
    /// than anything that could be inferred from the kind of source - an
    /// unmounted share, a refused connection and a missing file all arrive
    /// here, and guessing between them would sometimes be wrong.
    fn show_source_error(self: &Rc<Self>, source: &Source, error: &str, fatal: bool) {
        // Percent escaping is how a URI carries a space; it is not how anyone
        // wants to read a path. Decoded for display only - what gets opened is
        // still the escaped form. Anything that is not valid escaping is left
        // alone rather than mangled.
        let readable = |text: &str| {
            glib::Uri::unescape_string(text, None)
                .map(|decoded| decoded.to_string())
                .unwrap_or_else(|| text.to_string())
        };

        let mut message = format!(
            "Couldn't open:\n{}\n\n{}",
            readable(&source.uri()),
            readable(error)
        );
        // Whatever launched us handed over a path or URL this machine could
        // not open, so what helps is knowing which paths and URLs work, rather
        // than anything about the launcher itself.
        if self.external {
            message.push_str("\n\nSee docs/usage.md for the paths and URLs that can be played.");
        }
        self.show_error(&message, fatal);
    }

    // --- Browsing ------------------------------------------------------

    /// Notes the screen a modal is about to cover.
    ///
    /// Only the screens that are not themselves modals, so that one modal
    /// replacing another leaves the pair's origin alone. A modal recorded as
    /// its own origin is a trap: backing out of it returns to itself, and
    /// nothing closes it.
    fn remember_origin(&self) {
        let screen = *self.screen.borrow();
        if matches!(screen, Screen::Menu | Screen::VideoSource) {
            self.origin.set(screen);
        }
    }

    /// Back to whatever the modal was opened over.
    fn return_to_origin(self: &Rc<Self>) {
        match self.origin.get() {
            Screen::VideoSource => self.choose_video(),
            _ => self.show_menu(),
        }
    }

    /// Floats a page over the main menu, dimmed and unresponsive behind it.
    ///
    /// The menu is rebuilt rather than kept aside, because every screen here
    /// replaces the window's child outright and there is no earlier page still
    /// around to reuse. Building a second one is cheap next to what it buys:
    /// the browser reads as something opened over the menu instead of as
    /// another step deeper into it.
    fn modal(self: &Rc<Self>, page: &gtk::Box) -> gtk::Overlay {
        // Whatever is on screen right now, so the modal opens over the screen
        // it was actually opened from rather than always over the main menu.
        //
        // One modal replacing another hands back the page *behind* it instead
        // of the modal itself, or the dimming would stack up a layer deeper
        // every time.
        //
        // Nothing behind it is drawn as nothing. A menu built to stand in for
        // the screen behind was what this did before there was a real one to
        // use, and a rebuilt menu is not the screen it claims to be: it shows
        // the main menu behind a dialog opened from somewhere else entirely.
        // The window has a child from the first screen onwards, so what is
        // left here is the moment before that.
        // Only a *modal's* overlay is unwrapped, which is what the marker
        // class is for. The media page is an overlay too - artwork behind,
        // page in front - and taking its child handed back the bare backdrop
        // and threw the page away, so the browser opened over a film's
        // wallpaper with nothing on it.
        let modal_stack = |child: &gtk::Widget| {
            child
                .downcast_ref::<gtk::Overlay>()
                .is_some_and(|overlay| overlay.has_css_class(MODAL_STACK))
        };
        let backdrop: gtk::Widget = match self.window.child() {
            Some(child) if modal_stack(&child) => {
                let overlay = child.downcast::<gtk::Overlay>().expect("checked above");
                let under = overlay.child();
                overlay.set_child(None::<&gtk::Widget>);
                under.unwrap_or_else(|| empty_backdrop().upcast())
            }
            Some(child) => {
                self.window.set_child(None::<&gtk::Widget>);
                child
            }
            None => empty_backdrop().upcast(),
        };
        // Not just visually behind: an insensitive page cannot take focus, so
        // neither tab nor the gamepad can reach what is underneath.
        backdrop.set_sensitive(false);

        let scrim = gtk::Box::builder().css_classes(["tp-scrim"]).build();

        page.add_css_class("tp-modal");

        let overlay = gtk::Overlay::new();
        overlay.add_css_class(MODAL_STACK);
        overlay.set_child(Some(&backdrop));
        overlay.add_overlay(&scrim);
        overlay.add_overlay(page);
        overlay
    }

    /// A panel for the one thing browsing folders cannot reach: an address.
    ///
    /// Its own screen rather than a field in the browser, because a text field
    /// among the folders is a trap for a controller, which can neither type
    /// into one nor easily get out of it. Behind a row, it is only ever
    /// entered on purpose, and there is room to say what may be pasted.
    fn show_paste_uri(self: &Rc<Self>) {
        // Built by hand rather than from the list page every other screen
        // uses: that one leads with a header and a list, and here both would
        // be empty space above the only thing on the panel.
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(28)
            .valign(gtk::Align::Center)
            .margin_top(48)
            .margin_bottom(48)
            .margin_start(56)
            .margin_end(56)
            .build();

        let heading = heading_label("Open a URL");
        heading.set_halign(gtk::Align::Center);
        page.append(&heading);

        let blurb = gtk::Label::builder()
            .label(
                "Enter an address to a video file, such as a link from a media server, a local file path, or a network path.",
            )
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&blurb);

        let field = gtk::Entry::new();
        field.add_css_class("tp-path");
        field.set_placeholder_text(Some("http://…"));
        gtk::prelude::EditableExt::set_alignment(&field, 0.5);
        field.set_hexpand(true);
        page.append(&field);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let open = gtk::Button::with_label("Open");
        open.add_css_class("tp-button");
        open.add_css_class("tp-action");
        // Nothing to open until there is something in the field, and an empty
        // one would only fail slowly against a source that does not exist.
        open.set_sensitive(false);
        {
            let open = open.clone();
            field.connect_changed(move |field| {
                open.set_sensitive(!field.text().trim().is_empty());
            });
        }
        buttons.append(&cancel);
        buttons.append(&open);
        page.append(&buttons);

        {
            let app = self.clone();
            let field = field.clone();
            open.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.open_typed_path(&field.text());
            });
        }
        {
            let app = self.clone();
            field.connect_activate(move |field| {
                if !field.text().trim().is_empty() {
                    app.open_typed_path(&field.text());
                }
            });
        }
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.remember_origin();
        // Its own tab order: the field, then the two buttons. Without stops
        // of its own there is nothing for Tab to move between, and the Open
        // button cannot be reached without a pointer.
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&field);
        self.add_nav_stop(&cancel);
        self.add_nav_stop(&open);
        *self.screen.borrow_mut() = Screen::PasteUri;
        self.window.set_child(Some(&self.modal(&page)));
        // The field wants the caret from the moment it opens: this screen
        // exists to be typed into.
        field.grab_focus();

        // Filled in for you when the clipboard already holds something this
        // panel could open, and selected so typing replaces it. Better than a
        // Paste button: a controller cannot reach one, and a button says
        // nothing about whether pressing it would help.
        {
            let field = field.clone();
            gtk::prelude::WidgetExt::display(&self.window)
                .clipboard()
                .read_text_async(gtk::gio::Cancellable::NONE, move |text| {
                    let Ok(Some(text)) = text else { return };
                    let text = text.trim();
                    if looks_openable(text) {
                        field.set_text(text);
                        field.select_region(0, -1);
                    }
                });
        }
    }

    /// Opens whatever was typed into the paste panel.
    ///
    /// A folder browses to it, so typing a path is another way to navigate.
    /// Anything else is handed to [`Source`], which is what decides whether a
    /// string is a file or a URL, so this cannot disagree with what the
    /// command line accepts.
    fn open_typed_path(self: &Rc<Self>, text: &str) {
        let text = text.trim();
        let as_path = std::path::Path::new(text);
        if as_path.is_dir() {
            self.show_browser(as_path, None);
            return;
        }

        self.show_opening(Source::parse(text));
    }

    /// Waits for a source to answer, with something on screen that says so.
    ///
    /// Reading a remote source is not quick and can fail slowly: an address
    /// nothing answers at takes the discoverer's full ten seconds. Doing that
    /// on the main thread froze the whole window, which reads as a crash
    /// rather than as waiting, so the probe runs on a thread of its own.
    fn show_opening(self: &Rc<Self>, source: Source) {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(28)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .margin_top(48)
            .margin_bottom(48)
            .margin_start(56)
            .margin_end(56)
            .build();

        // A floor rather than a fixed size: with only a spinner and a short
        // address on it, the panel would otherwise shrink to something much
        // narrower than the one it replaces, and the swap would read as the
        // window jumping about.
        page.set_size_request((560.0 * self.scale.get()).round() as i32, -1);

        let spinner = gtk::Spinner::new();
        spinner.set_size_request(
            (48.0 * self.scale.get()).round() as i32,
            (48.0 * self.scale.get()).round() as i32,
        );
        spinner.start();
        page.append(&spinner);
        page.append(&heading_label("Opening"));

        let what = gtk::Label::builder()
            .label(source.label())
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&what);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.set_halign(gtk::Align::Center);
        page.append(&cancel);
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.show_paste_uri());
        }

        *self.screen.borrow_mut() = Screen::Opening;
        self.window.set_child(Some(&self.modal(&page)));
        self.set_nav(None, &[], &[]);
        cancel.grab_focus();

        // A plain channel polled from the main loop, rather than anything
        // asynchronous: the probe returns once, and the result has to be
        // applied on this thread because everything it touches is `Rc`.
        let (sender, receiver) = std::sync::mpsc::channel();
        let probing = source.clone();
        std::thread::spawn(move || {
            let _ = sender.send(crate::probe::probe_media(&probing));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                // The thread is gone without an answer, which leaves nothing
                // to report and no reason to keep looking.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            // Cancelled, or moved on some other way, while it was working.
            if *app.screen.borrow() != Screen::Opening {
                return glib::ControlFlow::Break;
            }

            match result.and_then(|media| app.apply_media(&source, media)) {
                Ok(()) => app.show_menu(),
                Err(e) => {
                    eprintln!("Couldn't read {}: {e}", source.uri());
                    app.forget_file();
                    app.show_source_error(&source, &e, false);
                }
            }
            glib::ControlFlow::Break
        });
    }

    /// The built-in browser: another list screen, so it navigates exactly
    /// like the menus and needs no pointer.
    ///
    /// `select` names the folder just stepped out of, which is then the row
    /// focus lands on. Going up otherwise dumps you at the top of a long
    /// list with no sense of where you were.
    /// The screen for choosing a video: folders, and the videos in them.
    fn show_browser(
        self: &Rc<Self>,
        directory: &std::path::Path,
        select: Option<&std::path::Path>,
    ) {
        // The same screen chooses a video and a separate soundtrack for one,
        // differing only in what it lists and what activating a row does.
        // Which of the two is in hand is held on the application rather than
        // passed down, because stepping into a folder re-enters here and would
        // otherwise forget what was being looked for.
        let mode = match self.errand.get() {
            Errand::Audio(_) => Browse::Audio,
            Errand::Subtitle => Browse::Subtitles,
            Errand::Video => Browse::Videos,
        };
        let directory = crate::browser::rooted(directory);
        let page = self.browser_page(&directory, mode);
        let entries = browser_entries(&directory, mode);

        // The two things done with a selection, together in the middle, in
        // the order every other pair in the application uses: the way out
        // first, then the action. Opening the system browser stays off to one
        // side, being a way out of this screen rather than a use of it.
        let choices = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing((24.0 * self.scale.get()).round() as i32)
            .build();
        choices.append(&page.cancel);
        choices.append(&page.open);

        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&page.browse));
        footer.set_center_widget(Some(&choices));
        page.page.append(&footer);

        fill_browser_list(&page.list, &entries, self.scale.get());

        {
            let app = self.clone();
            let entries = entries.clone();
            let here = directory.clone();
            page.list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let Some(entry) = entries.get(row.index() as usize) else {
                    return;
                };
                match &entry.path {
                    Some(path) if path.is_dir() => app.show_browser(path, None),
                    // A soundtrack for the video already chosen, rather than a
                    // video: it replaces whatever track that output was on and
                    // hands straight back to the menu, where the row now names
                    // the file.
                    Some(path) if app.errand.get() == Errand::Subtitle => {
                        app.set_subtitle_file(path);
                        app.show_menu();
                    }
                    Some(path) if matches!(app.errand.get(), Errand::Audio(_)) => {
                        app.set_audio_file(path);
                        app.show_menu();
                    }
                    // Through the same screen a URL opens through, rather
                    // than reading the file here and moving when it is done.
                    // Probing is not instant - it starts a GStreamer
                    // discoverer, and a file on a network share can take a
                    // second or two - and doing it on this thread left the
                    // browser standing there with the row lit, looking like
                    // the press had been missed. This puts the spinner up
                    // first and reads the file behind it.
                    Some(path) => app.show_opening(Source::File(path.to_path_buf())),
                    // Up. Only offered when there is somewhere above to go:
                    // at the top of the tree the column to the left is how
                    // you reach anywhere else.
                    None => {
                        if let Some(parent) = here.parent() {
                            app.show_browser(parent, Some(&here));
                        }
                    }
                }
            });
        }
        {
            let app = self.clone();
            page.cancel.connect_clicked(move |_| app.go_back());
        }
        // The button does what a double click does, by asking the list to
        // activate the row rather than repeating what activation means. One
        // description of what opening a row is, in the handler above.
        {
            let list = page.list.clone();
            page.open.connect_clicked(move |_| {
                if let Some(row) = list.selected_row() {
                    list.emit_by_name::<()>("row-activated", &[&row]);
                }
            });
        }
        // Off unless a file is selected. Not a folder, which a double click
        // or Enter still steps into - the button is for choosing the thing
        // this screen exists to choose, and a folder is not it. Not the way
        // up, and not the notice a folder with nothing in it shows, which is
        // a row like any other to GTK.
        {
            let open = page.open.clone();
            let openable: Vec<bool> = entries.iter().map(|entry| entry.openable).collect();
            page.list.connect_row_selected(move |_, row| {
                let selected = row
                    .map(|row| row.index() as usize)
                    .and_then(|index| openable.get(index).copied())
                    .unwrap_or(false);
                open.set_sensitive(selected);
            });
        }

        {
            let mut config = self.config.borrow_mut();
            config.last_folder = Some(directory.to_path_buf());
            let _ = config.save();
        }

        // The trail alone now that the arrow has gone: left from the current
        // folder simply walks back up it.
        // Typing a letter jumps to the first name that begins with it, which
        // is how a folder of two hundred films is reached without holding an
        // arrow key. Attached here rather than to every list: the browser is
        // the one screen whose rows are named by something other than us, and
        // so the one where a name cannot be predicted.
        {
            let labels: Vec<String> = entries
                .iter()
                .map(|entry| entry.label.trim().to_lowercase())
                .collect();
            let list = page.list.clone();
            let app = self.clone();
            // What was typed last, so a repeat of it can be told from a new
            // letter. Held by the controller rather than the application: it
            // belongs to this listing and is meaningless once it is gone.
            let last: RefCell<Option<String>> = RefCell::new(None);
            let controller = gtk::EventControllerKey::new();
            controller.connect_key_pressed(move |_, key, _, state| {
                // Nothing with a modifier on it: those are shortcuts, and
                // Ctrl+C on a browser row should stay Ctrl+C. Shift is let
                // through, being how a capital arrives.
                if state.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::META_MASK,
                ) {
                    return glib::Propagation::Proceed;
                }
                let Some(typed) = key.to_unicode().filter(|c| c.is_alphanumeric()) else {
                    return glib::Propagation::Proceed;
                };
                let typed = typed.to_lowercase().to_string();
                // The same letter again walks on to the next name that starts
                // with it, wrapping at the end; a different letter starts from
                // the top. Without that, a folder holding a dozen films
                // beginning with "The" would answer every press with the same
                // row and look as though the key had done nothing.
                let again = last.borrow().as_deref() == Some(typed.as_str());
                *last.borrow_mut() = Some(typed.clone());
                let from = match again {
                    true => list
                        .selected_row()
                        .map_or(0, |row| row.index() as usize + 1),
                    false => 0,
                };
                let matching = |offset: usize| {
                    let index = (from + offset) % labels.len().max(1);
                    labels
                        .get(index)
                        .filter(|label| label.starts_with(&typed))
                        .map(|_| index)
                };
                let Some(index) = (0..labels.len()).find_map(matching) else {
                    // Nothing starts with it. Swallowed all the same, so a
                    // stray letter cannot fall through to whatever else on the
                    // screen might answer it.
                    return glib::Propagation::Stop;
                };
                if let Some(row) = list.row_at_index(index as i32) {
                    app.sounds.borrow().click();
                    list.select_row(Some(&row));
                    settle_on(&row);
                }
                glib::Propagation::Stop
            });
            page.list.add_controller(controller);
        }

        self.wire_navigation(
            &page.list,
            &page.crumbs,
            &[page.cancel.clone(), page.open.clone()],
        );
        self.remember_origin();
        *self.screen.borrow_mut() = Screen::Browser;
        self.window.set_child(Some(&self.modal(&page.page)));

        let opening = select
            .and_then(|wanted| {
                entries
                    .iter()
                    .position(|entry| entry.path.as_deref() == Some(wanted))
            })
            // Otherwise the first real entry, skipping the rows that only
            // lead somewhere else: up, and the empty-folder notice.
            .or_else(|| entries.iter().position(|entry| entry.path.is_some()))
            // Nothing to open: the way up, rather than the line saying so.
            .unwrap_or(0) as i32;
        if let Some(row) = page.list.row_at_index(opening) {
            page.list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// The scaffolding every browsing screen is built on.
    ///
    /// One page for two jobs - choosing a video, and choosing a folder to set
    /// Kodi up in - because they are the same screen with different rows in
    /// it. Built separately they drifted: the same trail, places column and
    /// system-browser button written twice, so a change to how browsing looks
    /// had to be made in both and was once made in only one.
    ///
    /// What differs is left to the caller: what the footer holds, what a row
    /// does when it is chosen, and where the cursor starts.
    fn browser_page(self: &Rc<Self>, directory: &std::path::Path, mode: Browse) -> BrowserPage {
        let (crumbs, crumb_buttons) = self.breadcrumbs(directory, mode.folders_only());

        let (page, list, _back, slot) = list_page_with(&crumbs, false);
        // The arrow's slot holds a fixed width for every screen to line up
        // against. With no arrow in it, that is just a gap before the trail.
        slot.set_visible(false);
        self.add_places_column(&page, directory, mode.folders_only(), &crumb_buttons);
        self.follow_focus(&list);

        // Along the foot with the way out, rather than tucked into the header:
        // both are things done with the browser rather than places inside it.
        // Still not focusable, and last: it exists for a pointer, and the
        // dialog it opens cannot be driven by a controller anyway.
        let browse_face = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        // Larger than the lettering beside it: at this size the icon is what
        // the eye finds first, and the words only confirm it.
        // The same folder the rows are drawn with, so the button that opens
        // another browser is marked with what it opens - smaller than in a
        // row, where it stands alone against a name; here it sits beside a
        // line of text on a button and should not outweigh it.
        let browse_icon = RowIcon::Folder.image_at(BUTTON_FOLDER_PX, self.scale.get());
        browse_face.append(&browse_icon);
        browse_face.append(&gtk::Label::new(Some("Open System Browser")));
        let browse = gtk::Button::builder().child(&browse_face).build();
        browse.add_css_class("tp-button");
        browse.add_css_class("tp-secondary");
        browse.set_can_focus(false);
        browse.set_valign(gtk::Align::Start);
        {
            let app = self.clone();
            // Wherever the listing behind it has reached. Handing over at the
            // top of the tree, or wherever the system dialog last was, means
            // walking back down a path already walked.
            let here = directory.to_path_buf();
            browse.connect_clicked(move |_| match mode {
                Browse::Videos => app.open_file_chooser(&here),
                Browse::Folders => app.choose_kodi_folder_natively(&here),
                // The same dialog, filtered to whatever is being looked for:
                // it reads which errand it is on for itself.
                Browse::Audio | Browse::Subtitles => app.open_file_chooser(&here),
            });
        }

        // What a click used to do on its own. A single click selects now, so
        // there has to be something a pointer can press to act on what it
        // selected - a double click is the shortcut, not the only way.
        let open = gtk::Button::with_label("Open");
        open.add_css_class("tp-button");
        open.add_css_class("tp-action");
        // Nothing is selected until the list is filled, and a row that opens
        // nothing leaves it off again. See `follow_open`.
        open.set_sensitive(false);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");

        // A click selects; it takes a second one to open. Set here rather than
        // on each screen, so the file browser and the folder chooser cannot
        // come to disagree about what a click does.
        //
        // The keyboard is untouched by it. GtkListBox emits `row-activated` on
        // a double click and on Enter either way, and Enter here goes through
        // `activate_focused`, which emits it by hand - so every handler is
        // reached exactly as it was.
        list.set_activate_on_single_click(false);

        BrowserPage {
            page,
            list,
            crumbs: crumb_buttons,
            browse,
            open,
            cancel,
        }
    }

    /// The current directory as a row of buttons, one per level, so any
    /// ancestor is a single press away rather than several trips through Up.
    ///
    /// Capped at the last few levels: a deep path would otherwise run off the
    /// side, and the leading button stands in for everything trimmed away.
    /// `folders` decides which browser a crumb reopens. Without it, stepping
    /// up the trail from the folder browser lands in the video browser, which
    /// is the same shape of screen doing an entirely different job.
    fn breadcrumbs(
        self: &Rc<Self>,
        directory: &std::path::Path,
        folders: bool,
    ) -> (gtk::Box, Vec<gtk::Button>) {
        use std::path::{Component, PathBuf};

        // Each level paired with the path that reaches it.
        let mut levels: Vec<(String, PathBuf)> = Vec::new();
        let mut walked = PathBuf::new();
        for component in directory.components() {
            match component {
                Component::Prefix(prefix) => {
                    walked.push(prefix.as_os_str());
                    // Rooted right here, because `H:` on its own does not mean
                    // the top of that drive: it means wherever that drive was
                    // last left, which is a relative path. Browsing to one
                    // works, since reading it still finds the right folder,
                    // but every entry under it is relative too and no URI can
                    // be made from those.
                    walked.push(std::path::MAIN_SEPARATOR_STR);
                    levels.push((
                        prefix.as_os_str().to_string_lossy().to_string(),
                        walked.clone(),
                    ));
                }
                Component::RootDir => {
                    if levels.is_empty() {
                        walked.push(std::path::MAIN_SEPARATOR_STR);
                        levels.push(("/".to_string(), walked.clone()));
                    }
                }
                Component::Normal(name) => {
                    walked.push(name);
                    levels.push((name.to_string_lossy().to_string(), walked.clone()));
                }
                _ => {}
            }
        }

        const SHOWN: usize = 4;
        let mut trimmed = Vec::new();
        if levels.len() > SHOWN {
            let hidden = levels.len() - SHOWN;
            // Leads to the level just above the first one still shown.
            trimmed.push(("…".to_string(), levels[hidden - 1].1.clone()));
            trimmed.extend_from_slice(&levels[hidden..]);
        } else {
            trimmed = levels;
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .hexpand(true)
            .build();
        let mut buttons = Vec::new();

        for (position, (label, target)) in trimmed.iter().enumerate() {
            if position > 0 {
                let separator = gtk::Label::new(Some("›"));
                separator.add_css_class("tp-crumb-separator");
                row.append(&separator);
            }

            let button = gtk::Button::with_label(label);
            button.add_css_class("tp-crumb");
            {
                let app = self.clone();
                let target = target.clone();
                let here = directory.to_path_buf();
                button.connect_clicked(move |_| {
                    app.sounds.borrow().click();
                    if folders {
                        app.show_kodi_folder(&target);
                        return;
                    }
                    // Selecting the folder you are already in should settle
                    // focus back on the listing rather than rebuild nothing.
                    let select = (target != here).then(|| here.clone());
                    app.show_browser(&target, select.as_deref());
                });
            }
            row.append(&button);
            buttons.push(button);
        }

        (row, buttons)
    }

    /// Where a video comes from: a folder on this machine, or an address.
    ///
    /// A step of its own rather than opening the browser straight away,
    /// because the two are not the same kind of thing. Walking folders finds
    /// what is here; an address reaches what is not, and no amount of
    /// browsing would ever lead to it.
    fn choose_video(self: &Rc<Self>) {
        let scale = self.scale.get();
        let (panel, browse, address, cancel) = self.choose_source_panel(scale, true);
        let cancel = cancel.expect("asked for with a cancel button");

        // A floor rather than a fixed size, the way the Opening panel has one:
        // three buttons and a line of text would otherwise make a panel much
        // narrower than the page behind it, and the swap would read as the
        // window jumping about.
        panel.set_size_request((560.0 * scale).round() as i32, -1);

        {
            let app = self.clone();
            browse.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.browse_for_file();
            });
        }
        {
            let app = self.clone();
            address.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_paste_uri();
            });
        }
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_menu();
            });
        }

        // The same words the empty screen shows, floated over the film rather
        // than replacing it: what is loaded is still loaded, and backing out
        // returns to it.
        self.remember_origin();
        self.set_nav(None, &[], &[cancel.clone(), browse.clone(), address]);
        *self.screen.borrow_mut() = Screen::VideoSource;
        self.window.set_child(Some(&self.modal(&panel)));
        browse.grab_focus();
    }

    /// Opens the file browser where browsing last stopped.
    ///
    /// Always the built-in browser. Guessing from the last input was
    /// unpredictable: the same button opened different things depending on
    /// what you had touched. The system dialog is still reachable, from a
    /// pointer-only button in the footer.
    fn browse_for_file(self: &Rc<Self>) {
        // Whatever errand the browser was last on, this one is a video.
        self.errand.set(Errand::Video);
        self.open_browser();
    }

    /// The same browser, looking for a soundtrack to put on one output.
    ///
    /// Starts where the video is rather than where browsing left off: a
    /// separate audio track is usually downloaded to sit beside the film, and
    /// when it is not, the film's folder is still a better place to start from
    /// than wherever a video was last chosen.
    fn browse_for_audio(self: &Rc<Self>, role: Role) {
        self.errand.set(Errand::Audio(role));
        let beside = self
            .file
            .borrow()
            .as_ref()
            .and_then(|file| file.local().and_then(|path| path.parent()))
            .map(|folder| folder.to_path_buf());
        match beside {
            Some(folder) => self.show_browser(&folder, None),
            None => self.open_browser(),
        }
    }

    fn open_browser(self: &Rc<Self>) {
        let (remembered, last_video) = {
            let config = self.config.borrow();
            (config.last_folder.clone(), config.last_video.clone())
        };
        let start = crate::browser::start_location(remembered.as_deref(), last_video.as_deref());
        self.show_browser(&start, None);
    }

    /// What an output is playing, for its row on the menu: the name of a
    /// separate audio file when one is chosen, and otherwise the track.
    fn describe_audio(&self, role: Role) -> String {
        if let Some(file) = self.file_for(role).borrow().as_ref() {
            return file.label();
        }
        let chosen = *self.track_for(role).borrow();
        let tracks = self.tracks.borrow();
        match chosen {
            Some(index) => tracks
                .iter()
                .find(|track| track.index == index)
                .map(describe_audio_track)
                .unwrap_or_else(|| "None".to_string()),
            None => "None".to_string(),
        }
    }

    /// The alignment row for one output, when there is anything to align.
    ///
    /// Only offered against a separate audio file: a track inside the video
    /// shares the video's timeline and cannot be out of step with it. The rest
    /// are the things measuring needs and cannot do without - a track inside
    /// the video to line the file up against, a running time to place the
    /// three windows across, and a path on disk to file the answer under.
    fn alignment_row(&self, role: Role) -> Option<(String, String, bool, MenuAction)> {
        let file = self.file_for(role).borrow();
        let path = file.as_ref()?.local()?;
        if self.tracks.borrow().is_empty() || self.duration_s.get() <= 0.0 {
            return None;
        }
        let stored = self
            .storage_key()
            .and_then(|key| crate::config::load_alignment(&key, path));
        Some((
            // One name whether or not there is a stored answer. It used to say
            // "Auto-align" or "Re-align" to name what pressing it would do,
            // which the value beside it now says better: "Unsynced" against a
            // measured offset is the same distinction, in the column that
            // exists to carry state.
            "Sync".to_string(),
            match stored {
                Some(millis) => describe_lateness(millis),
                None => "Unsynced".to_string(),
            },
            true,
            MenuAction::Align(role),
        ))
    }

    /// Reads back what alignment worked out for whatever each output is
    /// playing, so the baseline is in force before the pipeline is built.
    ///
    /// Zero for a track inside the video: alignment is about a pairing of two
    /// files and there is nothing to pair a track with.
    fn load_baselines(&self) {
        let key = self.storage_key();
        for role in [Role::Primary, Role::Secondary] {
            let stored = key.as_deref().and_then(|key| {
                let file = self.file_for(role).borrow();
                let path = file.as_ref()?.local()?;
                crate::config::load_alignment(key, path)
            });
            // Negated on the way in: alignment says how late the audio runs,
            // and a sink is held back by a negative offset.
            let cell = match role {
                Role::Primary => &self.primary_baseline,
                Role::Secondary => &self.secondary_baseline,
            };
            cell.set(-stored.unwrap_or(0.0));
        }
    }

    /// The alignment baseline for one output.
    fn baseline_ms(&self, role: &str) -> f64 {
        match role {
            "primary" => self.primary_baseline.get(),
            _ => self.secondary_baseline.get(),
        }
    }

    /// What the sink should actually be held back by: what the viewer asked
    /// for, plus what alignment worked out. The two are separate quantities -
    /// one describes the headphones, the other describes the pair of files -
    /// and only the first is ever shown on the slider.
    fn offset_for(&self, role: &str) -> f64 {
        self.config.borrow().applied_offset_ms(role) + self.baseline_ms(role)
    }

    /// Sends an output's whole delay to the pipeline: what the viewer asked
    /// for, plus what alignment worked out for the file being played.
    ///
    /// The one road to a sink, deliberately. The sum used to be rebuilt by
    /// hand at each of the four places that change either half, and the one
    /// behind the sync control during playback rebuilt it wrong - it sent the
    /// slider's own value, so touching sync threw the alignment away and left
    /// the audio seconds out. A half-applied offset is worse than none, and
    /// the way to stop that recurring is to leave nowhere else to apply one.
    fn push_offset(&self, playback: &Playback, role: &str) {
        playback.set_offset_ms(role, self.offset_for(role));
    }

    /// The same, for whatever is playing now, if anything is. Cloned out of
    /// the cell rather than borrowed across the call, since what it reaches
    /// takes the same borrows.
    fn push_offset_live(&self, role: &str) {
        if let Some(playback) = self.playback.borrow().clone() {
            self.push_offset(&playback, role);
        }
    }

    /// The track chosen for one output, and the file chosen for it, where the
    /// two outputs are otherwise handled by the same code.
    fn track_for(&self, role: Role) -> &RefCell<Option<u32>> {
        match role {
            Role::Primary => &self.primary_track,
            Role::Secondary => &self.secondary_track,
        }
    }

    fn file_for(&self, role: Role) -> &RefCell<Option<Source>> {
        match role {
            Role::Primary => &self.primary_file,
            Role::Secondary => &self.secondary_file,
        }
    }

    /// Puts a chosen audio file on the output the browser was opened for.
    /// Opens the browser to find a subtitle file, starting where the video is.
    fn browse_for_subtitle(self: &Rc<Self>) {
        self.errand.set(Errand::Subtitle);
        let beside = self
            .file
            .borrow()
            .as_ref()
            .and_then(|file| file.local().and_then(|path| path.parent()))
            .map(|folder| folder.to_path_buf());
        match beside {
            Some(folder) => self.show_browser(&folder, None),
            None => self.open_browser(),
        }
    }

    /// Takes a subtitle file chosen by hand.
    ///
    /// Added to the options as well as chosen, so the menu can show it and the
    /// chooser can show it selected. Everything else in that list was found by
    /// looking beside the video, and this one never would be.
    fn set_subtitle_file(self: &Rc<Self>, path: &std::path::Path) {
        let option = crate::subtitles::chosen_file(path);
        let choice = option.choice();
        {
            let mut options = self.subtitle_options.borrow_mut();
            if !options.iter().any(|other| other.choice() == choice) {
                options.push(option);
            }
        }
        *self.subtitle.borrow_mut() = Some(choice);
        // Choosing a subtitle is asking to see it, whatever the toggle was
        // doing for the last one.
        self.subtitles_hidden.set(false);
        self.errand.set(Errand::Video);
        self.remember_tracks();
    }

    fn set_audio_file(self: &Rc<Self>, path: &std::path::Path) {
        let Errand::Audio(role) = self.errand.get() else {
            return;
        };
        let source = Source::File(path.to_path_buf());
        match role {
            Role::Primary => *self.primary_file.borrow_mut() = Some(source),
            Role::Secondary => *self.secondary_file.borrow_mut() = Some(source),
        }
        self.errand.set(Errand::Video);
        // Written down here, not left to playback to save: choosing a
        // soundtrack and then quitting without pressing play is choosing it,
        // and every other chooser on this screen remembers itself the same way.
        self.remember_tracks();
        // A pairing measured before comes back already lined up.
        self.load_baselines();
    }

    // --- Alignment -----------------------------------------------------

    /// The frame the three alignment steps share.
    ///
    /// One panel carrying all three in turn, rather than three screens: it is
    /// one errand, and the film it belongs to should stay visible behind it
    /// throughout. An overlay rather than a real modal window, for the reason
    /// the browser is one - a `transient_for` window takes the pointer but not
    /// the keyboard or the gamepad, both of which are driven from the main
    /// window and would carry on working the menu hidden behind it.
    fn align_page(&self, hint: &str) -> gtk::Box {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(20)
            // Centered and no taller than its contents, so the panel is the
            // size of the question rather than the size of the window.
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(32)
            .margin_bottom(32)
            .margin_start(44)
            .margin_end(44)
            .build();
        // The floor. Without it the panel shrinks around whatever the shortest
        // step has on it, and the three read as three differently sized
        // windows rather than one panel changing what it says.
        page.set_size_request((ALIGN_PANEL_MIN * self.scale.get()).round() as i32, -1);

        let heading = heading_label("Auto-Align");
        heading.set_halign(gtk::Align::Center);
        page.append(&heading);

        let hint = gtk::Label::builder()
            .label(hint)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            // The ceiling, and with the two set alike the floor as well. A
            // GtkBox has no maximum width, so the cap has to sit on the text
            // that would otherwise push it wide - and asking for the same
            // measure as a minimum is what makes all three steps come out the
            // same width instead of each shrinking to fit its own sentence.
            // In characters rather than pixels because that is what wraps, and
            // it holds at every interface scale without being multiplied.
            .width_chars(ALIGN_PANEL_CHARS)
            .max_width_chars(ALIGN_PANEL_CHARS)
            .css_classes(["tp-hint"])
            .build();
        page.append(&hint);
        page
    }

    /// Step one: which track inside the video to measure the audio file
    /// against.
    ///
    /// Asked rather than inferred, so the viewer can point it at the original
    /// soundtrack when the automatic pick would have taken a dub. It arrives
    /// with a sensible one already selected, so the common answer is a single
    /// press of Next.
    fn show_align(self: &Rc<Self>, role: Role) {
        // Nothing to align without both halves of the pairing.
        let tracks = self.tracks.borrow().clone();
        if self.file_for(role).borrow().is_none() || tracks.is_empty() {
            return;
        }

        let page = self.align_page(
            "Choose a reference audio track to align the external audio file with. \
             Usually the original language, or a language that matches the audio \
             description.",
        );

        let (scroller, list) = scrolling_list();
        name_it(&list, "Reference track");
        // Only as tall as the tracks need, up to a few rows. A list left to
        // expand makes the panel the height of the window whether it holds one
        // track or twelve, which is the opposite of what a short question wants.
        scroller.set_vexpand(false);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height((240.0 * self.scale.get()).round() as i32);
        page.append(&scroller);
        for track in &tracks {
            let text = describe_audio_track(track);
            let row = chooser_row(&text);
            row.set_xalign(0.5);
            // Held to the same measure as the body text. A track carrying a
            // long title would otherwise widen the whole panel, and it already
            // ellipsizes rather than wrapping.
            row.set_max_width_chars(ALIGN_PANEL_CHARS);
            append_named(&list, &row, &text);
        }

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let next = gtk::Button::with_label("Next");
        next.add_css_class("tp-button");
        next.add_css_class("tp-action");
        buttons.append(&cancel);
        buttons.append(&next);
        page.append(&buttons);

        // What the list is pointing at when Next is pressed, and what
        // activating a row means, are the same thing: the row is the choice.
        let start = {
            let app = self.clone();
            let list = list.clone();
            let tracks = tracks.clone();
            move || {
                let index = list.selected_row().map(|row| row.index()).unwrap_or(0);
                let Some(track) = tracks.get(index.max(0) as usize) else {
                    return;
                };
                app.sounds.borrow().click();
                app.show_align_progress(role, track.index);
            }
        };
        {
            let start = start.clone();
            list.connect_row_activated(move |_, _| start());
        }
        next.connect_clicked(move |_| start());
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.wire_navigation(&list, &[], &[cancel.clone(), next.clone()]);
        self.remember_origin();
        *self.screen.borrow_mut() = Screen::AlignChoose;
        self.window.set_child(Some(&self.modal(&page)));

        // The first track that is not a description, because a description is
        // the thing being lined up rather than the thing to line it up with -
        // it correlates against itself perfectly and says nothing. Falls back
        // to the first track when description is all the file has.
        let opening = tracks
            .iter()
            .position(|track| !crate::probe::is_audio_description(&track.title))
            .unwrap_or(0);
        if let Some(row) = list.row_at_index(opening as i32) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Step two: the measuring, which happens on a thread.
    ///
    /// Three sixty-second windows out of each of two files is around twelve
    /// seconds on a desktop and several times that on a Pi, so it cannot run
    /// on the main loop: the window would stop redrawing and the interface
    /// would read as having crashed. The thread reports through a channel this
    /// polls, which is how the rest of the application already waits on work -
    /// everything the answer touches is `Rc` and has to be applied here.
    fn show_align_progress(self: &Rc<Self>, role: Role, reference: u32) {
        let (video, audio) = {
            let file = self.file.borrow().clone();
            let audio = self.file_for(role).borrow().clone();
            match (file, audio) {
                (Some(video), Some(audio)) => (video, audio),
                _ => return,
            }
        };

        let page =
            self.align_page("Analyzing audio to align the tracks. This may take a few moments.");

        let bar = gtk::ProgressBar::new();
        bar.add_css_class("tp-align-bar");
        page.append(&bar);

        let status = gtk::Label::builder()
            .label("0%")
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&status);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.set_halign(gtk::Align::Center);
        page.append(&cancel);
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.remember_origin();
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&cancel);
        *self.screen.borrow_mut() = Screen::AlignProgress;
        self.window.set_child(Some(&self.modal(&page)));
        cancel.grab_focus();

        let (sender, receiver) = std::sync::mpsc::channel();
        let duration = self.duration_s.get();
        let (video_uri, audio_uri) = (video.uri(), audio.uri());
        std::thread::spawn(move || {
            let progress = sender.clone();
            let verdict = crate::align::align(
                &video_uri,
                &audio_uri,
                duration,
                reference,
                // A failed send means nobody is listening any more, which is
                // what cancelling looks like from here. There is no way to
                // stop a decode part-way, so the thread runs to the end and
                // its answer is dropped.
                move |done| {
                    let _ = progress.send(Step::Window(done));
                },
            );
            let _ = sender.send(Step::Done(verdict));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            // Cancelled, or moved on some other way, while it was working.
            if *app.screen.borrow() != Screen::AlignProgress {
                return glib::ControlFlow::Break;
            }
            loop {
                match receiver.try_recv() {
                    Ok(Step::Window(done)) => {
                        // Three steps rather than a smooth climb: a window is
                        // one decode and cannot report its own progress, so
                        // anything finer would be invented.
                        let fraction = done as f64 / crate::align::WINDOWS as f64;
                        bar.set_fraction(fraction);
                        status.set_label(&format!("{:.0}%", fraction * 100.0));
                    }
                    Ok(Step::Done(verdict)) => {
                        app.show_align_result(role, verdict);
                        return glib::ControlFlow::Break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    // The thread is gone without an answer, which leaves
                    // nothing to report and no reason to keep looking.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        return glib::ControlFlow::Break;
                    }
                }
            }
        });
    }

    /// Step three: what it found, and applying it when there is anything to
    /// apply.
    ///
    /// A hidden baseline must never hide a wrong answer, so every outcome is
    /// said out loud. The two that change nothing say so plainly and point at
    /// the sync slider, which is what someone is left with when measuring
    /// cannot help.
    fn show_align_result(self: &Rc<Self>, role: Role, verdict: crate::align::Verdict) {
        use crate::align::Verdict;

        // Never named by output, because the answer is not one: it belongs to
        // this video and this audio file, and applies wherever that file is
        // played.
        let (hint, retry) = match verdict {
            Verdict::Offset { millis, .. } => {
                self.apply_alignment(role, millis);
                let rounded = millis.round();
                let shift = if rounded > 0.0 {
                    format!(
                        "The audio file runs {rounded:.0}ms late, and has been adjusted to \
                         sync with the video."
                    )
                } else if rounded < 0.0 {
                    format!(
                        "The audio file runs {:.0}ms early, and has been adjusted to sync \
                         with the video.",
                        -rounded
                    )
                } else {
                    "The audio file is already in sync with the video, no adjustment needed."
                        .to_string()
                };
                (shift, false)
            }
            // A rate difference is a slope rather than a shift, so no single
            // offset fixes it and averaging one would be a guess that drifts.
            Verdict::RateMismatch { .. } => (
                "The audio file runs at a different speed than the video and cannot be \
                 automatically adjusted."
                    .to_string(),
                true,
            ),
            Verdict::Unsure => (
                "The audio file could not be matched with the video.".to_string(),
                true,
            ),
        };

        let page = self.align_page(&hint);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        // Offered only where it could help. Trying another reference track is
        // the answer when the one measured against was a dub and the separate
        // recording was made from the original.
        let again = gtk::Button::with_label("Try another reference track");
        again.add_css_class("tp-button");
        again.add_css_class("tp-action");

        // What the second button means depends on what happened. Where the
        // measurement worked there is nothing to do but accept it; where it
        // did not, the useful thing is to measure again against a different
        // track, and this button becomes the way out beside it.
        let done = gtk::Button::with_label(match retry {
            true => "Cancel",
            false => "Finish",
        });
        done.add_css_class("tp-button");
        if !retry {
            done.add_css_class("tp-action");
        }
        // Cancel first, then the action, which is the order every other pair
        // in the application sits in.
        buttons.append(&done);
        if retry {
            buttons.append(&again);
        }
        page.append(&buttons);

        {
            let app = self.clone();
            again.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_align(role);
            });
        }
        {
            let app = self.clone();
            done.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.go_back();
            });
        }

        self.remember_origin();
        // In the order they now sit, so Tab walks the row left to right.
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&done);
        if retry {
            self.add_nav_stop(&again);
        }
        *self.screen.borrow_mut() = Screen::AlignResult;
        self.window.set_child(Some(&self.modal(&page)));
        // Whichever button is the action here: measuring again where that is
        // still worth doing, and accepting the answer where it is not.
        match retry {
            true => again.grab_focus(),
            false => done.grab_focus(),
        };
    }

    /// Writes an alignment down and puts it into force.
    ///
    /// Stored against the two paths together, so the same pairing never pays
    /// for the measuring twice, and read straight back rather than set here -
    /// `load_baselines` owns the sign convention, and two places deciding it
    /// would eventually disagree.
    fn apply_alignment(&self, role: Role, millis: f64) {
        let stored = {
            let file = self.file_for(role).borrow();
            file.as_ref()
                .and_then(Source::local)
                .map(|path| path.to_path_buf())
        };
        if let Some((key, path)) = self.storage_key().zip(stored) {
            crate::config::save_alignment(&key, &path, Some(millis));
        }
        self.load_baselines();
    }

    // --- Settings ------------------------------------------------------

    /// Everything that applies to the application rather than to the video
    /// currently loaded. Reached from the gear in the footer.
    /// What a settings row is called.
    fn item_label(&self, item: Item) -> String {
        match item {
            Item::InterfaceScale => "Interface Size".to_string(),
            Item::Sounds => "Navigation Sounds".to_string(),
            Item::StartFullscreen => "Start Fullscreen".to_string(),
            Item::ReadMetadata => "Read Metadata Beside Files".to_string(),
            Item::ShowBackdrop => "Show Backdrop Artwork".to_string(),
            Item::ResumeThreshold => "Resume Threshold".to_string(),
            Item::WatchedThreshold => "Watched Threshold".to_string(),
            Item::Updates => "Check for updates".to_string(),
            Item::UpdateStatus => self.version_label(),
            Item::ClearData => "Clear Saved Playback Data".to_string(),
            Item::Device(_) => "Output Device".to_string(),
            Item::Language(_) => "Preferred Language".to_string(),
            Item::Description(_) => "Prefer Audio Description".to_string(),
            Item::Volume(_) => "Volume".to_string(),
            Item::Sync(_) => "Audio Sync".to_string(),
            Item::SubtitlePreference => "Subtitle Preference".to_string(),
            Item::SubtitleSize => "Subtitle Size".to_string(),
            Item::SubtitleFont => "Subtitle Font".to_string(),
            Item::Kodi => "Kodi".to_string(),
            Item::About => "About TinePlayer".to_string(),
            Item::Notices => "Third Party Notices".to_string(),
        }
    }

    /// What it reads against the label. Empty for the rows that carry a
    /// switch or a bar, which show their state in the control itself, and for
    /// the ones that only open something.
    fn item_value(&self, item: Item) -> String {
        let config = self.config.borrow();
        match item {
            Item::Device(role) => {
                let sink = match role {
                    Role::Primary => config.primary_sink.clone(),
                    Role::Secondary => config.secondary_sink.clone(),
                };
                sink.unwrap_or_else(|| match role {
                    Role::Primary => "Not set".to_string(),
                    Role::Secondary => "None".to_string(),
                })
            }
            Item::Language(role) => {
                let (code, unset) = match role {
                    Role::Primary => (&config.primary_language, "First track"),
                    Role::Secondary => (&config.secondary_language, "Second track"),
                };
                match code {
                    Some(code) => crate::languages::name_for(code),
                    None => unset.to_string(),
                }
            }
            Item::SubtitlePreference => {
                crate::subtitles::describe(config.subtitle_language.as_deref())
            }
            Item::SubtitleFont => config
                .subtitle_font
                .clone()
                .unwrap_or_else(|| crate::pipeline::DEFAULT_SUBTITLE_FONT.to_string()),
            // Deliberately blank. Saying what Kodi is set to means finding
            // every Kodi on the machine and reading its configuration file,
            // and this row is passed by everyone who came here for something
            // else. The answer is on the screen it opens.
            Item::Kodi => String::new(),
            Item::UpdateStatus => {
                drop(config);
                self.version_status()
            }
            _ => String::new(),
        }
    }

    /// Whether the switch on this row is on, for the rows that have one.
    fn item_switch(&self, item: Item) -> Option<bool> {
        let config = self.config.borrow();
        Some(match item {
            // On means the size is worked out from the screen, which is the
            // one switch here that turns the bar beside it off rather than on.
            Item::InterfaceScale => config.ui_scale.is_none(),
            Item::Sounds => config.sounds,
            Item::StartFullscreen => config.fullscreen,
            Item::ReadMetadata => config.read_metadata,
            Item::ShowBackdrop => config.show_backdrop,
            Item::Description(Role::Primary) => config.primary_audio_description,
            Item::Description(Role::Secondary) => config.secondary_audio_description,
            Item::Volume(role) => !config.muted(role.key()),
            Item::Sync(role) => config.offset_on(role.key()),
            Item::Updates => config.check_for_updates,
            _ => return None,
        })
    }

    /// Whether the row can be worked at all.
    ///
    /// One case, and it is the reason this exists rather than everything being
    /// live: with nothing read from beside the file there is no artwork to
    /// draw, so the backdrop switch would be a control over nothing.
    fn item_enabled(&self, item: Item) -> bool {
        match item {
            Item::ShowBackdrop => self.config.borrow().read_metadata,
            _ => true,
        }
    }

    /// Settings, as a column of categories and the rows of whichever one is
    /// chosen.
    ///
    /// One flat list of twenty-three rows before this, which is how it came to
    /// hold two rows called Volume and two called Audio Sync with nothing but
    /// their position to tell them apart.
    fn show_settings(self: &Rc<Self>) {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;
        let (page, list, back, _header) = list_page("Settings", true);

        // A fifth of what the window has, so a bar is a consistent share of
        // the screen whether that is a laptop or a television. The monitor
        // stands in before the window has been given a size.
        let slider_width = match self.window.width() {
            0 => appearance::monitor_for_window(&self.window)
                .map(|monitor| monitor.geometry().width())
                .unwrap_or(1920),
            width => width,
        } / 5;

        // The right-hand pane, rebuilt in place when the category changes
        // rather than by rebuilding the screen: the cursor is in the column on
        // the left at that moment, and rebuilding around it would take it away.
        let fill: Rc<Fill> = {
            let list = list.clone();
            Rc::new(move |app: &Rc<Self>| {
                while let Some(row) = list.row_at_index(0) {
                    list.remove(&row);
                }
                app.settings_switches.borrow_mut().clear();
                app.settings_sliders.borrow_mut().clear();

                let entries = app.settings_category.get().items();
                *app.pane_items.borrow_mut() = entries.iter().map(|(_, item)| *item).collect();

                for (index, (_, item)) in entries.iter().enumerate() {
                    let item = *item;
                    let label = app.item_label(item);
                    let enabled = app.item_enabled(item);

                    // Three kinds of row, and which one it is belongs to the
                    // item rather than to where it sits.
                    let widget = match (item.slider(), app.item_switch(item)) {
                        (Some(kind), on) => {
                            let (now, reading) = app.slider_state(kind);
                            let (widget, bar, value, switch) =
                                slider_row(&label, slider_width, kind.range(), now, &reading, on);
                            if kind == Slider::Scale {
                                let by_hand = app.config.borrow().ui_scale.is_some();
                                bar.set_sensitive(by_hand);
                                value.set_sensitive(by_hand);
                            }
                            app.wire_slider(kind, &bar, &value);
                            if let Some(switch) = switch {
                                app.settings_switches.borrow_mut().push((item, switch));
                            }
                            app.settings_sliders
                                .borrow_mut()
                                .push((item, kind, bar, value));
                            widget
                        }
                        (None, Some(on)) => {
                            let (widget, switch) = switch_row(&label, on);
                            switch.set_sensitive(enabled);
                            app.settings_switches.borrow_mut().push((item, switch));
                            widget
                        }
                        (None, None) => menu_row(&label, &app.item_value(item), enabled),
                    };

                    let name = row_name(&label, &app.item_value(item));
                    append_named(&list, &widget, &name);
                    let Some(row) = list.row_at_index(index as i32) else {
                        continue;
                    };
                    row.set_sensitive(enabled);
                    if item == Item::UpdateStatus {
                        app.watch_update_row(&row);
                    }
                }

                // Each switch reports its own presses, now that it takes them
                // rather than letting them fall through to the row. Guarded
                // against the moves made from here when the same setting is
                // worked another way.
                for (item, switch) in app.settings_switches.borrow().iter() {
                    let app = app.clone();
                    let item = *item;
                    switch.connect_state_set(move |_, _| {
                        if !app.settling_switch.get() {
                            app.sounds.borrow().click();
                            app.apply_switch_item(item);
                        }
                        glib::Propagation::Proceed
                    });
                }

                // A heading above the row that opens a group, by the same
                // mechanism the media page uses: headers are not rows, so they
                // cannot be landed on.
                let headings: Vec<Option<&'static str>> =
                    entries.iter().map(|(heading, _)| *heading).collect();
                list.set_header_func(move |row, _| {
                    let index = row.index();
                    match headings.get(index as usize).copied().flatten() {
                        Some(heading) => {
                            row.set_header(Some(&group_heading(heading, scale, index == 0)))
                        }
                        None => row.set_header(None::<&gtk::Widget>),
                    }
                });
                app.refresh_version_row();
            })
        };
        fill(self);

        // The categories, down the left.
        let (categories_scroller, categories) = scrolling_list();
        categories_scroller.set_size_request(px(CATEGORY_WIDTH), -1);
        for category in Category::ALL {
            append_named(
                &categories,
                &menu_row(category.title(), "", true),
                category.title(),
            );
        }
        if let Some(row) = Category::ALL
            .iter()
            .position(|category| *category == self.settings_category.get())
            .and_then(|index| categories.row_at_index(index as i32))
        {
            categories.select_row(Some(&row));
        }
        // Immediately, on the selection moving, rather than on the row being
        // activated: this is a column of what is being looked at, not a list of
        // things to do, and having to press a category to see it is a step that
        // says nothing.
        {
            let app = self.clone();
            let fill = fill.clone();
            categories.connect_row_selected(move |_, row| {
                let Some(category) = row
                    .map(|row| row.index() as usize)
                    .and_then(|index| Category::ALL.get(index).copied())
                else {
                    return;
                };
                if category == app.settings_category.get() {
                    return;
                }
                app.settings_category.set(category);
                // The remembered row belongs to the category it was in.
                *app.settings_row.borrow_mut() = 0;
                fill(&app);
            });
        }

        // Both panes on grounds of their own, the way the media page's rows
        // are: two lists side by side on a bare page have nothing to say where
        // either one ends.
        let Some(listing) = page.last_child() else {
            return;
        };
        page.remove(&listing);
        let columns = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(16.0))
            .vexpand(true)
            .build();
        for (pane, expand) in [
            (categories_scroller.clone().upcast::<gtk::Widget>(), false),
            (listing.clone(), true),
        ] {
            let panel = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .hexpand(expand)
                .css_classes(["tp-menu-panel"])
                .build();
            panel.append(&pane);
            columns.append(&panel);
        }
        page.append(&columns);

        // Watched in the capture phase, so a press is known about before
        // anything else handles it. Cleared on the way out rather than on
        // release, because the row is activated in between - and a press that
        // never activates a row must not leave the next key press looking like
        // a click.
        {
            let app = self.clone();
            let click = gtk::GestureClick::new();
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed(move |_, _, _, _| app.clicked_row.set(true));
            let app = self.clone();
            click.connect_released(move |_, _, _, _| {
                let app = app.clone();
                glib::idle_add_local_once(move || app.clicked_row.set(false));
            });
            list.add_controller(click);
        }

        *self.settings_list.borrow_mut() = Some(list.clone());

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
                let Some(item) = app.item_at(row.index()) else {
                    return;
                };
                // A switch is worked by pressing the switch, not by clicking
                // the row it sits on: the row is a wide target, and hitting it
                // on the way past should not change a setting. Enter on the
                // selected row still does, which arrives here with nothing
                // having been clicked.
                if app.clicked_row.replace(false) && item.has_switch() {
                    return;
                }
                // A switch row is answered by the switch, which plays its own
                // click when it moves. Playing one here too would double it.
                if !item.has_switch() {
                    app.sounds.borrow().click();
                }
                // Remembered so returning from a chooser lands back on the row
                // it was opened from, as the main menu does.
                *app.settings_row.borrow_mut() = row.index();
                app.activate_item(item, row);
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.show_menu());
        }

        // The column of categories goes in the order ahead of the list it sits
        // left of, which is what makes left and right move between them - the
        // same arrangement the browser's drives column uses.
        *self.nav_side_list.borrow_mut() = Some(categories.clone());
        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        self.wire_arrows(categories.upcast_ref());
        announce_selection(&categories);
        *self.screen.borrow_mut() = Screen::Settings;
        self.window.set_child(Some(&page));
        let remembered = (*self.settings_row.borrow()).min(last_row_index(&list));
        if let Some(row) = list.row_at_index(remembered) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Which setting a row in the right-hand pane is.
    fn item_at(&self, index: i32) -> Option<Item> {
        self.pane_items.borrow().get(index as usize).copied()
    }

    /// Takes the mark off the settings button once the version row is reached.
    ///
    /// Arriving on it is the moment somebody has been told, and pressing it
    /// should not be required to stop being nagged about something already
    /// seen. Attached whether or not there is anything new, since a check
    /// finishing while this screen is open can make there be.
    fn watch_update_row(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let app = self.clone();
        let controller = gtk::EventControllerFocus::new();
        controller.connect_enter(move |_| {
            let mut state = app.updates.borrow_mut();
            crate::updates::acknowledge(&mut state);
            drop(state);
            app.draw_update_badge();
        });
        row.add_controller(controller);
    }

    /// What a row does when it is chosen.
    fn activate_item(self: &Rc<Self>, item: Item, row: &gtk::ListBoxRow) {
        if let Some(setting) = item.setting() {
            self.show_selector(setting, row);
            return;
        }
        if item.has_switch() {
            self.work_switch_item(item);
            return;
        }
        match item {
            Item::ClearData => self.confirm_clear_data(),
            Item::Kodi => self.show_kodi(),
            Item::About => self.show_about(),
            Item::Notices => self.show_notices(),
            Item::UpdateStatus => self.open_release_page(),
            _ => {}
        }
    }

    /// Wires a bar to the setting it moves.
    fn wire_slider(self: &Rc<Self>, kind: Slider, bar: &gtk::Scale, value: &gtk::Label) {
        {
            let app = self.clone();
            let value = value.clone();
            bar.connect_change_value(move |_, scroll, moved| {
                app.set_slider(kind, moved, &value);
                if kind == Slider::Scale {
                    // A drag reports Jump, over and over, while the pointer
                    // holds the bar. Anything else - a step, a page, a scroll
                    // wheel - is finished by the time it arrives and can be
                    // drawn straight away.
                    if scroll == gtk::ScrollType::Jump {
                        app.wanted_scale.set(Some(moved));
                    } else {
                        app.apply_scale(moved);
                    }
                }
                glib::Propagation::Proceed
            });
        }
        // Let go of, and only then redrawn. Watched rather than handled, so the
        // bar keeps its own grip on the pointer while it is being dragged.
        if kind == Slider::Scale {
            let app = self.clone();
            let watcher = gtk::EventControllerLegacy::new();
            watcher.set_propagation_phase(gtk::PropagationPhase::Bubble);
            watcher.connect_event(move |_, event| {
                let done = matches!(
                    event.event_type(),
                    gdk::EventType::ButtonRelease | gdk::EventType::TouchEnd
                );
                if done && let Some(steps) = app.wanted_scale.take() {
                    app.apply_scale(steps);
                }
                glib::Propagation::Proceed
            });
            bar.add_controller(watcher);
        }
    }

    /// Turns the described-audio preference on or off for one output.
    ///
    /// A toggle rather than a chooser: there are two answers, and a screen to
    /// pick between them would be a screen with two rows on it.
    fn toggle_audio_description(self: &Rc<Self>, primary: bool) {
        {
            let mut config = self.config.borrow_mut();
            if primary {
                config.primary_audio_description = !config.primary_audio_description;
            } else {
                config.secondary_audio_description = !config.secondary_audio_description;
            }
            let _ = config.save();
        }
        // In place rather than rebuilding the screen: a rebuild reselects the
        // row but loses where the list was scrolled to, which threw the row
        // being pressed off the screen.
        let on = if primary {
            self.config.borrow().primary_audio_description
        } else {
            self.config.borrow().secondary_audio_description
        };
        self.set_settings_switch(
            if primary {
                Item::Description(Role::Primary)
            } else {
                Item::Description(Role::Secondary)
            },
            on,
        );
    }

    /// Moves the switch on a settings row to match what it now reports.
    fn set_settings_switch(&self, item: Item, on: bool) {
        self.settling_switch.set(true);
        if let Some((_, switch)) = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == item)
        {
            switch.set_active(on);
        }
        self.settling_switch.set(false);
    }

    /// Works the switch on a row the way a click on it would.
    ///
    /// Through the switch rather than straight to the setting, because GTK
    /// only runs the sliding animation from the switch's own gesture and
    /// activation. Setting its state moves it there in one frame, which is
    /// what made a key press look different from a click.
    fn work_switch_item(self: &Rc<Self>, item: Item) {
        let switch = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == item)
            .map(|(_, switch)| switch.clone());
        match switch {
            // Its own handler carries on from here, as it does for a click.
            Some(switch) => {
                switch.activate();
            }
            None => self.apply_switch_item(item),
        }
    }

    /// What a switch row actually changes, once something has asked for it.
    fn apply_switch_item(self: &Rc<Self>, item: Item) {
        match item {
            Item::InterfaceScale => self.toggle_automatic_scale(),
            Item::Sounds => self.toggle_sounds(),
            Item::StartFullscreen => self.toggle_start_fullscreen(),
            Item::ReadMetadata => self.toggle_read_metadata(),
            Item::ShowBackdrop => self.toggle_show_backdrop(),
            Item::Description(role) => self.toggle_audio_description(role == Role::Primary),
            Item::Volume(_) => self.toggle_settings_mute(item),
            Item::Sync(_) => self.toggle_settings_offset(item),
            Item::Updates => self.toggle_update_checks(),
            _ => {}
        }
    }

    /// Turns "open fullscreen" on or off.
    ///
    /// Only this changes it. Pressing F11 or the fullscreen mark is about the
    /// session in hand and leaves this alone - see [`App::toggle_fullscreen`].
    fn toggle_start_fullscreen(self: &Rc<Self>) {
        let mut config = self.config.borrow_mut();
        config.fullscreen = !config.fullscreen;
        let _ = config.save();
    }

    /// Turns the reading of sidecars and artwork beside a video on or off.
    ///
    /// The page is rebuilt afterwards, since what it can show has changed -
    /// and the backdrop row with it, which is only workable while this is on.
    fn toggle_read_metadata(self: &Rc<Self>) {
        {
            let mut config = self.config.borrow_mut();
            config.read_metadata = !config.read_metadata;
            let _ = config.save();
        }
        self.reread_details();
        self.show_settings();
    }

    /// Turns the film's fanart behind the media page on or off.
    fn toggle_show_backdrop(self: &Rc<Self>) {
        {
            let mut config = self.config.borrow_mut();
            config.show_backdrop = !config.show_backdrop;
            let _ = config.save();
        }
        self.reread_details();
    }

    /// Reads what is beside the file again, after a setting changed what may
    /// be read at all.
    ///
    /// Nothing to do without a file: the answer is about a video, and the
    /// next one loaded will be read under whatever the setting now says.
    fn reread_details(self: &Rc<Self>) {
        let Some(source) = self.file.borrow().clone() else {
            return;
        };
        let beside = {
            let config = self.config.borrow();
            crate::metadata::Beside {
                metadata: config.read_metadata,
                backdrop: config.show_backdrop,
            }
        };
        let media = crate::probe::Media {
            audio: Vec::new(),
            subtitles: Vec::new(),
            duration_ns: 0,
            video: self.details.borrow().video.clone(),
            tags: Default::default(),
        };
        let mut details = crate::metadata::resolve(&source, &media, beside);
        // The parts that came from the container rather than from beside the
        // file are already known and are not re-probed for a toggle.
        let held = self.details.borrow();
        details.duration_s = held.duration_s;
        details.container = held.container.clone();
        drop(held);
        *self.details.borrow_mut() = details;
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        self.art_generation.set(self.art_generation.get() + 1);
        self.start_art_load();
    }

    /// Turns the version check on or off.
    ///
    /// Rebuilds the screen rather than only moving the switch, because the row
    /// underneath comes and goes with it. Turning it on asks straight away:
    /// somebody who has just switched it on is asking the question now, and
    /// waiting until tomorrow to answer would look like it does not work.
    fn toggle_update_checks(self: &Rc<Self>) {
        let on = {
            let mut config = self.config.borrow_mut();
            config.check_for_updates = !config.check_for_updates;
            let _ = config.save();
            config.check_for_updates
        };
        if on {
            self.check_for_updates(true);
        }
        self.set_settings_switch(Item::Updates, on);
        self.refresh_version_row();
    }

    /// The version this is, on the left of its row.
    fn version_label(&self) -> String {
        format!("Current Version: v{}", env!("CARGO_PKG_VERSION"))
    }

    /// What the check made of it, on the right, or nothing while checking is
    /// off. "Up to date" rather than "Latest", which beside an arrow read as
    /// an instruction to go and get the latest rather than as a statement
    /// that this is it.
    fn version_status(&self) -> String {
        if !self.config.borrow().check_for_updates {
            return String::new();
        }
        match crate::updates::newer(&self.updates.borrow()) {
            Some((version, _)) => {
                format!(
                    "Update available: v{}",
                    version.trim_start_matches(['v', 'V'])
                )
            }
            None => "Up to date".to_string(),
        }
    }

    /// Redraws the row naming the version, in place.
    ///
    /// In place rather than by rebuilding the screen: turning the check on or
    /// off changes two words, and rebuilding for it threw the whole page away
    /// and drew it again - which flickers and moves every row under whatever
    /// was pointing at one.
    fn refresh_version_row(&self) {
        // Found by asking which row is the version one, rather than by a fixed
        // number: it is only in the pane at all when General is the category
        // being shown.
        let Some(index) = self
            .pane_items
            .borrow()
            .iter()
            .position(|item| *item == Item::UpdateStatus)
        else {
            return;
        };
        let list = self.settings_list.borrow().clone();
        let Some(row) = list.and_then(|list| list.row_at_index(index as i32)) else {
            return;
        };
        let (label, value) = (self.version_label(), self.version_status());
        let widget = menu_row(&label, &value, true);
        // The arrow means "this opens something", so it belongs only when
        // there is a release to go and look at.
        let newer = crate::updates::newer(&self.updates.borrow()).is_some();
        if let Some(chevron) = widget.last_child() {
            chevron.set_visible(newer);
        }
        row.set_child(Some(&widget));
        name_it(&row, &row_name(&label, &value));
        if newer {
            row.add_css_class("tp-badge-row");
        } else {
            row.remove_css_class("tp-badge-row");
        }
    }

    /// Opens the release page in whatever the machine uses for links.
    fn open_release_page(self: &Rc<Self>) {
        let url = {
            let state = self.updates.borrow();
            crate::updates::newer(&state).map(|(_, url)| url.to_string())
        };
        if let Some(url) = url {
            gtk::gio::AppInfo::launch_default_for_uri(&url, None::<&gtk::gio::AppLaunchContext>)
                .unwrap_or_else(|e| eprintln!("Could not open {url}: {e}"));
        }
    }

    /// Looks for a newer release, unless it is too soon to ask again.
    ///
    /// Off the main thread and reported back through a polled channel, the
    /// same way reading a video is: everything it touches afterwards is `Rc`
    /// and belongs to this thread.
    fn check_for_updates(self: &Rc<Self>, now: bool) {
        if !self.config.borrow().check_for_updates {
            return;
        }
        let previous = self.updates.borrow().clone();
        if !now && !crate::updates::due(&previous) {
            return;
        }

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(crate::updates::check(&previous));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
            let state = match receiver.try_recv() {
                Ok(state) => state,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            crate::updates::save(&state);
            *app.updates.borrow_mut() = state;
            app.draw_update_badge();
            // Only if Settings is open behind it, so the answer appears
            // rather than waiting to be opened again. The one row, not the
            // screen: rebuilding it under somebody reading it is a flicker
            // and a jump for the sake of two words.
            if *app.screen.borrow() == Screen::Settings {
                app.refresh_version_row();
            }
            glib::ControlFlow::Break
        });
    }

    /// Marks or unmarks the button that opens Settings.
    ///
    /// The mark says there is something in there worth seeing, which is true
    /// exactly until somebody has seen it - so reaching the row that names the
    /// version clears this, while the row keeps its own mark for as long as
    /// the version is there to be had.
    fn draw_update_badge(&self) {
        let wanted = crate::updates::unseen(&self.updates.borrow());
        for button in self.update_badges.borrow().iter() {
            if wanted {
                button.add_css_class("tp-badge");
            } else {
                button.remove_css_class("tp-badge");
            }
        }
    }

    fn toggle_sounds(self: &Rc<Self>) {
        let (enabled, device) = {
            let mut config = self.config.borrow_mut();
            config.sounds = !config.sounds;
            let _ = config.save();
            (config.sounds, config.primary_sink.clone())
        };
        *self.sounds.borrow_mut() = Sounds::new(enabled, device);
        self.set_settings_switch(Item::Sounds, enabled);
    }

    /// Hands the size back to the screen, or takes it over by hand.
    ///
    /// Taking it over keeps whatever is on screen now, so the switch changes
    /// who decides the size rather than the size itself.
    fn toggle_automatic_scale(self: &Rc<Self>) {
        let now_automatic = self.config.borrow().ui_scale.is_some();
        {
            let mut config = self.config.borrow_mut();
            // Taking it over keeps what is on screen, so the switch changes
            // who decides the size rather than the size itself.
            config.ui_scale = if now_automatic {
                None
            } else {
                Some(self.scale.get())
            };
            let _ = config.save();
        }
        if now_automatic {
            self.follow_automatic_scale(&self.window.clone());
        }
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == Item::InterfaceScale)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        if let Some((kind, scale, value)) = found {
            let (now, reading) = self.slider_state(kind);
            scale.set_value(now);
            value.set_text(&reading);
            scale.set_sensitive(!now_automatic);
            value.set_sensitive(!now_automatic);
        }
        self.set_settings_switch(Item::InterfaceScale, now_automatic);
    }

    /// Redraws the interface at the size the bar is now at.
    fn apply_scale(self: &Rc<Self>, steps: f64) {
        let scale = scale_from_steps(steps);
        if scale != self.scale.get() {
            self.restyle(scale);
        }
        let _ = self.config.borrow().save();
    }

    /// Re-renders at whatever the automatic size should be now.
    ///
    /// The screen's own scale while the window fills it, and 1x while it does
    /// not. The automatic size exists for a television read from a sofa, and
    /// a window on the same 4K monitor is read from arm's length - scaling
    /// that up only leaves less room in a window somebody chose the size of.
    ///
    /// A size set by hand is that size in both, which is what asking for one
    /// means.
    fn follow_automatic_scale(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        if self.config.borrow().ui_scale.is_some() {
            return;
        }
        let wanted = if window.is_fullscreen() {
            appearance::monitor_for_window(window)
                .map(|monitor| appearance::scale_for(&monitor))
                .unwrap_or(1.0)
        } else {
            1.0
        };
        if wanted != self.scale.get() {
            self.restyle(wanted);
        }
    }

    /// Re-renders every size in the interface at a new scale.
    fn restyle(self: &Rc<Self>, scale: f64) {
        self.scale.set(scale);
        self.styles.load_from_data(&style_css(scale));

        // The stylesheet is only half of a size. Everything drawn rather than
        // styled takes its size in Rust at the moment the page is built - the
        // poster, the marks on the buttons, every margin, the width the page
        // is held to - and none of that moves when the stylesheet is
        // reloaded. Restyling alone therefore left the two halves disagreeing:
        // type at the new size inside a page laid out for the old one.
        //
        // It shows worst where the change is largest. A 4K television picks
        // 2x, so a page built at 1x and restyled kept a half-size poster and
        // half-size margins under full-size text, and the whole composition
        // sat in the top of the screen with the bottom third empty.
        //
        // Rebuilding is cheap here and this happens on a monitor change or a
        // fullscreen toggle, not on a drag.
        if *self.screen.borrow() == Screen::Menu {
            let app = self.clone();
            glib::idle_add_local_once(move || {
                if *app.screen.borrow() == Screen::Menu {
                    app.show_menu();
                }
            });
        }
    }

    /// What is running, who wrote what it is built on, and under what terms.
    ///
    /// Prose rather than the two version rows this replaced: the versions were
    /// only ever there to be read out when something went wrong, and the
    /// licenses of the work TinePlayer is built on ask to be acknowledged
    /// somewhere a person can find them. A packaged application with no About
    /// page has nowhere to put either.
    fn show_about(self: &Rc<Self>) {
        let (page, scroller, body, back) = text_page("About");

        let version = format!("TinePlayer {}", env!("CARGO_PKG_VERSION"));
        body.append(&about_heading(&version));
        body.append(&about_text(
            "Free software under the MIT License, Copyright (c) 2026 Scott Bounds. You may use, change and pass it on, provided the copyright notice travels with it. It comes with no warranty of any kind.",
        ));
        // The domain rather than the repository, and followed rather than
        // only shown. A released binary cannot be edited: if the repository
        // is ever renamed or moved, a link baked into it breaks for good,
        // where a domain we own can simply be pointed somewhere else. It is
        // also shorter to read from across a room and possible to type from
        // memory, which a full GitHub path is not.
        //
        // The deeper links below stay on github.com. Domain forwarding
        // carries the root and not the path, so sending those through it
        // would land people on the front page instead of the file named.
        body.append(&about_link(
            "Report issues or check for updates at",
            "https://tineplayer.app",
            "tineplayer.app",
            Address::Inline,
        ));

        body.append(&about_heading("Built with"));
        body.append(&about_text(&format!(
            "{} and GTK {}.{}.{}, both free software under the GNU Lesser General Public License.",
            gstreamer::version_string(),
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version(),
        )));
        body.append(&about_link(
            "Also the work of a good many people writing Rust libraries, all attributed here:",
            "https://github.com/scottarius/TinePlayer/blob/main/THIRD-PARTY.md",
            "https://github.com/scottarius/TinePlayer/THIRD-PARTY.md",
            Address::OwnLine,
        ));

        body.append(&about_heading("Where things are kept"));
        for (label, path) in [
            ("Settings", crate::config::config_path()),
            ("Saved positions", crate::config::positions_path()),
        ] {
            body.append(&about_text(&format!("{label}: {}", path.display())));
        }

        {
            let app = self.clone();
            back.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }

        // No list to move through, so up and down scroll the page instead.
        // Without this the only way down a page longer than the screen would
        // be a mouse, on an interface built not to need one.
        self.set_nav(None, std::slice::from_ref(&back), &[]);
        *self.about_scroll.borrow_mut() = Some(scroller.vadjustment());
        *self.copy_root.borrow_mut() = Some(body.upcast());
        *self.screen.borrow_mut() = Screen::About;
        self.window.set_child(Some(&page));
        back.grab_focus();
    }

    /// The notices for everything TinePlayer is built from, in the
    /// application rather than only on a web page.
    ///
    /// Every package already carries THIRD-PARTY.md as a file, which is what
    /// the licenses actually ask for. This is about being able to read it: the
    /// machines this player is built for are televisions and HTPCs driven by a
    /// gamepad, where there may be no browser at all and opening one is not
    /// something a D-pad does well. The link on the About page stays for
    /// anyone who would rather read it on the web.
    ///
    /// Built into the binary rather than read from beside it, so it is there
    /// whichever way TinePlayer was installed, and cannot be separated from
    /// the thing it describes.
    fn show_notices(self: &Rc<Self>) {
        let (page, scroller, body, back) = text_page("Third Party Notices");

        let blocks = notices_blocks(include_str!("../THIRD-PARTY.md"));
        let last = blocks.len().saturating_sub(1);
        for (index, block) in blocks.into_iter().enumerate() {
            let widget = match block {
                Notice::Heading(text) => about_heading(&text),
                Notice::Text(text) => about_text(&text),
            };
            // The closing line is a remark about the list rather than part of
            // it, and sitting one row's gap under two hundred crates it read
            // as another entry. A heading would be too much for one sentence;
            // the space is enough to separate it.
            if index == last {
                widget.set_margin_top((24.0 * self.scale.get()).round() as i32);
                // And room under it, so scrolling to the end stops with the
                // last line clear of the edge rather than against it.
                widget.set_margin_bottom((32.0 * self.scale.get()).round() as i32);
            }
            body.append(&widget);
        }

        {
            let app = self.clone();
            back.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }

        // The same arrangement the About page uses: nothing to select, so up
        // and down scroll instead.
        self.set_nav(None, std::slice::from_ref(&back), &[]);
        *self.about_scroll.borrow_mut() = Some(scroller.vadjustment());
        *self.copy_root.borrow_mut() = Some(body.upcast());
        *self.screen.borrow_mut() = Screen::Notices;
        self.window.set_child(Some(&page));
        back.grab_focus();
    }

    /// Copies whatever is selected on the screen being shown, and says
    /// whether there was anything. Each paragraph is its own label and holds
    /// its own selection, so the first one holding any is the one that was
    /// dragged across.
    ///
    /// Done by hand because GTK delivers Ctrl+C to whichever widget has
    /// focus, and selectable text here deliberately never takes focus: it
    /// would put a caret in the middle of a screen driven by arrow keys.
    fn copy_selection(&self) -> bool {
        let root = self.copy_root.borrow().clone();
        let Some(root) = root else { return false };
        self.copy_from(&root)
    }

    fn copy_from(&self, widget: &gtk::Widget) -> bool {
        if let Some(label) = widget.downcast_ref::<gtk::Label>()
            && let Some((from, to)) = label.selection_bounds()
        {
            let selected: String = label
                .text()
                .chars()
                .skip(from as usize)
                .take((to - from) as usize)
                .collect();
            self.window.clipboard().set_text(&selected);
            return true;
        }
        let mut next = widget.first_child();
        while let Some(child) = next {
            if self.copy_from(&child) {
                return true;
            }
            next = child.next_sibling();
        }
        false
    }

    /// Moves the About page when there is nothing to select on it. Says
    /// whether it did, so ordinary navigation can carry on elsewhere.
    fn scroll_about(&self, delta: i32) -> bool {
        if *self.screen.borrow() != Screen::About {
            return false;
        }
        let Some(adjustment) = self.about_scroll.borrow().clone() else {
            return false;
        };
        // A third of a screenful a press: enough to make progress, little
        // enough to keep your place on the page.
        let step = adjustment.page_size() / 3.0;
        let moved = adjustment.value() + delta as f64 * step;
        adjustment.set_value(moved.clamp(adjustment.lower(), about_bottom(&adjustment)));
        true
    }

    /// The same for Home and End, which the About page has no rows to give to.
    fn scroll_about_edge(&self, end: bool) -> bool {
        if *self.screen.borrow() != Screen::About {
            return false;
        }
        let Some(adjustment) = self.about_scroll.borrow().clone() else {
            return false;
        };
        adjustment.set_value(if end {
            about_bottom(&adjustment)
        } else {
            adjustment.lower()
        });
        true
    }

    /// Registering with Kodi, which Kodi itself gives no way to do.
    ///
    /// The list of what is set up, and the way to add more. Only configured
    /// instances appear here: an unconfigured Kodi is something to add, not
    /// something with a state worth reporting, and listing every Kodi on the
    /// machine alongside the one you set up buries it.
    fn show_kodi(self: &Rc<Self>) {
        let (page, list, back, _slot) = list_page("Kodi", true);

        let configured = self.configured_kodis();
        let mut rows: Vec<(String, String)> = configured
            .iter()
            .map(|setup| (setup.label(), setup.state.describe().to_string()))
            .collect();
        rows.push(("Add Configuration".to_string(), String::new()));

        for (label, value) in &rows {
            append_named(
                &list,
                &menu_row(label, value, true),
                &row_name(label, value),
            );
        }

        {
            let app = self.clone();
            let paths: Vec<std::path::PathBuf> = configured
                .iter()
                .map(|setup| setup.userdata().to_path_buf())
                .collect();
            let add_row = rows.len() - 1;
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let index = row.index() as usize;
                if index == add_row {
                    app.start_kodi_wizard();
                } else if let Some(userdata) = paths.get(index) {
                    app.confirm_kodi_remove(userdata.clone());
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }

        *self.kodi_draft.borrow_mut() = None;
        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        *self.screen.borrow_mut() = Screen::Kodi;
        self.window.set_child(Some(&page));
        Self::open_on_first_usable(&list, &back);
    }

    /// Every Kodi on this machine, including any folder named by hand that we
    /// are still keeping track of.
    fn known_kodis(&self) -> Vec<crate::kodi_setup::Setup> {
        let extra = self.config.borrow().kodi_paths.clone();
        crate::kodi_setup::find_all(&extra)
    }

    /// Just the ones TinePlayer is set up in.
    fn configured_kodis(&self) -> Vec<crate::kodi_setup::Setup> {
        self.known_kodis()
            .into_iter()
            .filter(|setup| setup.is_configured())
            .collect()
    }

    /// Selects the first row that can actually be pressed, since some are
    /// there to be read, and falls back to the Back button when none can.
    fn open_on_first_usable(list: &gtk::ListBox, back: &gtk::Button) {
        let opening = (0..).find(|index| {
            list.row_at_index(*index)
                .is_none_or(|row| row.is_sensitive())
        });
        if let Some(row) = opening.and_then(|index| list.row_at_index(index)) {
            list.select_row(Some(&row));
            settle_on(&row);
        } else {
            back.grab_focus();
        }
    }

    /// Taking TinePlayer back out of one Kodi.
    ///
    /// Asked first, because it changes a file outside TinePlayer's own
    /// keeping. No backup is restored and none is taken: the file may have
    /// been edited since, by hand or by Kodi itself, and putting an old copy
    /// back would undo that. Our entry is cut out and the rest is left alone.
    fn confirm_kodi_remove(self: &Rc<Self>, userdata: std::path::PathBuf) {
        let setup = crate::kodi_setup::setup_at(userdata.clone());
        let app = self.clone();
        let back = {
            let app = self.clone();
            move || app.show_kodi()
        };
        self.show_kodi_dialog(
            &format!("Remove configuration from\n{}?", setup.label()),
            &["TinePlayer's entry will be removed from Kodi's configuration file"],
            Confirm {
                label: "Remove",
                destructive: true,
            },
            Screen::KodiConfirm,
            back,
            move || {
                let setup = crate::kodi_setup::setup_at(userdata.clone());
                match crate::kodi_setup::apply(
                    &setup,
                    crate::kodi_setup::Registration::Absent,
                    None,
                    false,
                ) {
                    Ok(_) => {
                        // A folder named by hand is only worth remembering
                        // while something is set up in it.
                        {
                            let mut config = app.config.borrow_mut();
                            config.kodi_paths.retain(|path| path != &userdata);
                            let _ = config.save();
                        }
                        app.show_kodi();
                    }
                    Err(e) => app.show_kodi_error(&e, {
                        let app = app.clone();
                        move || app.show_kodi()
                    }),
                }
            },
        );
    }

    // --- The wizard ----------------------------------------------------
    //
    // Screens that collect answers and write nothing. Only Configure, on the
    // last one, touches Kodi - so backing out is free at every point, which
    // is the whole reason it is shaped this way rather than as a screen of
    // switches that act as they are flipped.
    //
    // The two that ask something are ordinary lists with a back arrow, driven
    // like every other screen in the application: choosing a row is the
    // answer and moves on, so there is no Next to press and nothing to
    // explain about how to proceed.

    fn start_kodi_wizard(self: &Rc<Self>) {
        *self.kodi_draft.borrow_mut() = Some(KodiDraft::default());
        self.show_kodi_choose();
    }

    /// Which Kodi. Every one found, whether or not it is already set up:
    /// choosing one that is becomes an update rather than a second entry.
    fn show_kodi_choose(self: &Rc<Self>) {
        let (page, list, back, _slot) = list_page("Choose a Kodi Installation", true);

        let found = self.known_kodis();
        // An install that cannot be set up is still listed, and still says
        // what it is: leaving one out looks like it was not found, which
        // sends somebody hunting for a folder rather than telling them the
        // answer.
        let mut rows: Vec<(String, String, bool)> = found
            .iter()
            .map(|setup| {
                let state = match setup.confinement.unsupported_reason() {
                    Some(reason) => reason.to_string(),
                    None if setup.is_configured() => {
                        format!("Already set up - {}", setup.state.describe())
                    }
                    None => setup.userdata().display().to_string(),
                };
                (setup.label(), state, setup.confinement.supported())
            })
            .collect();
        rows.push(("Custom install location".to_string(), String::new(), true));

        for (label, value, usable) in &rows {
            append_named(
                &list,
                &menu_row(label, value, *usable),
                &row_name(label, value),
            );
        }

        {
            let app = self.clone();
            let paths: Vec<std::path::PathBuf> = found
                .iter()
                .map(|setup| setup.userdata().to_path_buf())
                .collect();
            let browse_row = rows.len() - 1;
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let index = row.index() as usize;
                if index == browse_row {
                    // Home, every time. Kodi's userdata lives under it on
                    // every platform, and where the video browser was last
                    // says nothing about where Kodi keeps its settings.
                    app.show_kodi_folder(&crate::browser::home());
                } else if let Some(userdata) = paths.get(index) {
                    app.with_draft(|draft| draft.userdata = Some(userdata.clone()));
                    app.show_kodi_how();
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_kodi();
            });
        }

        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        *self.screen.borrow_mut() = Screen::KodiChoose;
        self.window.set_child(Some(&page));
        Self::open_on_first_usable(&list, &back);
    }

    /// The places column that sits to the left of a browser's listing.
    ///
    /// Home, the drives or filesystem, and whatever is mounted - all at once
    /// rather than on a separate screen reached by stepping off the top of
    /// the tree. Moving between the two lists is left and right, which the
    /// keyboard and the gamepad both do by ordinary directional focus.
    ///
    /// `folders` says which browser a drive reopens, the same way the
    /// breadcrumbs do.
    fn places_column(
        self: &Rc<Self>,
        current: &std::path::Path,
        folders: bool,
    ) -> Option<(gtk::ScrolledWindow, gtk::ListBox)> {
        let roots = crate::browser::places();
        if roots.is_empty() {
            return None;
        }

        let list = gtk::ListBox::new();
        list.add_css_class("tp-menu");
        list.set_selection_mode(gtk::SelectionMode::Browse);
        list.set_activate_on_single_click(true);

        // Which place the listing is inside, so the column says where you are
        // as well as where you could go. The longest match wins: a volume
        // under /mnt is a better answer than the filesystem root that also
        // contains it.
        let here = crate::browser::rooted(current);
        let mut selected: Option<(i32, usize)> = None;
        for (index, entry) in roots.iter().enumerate() {
            append_named(&list, &chooser_row(&entry.label), &entry.label);
            if here.starts_with(&entry.path) {
                let depth = entry.path.components().count();
                if selected.is_none_or(|(_, best)| depth > best) {
                    selected = Some((index as i32, depth));
                }
            }
        }
        let selected = selected.map(|(index, _)| index);
        if let Some(row) = selected.and_then(|index| list.row_at_index(index)) {
            // Marked as the one in force, and the cursor starts there - but
            // the two part company as soon as the viewer moves, which is the
            // whole point of marking it separately.
            row.add_css_class("tp-current");
            list.select_row(Some(&row));
        }

        {
            let app = self.clone();
            let paths: Vec<std::path::PathBuf> = roots.iter().map(|e| e.path.clone()).collect();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                if let Some(path) = paths.get(row.index() as usize) {
                    if folders {
                        app.show_kodi_folder(path);
                    } else {
                        app.show_browser(path, None);
                    }
                }
            });
        }
        self.follow_focus(&list);

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .width_request((220.0 * self.scale.get()).round() as i32)
            .child(&list)
            .build();
        scroller.set_focusable(false);
        list.set_focusable(true);
        Some((scroller, list))
    }

    /// Sends this widget's up and down keys through `move_selection`, which
    /// knows where the focus is and what each boundary should do.
    ///
    /// Needed on anything that can hold focus beside a list, now that rows
    /// cannot: GtkListBox moves its cursor by moving focus between rows, and
    /// with nothing able to take it that does nothing at all. Capture phase,
    /// so this runs before the list's own bindings swallow the key.
    fn wire_arrows(self: &Rc<Self>, widget: &gtk::Widget) {
        let app = self.clone();
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        controller.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Up => {
                app.move_selection(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                app.move_selection(1);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        widget.add_controller(controller);
    }

    /// Puts a widget into the tab order at the end.
    fn add_nav_stop(&self, widget: &impl IsA<gtk::Widget>) {
        self.nav_stops.borrow_mut().push(widget.clone().upcast());
    }

    /// Moves to the next or previous thing on this screen worth stopping on.
    ///
    /// Returns whether it did, so a screen with no stops of its own - a text
    /// panel, say - falls back to GTK's own handling rather than trapping the
    /// key.
    fn move_focus_stop(self: &Rc<Self>, delta: isize) -> bool {
        let stops = self.nav_stops.borrow().clone();
        if stops.is_empty() {
            return false;
        }
        let focused = gtk::prelude::GtkWindowExt::focus(&self.window);
        // Which stop the focus is in, rather than which stop it is: focus on
        // a button inside a stop still counts as being there.
        let at = focused.and_then(|widget| {
            stops.iter().position(|stop| {
                *stop == widget || stop.is_ancestor(&widget) || widget.is_ancestor(stop)
            })
        });
        let next = match at {
            Some(at) => (at as isize + delta).rem_euclid(stops.len() as isize) as usize,
            // Nowhere in particular yet: forwards starts at the beginning,
            // backwards at the end.
            None if delta > 0 => 0,
            None => stops.len() - 1,
        };
        if let Some(stop) = stops.get(next) {
            self.sounds.borrow().click();
            stop.grab_focus();
        }
        true
    }

    /// Moves between two lists sitting side by side, and does nothing
    /// anywhere else: left and right are for the panes of the browser, not a
    /// second way to reach the buttons.
    fn move_between_lists(self: &Rc<Self>, delta: isize) -> bool {
        let stops = self.nav_stops.borrow().clone();
        let Some(focused) = gtk::prelude::GtkWindowExt::focus(&self.window) else {
            return false;
        };
        let Some(at) = stops.iter().position(|stop| {
            *stop == focused || stop.is_ancestor(&focused) || focused.is_ancestor(stop)
        }) else {
            return false;
        };
        if !stops[at].is::<gtk::ListBox>() {
            return false;
        }
        let next = at as isize + delta;
        if next < 0 || next as usize >= stops.len() {
            return false;
        }
        let next = &stops[next as usize];
        if !next.is::<gtk::ListBox>() {
            return false;
        }
        self.sounds.borrow().click();
        next.grab_focus();
        true
    }

    /// Makes a list the one the gamepad drives whenever it holds the focus.
    ///
    /// The navigation machinery knows about a single list at a time, which is
    /// all any other screen needs. With two side by side, which one is "the"
    /// list has to follow the focus, or the gamepad keeps driving whichever
    /// was wired last however far the viewer has moved away from it.
    fn follow_focus(self: &Rc<Self>, list: &gtk::ListBox) {
        let app = self.clone();
        let controller = gtk::EventControllerFocus::new();
        {
            let list = list.clone();
            controller.connect_enter(move |_| {
                *app.nav_list.borrow_mut() = Some(list.clone());
            });
        }
        list.add_controller(controller);
    }

    /// Puts a browser's listing beside its drive column.
    ///
    /// `list_page_with` has already put the listing in the page; this takes
    /// it back out and rebuilds that row with the drives to its left.
    fn add_places_column(
        self: &Rc<Self>,
        page: &gtk::Box,
        current: &std::path::Path,
        folders: bool,
        header: &[gtk::Button],
    ) {
        let Some(listing) = page.last_child() else {
            return;
        };
        let Some((places, list)) = self.places_column(current, folders) else {
            return;
        };
        page.remove(&listing);

        // The column takes the width it asked for and the listing takes the
        // rest. Without this the listing is given its minimum, which for a
        // list of names is very little, and the folders end up in a ribbon
        // down one side of the screen.
        places.set_hexpand(false);
        listing.set_hexpand(true);

        // Handed to set_nav, which puts it in the order ahead of the listing
        // it sits left of, and driven by the same keys once it has focus.
        *self.nav_side_list.borrow_mut() = Some(list.clone());
        self.wire_arrows(list.upcast_ref());
        // Its own, since wire_navigation only ever sees a screen's main list.
        announce_selection(&list);

        // Up from the top of the column reaches the trail above it, the same
        // way it does from the listing.
        {
            let app = self.clone();
            let header: Vec<glib::WeakRef<gtk::Button>> =
                header.iter().map(|button| button.downgrade()).collect();
            let controller = gtk::EventControllerKey::new();
            // Weak, since the controller is added to the very list it watches
            // and holding a strong reference would keep the pair alive.
            let watched = list.downgrade();
            controller.connect_key_pressed(move |_, key, _, _| {
                let Some(list) = watched.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if key != gdk::Key::Up || list.selected_row().map(|row| row.index()) != Some(0) {
                    return glib::Propagation::Proceed;
                }
                let buttons: Vec<gtk::Button> = header
                    .iter()
                    .filter_map(|button| button.upgrade())
                    .collect();
                if let Some(button) = App::last_header(&buttons) {
                    app.sounds.borrow().click();
                    button.grab_focus();
                }
                glib::Propagation::Stop
            });
            list.add_controller(controller);
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .vexpand(true)
            .build();
        row.append(&places);
        row.append(&listing);
        page.append(&row);
    }

    /// Browsing for Kodi's userdata folder, in TinePlayer's own browser.
    ///
    /// The system's folder chooser would do the job, but not from a sofa: it
    /// is a desktop dialog that a gamepad cannot drive and that draws itself
    /// at desktop sizes on a television. This is the same browser used for
    /// finding a video, showing only folders, with choosing the current one
    /// on a button beside the way out.
    ///
    /// Deliberately a sibling of `show_browser` rather than a mode of it.
    /// That one carries a paste row, video entries, a remembered location and
    /// an origin to return to, none of which belong here, and threading a
    /// purpose through all of it would put the video browser at risk for the
    /// sake of a screen that shares only its shape.
    /// The screen for choosing the folder Kodi keeps its settings in.
    fn show_kodi_folder(self: &Rc<Self>, directory: &std::path::Path) {
        let directory = crate::browser::rooted(directory);
        let page = self.browser_page(&directory, Browse::Folders);
        let entries = browser_entries(&directory, Browse::Folders);

        let choose = gtk::Button::with_label("Choose");
        choose.add_css_class("tp-button");
        choose.add_css_class("tp-action");
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        buttons.append(&page.cancel);
        buttons.append(&choose);
        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&page.browse));
        footer.set_center_widget(Some(&buttons));
        page.page.append(&footer);

        fill_browser_list(&page.list, &entries, self.scale.get());

        {
            let app = self.clone();
            let entries = entries.clone();
            let here = directory.clone();
            page.list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let Some(entry) = entries.get(row.index() as usize) else {
                    return;
                };
                match &entry.path {
                    Some(path) => app.show_kodi_folder(path),
                    None => {
                        if let Some(parent) = here.parent() {
                            app.show_kodi_folder(parent);
                        }
                    }
                }
            });
        }
        {
            let app = self.clone();
            let directory = directory.clone();
            choose.connect_clicked(move |_| {
                app.sounds.borrow().click();
                let userdata = crate::kodi_setup::userdata_from(directory.clone());
                app.with_draft(|draft| draft.userdata = Some(userdata));
                app.show_kodi_how();
            });
        }
        {
            let app = self.clone();
            page.cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_kodi_choose();
            });
        }

        // Same order they are laid out in, or moving between them runs
        // backwards against what is on screen.
        self.wire_navigation(
            &page.list,
            &page.crumbs,
            &[page.cancel.clone(), choose.clone()],
        );
        *self.screen.borrow_mut() = Screen::KodiFolder;
        self.window.set_child(Some(&self.modal(&page.page)));
        if let Some(row) = page.list.row_at_index(0) {
            page.list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// The system's own folder chooser, for anyone who would rather use it.
    fn choose_kodi_folder_natively(self: &Rc<Self>, start: &std::path::Path) {
        let chooser = gtk::FileChooserNative::new(
            Some("Choose Kodi's userdata folder"),
            Some(&self.window),
            gtk::FileChooserAction::SelectFolder,
            Some("Choose"),
            Some("Cancel"),
        );
        open_at(&chooser, start);
        let app = self.clone();
        // Held by the closure so the dialog outlives this function; a dropped
        // FileChooserNative closes before the user can answer. Same handling
        // as the video chooser.
        let held = RefCell::new(Some(chooser.clone()));
        chooser.connect_response(move |chooser, response| {
            let chosen = (response == gtk::ResponseType::Accept)
                .then(|| chooser.file().and_then(|file| file.path()))
                .flatten();
            held.borrow_mut().take();
            if let Some(folder) = chosen {
                let userdata = crate::kodi_setup::userdata_from(folder);
                app.with_draft(|draft| draft.userdata = Some(userdata));
                app.show_kodi_how();
            }
        });
        chooser.show();
    }

    /// What Kodi should do with TinePlayer. Choosing is the answer, and leads
    /// straight to what it would mean.
    fn show_kodi_how(self: &Rc<Self>) {
        use crate::kodi_setup::Registration;

        if self.draft_userdata().is_none() {
            return self.show_kodi();
        }
        let (page, list, back, _slot) = list_page("How to Configure", true);

        let choices = [
            (
                "Default Player",
                "Kodi hands every video straight to TinePlayer",
                Registration::Default,
            ),
            (
                "Optional Player",
                "TinePlayer appears under \"Play using...\" in a video's menu",
                Registration::Offered,
            ),
        ];
        for (label, value, _) in &choices {
            append_named(
                &list,
                &menu_row(label, value, true),
                &row_name(label, value),
            );
        }

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                if let Some((_, _, want)) = choices.get(row.index() as usize) {
                    app.with_draft(|draft| draft.want = Some(*want));
                    app.show_kodi_handover();
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_kodi_choose();
            });
        }

        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        *self.screen.borrow_mut() = Screen::KodiHow;
        self.window.set_child(Some(&page));
        Self::open_on_first_usable(&list, &back);
    }

    /// What should happen when Kodi hands a video over: start the film, or
    /// open the menu so the tracks can be chosen for it.
    ///
    /// Worth asking rather than assuming. A television with one pair of
    /// headphones on it wants the film to start; a household that picks
    /// different languages each time wants the menu. The answer is written as
    /// `--play` in Kodi's own configuration, so it can be changed there by
    /// hand afterwards as well.
    fn show_kodi_handover(self: &Rc<Self>) {
        if self.draft_userdata().is_none() {
            return self.show_kodi();
        }
        let (page, list, back, _slot) = list_page("When TinePlayer Starts", true);

        let choices = [
            (
                "Play Video",
                "Play video right away with default options",
                true,
            ),
            (
                "Show the Menu",
                "Choose the audio tracks and subtitles for each video",
                false,
            ),
        ];
        for (label, value, _) in &choices {
            append_named(
                &list,
                &menu_row(label, value, true),
                &row_name(label, value),
            );
        }

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                if let Some((_, _, play)) = choices.get(row.index() as usize) {
                    app.with_draft(|draft| draft.play = *play);
                    app.show_kodi_manual_or_summary();
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_kodi_how();
            });
        }

        // Nothing is marked as current here. The absence of --play reads as
        // "show the menu", so an unconfigured Kodi - the ordinary case on the
        // way through this wizard - marked that row every time, presenting a
        // default as though it were a setting already in force.
        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        *self.screen.borrow_mut() = Screen::KodiHandover;
        self.window.set_child(Some(&page));
        Self::open_on_first_usable(&list, &back);
    }

    /// The manual step, when there is one, and otherwise straight to the
    /// summary. Skipped rather than shown empty: a screen saying "nothing to
    /// do here" is a screen worth not having.
    fn show_kodi_manual_or_summary(self: &Rc<Self>) {
        let confinement = self
            .draft_userdata()
            .map(|userdata| crate::kodi_setup::setup_at(userdata).confinement);
        match confinement.and_then(crate::kodi_setup::manual_step) {
            Some(manual) => self.show_kodi_manual(manual),
            None => self.show_kodi_summary(),
        }
    }

    /// Back out of the summary onto whichever screen led to it: the manual
    /// step where the installation needs one, and the handover question where
    /// it does not. Chosen the same way the forward path chooses, so a Back
    /// press cannot land somewhere the viewer never came through.
    fn show_kodi_back_from_summary(self: &Rc<Self>) {
        let confinement = self
            .draft_userdata()
            .map(|userdata| crate::kodi_setup::setup_at(userdata).confinement);
        match confinement.and_then(crate::kodi_setup::manual_step) {
            Some(manual) => self.show_kodi_manual(manual),
            None => self.show_kodi_handover(),
        }
    }

    /// Something TinePlayer cannot do for you, and will not do quietly.
    /// Continuing is what says it has been done.
    fn show_kodi_manual(self: &Rc<Self>, manual: crate::kodi_setup::ManualStep) {
        let mut lines = vec![manual.what, manual.why];
        if let Some(command) = manual.command {
            lines.push(command);
        }
        lines.push(manual.cost);

        let back = {
            let app = self.clone();
            move || app.show_kodi_handover()
        };
        let app = self.clone();
        self.show_kodi_dialog(
            "One thing to do yourself",
            &lines,
            Confirm {
                label: "Continue",
                destructive: false,
            },
            Screen::KodiManual,
            back,
            move || app.show_kodi_summary(),
        );
    }

    /// Everything that is about to happen, before any of it happens.
    fn show_kodi_summary(self: &Rc<Self>) {
        let Some((userdata, want)) = self.draft_parts() else {
            return self.show_kodi();
        };
        let play = self
            .kodi_draft
            .borrow()
            .as_ref()
            .is_some_and(|draft| draft.play);
        let setup = crate::kodi_setup::setup_at(userdata);

        // Settled here rather than at write time, because this screen names
        // the file and the write has to produce that same one. Whether to
        // take one at all is not asked: a copy is kept the first time
        // TinePlayer touches a file, and not on later runs, which is the
        // answer almost everyone would give and saves a question.
        let backup_to = setup
            .backup_by_default()
            .then(|| crate::kodi_setup::backup_path(&setup.file));
        self.with_draft(|draft| draft.backup_to = backup_to.clone());

        let mut lines = vec![format!(
            "{} {}",
            if setup.file.exists() {
                "Edit"
            } else {
                "Create"
            },
            setup.file.display()
        )];
        if let Some(name) = backup_to.as_ref().and_then(|path| path.file_name()) {
            lines.push(format!("Backup file: {}", name.to_string_lossy()));
        }
        lines.push(format!("Add TinePlayer as {}", want.describe()));
        lines.push(
            if play {
                "Start playing when Kodi hands a video over"
            } else {
                "Open the menu when Kodi hands a video over"
            }
            .to_string(),
        );

        let back = {
            let app = self.clone();
            move || app.show_kodi_back_from_summary()
        };
        let app = self.clone();
        self.show_kodi_dialog(
            "Confirm Configuration",
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
            Confirm {
                label: "Configure",
                destructive: false,
            },
            Screen::KodiSummary,
            back,
            move || app.apply_kodi_draft(),
        );
    }

    /// The only place anything is written.
    fn apply_kodi_draft(self: &Rc<Self>) {
        let Some((userdata, want)) = self.draft_parts() else {
            return self.show_kodi();
        };
        let setup = crate::kodi_setup::setup_at(userdata.clone());
        // The path the summary named, not a freshly computed one: the file
        // written has to be the file the viewer was shown.
        let backup_to = self
            .kodi_draft
            .borrow()
            .as_ref()
            .and_then(|draft| draft.backup_to.clone());
        let play = self
            .kodi_draft
            .borrow()
            .as_ref()
            .is_some_and(|draft| draft.play);
        match crate::kodi_setup::apply(&setup, want, backup_to.as_deref(), play) {
            Ok(_) => {
                // A folder named by hand is worth keeping track of now that
                // something is set up in it, so it can be found again to
                // change or remove.
                let known = crate::kodi_setup::find_all(&[])
                    .iter()
                    .any(|found| found.userdata() == userdata);
                if !known {
                    let mut config = self.config.borrow_mut();
                    if !config.kodi_paths.contains(&userdata) {
                        config.kodi_paths.push(userdata);
                        let _ = config.save();
                    }
                }
                self.show_kodi_configured();
            }
            // Back to the summary, which is still true and still the place to
            // press Configure from.
            Err(e) => self.show_kodi_error(&e, {
                let app = self.clone();
                move || app.show_kodi_summary()
            }),
        }
    }

    /// Done, with the one thing left for the viewer to do.
    fn show_kodi_configured(self: &Rc<Self>) {
        let page = wizard_page("Configuration Successful");
        page.append(&wizard_text(
            "Restart Kodi for the configuration to take effect.",
            false,
        ));

        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("tp-button");
        // An action, unlike its twin on the error screen: this screen is the
        // end of a wizard that has just done something, and OK is the way on
        // from it rather than a way to dismiss bad news.
        ok.add_css_class("tp-action");
        ok.set_halign(gtk::Align::Center);
        page.append(&ok);
        {
            let app = self.clone();
            ok.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_kodi();
            });
        }

        *self.kodi_draft.borrow_mut() = None;
        self.set_nav(None, std::slice::from_ref(&ok), &[]);
        *self.screen.borrow_mut() = Screen::KodiDone;
        self.window.set_child(Some(&self.modal(&page)));
        ok.grab_focus();
    }

    /// Something went wrong, said plainly, with a way back to where it was
    /// worth trying from.
    fn show_kodi_error(self: &Rc<Self>, message: &str, back: impl Fn() + 'static) {
        let page = wizard_page("Configuration Error");
        page.append(&wizard_text(message, false));

        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("tp-button");
        ok.set_halign(gtk::Align::Center);
        page.append(&ok);
        {
            let app = self.clone();
            ok.connect_clicked(move |_| {
                app.sounds.borrow().click();
                back();
            });
        }

        self.set_nav(None, std::slice::from_ref(&ok), &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::KodiError;
        self.window.set_child(Some(&self.modal(&page)));
        ok.grab_focus();
    }

    /// A panel that states something and asks whether to go ahead.
    ///
    /// Cancel returns wherever the caller says, which is not always the Kodi
    /// list: backing out of Confirm Configuration belongs on the screen the
    /// choice was made on, so the answer can be changed rather than the whole
    /// wizard restarted.
    fn show_kodi_dialog(
        self: &Rc<Self>,
        title: &str,
        lines: &[&str],
        confirm: Confirm<'_>,
        screen: Screen,
        back: impl Fn() + 'static,
        action: impl Fn() + 'static,
    ) {
        let page = wizard_page(title);
        for line in lines {
            // A command is the one thing somebody has to reproduce exactly,
            // so it is set apart and wraps by character rather than by word.
            let command = line.starts_with("flatpak ");
            page.append(&wizard_text(line, command));
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        // Backing out is never the hazard, so Cancel is never the red one.
        // It used to be: red was put on whichever button was left over, so a
        // confirmation of something harmless painted the way out as the
        // dangerous choice.
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let destructive = confirm.destructive;
        let confirm = gtk::Button::with_label(confirm.label);
        confirm.add_css_class("tp-button");
        confirm.add_css_class(match destructive {
            true => "tp-danger",
            false => "tp-action",
        });
        row.append(&cancel);
        row.append(&confirm);
        page.append(&row);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                back();
            });
        }
        {
            let app = self.clone();
            confirm.connect_clicked(move |_| {
                app.sounds.borrow().click();
                action();
            });
        }

        self.set_nav(None, &[cancel.clone(), confirm.clone()], &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = screen;
        self.window.set_child(Some(&self.modal(&page)));
        // Cancel, so a reflexive second press changes nothing.
        cancel.grab_focus();
    }

    fn with_draft(&self, edit: impl FnOnce(&mut KodiDraft)) {
        if let Some(draft) = self.kodi_draft.borrow_mut().as_mut() {
            edit(draft);
        }
    }

    fn draft_userdata(&self) -> Option<std::path::PathBuf> {
        self.kodi_draft
            .borrow()
            .as_ref()
            .and_then(|draft| draft.userdata.clone())
    }

    /// Where and what, or nothing if the wizard is not far enough along to
    /// act on.
    fn draft_parts(&self) -> Option<(std::path::PathBuf, crate::kodi_setup::Registration)> {
        let draft = self.kodi_draft.borrow();
        let draft = draft.as_ref()?;
        Some((draft.userdata.clone()?, draft.want?))
    }

    fn confirm_clear_data(self: &Rc<Self>) {
        let app = self.clone();
        self.show_confirm(
            "Forget saved positions and track choices\nfor every video?",
            "Clear",
            move || {
                if let Err(e) = crate::config::clear_all_resume() {
                    eprintln!("{e}");
                }
                // The loaded file keeps its choices for this session; only
                // what was written down is gone.
                app.show_settings();
            },
        );
    }

    /// A yes-or-no page in the same style as the rest, since a dialog would
    /// be unreadable at a distance and awkward with a controller.
    fn show_confirm(
        self: &Rc<Self>,
        message: &str,
        confirm_label: &str,
        action: impl Fn() + 'static,
    ) {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        page.append(&heading_label(message));

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let confirm = gtk::Button::with_label(confirm_label);
        confirm.add_css_class("tp-button");
        buttons.append(&cancel);
        buttons.append(&confirm);
        page.append(&buttons);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }
        {
            let app = self.clone();
            confirm.connect_clicked(move |_| {
                app.sounds.borrow().click();
                action();
            });
        }

        self.set_nav(None, &[], &[]);
        *self.screen.borrow_mut() = Screen::Confirm;
        self.window.set_child(Some(&page));
        // Cancel takes focus, so a reflexive second press doesn't destroy
        // anything.
        cancel.grab_focus();
    }

    // --- Playback ------------------------------------------------------

    /// Shows the black video surface, then starts playback a frame later.
    ///
    /// Building the pipeline and seeking to a resume position both happen on
    /// this thread, so nothing repaints until they finish. Swapping the window
    /// first and letting one frame through means the menu disappears the
    /// instant Play is pressed, and the wait happens against black - which is
    /// what a video starting looks like anyway. Accurate seeking made this
    /// worth doing: it decodes forward to the exact position, and on a long
    /// film that is visible.
    fn start_playback(self: &Rc<Self>, restart: bool) {
        if self.file.borrow().is_none() {
            return;
        }

        let waiting = gtk::Box::builder()
            .css_classes([crate::player::VIDEO_CSS_CLASS])
            .hexpand(true)
            .vexpand(true)
            .build();
        self.window.set_child(Some(&waiting));

        // A timeout rather than an idle callback: idle can run before the
        // frame it was queued behind has actually been drawn, which puts the
        // block back where it started.
        let app = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(16), move || {
            app.begin_playback(restart);
        });
    }

    /// Swaps the black surface for the video once a frame from the resume
    /// point has actually been drawn, or after a moment if none ever is.
    ///
    /// Waiting on the reported position is not enough. A flushing seek updates
    /// the pipeline's segment before the sink has rendered anything, so
    /// position says "arrived" while the picture on screen is still the
    /// opening frame from the preroll - which is exactly the flash this exists
    /// to prevent. The paintable tells us when a frame is genuinely there.
    ///
    /// Not driven by the pipeline's asynchronous-done message either: that
    /// fires for the preroll as well, so acting on it would reveal the picture
    /// at precisely the wrong moment.
    fn reveal_when_resumed(
        self: &Rc<Self>,
        widget: gtk::Overlay,
        paintable: Option<gdk::Paintable>,
        target: u64,
    ) {
        // Well inside a keyframe interval, and far enough from the opening
        // frame that the two cannot be mistaken for each other.
        const CLOSE_ENOUGH: u64 = 500_000_000;

        let reveal = {
            let app = self.clone();
            let widget = widget.clone();
            move || {
                // Playback may have been left while waiting, in which case
                // whatever replaced it should stay.
                if *app.screen.borrow() == Screen::Playing {
                    app.window.set_child(Some(&widget));
                }
            }
        };

        let Some(paintable) = paintable else {
            reveal();
            return;
        };

        let done = Rc::new(Cell::new(false));
        let handler = Rc::new(RefCell::new(None));
        {
            let app = self.clone();
            let reveal = reveal.clone();
            let done = done.clone();
            // Its own handle, so the outer one survives to be stored below.
            let registered = handler.clone();
            let id = paintable.connect_invalidate_contents(move |paintable| {
                if done.get() {
                    return;
                }
                let arrived = app
                    .playback
                    .borrow()
                    .as_ref()
                    .and_then(|playback| playback.position())
                    .is_some_and(|position| position.nseconds() + CLOSE_ENOUGH >= target);
                if !arrived {
                    return;
                }
                done.set(true);
                if let Some(id) = registered.borrow_mut().take() {
                    paintable.disconnect(id);
                }
                reveal();
            });
            *handler.borrow_mut() = Some(id);
        }

        // A seek that fails, or a source that never produces another frame,
        // would otherwise leave a black window and nothing to explain it.
        glib::timeout_add_local_once(std::time::Duration::from_secs(4), move || {
            if done.replace(true) {
                return;
            }
            if let Some(id) = handler.borrow_mut().take() {
                paintable.disconnect(id);
            }
            reveal();
        });
    }

    fn begin_playback(self: &Rc<Self>, restart: bool) {
        let Some(path) = self.file.borrow().clone() else {
            return;
        };
        self.stop_playback();

        // Belt and braces against the pipeline being asked for an output that
        // was never configured, whatever left the choice set.
        //
        // The file as well as the track, which it did not used to cover: an
        // audio file on the secondary output with no secondary device asked
        // for a sink that cannot be built, and the whole pipeline failed - so
        // a film with a perfectly good primary output would not play at all.
        let has_secondary_device = self.config.borrow().secondary_sink.is_some();
        let primary = *self.primary_track.borrow();
        let secondary = if has_secondary_device {
            *self.secondary_track.borrow()
        } else {
            None
        };
        let secondary_file = if has_secondary_device {
            self.secondary_file.borrow().clone()
        } else {
            None
        };
        // A separate audio file wins for that output. The track it displaces
        // is still remembered below, so clearing the file falls back to it.
        let audio_for = |file: Option<Source>, track: Option<u32>| match file {
            Some(file) => Some(crate::pipeline::AudioSource::File(file)),
            None => track.map(crate::pipeline::AudioSource::Track),
        };
        let primary_audio = audio_for(self.primary_file.borrow().clone(), primary);
        let secondary_audio = audio_for(secondary_file, secondary);

        let subtitle = self.subtitle.borrow().clone();
        if let Some(key) = self.storage_key() {
            crate::config::save_tracks(
                &key,
                primary,
                secondary,
                subtitle.clone(),
                self.saved_path(Role::Primary),
                self.saved_path(Role::Secondary),
            );
        }

        let app = self.clone();
        let on_ended = move |ended| {
            // Something else picked this video and is waiting for the playback
            // to finish, so reaching the end of it means there is nothing left
            // to do and the menu would only be in the way. An error is not the
            // same: quitting would take the reason off the screen with it.
            if app.external && ended == crate::player::Ended::Finished {
                app.finish_playback(true);
                app.window.close();
                return;
            }
            app.stop_playback();
            app.show_menu();
        };

        // "Restart" means start from the beginning whoever is asking, so it
        // beats both our saved position and Kodi's. Bound rather than passed
        // inline because the reveal below waits for playback to reach it.
        let resume = (!restart).then(|| self.resume_position()).flatten();

        let result = Playback::start(
            &path,
            primary_audio.as_ref(),
            secondary_audio.as_ref(),
            subtitle.as_ref(),
            &self.config.borrow(),
            resume,
            self.storage_key().unwrap_or_default(),
            // Kodi's own path for the item, which is what it accepts progress
            // against. Empty when Kodi is not involved, which turns reporting
            // off rather than needing a flag of its own.
            self.kodi_item
                .borrow()
                .as_ref()
                .map(|item| item.file.clone())
                .unwrap_or_default(),
            on_ended,
        );

        match result {
            Ok(playback) => {
                // The pipeline set each sink from the configuration alone,
                // which is all it knows about. Any alignment baseline is added
                // here, once, before a frame has played.
                for role in ["primary", "secondary"] {
                    if self.baseline_ms(role) != 0.0 {
                        self.push_offset(&playback, role);
                    }
                }
                // Named by device rather than by role: "primary" and
                // "secondary" mean something to the configuration and nothing
                // to somebody trying to turn the headphones down.
                let outputs: Vec<(&'static str, String)> = {
                    let config = self.config.borrow();
                    [
                        ("primary", config.primary_sink.clone()),
                        ("secondary", config.secondary_sink.clone()),
                    ]
                    .into_iter()
                    .filter_map(|(role, name)| {
                        name.filter(|_| playback.has_output(role))
                            .map(|name| (role, name))
                    })
                    .collect()
                };
                let levels: Vec<(&str, f64, bool)> = outputs
                    .iter()
                    .map(|(role, _)| {
                        (
                            *role,
                            playback.volume(role).unwrap_or(1.0),
                            playback.muted(role),
                        )
                    })
                    .collect();

                let controls = Controls::new(
                    playback.widget(),
                    self.scale.get(),
                    self.window.is_fullscreen(),
                    self.locked_fullscreen,
                    &outputs,
                );
                controls.set_levels(&levels);
                // What the configuration holds for each output, so the panel
                // opens showing the shift already in force rather than zero.
                let syncs: Vec<(&str, f64, bool)> = {
                    let config = self.config.borrow();
                    outputs
                        .iter()
                        .map(|(role, _)| (*role, config.offset_ms(role), config.offset_on(role)))
                        .collect()
                };
                controls.set_syncs(&syncs);
                {
                    // Kept in the configuration, so a level set once holds for
                    // the next film: two outputs are rarely matched in
                    // loudness, and correcting that every time would be a
                    // chore rather than a control.
                    let app = self.clone();
                    controls.connect_volume(move |role, level, muted, persist| {
                        if let Some(playback) = app.playback.borrow().as_ref() {
                            playback.set_volume(role, level);
                            playback.set_muted(role, muted);
                        }
                        if !persist {
                            return;
                        }
                        {
                            let mut config = app.config.borrow_mut();
                            config.set_volume(role, level);
                            config.set_muted(role, muted);
                        }
                        app.save_volume_soon();
                    });

                    // Always kept, unlike a level silenced for a knock at the
                    // door: how far an output runs behind describes the
                    // equipment, not the moment.
                    let app = self.clone();
                    controls.connect_sync(move |role, ms, on| {
                        {
                            let mut config = app.config.borrow_mut();
                            config.set_offset_ms(role, ms);
                            config.set_offset_on(role, on);
                        }
                        app.push_offset_live(role);
                        app.save_volume_soon();
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_play_pause(move || {
                        if let Some(playback) = app.playback.borrow().as_ref() {
                            playback.toggle_pause();
                            app.awake.set(playback.is_playing());
                        }
                        app.wake_controls();
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_fullscreen(move || app.toggle_fullscreen());
                }
                {
                    let app = self.clone();
                    controls.connect_subtitles(move || app.toggle_subtitles());
                }
                {
                    // Under a launcher there is no menu worth returning to:
                    // something else chose this video and is waiting for the
                    // playback to end, which stopping is a way of saying.
                    let app = self.clone();
                    controls.connect_stop(move || {
                        if app.external {
                            app.finish_playback(true);
                            app.window.close();
                        } else {
                            app.leave_playback();
                        }
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_settings(move || app.leave_playback());
                }
                {
                    // The same step the arrow keys take, through the same
                    // path, so a tap of either lands in the same place.
                    let app = self.clone();
                    controls.connect_skip(move |seconds| {
                        app.scrub(seconds);
                        app.end_scrub();
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_double_click(move || app.toggle_fullscreen());
                }
                {
                    let app = self.clone();
                    controls.connect_motion(move || app.wake_controls());
                }
                {
                    // Dragging emits a value for every pointer movement, and
                    // seeking on each one asks the pipeline to decode to a
                    // position that is already out of date - which is what
                    // made dragging unusable on a Pi. Only the latest target
                    // is kept, and one timer does the work.
                    //
                    // That timer also decides when the drag is over, by asking
                    // whether the pointer button is still down. A release
                    // event is no use here: the scale claims the button
                    // sequence for its own dragging, and a claimed sequence
                    // stops reaching anything else, in any phase.
                    let app = self.clone();
                    let pending: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));
                    let running = Rc::new(Cell::new(false));
                    let scrubbing = Rc::new(Cell::new(false));

                    // The end of a drag, taken from the raw event stream. Not
                    // a gesture: GtkScale claims the button sequence while it
                    // drags, and a claimed sequence never reaches another
                    // gesture in any phase - watching for a release that way
                    // saw the press and nothing after it. Asking the pointer
                    // for its button state instead goes stale as soon as it
                    // stops moving. A legacy controller is not a gesture, so
                    // nothing can claim the event away from it.
                    {
                        let app = self.clone();
                        let pending = pending.clone();
                        let scrubbing = scrubbing.clone();
                        let watcher = gtk::EventControllerLegacy::new();
                        watcher.set_propagation_phase(gtk::PropagationPhase::Capture);
                        watcher.connect_event(move |_, event| {
                            if event.event_type() != gdk::EventType::ButtonRelease
                                || !scrubbing.replace(false)
                            {
                                return glib::Propagation::Proceed;
                            }
                            if let Some(playback) = app.playback.borrow().as_ref() {
                                if let Some(target) = pending.take() {
                                    playback.aim_at(gstreamer::ClockTime::from_nseconds(target));
                                    playback.commit_seek();
                                }
                                playback.release_from_scrub();
                            }
                            glib::Propagation::Proceed
                        });
                        self.window.add_controller(watcher);
                    }
                    controls.connect_seek(move |fraction| {
                        let playback = app.playback.borrow().clone();
                        let Some(playback) = playback else { return };
                        let Some(duration) = playback.duration() else {
                            return;
                        };

                        let target = (duration.nseconds() as f64 * fraction) as u64;
                        // Aimed at straight away, so the readout follows the
                        // pointer rather than being pulled back to where
                        // playback still is by the next tick.
                        playback.aim_at(gstreamer::ClockTime::from_nseconds(target));
                        pending.set(Some(target));
                        app.wake_controls();

                        scrubbing.set(true);
                        if running.replace(true) {
                            return;
                        }
                        // Held still while the drag lasts, so the picture stays
                        // where the pointer puts it instead of running on
                        // underneath it.
                        playback.hold_for_scrub();

                        let app = app.clone();
                        let pending = pending.clone();
                        let running = running.clone();
                        let scrubbing = scrubbing.clone();
                        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                            let playback = app.playback.borrow().clone();
                            let Some(playback) = playback else {
                                running.set(false);
                                return glib::ControlFlow::Break;
                            };
                            if let Some(target) = pending.take() {
                                playback.aim_at(gstreamer::ClockTime::from_nseconds(target));
                                playback.commit_seek();
                            }
                            if scrubbing.get() {
                                return glib::ControlFlow::Continue;
                            }
                            // Releasing has already committed the last target
                            // and let playback go.
                            running.set(false);
                            glib::ControlFlow::Break
                        });
                    });
                }
                {
                    // The mark has to follow the state however it changed:
                    // this button, the menu's, the F key, or the window
                    // manager.
                    let weak = Rc::downgrade(&controls);
                    self.window.connect_fullscreened_notify(move |window| {
                        if let Some(controls) = weak.upgrade() {
                            controls.set_fullscreen(window.is_fullscreen());
                        }
                    });
                }
                // Carried across leaving playback and coming back, since the
                // pipeline is rebuilt each time and starts with them on.
                if self.subtitles_hidden.get() && playback.subtitles_showing() {
                    playback.toggle_subtitles();
                }
                controls.set_subtitles(playback.has_subtitles(), playback.subtitles_showing());
                controls.update(&playback);
                // Where playback has reached, and nothing else. A film
                // opening with a full row of buttons over it announces the
                // interface rather than the video.
                controls.peek();
                let widget = controls.widget().clone();
                // Taken before the playback is moved into its cell, since the
                // reveal below watches it for the first frame that lands.
                let paintable = playback.widget().paintable();
                *self.controls.borrow_mut() = Some(controls);
                self.start_tick();
                self.window
                    .set_title(Some(&self.file_label().unwrap_or_default()));
                *self.playback.borrow_mut() = Some(playback);
                // Playback begins playing, so the display is held from here
                // until it is paused or torn down.
                self.awake.set(true);

                // Held back until playback has actually reached the resume
                // point. The pipeline prerolls before the seek completes, so
                // revealing it straight away shows the opening frame and then
                // jumps - which reads as a glitch rather than as resuming.
                // Everything above has already happened; only what is on
                // screen waits.
                match resume {
                    Some(target) => self.reveal_when_resumed(widget, paintable, target),
                    None => self.window.set_child(Some(&widget)),
                }
                // Nothing to move a selection through here.
                self.set_nav(None, &[], &[]);
                *self.screen.borrow_mut() = Screen::Playing;
            }
            Err(e) => self.show_error(&format!("Couldn't play that file.\n\n{e}"), false),
        }
    }

    /// Centered rather than top-aligned, and a full screen rather than a
    /// modal dialog: it has to be readable at the same distance as
    /// everything else and navigable without a pointer.
    ///
    /// Skipped when something else launched us, which closes straight away.
    /// The question guards against losing your place by accident, and there is
    /// nothing to lose here: the launcher is waiting for this process to end,
    /// and under Kodi the position has already gone back to its library.
    fn show_confirm_quit(self: &Rc<Self>) {
        if self.external {
            self.window.close();
            return;
        }

        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();

        page.append(&heading_label("Close the Player?"));

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let quit = gtk::Button::with_label("Close");
        quit.add_css_class("tp-button");
        quit.add_css_class("tp-danger");
        buttons.append(&cancel);
        buttons.append(&quit);
        page.append(&buttons);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_menu();
            });
        }
        {
            let app = self.clone();
            quit.connect_clicked(move |_| app.window.close());
        }

        // Nothing to move a selection through here.
        self.set_nav(None, &[], &[]);
        *self.screen.borrow_mut() = Screen::ConfirmQuit;
        self.window.set_child(Some(&page));
        // Cancel takes focus so a reflexive second Enter doesn't quit.
        cancel.grab_focus();
    }

    fn show_error(self: &Rc<Self>, message: &str, fatal: bool) {
        self.error_is_fatal.set(fatal);

        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(32)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Center)
            .margin_start(48)
            .margin_end(48)
            .build();

        page.append(&heading_label("Something went wrong"));

        // Given the window's width rather than a fixed column: these messages
        // carry paths and URLs, which are long, and wrapping them into a
        // narrow strip makes them harder to read than they need to be.
        let label = gtk::Label::new(Some(message));
        label.add_css_class("tp-hint");
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_justify(gtk::Justification::Center);
        label.set_hexpand(true);
        // So a path or a URL that went wrong can be copied out and pasted
        // somewhere useful, which is most of what anyone wants from an error.
        // Not focusable: the selection is for a pointer, and leaving it in the
        // focus order would put a stop between the message and the way out.
        label.set_selectable(true);
        label.set_can_focus(false);
        page.append(&label);

        // Only an unopenable video named on the command line ends the session:
        // it was the whole reason the player was started, and under a launcher
        // there is no menu behind it worth returning to.
        let back = gtk::Button::with_label(if fatal { "Close" } else { "Back" });
        back.add_css_class("tp-button");
        back.set_halign(gtk::Align::Center);
        page.append(&back);

        let app = self.clone();
        back.connect_clicked(move |_| {
            if app.error_is_fatal.get() {
                app.window.close();
            } else {
                app.show_menu();
            }
        });

        // Nothing to move a selection through here.
        self.set_nav(None, &[], &[]);
        // A path or a message that went wrong is the thing most worth copying
        // in the whole application.
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::Error;
        self.window.set_child(Some(&page));
        back.grab_focus();
    }
}

// --- Widget helpers ----------------------------------------------------

/// A screen laid out as fixed header, scrolling list, and whatever the
/// caller pins below. The list scrolls rather than the page as a whole, so
/// a long list can never push the header or a footer button off-screen.
/// Always builds the back button, even on screens that have nowhere to go
/// back to, where it's made invisible instead of omitted. Leaving it out
/// changes the header's height, which shifted the heading and the whole
/// list every time the user moved between the menu and a chooser.
fn list_page(title: &str, show_back: bool) -> (gtk::Box, gtk::ListBox, gtk::Button, gtk::Box) {
    let heading = heading_label(title);
    heading.set_xalign(0.0);
    let page = list_page_with(&heading, show_back);
    // The list carries the page's title, so arriving on one says where you
    // are before it says what row you are on. A reader gives the container's
    // name, then the position, then the row - which is the whole context in
    // one breath, and none of it read out unasked.
    name_it(&page.1, title);
    page
}

/// The same page with a heading of the caller's choosing, for the browser's
/// path trail.
fn list_page_with(
    heading: &impl IsA<gtk::Widget>,
    show_back: bool,
) -> (gtk::Box, gtk::ListBox, gtk::Button, gtk::Box) {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(40)
        .margin_bottom(40)
        .margin_start(56)
        .margin_end(56)
        .build();

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(16)
        .css_classes(["tp-header"])
        .build();
    // Its own box rather than sizing the widgets themselves: a button adds
    // padding and borders to whatever minimum it is given, so the arrow and
    // the mark never agree on a size. An empty box takes exactly the size the
    // stylesheet asks for, and the child sits centered inside it.
    let slot = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["tp-leading"])
        .build();

    let back = back_button();
    if !show_back {
        // Kept in the layout so it still occupies its space, but invisible
        // and skipped by focus.
        back.set_opacity(0.0);
        back.set_sensitive(false);
        back.set_can_focus(false);
    }
    slot.append(&back);
    header.append(&slot);

    header.append(heading);
    page.append(&header);

    let (scroller, list) = scrolling_list();
    page.append(&scroller);

    (page, list, back, slot)
}

/// The scrolling list every screen built around one shares, wired the way
/// navigation here expects to find it.
fn scrolling_list() -> (gtk::ScrolledWindow, gtk::ListBox) {
    let list = gtk::ListBox::new();
    list.add_css_class("tp-menu");
    // Browse keeps exactly one row selected as focus moves, which is what
    // the boundary checks in wire_navigation rely on.
    list.set_selection_mode(gtk::SelectionMode::Browse);
    list.set_activate_on_single_click(true);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    // Tab has to land on the list itself. The rows cannot take focus, and a
    // ScrolledWindow will take it to scroll with the arrow keys, so without
    // this the stop is the scroller and every key goes to it instead.
    scroller.set_focusable(false);
    list.set_focusable(true);
    (scroller, list)
}

/// The application mark, decoded from the PNG compiled into the binary.
///
/// A PNG rather than the SVG it was drawn from, because GStreamer's Windows
/// distribution ships no gdk-pixbuf loaders at all and so cannot decode SVG
/// at runtime. The SVG is still what Linux installs, where librsvg is present.
fn logo_image(scale: f64) -> gtk::Image {
    const LOGO: &[u8] = include_bytes!("../data/ui/tineplayer.png");

    let image = gtk::Image::new();
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(LOGO)) {
        Ok(texture) => image.set_paintable(Some(&texture)),
        Err(e) => eprintln!("Could not load the application icon: {e}"),
    }
    // Shares the back arrow's fixed slot, so the title beside it sits in the
    // same place on every screen instead of shifting as you move between
    // them. Drawn a little smaller than the slot so it cannot force it wider.
    image.set_valign(gtk::Align::Center);
    image.set_pixel_size((30.0 * scale).round() as i32);
    image
}

/// How much room the film's description may take, in interface units.
///
/// Interface units rather than pixels because that is the question actually
/// being asked. Everything on the page scales together, so what decides
/// whether the plot fits is not how many pixels tall the window is but how
/// many rows-worth of interface fit in it - and at 3x on a 1440px screen that
/// is a third of what it is at 1x on the same screen.
///
/// The reservation is what the page cannot do without: the choosers, the
/// footer that plays the film, and the margins around them. Whatever is left
/// over is what the description gets, and at 3x on a modest display that is
/// nothing - which is the right answer. A page that shows a plot summary and
/// no way to press play has its priorities backwards.
///
/// This is the plan's open question about `ui_scale` answered: no-scroll and
/// 3x cannot both hold, and what yields is the artwork and the prose.
/// A button face: a drawn mark, and the words beside it.
///
/// A box rather than a label with a mark in the text, and both halves are
/// centered on their own terms. The marks were glyphs to begin with, which
/// meant they took the label's font size and came out smaller than the type
/// they sat with; sizing them up through markup then made the whole *line*
/// taller, so the words sat on a baseline set by the mark and read as having
/// slipped downwards. An image beside a label has neither problem, and the
/// mark is drawn at whatever size suits rather than at whatever the text is.
fn marked_face(mark: gtk::Image, words: &str) -> gtk::Box {
    let face = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();

    mark.set_valign(gtk::Align::Center);
    face.append(&mark);

    if !words.is_empty() {
        let text = gtk::Label::new(Some(words));
        text.set_valign(gtk::Align::Center);
        face.append(&text);
    }
    face
}

/// How wide the media page is allowed to get, in interface units.
///
/// A ceiling rather than a shape. The reason to stop widening is that a line
/// of prose gets too long to read and a row's value drifts too far from its
/// label - both of which are about width alone, so nothing here consults the
/// height. Below this the page simply fills the window at any proportion.
///
/// **1920 is not an arbitrary round number.** The automatic interface size is
/// the display's height over 1080, so on any 16:9 screen shown fullscreen this
/// works out to `1920 * height / 1080`, which is `height * 16/9` - the screen's
/// own width. A television therefore fills edge to edge, which is what the
/// page is composed for and what the 16:9 rule this replaced did directly.
/// Anything wider than 16:9, or any window short of fullscreen, is where the
/// ceiling starts doing something, and what is left over goes to the backdrop
/// on either side.
///
/// Set at 1600 first, which quietly left a 1920px screen with 320px of
/// backdrop down the sides at fullscreen - the one case that most wants
/// filling.
const PAGE_MAX_UNITS: f64 = 1920.0;

/// How much of the page's height the poster takes.
///
/// Wider than it was, and the width is the point: the poster and the column
/// beside it share one line, so a broader poster is what sets how wide the
/// summary runs. The extra depth on both sides is what fills a 16:9 screen
/// rather than leaving a band along the bottom.
const POSTER_SHARE: f64 = 0.58;

/// The padding `.tp-selector > contents` draws around a selector's list,
/// which its own width has to account for. Kept beside the stylesheet value it
/// mirrors - `panel_pad` - because the two have to agree.
const SELECTOR_PAD: f64 = 8.0;

/// How narrow a selector is allowed to get, in interface units.
///
/// A list of short entries - "None", "Stereo", a two-word device name - would
/// otherwise open as a sliver, which reads as something gone wrong rather than
/// as a deliberately small menu.
const SELECTOR_MIN_WIDTH: f64 = 300.0;

/// How wide a selector is allowed to get before its entries ellipsize.
const SELECTOR_MAX_WIDTH: f64 = 900.0;

/// How tall a selector is allowed to get before it scrolls instead.
///
/// Not a share of the window, deliberately: a popover that fills the screen is
/// the full-screen chooser this replaces. This is roughly a dozen rows, which
/// is enough for every device list and short enough that the page it belongs
/// to is still visible around it - which is the whole reason for a popover.
const SELECTOR_HEIGHT: f64 = 520.0;

/// Three lines of summary, in interface units, reserved whether the film has
/// a summary or not.
///
/// The one fixed height on the page, and the only one that earns it: a plot
/// runs from nothing to a paragraph while everything else here is one line or
/// absent, so it is the only thing that would move the rows underneath as you
/// step from one film to the next. A film with no summary gets the space as
/// blank rather than getting it back.
const PLOT_UNITS: f64 = 90.0;

/// What stands in for a poster when there is none, which is most of the time.
///
/// A PNG per theme rather than the SVG it was drawn from, for the reason
/// [`logo_image`] gives: GStreamer's Windows distribution ships no gdk-pixbuf
/// loaders, so nothing there can decode an SVG at runtime. The two versions
/// carry the same ink as the fullscreen marks beside them.
fn video_file_image(size: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/video-file.png");

    let image = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON)) {
        image.set_paintable(Some(&texture));
    }
    // Drawn well inside the frame rather than filling it: the mark is saying
    // there is no artwork, and one that reached the edges would read as
    // artwork.
    image.set_pixel_size(size.round().max(1.0) as i32);
    image.set_halign(gtk::Align::Center);
    image.set_valign(gtk::Align::Center);
    // Expands to centre itself in the frame. The request stops at the poster
    // column, which sets its own `hexpand` explicitly.
    image.set_hexpand(true);
    image.set_vexpand(true);
    // Decoration beside a title that already names the file.
    image.set_accessible_role(gtk::AccessibleRole::Presentation);
    image
}

/// Uppercased here rather than with the `text-transform` CSS property,
/// which needs a newer GTK than this project's baseline.
/// Whether a scrap of clipboard text is worth offering as something to open.
///
/// Deliberately shallow: it is looking for a mistake worth not making, not
/// deciding whether the thing exists. A sentence someone happened to copy is
/// rejected, an address or a path is offered, and being wrong costs a
/// selected field the next keystroke replaces.
fn looks_openable(text: &str) -> bool {
    if text.is_empty() || text.lines().count() > 1 {
        return false;
    }
    text.contains("://") || text.starts_with("\\\\") || std::path::Path::new(text).is_absolute()
}

fn heading_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(&text.to_uppercase()));
    label.add_css_class("tp-title");
    label
}

/// The four-corner mark for entering or leaving fullscreen.
///
/// The subtitle mark for the control bar.
///
/// One white version rather than a light and a dark one: unlike the menus,
/// the control strip draws its own dark background whatever the theme is, so
/// there is nothing for a second version to adapt to.
pub fn subtitles_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/subtitles.png");
    marked_image(ICON, 26.0 * scale)
}

/// The mark on the button that puts an output back in sync.
///
/// Drawn rather than taken from the icon theme, for the same reason the
/// fullscreen and subtitle marks are: nothing in the theme means "line these
/// up". `emblem-synchronizing-symbolic` comes closest and is in Adwaita, but
/// GStreamer's Windows bundle ships no icon theme at all - there is only the
/// set GTK compiles into itself - and a missing icon draws nothing rather
/// than failing, which is the worst way to find out.
///
/// One version rather than a light and a dark one, like the subtitle mark:
/// the control strip draws its own dark background whatever the theme is.
/// The size of the strip's icons, before scaling: the transport buttons, the
/// gear, and the buttons in the volume panel.
pub const ICON_PX: f64 = 24.0;

pub fn sync_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/sync.png");
    marked_image(ICON, SYNC_MARK_PX * scale)
}

/// The sync mark's size before scaling, which is deliberately not [`ICON_PX`]
/// like everything else in that panel.
///
/// Larger, so that it *looks* the same size. The speaker above it is a themed
/// icon whose glyph fills its box, while this is a drawn mark with clear space
/// around it - so at the same nominal size the stopwatch came out visibly the
/// smaller of the two.
///
/// The number is arithmetic rather than taste: the mark's ink fills 83% of its
/// canvas, the speaker draws 29px, and 29 / 0.83 / 1.25 lands here. Eyeballing
/// the screenshot first gave 32, which would have overshot to 34px - so if the
/// artwork is ever redrawn with different margins, measure the ink rather than
/// nudging this by feel.
const SYNC_MARK_PX: f64 = 28.0;

/// The fullscreen mark, in the direction it will take you.
///
/// Drawn for this application rather than taken from the icon theme: the
/// bundled theme has 157 icons and none of them mean fullscreen. The nearest,
/// `window-maximize-symbolic`, is a small square that reads as "maximize".
///
/// Drawn twice in each direction, once in each theme's foreground color,
/// because an embedded image cannot be recoloured the way a symbolic icon is.
/// A single compromise gray read poorly against both.
///
/// **`dark` is about the surface, not about the theme.** The control strip
/// draws its own near-black background under either theme, so it asks for the
/// dark-theme mark always - see [`marked_image`].
pub fn fullscreen_image(fullscreen: bool, scale: f64) -> gtk::Image {
    const ENTER: &[u8] = include_bytes!("../data/ui/fullscreen.png");
    const LEAVE: &[u8] = include_bytes!("../data/ui/restore.png");

    let bytes = match fullscreen {
        true => LEAVE,
        false => ENTER,
    };
    marked_image(bytes, CORNER_MARK_PX * scale)
}

/// The gear, for the settings screen.
///
/// A pair like the fullscreen marks, and for the same reason: it sits on the
/// page under either theme. It used to be `emblem-system-symbolic` from the
/// icon theme, which GTK recolors from the foreground - including dimming it
/// when the window loses focus, while the drawn mark beside it did not, so the
/// two came apart every time the window went to the back.
/// `size` is in real pixels and is the caller's to decide, because the gear
/// appears at two sizes: beside the fullscreen mark on the media page, where
/// the two have to agree, and among the transport icons on the control strip,
/// where it has to agree with those instead.
pub fn settings_image(size: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/settings.png");

    marked_image(ICON, size)
}

/// How large the two marks in the media page's corner are drawn, before
/// scaling. One number for both, so they cannot drift apart.
const CORNER_MARK_PX: f64 = 26.0;

/// The mark beside a name in the file browser, in interface units: the height
/// of the box a file mark is drawn into.
///
/// The marks are cropped to their ink before they are bundled, which is what
/// makes one number mean the same thing for all of them. Drawn as exported
/// they carried a wide empty margin - the page shape filled 54% of its canvas
/// across and 67% down - so a size set by eye against the icons that came
/// before was mostly padding, and the marks came out small however large the
/// number grew.
/// How wide the settings screen's column of categories is, in interface
/// units. Fixed rather than sized to its contents, so the pane beside it does
/// not move when the longest category name changes.
const CATEGORY_WIDTH: f64 = 260.0;

const ROW_MARK_PX: f64 = 34.0;

/// The same, for a folder in a listing. A little smaller: a folder is a wide
/// shape where a page is a tall one, so an equal box fills more of the line
/// with ink and puts the folders ahead of the files in a list that is mostly
/// files.
const FOLDER_MARK_PX: f64 = 29.0;

/// The folder on the button that opens the system browser, which is smaller
/// again: a mark beside a line of text rather than one standing on its own.
const BUTTON_FOLDER_PX: f64 = 24.0;

/// How wide the marks' column is, whichever mark is in it. Wide enough for the
/// broadest of them with a little air, so the names line up down the list.
const MARK_COLUMN_PX: f64 = 32.0;

/// The triangle on the play button, and the arrow on restart.
///
/// White under either theme, because both sit on the blue button rather than
/// on the page. The play mark is deliberately not the one the control strip
/// uses: that one is the theme's own transport icon, and this is the button a
/// whole page is pointing at.
pub fn play_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/play.png");
    marked_image(ICON, PLAY_MARK_PX * scale)
}

pub fn restart_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/restart.png");
    marked_image(ICON, PLAY_MARK_PX * scale)
}

/// How large the marks on the play and restart buttons are drawn, before
/// scaling. Bigger than the strip's icons: these are the one action the page
/// exists to offer, and they are read from across a room.
const PLAY_MARK_PX: f64 = 26.0;

/// An image from bytes compiled into the binary, at a size in real pixels.
///
/// The size is set here rather than in the stylesheet because `-gtk-icon-size`
/// sizes icon *names*, and every mark in this application is a paintable - so
/// the CSS that catches a themed icon passes silently over these. A pixel or
/// two out and a button is a different width, which in the volume panel moves
/// the start of a bar and leaves the two bars visibly different lengths.
fn marked_image(bytes: &'static [u8], size: f64) -> gtk::Image {
    let image = gtk::Image::new();
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)) {
        Ok(texture) => image.set_paintable(Some(&texture)),
        // Said out loud: a mark that silently fails to appear looks like a
        // button with nothing on it, which is not a clue anyone can act on.
        Err(e) => eprintln!("Could not load an interface mark: {e}"),
    }
    image.set_pixel_size(size.round().max(1.0) as i32);
    image
}

/// Publishes the current row as the list's `active-descendant`.
///
/// **This is not what makes a list audible.** Rows take focus and the focus
/// moves with the selection, and a screen reader speaks on focus changes and
/// on nothing else. Verified 2026-08-05 against Windows UI Automation:
/// stepping down the settings list moves the focused element from one
/// `ListItem` to the next, each named with its full row text.
///
/// Kept because the relation is correct by the specification and costs
/// nothing, but nothing should be built on it announcing anything. Publishing
/// the current item as state alone was tried twice - selection on the rows,
/// then this relation - and both were silent in practice.
///
/// Hung off `row-selected` rather than off the places that select, because
/// there are many of those - arrow keys, the gamepad, page keys, a pointer,
/// and every screen that opens on a remembered row - and one signal catches
/// them all.
fn announce_selection(list: &gtk::ListBox) {
    list.connect_row_selected(|list, row| match row {
        Some(row) => {
            list.update_relation(&[gtk::accessible::Relation::ActiveDescendant(
                row.upcast_ref(),
            )]);
        }
        None => list.reset_relation(gtk::AccessibleRelation::ActiveDescendant),
    });
}

/// Appends a row to a list and gives it a name.
///
/// The name goes on the row GTK wraps around the widget, not on the labels
/// inside it, because GTK derives a name from a child label but not from a
/// grandchild. A row built as a box of two labels therefore had no name, and
/// a screen reader announced it as "3 of 6" and nothing more.
fn append_named(list: &gtk::ListBox, child: &impl IsA<gtk::Widget>, name: &str) {
    list.append(child);
    if let Some(row) = child.as_ref().parent().and_downcast::<gtk::ListBoxRow>() {
        name_it(&row, name);
        // The list is one stop in the tab order, not one per row. A folder of
        // two hundred files is otherwise two hundred presses between you and
        // the button below it, which is the difference between usable and
        // not for anyone who navigates by Tab.
        //
        // Rows stay focusable all the same, and the focus follows the
        // selection. Making them unfocusable was the obvious way to get one
        // tab stop and it silenced the screen reader completely: focus
        // arrived at the list and never moved again, so Narrator read the
        // list and its first row and then nothing, however far down somebody
        // travelled. Selection alone is not enough - checked against Windows
        // UI Automation, which showed `IsSelected` moving correctly from row
        // to row while the focused element stayed the list throughout, and a
        // screen reader speaks on focus.
        //
        // One tab stop comes from `move_focus_stop` instead, which finds the
        // stop containing the focus and steps to the next one, so a focused
        // row still counts as being on its list.
    }
}

fn row_name(label: &str, value: &str) -> String {
    if value.is_empty() {
        label.to_string()
    } else {
        format!("{label}, {value}")
    }
}

/// Gives a control a name for anyone who cannot see the picture on it. The
/// same reasoning as the copy in `controls`, which names the playback strip.
fn name_it(widget: &impl IsA<gtk::Accessible>, name: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(name)]);
}

fn back_button() -> gtk::Button {
    // An icon rather than a text glyph: a "‹" character sits off the
    // vertical center because it's positioned by font metrics rather than
    // by the icon's own bounding box.
    let button = gtk::Button::from_icon_name("go-previous-symbolic");
    button.add_css_class("tp-back");
    name_it(&button, "Back");
    button.set_valign(gtk::Align::Center);
    button
}

/// How a level reads in the settings menu. A silenced output says so rather
/// than showing the level it will return to, which is what the panel during
/// playback does too.
pub fn volume_label(level: f64, muted: bool) -> String {
    if muted {
        "Muted".to_string()
    } else {
        format!("{}%", (level * 100.0).round() as u32)
    }
}

/// The go-ahead button on a dialog: what it says, and whether pressing it
/// destroys something.
///
/// The two travel together because the second decides which button wears the
/// warning colour, and answering one without the other is what produced a red
/// Cancel sitting beside a plain Remove.
struct Confirm<'a> {
    label: &'a str,
    destructive: bool,
}

/// What the Kodi wizard has been told so far.
///
/// Every field is optional because the wizard fills them in one screen at a
/// time, and none of it has been written to Kodi: dropping this is what
/// Cancel does, and it costs nothing.
#[derive(Default)]
struct KodiDraft {
    userdata: Option<std::path::PathBuf>,
    want: Option<crate::kodi_setup::Registration>,
    /// Whether Kodi's hand-off should start the film rather than open the
    /// menu. Written as `--play` in Kodi's arguments.
    play: bool,
    /// The exact file a backup would be copied to, settled when the summary
    /// names it so that what was promised is what gets written.
    backup_to: Option<std::path::PathBuf>,
}

/// A wizard panel: a heading, then whatever the step needs, centered over the
/// screen it was opened from.
fn wizard_page(title: &str) -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .valign(gtk::Align::Center)
        .margin_top(40)
        .margin_bottom(40)
        .margin_start(56)
        .margin_end(56)
        .build();
    let heading = heading_label(title);
    heading.set_halign(gtk::Align::Center);
    // halign centers the label in the panel; justify centers the lines within
    // the label. Without it a heading that wraps, or one written across two
    // lines, sits centered as a block with its second line ragged left.
    heading.set_justify(gtk::Justification::Center);
    page.append(&heading);
    page
}

/// A line of explanation on a wizard panel. Selectable, so a command or a
/// path can be copied out with Ctrl+C, but never focusable: these are read,
/// not operated.
fn wizard_text(text: &str, command: bool) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(if command {
            gtk::pango::WrapMode::Char
        } else {
            gtk::pango::WrapMode::WordChar
        })
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .css_classes([if command { "tp-path" } else { "tp-hint" }])
        .build();
    label.set_selectable(true);
    label.set_can_focus(false);
    label
}

/// A settings row carrying a switch rather than the word "On" or "Yes".
///
/// The switch is a readout, not a control: it cannot be clicked or focused,
/// and the row it sits in is what gets activated. That keeps one way of
/// working the menu - move to a row, press it - rather than a second target
/// inside the row that only a pointer could reach.
fn switch_row(label: &str, on: bool) -> (gtk::Box, gtk::Switch) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    row.append(&name);

    let switch = gtk::Switch::new();
    switch.set_active(on);
    // A switch already reports whether it is on; without a name it reports
    // that about nothing in particular.
    name_it(&switch, label);
    switch.set_can_focus(false);
    switch.set_valign(gtk::Align::Center);
    row.append(&switch);

    (row, switch)
}

/// A settings row carrying a slider rather than a value and a chevron.
///
/// A level is a quantity, not a choice from a list, and a list of ten
/// percentages was a menu pretending to be a dial. Left and right move it
/// where they would otherwise do nothing on this screen, and the row keeps
/// the reading beside it so it can be set without looking at the bar.
/// A row with a bar, its reading, and for the ones that can be turned off, a
/// switch beyond it.
///
/// The switch rather than a value of its own: muted is not a quieter level
/// and an unapplied delay is not a shorter one, so both are a second thing
/// about the row, and the bar keeps saying what it will be when it is back
/// on.
fn slider_row(
    label: &str,
    width: i32,
    range: std::ops::RangeInclusive<f64>,
    now: f64,
    reading: &str,
    toggle: Option<bool>,
) -> (gtk::Box, gtk::Scale, gtk::Label, Option<gtk::Switch>) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    // The label takes the slack instead of the slider, which keeps the bar
    // over on the right where every other row shows its value. A bar the
    // width of the screen also reads as far more precision than a level has.
    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    row.append(&name);

    let scale = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        *range.start(),
        *range.end(),
        1.0,
    );
    scale.set_draw_value(false);
    scale.set_size_request(width, -1);
    scale.set_can_focus(false);
    scale.set_value(now);
    scale.add_css_class("tp-progress");
    // Settings bars only. The same class draws the video timeline and the
    // bars in the volume panel, which sit over a picture rather than on a
    // page of rows and are not the ones that disappear into the background.
    scale.add_css_class("tp-bar");
    name_it(&scale, label);
    row.append(&scale);

    // Wide enough for the longest reading any slider shows, so the bar beside
    // it never shifts as the value changes. `set_width_chars` is a minimum
    // rather than a maximum, so a reading longer than this would still push
    // the bar - which is what made the sync slider jump under the pointer
    // while it was being dragged, since "In sync" and "1000 ms earlier" are
    // eight characters apart.
    //
    // The same width for every slider rather than one each. A per-row width
    // would leave the bars ending at different places down the column, which
    // is worse to look at than the whitespace a short reading leaves here.
    let value = gtk::Label::new(Some(reading));
    value.add_css_class("tp-value");
    value.set_xalign(1.0);
    value.set_width_chars(READING_CHARS);
    row.append(&value);

    // The wheel scrolls the list it is in, rather than moving the bar under
    // the pointer. A settings screen is a list first: passing over a slider
    // on the way down it should not change a setting, and the value that
    // changes is the one nobody was looking at.
    //
    // Taken in the capture phase so the bar never sees it, and passed on to
    // the scroller by hand, since stopping the event stops it reaching the
    // list as well.
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    wheel.connect_scroll(|controller, _, down| {
        let Some(scroller) = controller
            .widget()
            .and_then(|widget| widget.ancestor(gtk::ScrolledWindow::static_type()))
            .and_downcast::<gtk::ScrolledWindow>()
        else {
            return glib::Propagation::Stop;
        };
        let adjustment = scroller.vadjustment();
        // A row at a time, near enough: the step increment on a list is the
        // height of what it holds, and a tenth of a page where it is not set.
        let step = if adjustment.step_increment() > 0.0 {
            adjustment.step_increment()
        } else {
            adjustment.page_size() / 10.0
        };
        let wanted = adjustment.value() + down * step;
        adjustment.set_value(wanted.clamp(
            adjustment.lower(),
            (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
        ));
        glib::Propagation::Stop
    });
    scale.add_controller(wheel);

    let toggle = toggle.map(|on| {
        let switch = gtk::Switch::new();
        switch.set_active(on);
        name_it(&switch, label);
        switch.set_can_focus(false);
        switch.set_valign(gtk::Align::Center);
        row.append(&switch);
        // A bar that cannot be moved says so, rather than being moved to no
        // effect and leaving somebody to work out why nothing changed.
        scale.set_sensitive(on);
        value.set_sensitive(on);
        switch
    });

    (row, scale, value, toggle)
}

/// One piece of the notices page.
enum Notice {
    Heading(String),
    Text(String),
}

/// Turns THIRD-PARTY.md into something worth reading on a screen.
///
/// Not a Markdown renderer, and it does not need to be: the file is headings,
/// paragraphs and tables, and only the tables need doing anything to. A row of
/// pipes reads as punctuation rather than as a list, so the cells are joined
/// with a dash - `serde - 1.0.229 - MIT OR Apache-2.0` - and the rule under
/// each header is dropped, having nothing to say without the pipes around it.
///
/// Paragraphs are gathered rather than emitted line by line, so that text
/// wrapped at eighty columns in the file wraps to the window here instead.
fn notices_blocks(source: &str) -> Vec<Notice> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();

    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Notice>| {
        if !paragraph.is_empty() {
            blocks.push(Notice::Text(paragraph.join(" ")));
            paragraph.clear();
        }
    };

    for line in source.lines() {
        let line = line.trim();
        // The rule under a table header, which is pipes and dashes and no
        // words at all.
        if line.starts_with('|') && line.trim_matches(['|', '-', ':', ' ']).is_empty() {
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ").or_else(|| line.strip_prefix("# ")) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Notice::Heading(heading.to_string()));
        } else if let Some(row) = line.strip_prefix('|') {
            // A table row stands alone rather than joining the paragraph
            // around it: two hundred crates read as a list, not as prose.
            flush(&mut paragraph, &mut blocks);
            let cells: Vec<&str> = row
                .trim_end_matches('|')
                .split('|')
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .collect();
            blocks.push(Notice::Text(cells.join("  -  ")));
        } else if line.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else {
            // Markdown decoration that would otherwise be read aloud as
            // punctuation, and the note marker, which is a label for a
            // renderer rather than words for a reader.
            let text = line
                .trim_start_matches("> ")
                .trim_start_matches('>')
                .trim_start_matches("- ")
                .replace("**", "")
                .replace('`', "");
            if text.trim() == "[!NOTE]" {
                continue;
            }
            paragraph.push(text.trim().to_string());
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// A page of prose rather than of rows, for the one screen that is read
/// instead of navigated.
fn text_page(title: &str) -> (gtk::Box, gtk::ScrolledWindow, gtk::Box, gtk::Button) {
    let (page, list, back, _slot) = list_page(title, true);
    // The list that came with the page is not wanted here, but the header,
    // the back button and the margins are: taking the page apart is less
    // duplication than building a second one that has to be kept in step.
    if let Some(scroller) = page
        .last_child()
        .and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok())
    {
        list.unparent();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .build();
        scroller.set_child(Some(&body));
        return (page, scroller, body, back);
    }
    unreachable!("list_page always ends in its scroller");
}

/// A heading within a page of prose. Named rather than styled inline so the
/// About page reads as a document rather than as a form.
fn about_heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-about-heading");
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_selectable(true);
    label.set_can_focus(false);
    label
}

/// Where the address sits in relation to the sentence introducing it.
enum Address {
    /// Finishing the sentence, for one short enough to take in at a glance.
    Inline,
    /// On a line of its own. A long address is read character by character,
    /// and one wrapped mid-way through a paragraph is hard to pick back out
    /// of it.
    OwnLine,
}

/// A line ending in a link that opens in the machine's browser. The address
/// is shown as written rather than hidden behind words, since on a screen
/// nobody can click there is still a use in being able to read it out.
fn about_link(lead: &str, href: &str, shown: &str, place: Address) -> gtk::Label {
    let label = about_text("");
    let separator = match place {
        Address::Inline => " ",
        Address::OwnLine => "\n",
    };
    label.set_markup(&format!(
        "{}{separator}<a href=\"{}\">{}</a>",
        glib::markup_escape_text(lead),
        glib::markup_escape_text(href),
        glib::markup_escape_text(shown),
    ));
    label
}

fn about_text(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-about");
    label.set_xalign(0.0);
    label.set_wrap(true);
    // Selectable so a path or a version can be copied out rather than
    // transcribed, but never focusable: GTK gives a selectable label focus by
    // default, which would put a caret in the middle of a page navigated by
    // arrow keys.
    label.set_selectable(true);
    label.set_can_focus(false);
    // Long enough to read as paragraphs, short enough that the eye finds the
    // next line: a line the width of a television is unreadable.
    label.set_max_width_chars(72);
    label
}

/// Puts a list on a row, and scrolls it there once there is a page to scroll.
///
/// Focus alone is not enough. A screen is built and handed to the window in
/// one go, so at the moment a row is focused nothing has been laid out yet:
/// the scroller has no height, the row has no position, and the scroll that
/// would have followed the focus has nowhere to go. Coming back to a screen
/// therefore landed at the top of it, however far down you had been.
///
/// So the scroll is done by hand, and not until the row has been mapped -
/// which is the point at which it knows where it is.
fn settle_on(row: &gtk::ListBoxRow) {
    let ticket = claim_settling();
    // The row itself, so a screen reader has a focus change to announce.
    row.grab_focus();
    // Setting the window's child maps the new page there and then, so by the
    // time a screen picks its row the row is usually mapped already and
    // waiting for the signal would be waiting forever. Only the first screen
    // of a session arrives unmapped, because the window itself is not up yet.
    if row.is_mapped() {
        after_layout(row, ticket);
    } else {
        row.connect_map(move |row| after_layout(row, ticket));
    }
}

thread_local! {
    /// Which settling is the current one. See [`claim_settling`].
    static SETTLING: Cell<u64> = const { Cell::new(0) };
}

/// Claims the right to be the row the deferred work below settles on, and
/// supersedes whatever claimed it before.
///
/// Settling a row is not finished when [`settle_on`] returns: where the row is
/// and how far the scroller can travel are only known after a layout pass, so
/// the last of it waits for an idle. Nothing about that idle was tied to the
/// row still being the one wanted, and holding an arrow key queues one per
/// press - so the earlier ones came due against rows already left behind, and
/// scrolled them back into view. Arrowing quickly down a list threw the page
/// back to wherever the cursor had been a few presses ago.
///
/// A ticket rather than cancelling the pending idle: several things settle
/// rows - the arrow keys, a screen being built, a popover opening over one -
/// and none of them knows about the others. Each takes the next number, and
/// deferred work runs only while its number is still the current one, so the
/// most recent claim always wins without anybody having to be told.
///
/// This does not replace [`focus_is_outside`], which covers what a ticket
/// cannot: moving from the top row up to a header button focuses the button
/// without settling anything, so no new ticket is taken and only the focus
/// check sees that the row is no longer where the viewer is.
fn claim_settling() -> u64 {
    SETTLING.with(|settling| {
        let ticket = settling.get().wrapping_add(1);
        settling.set(ticket);
        ticket
    })
}

/// Whether this settling is still the one in force.
fn settling_is_current(ticket: u64) -> bool {
    SETTLING.with(|settling| settling.get() == ticket)
}

/// Runs once the page has been through a layout pass, which is when a row
/// finally knows where it is and the scroller knows how much of it there is
/// to move.
fn after_layout(row: &gtk::ListBoxRow, ticket: u64) {
    let row = row.clone();
    glib::idle_add_local_once(move || {
        // Only while this is still the row being settled on. Anything settled
        // since - the next row under a held arrow key, another screen, a
        // popover opening - has taken a later ticket and this one is stale.
        if !settling_is_current(ticket) {
            return;
        }
        // Only if the focus is still in this list. The grab below is a second
        // attempt, for the one case where the first one was too early to take
        // - and a second attempt that runs unconditionally is a second attempt
        // at stealing the focus back from wherever it has since gone.
        //
        // Arrowing up off the top row twice in quick succession did exactly
        // that: the first press selected the top row and queued this, the
        // second moved out to the Play button, and then this fired and pulled
        // the focus back down to the row. Slowly it was fine, because the idle
        // had already run before the second press arrived - which is the shape
        // of every bug that only happens when you are not being careful.
        if focus_is_outside(&row) {
            return;
        }
        // Only if it has not already taken. A focus grab inside a scroller
        // makes GTK scroll the row into view, so repeating one that already
        // succeeded sets a second scroll going against the one below.
        if !row.has_focus() {
            row.grab_focus();
        }
        show_row(&row);
    });
}

/// Whether the focus has left the list this row belongs to.
///
/// Nothing focused at all counts as inside: that is the state on the very
/// first screen of a session, before anything has taken focus, and it is
/// precisely when the deferred grab is needed.
fn focus_is_outside(row: &gtk::ListBoxRow) -> bool {
    let (Some(root), Some(list)) = (row.root(), row.parent()) else {
        return false;
    };
    match root.focus() {
        Some(focused) => focused != list && !focused.is_ancestor(&list),
        None => false,
    }
}

/// Moves the scroller so a row is fully on screen, by the smallest amount
/// that does it.
///
/// The minimum on purpose, and it used to place the row a third of the way
/// down the frame instead - which looks better in isolation and is the wrong
/// rule here, because this is not the only thing scrolling. Focusing a row
/// inside a scroller makes GTK bring it into view too, by the smallest amount.
/// Two rules that disagree about where a row belongs produce whichever answer
/// ran last: arrowing down kept the row at the bottom edge on the presses
/// where GTK's scroll had already satisfied this one, and threw the row up
/// near the top on the presses where it had not. Nothing about the input
/// differed, so it read as random.
///
/// Agreeing with GTK is what makes it predictable, and it is also the better
/// behaviour while arrowing: the row stays where it is and the list moves one
/// row under it, rather than the page jumping every time the edge is reached.
fn show_row(row: &gtk::ListBoxRow) {
    let Some(list) = row.parent() else { return };
    let mut ancestor = list.parent();
    let scroller = loop {
        match ancestor {
            Some(widget) => match widget.downcast::<gtk::ScrolledWindow>() {
                Ok(scroller) => break scroller,
                Err(widget) => ancestor = widget.parent(),
            },
            None => return,
        }
    };

    // The row's own allocation inside the list, which is where it sits in the
    // content and does not move when the content is scrolled.
    //
    // Asked of the widget tree with `translate_coordinates` before, which
    // looks equivalent and is not. The step above this one grabs the row's
    // focus, and GTK answers a focus grab inside a scroller by scrolling the
    // row into view itself - moving the adjustment and re-allocating the list
    // underneath us. `translate_coordinates` then reported whichever
    // allocation happened to be current: the row's place in the list on one
    // press, its place on screen on the next.
    //
    // On screen it is always the same place, hard against the bottom edge, so
    // every other press computed the same destination near the top of the list
    // and jumped there - and the two answers diverged further the further down
    // the list you had gone.
    let top = f64::from(row.allocation().y());
    let adjustment = scroller.vadjustment();
    let page = adjustment.page_size();
    // Already on screen: leave it where it is rather than jumping the page
    // about under someone who can see the row perfectly well.
    let value = adjustment.value();
    let bottom = top + f64::from(row.height());
    let wanted = if top < value {
        // Off the top: bring its top edge to the top of the frame.
        top
    } else if bottom > value + page {
        // Off the bottom: bring its bottom edge to the bottom of the frame.
        bottom - page
    } else {
        return;
    };
    adjustment.set_value(wanted.clamp(adjustment.lower(), (adjustment.upper() - page).max(0.0)));
}

/// Where a stored language code sits in the offered list.
fn language_position(code: Option<&str>) -> Option<usize> {
    let code = code?;
    crate::languages::LANGUAGES
        .iter()
        .position(|(stored, _, _, _)| *stored == code)
}

/// As far down the About page as it goes, which is the top of the last
/// screenful rather than the bottom of the text.
fn about_bottom(adjustment: &gtk::Adjustment) -> f64 {
    (adjustment.upper() - adjustment.page_size()).max(adjustment.lower())
}

/// Binds an action to each of `keys` under every modifier this platform
/// answers a shortcut on.
///
/// `<Primary>` everywhere, which is Control on all three platforms, plus
/// Command on macOS - where `<Primary>` is emphatically not it. See
/// `install_accelerators` for how that was measured.
fn bind_accels(gtk_app: &gtk::Application, action: &str, keys: &[&str]) {
    let mut accels = Vec::new();
    for key in keys {
        accels.push(format!("<Primary>{key}"));
        if cfg!(target_os = "macos") {
            accels.push(format!("<Meta>{key}"));
        }
    }
    let accels: Vec<&str> = accels.iter().map(String::as_str).collect();
    gtk_app.set_accels_for_action(action, &accels);
}

/// The modifiers a shortcut may be pressed with here, as one mask to test a
/// key event against.
///
/// `<Primary>` is asked of GTK rather than written out per platform, so the
/// keys matched by hand cannot drift from the ones bound as accelerators.
/// Command is added on macOS for the same reason it is bound there, and it is
/// why this is tested with `intersects` rather than `contains`: the mask holds
/// two modifiers on that platform and either one alone means yes.
fn primary_mask() -> gdk::ModifierType {
    let mut mask = gtk::accelerator_parse("<Primary>a")
        .map(|(_, mask)| mask)
        .unwrap_or(gdk::ModifierType::CONTROL_MASK);
    if cfg!(target_os = "macos") {
        mask |= gdk::ModifierType::META_MASK;
    }
    mask
}

fn last_row_index(list: &gtk::ListBox) -> i32 {
    let mut last = 0;
    while list.row_at_index(last + 1).is_some() {
        last += 1;
    }
    last
}

/// How a subtitle reads in a list.
///
/// The label of anything found beside the video is a language tag, written the
/// way the convention writes it - "en", "en.hi", "pt-BR" - and is put into
/// words. A file chosen by hand is labelled with its own name, which is not a
/// tag and would come out mangled if it were read as one.
fn subtitle_label(option: &Subtitle) -> String {
    match option {
        Subtitle::File { label, .. } => label.clone(),
        other => crate::languages::describe_tag(other.label()),
    }
}

fn describe_audio_track(track: &AudioTrack) -> String {
    // Checked against the title, which is where a language most often gets
    // named twice: a track tagged `eng` and titled "English Commentary" needs
    // no help, and would otherwise read "eng (English) - ... - English
    // Commentary".
    let mut text = format!(
        "{} — {} {}ch",
        crate::languages::describe_tag_unless(&track.language, &track.title),
        track.codec,
        track.channels
    );
    if !track.title.is_empty() {
        text.push_str(&format!(" — {}", track.title));
    }
    text
}

/// A stored alignment as a statement rather than as a signed number.
///
/// Which way the audio runs is the whole of what it says, and "+830ms" does
/// not say it. This is read by someone checking a correction they cannot see
/// the effect of, so it has to be unambiguous without a convention to look up.
fn describe_lateness(millis: f64) -> String {
    let rounded = millis.round();
    if rounded > 0.0 {
        format!("Audio {rounded:.0}ms late")
    } else if rounded < 0.0 {
        format!("Audio {:.0}ms early", -rounded)
    } else {
        "In step".to_string()
    }
}

/// A menu row: what the setting is on the left, its current value and a
/// chevron on the right.
/// The heading that opens a group of rows: which output the three rows under
/// it belong to.
///
/// A `GtkListBox` header rather than a row, which is what makes it
/// unselectable for free - headers sit outside the selection model and outside
/// the focus chain, so the arrow keys walk past without being told to.
///
/// Capitals with a little tracking, in the manner of a section label rather
/// than a title: it has to be legible enough to group what is under it and
/// quiet enough that the rows stay the thing being read. The tracking is a
/// Pango attribute rather than CSS `letter-spacing`, which GTK's stylesheet
/// parser accepts and does not apply.
fn group_heading(title: &str, scale: f64, first: bool) -> gtk::Label {
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.add_css_class("tp-group");
    // Nothing above the first heading. It opens the list rather than dividing
    // it, and the buttons already sit above with room of their own.
    if first {
        heading.add_css_class("tp-group-first");
    }
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrInt::new_letter_spacing(
        (1.5 * scale * gtk::pango::SCALE as f64) as i32,
    ));
    heading.set_attributes(Some(&attributes));
    heading
}

/// A heading's capitals turned back into words, for a screen reader.
///
/// "FIRST OUTPUT" read literally is a risk of being spelled out a letter at a
/// time, which is a real behaviour of several readers on all-capital text.
fn title_case(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn menu_row(label: &str, value: &str, enabled: bool) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    row.append(&name);

    let value_label = gtk::Label::new(Some(value));
    value_label.add_css_class("tp-value");
    value_label.set_hexpand(true);
    value_label.set_xalign(1.0);
    value_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&value_label);

    let chevron = gtk::Label::new(Some("›"));
    chevron.add_css_class("tp-chevron");
    row.append(&chevron);

    row.set_sensitive(enabled);
    row
}

/// What a browsing screen is for: opening a video, or choosing a folder.
///
/// The two screens differ in what they list, what the footer holds and what a
/// row does. Everything else - the trail, the places column, the system
/// browser - is the same, and used to be written twice.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Browse {
    Videos,
    /// A separate soundtrack for the video already chosen: the same screen,
    /// listing audio files instead.
    Audio,
    /// A subtitle file from somewhere other than beside the video.
    Subtitles,
    Folders,
}

impl Browse {
    /// Whether only folders are worth showing. A folder is being chosen here,
    /// so the files inside it would be a list of things that cannot be picked.
    fn folders_only(self) -> bool {
        self == Browse::Folders
    }

    fn wants(self) -> crate::browser::Kind {
        match self {
            Browse::Audio => crate::browser::Kind::Audio,
            Browse::Subtitles => crate::browser::Kind::Subtitle,
            _ => crate::browser::Kind::Video,
        }
    }
}

/// The parts of a browsing screen its caller still has to finish.
struct BrowserPage {
    page: gtk::Box,
    list: gtk::ListBox,
    crumbs: Vec<gtk::Button>,
    browse: gtk::Button,
    open: gtk::Button,
    cancel: gtk::Button,
}

/// One row of a listing: what it says, what it is drawn with, where it goes,
/// and how it reads aloud. A path of `None` is the way up.
#[derive(Clone)]
struct BrowserEntry {
    /// Whether the Open button acts on this row: a file, rather than a folder,
    /// the way up, or a notice.
    openable: bool,
    label: String,
    icon: RowIcon,
    path: Option<std::path::PathBuf>,
    spoken: String,
    /// Something to read rather than somewhere to go: the line saying a
    /// folder holds nothing worth listing.
    notice: bool,
}

/// What sits behind a modal opened before there is a screen to sit behind it.
///
/// Blank on purpose. The alternative - building a menu page to stand in for
/// the real one - draws a screen nobody navigated to, which is worse than an
/// empty background because it looks like somewhere you could go back to.
fn empty_backdrop() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, 0)
}

/// Fills a listing, and leaves the notice as a line of text.
///
/// A notice drawn like an entry invites being chosen, and choosing it walked
/// back up a level - which reads as a broken listing rather than as an empty
/// folder. Centred, dimmer, without an icon, and passed over by the cursor.
fn fill_browser_list(list: &gtk::ListBox, entries: &[BrowserEntry], scale: f64) {
    for entry in entries {
        if entry.notice {
            let label = gtk::Label::new(Some(&entry.label));
            label.add_css_class("tp-row");
            label.add_css_class("tp-hint");
            label.set_xalign(0.5);
            append_named(list, &label, &entry.spoken);
            if let Some(row) = label.parent().and_downcast::<gtk::ListBoxRow>() {
                row.set_selectable(false);
                row.set_activatable(false);
            }
        } else {
            append_named(
                list,
                &browser_row(entry.icon, &entry.label, scale),
                &entry.spoken,
            );
        }
    }
}

/// Opens a system dialog where the built-in browser already is.
///
/// Best effort: a folder that has since been unplugged or removed leaves the
/// dialog wherever it would have opened anyway, which is better than refusing
/// to open at all.
fn open_at(chooser: &gtk::FileChooserNative, start: &std::path::Path) {
    if start.is_dir() {
        let _ = chooser.set_current_folder(Some(&gtk::gio::File::for_path(start)));
    }
}

/// What a folder shows in a given mode: the way up, then what is inside.
fn browser_entries(directory: &std::path::Path, mode: Browse) -> Vec<BrowserEntry> {
    let mut entries = Vec::new();
    if let Some(parent) = directory.parent() {
        // Two dots rather than the word: it is what a file listing has always
        // called the folder above, and it needs no translating. Read aloud it
        // is punctuation and says nothing, so the spoken name says where it
        // goes instead.
        entries.push(BrowserEntry {
            openable: false,
            label: "..".to_string(),
            icon: RowIcon::Folder,
            path: None,
            spoken: match parent.file_name() {
                Some(name) => format!("Up to {}", name.to_string_lossy()),
                None => "Up to the list of drives".to_string(),
            },
            notice: false,
        });
    }
    for entry in crate::browser::read(directory, mode.wants()) {
        if mode.folders_only() && !entry.is_dir {
            continue;
        }
        // Which mark a file gets follows what this screen is for: the same
        // file is a video when a video is being chosen and a soundtrack when
        // one is. Nothing here inspects the file itself, which would mean
        // opening every one in the folder to draw a list.
        let icon = match (entry.is_dir, mode) {
            (true, _) => RowIcon::Folder,
            (false, Browse::Audio) => RowIcon::Audio,
            (false, Browse::Subtitles) => RowIcon::Subtitle,
            (false, _) => RowIcon::Video,
        };
        entries.push(BrowserEntry {
            openable: !entry.is_dir,
            label: entry.label.clone(),
            icon,
            path: Some(entry.path),
            spoken: entry.label,
            notice: false,
        });
    }
    // Only where the listing is what you came for. A folder with nothing to
    // play in it is worth saying, since the alternative reads as a folder
    // that failed to load; a folder with no folders under it is not empty at
    // all - it is full of files this screen has no reason to show, and
    // calling it empty would be wrong.
    //
    // Counting the way up as nothing, since it fills the list on its own and
    // is why this never appeared before.
    if mode == Browse::Videos && entries.iter().all(|entry| entry.path.is_none()) {
        entries.push(BrowserEntry {
            openable: false,
            label: "Nothing here".to_string(),
            icon: RowIcon::None,
            path: None,
            spoken: "Nothing here".to_string(),
            notice: true,
        });
    }
    entries
}

/// What a browser row draws beside its name.
///
/// The file marks are bundled rather than named from the desktop's icon set.
/// The set is what a row used to ask for, and the theme decides what turns up:
/// a generic video icon is absent from the Pi's theme entirely and fell back
/// to the missing-image glyph, which reads as a warning about the file. These
/// three are the same on every machine.
///
/// The folder is bundled with them. It could have stayed the theme's - a
/// folder is the one icon every theme has - but then one mark in a column of
/// four would be drawn in somebody else's hand, and which one would depend on
/// the machine.
#[derive(Clone, Copy, PartialEq)]
enum RowIcon {
    Folder,
    Video,
    Audio,
    Subtitle,
    /// A notice rather than a file - "Nothing here" - which draws no mark.
    None,
}

impl RowIcon {
    /// The mark at the size a listing draws it.
    fn image(self, scale: f64) -> gtk::Image {
        let size = match self {
            Self::Folder => FOLDER_MARK_PX,
            _ => ROW_MARK_PX,
        };
        self.image_at(size, scale)
    }

    /// The mark at a size of the caller's choosing, for the places that are
    /// not a row in a listing.
    fn image_at(self, size: f64, scale: f64) -> gtk::Image {
        const VIDEO: &[u8] = include_bytes!("../data/ui/file-video.png");
        const AUDIO: &[u8] = include_bytes!("../data/ui/file-audio.png");
        const SUBTITLE: &[u8] = include_bytes!("../data/ui/file-subtitle.png");
        const FOLDER: &[u8] = include_bytes!("../data/ui/folder.png");

        let bytes = match self {
            Self::Video => VIDEO,
            Self::Audio => AUDIO,
            Self::Subtitle => SUBTITLE,
            Self::Folder => FOLDER,
            Self::None => return gtk::Image::new(),
        };
        marked_image(bytes, size * scale)
    }
}

/// A browser row: a mark, then the name.
///
/// Icons rather than emoji, because emoji depend on a color font being
/// installed. The Pi has none, so a folder character rendered as an empty box
/// with the codepoint inside it.
fn browser_row(icon: RowIcon, text: &str, scale: f64) -> gtk::Box {
    // The padding goes on the row rather than the label, so it applies
    // before the icon as well as around the text.
    //
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["tp-row"])
        .build();

    let image = icon.image(scale);
    image.add_css_class("tp-row-icon");
    // A column of its own, the same width whatever is drawn in it, so every
    // name in the list starts at the same place. The marks are cropped to
    // their ink and no two are the same shape - a page is tall and narrow, a
    // folder wide - so left to size themselves the folder rows and the file
    // rows put their names a couple of pixels apart, which is the sort of
    // thing that reads as sloppiness without being obvious enough to name.
    image.set_size_request((MARK_COLUMN_PX * scale).round() as i32, -1);
    image.set_halign(gtk::Align::Center);
    row.append(&image);

    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    row
}

/// Lines a selector's right edge up with the right edge of the row that opened
/// it, leaving the vertical placement to GTK.
///
/// GTK positions a popover by centering it on a rectangle you nominate, in the
/// parent's coordinates. Here that is a one-pixel sliver at
/// `row_width - popover_width / 2`, so centering the popover on it lands the
/// two right edges together. The entries inside are right-aligned because they
/// are alternatives to the value on the right of the row, and a centered
/// popover would sit just left of it - close enough to read as a mistake
/// rather than a margin.
///
/// The rectangle spans the row's full height, which is GTK's own default and
/// gives its ordinary vertical behaviour: below the row when there is room,
/// flipped above it when there is not.
///
/// Aligning an edge of the popover to an edge of the row was tried and taken
/// out again. It is possible - a zero-height rectangle at `y` puts the
/// popover's near edge on that line - but it requires predicting which way GTK
/// will open, and the popover then covers the row it belongs to, which leaves
/// a choice sitting under the pointer where the row used to be. Clicking again
/// to dismiss picks that choice instead. macOS avoids this by aligning the
/// *selected* entry to the row rather than the first one, so a second click
/// picks what was already set; without that, overlapping is worse than not.
///
/// **The width cannot come from measuring the popover.** A popover is a
/// `GtkNative`: it takes no room in the widget that parents it, so measuring
/// it as a child answers zero however wide it will actually open. That zero is
/// what left an earlier attempt at this centered. The number has to come from
/// what is inside it, plus the padding the stylesheet puts around that.
fn aim_right(popover: &gtk::Popover, anchor: &gtk::ListBoxRow, width: i32) {
    if width <= 0 || anchor.width() <= 0 {
        return;
    }
    let center = anchor.width() - width / 2;
    popover.set_pointing_to(Some(&gdk::Rectangle::new(center, 0, 1, anchor.height())));
}

fn chooser_row(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-row");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

/// GTK rings the system bell when a keyboard move can't go anywhere - at
/// the ends of a list, which happens constantly when navigating by
/// arrow key or D-pad. The application provides its own click instead.
fn suppress_error_bell() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_error_bell(false);
        // Clicking the timeline jumps to that point. Left to the platform
        // default this differs by system: on macOS a click on the trough
        // steps toward the pointer instead of going there, which reads as a
        // seek that ignored where you clicked.
        settings.set_gtk_primary_button_warps_slider(true);
        // Holding the timeline still for a moment otherwise puts GtkRange into
        // its fine-adjustment mode: the trough grows and the slider starts
        // moving a fraction of the distance the pointer does. That is a useful
        // affordance for choosing an exact value in a settings dialog, and a
        // baffling one on a video timeline, where it looks like the playhead
        // has come unstuck from the mouse. Nothing here wants a long press, so
        // the threshold is put beyond reach rather than the behavior fought.
        // An hour, not u32::MAX: the property has a range and refuses
        // anything past it, which panics on the way up.
        settings.set_gtk_long_press_time(60 * 60 * 1000);
    }
}

/// Sizes are set here rather than left to the theme because the interface
/// is meant to be read from across a room. Everything scales from one
/// factor so it can be dialled down for close-range use.
/// Starting window size, in the same units as the interface inside it.
///
/// A fixed size would mean a 2x menu opening into a 1x frame, which is how a
/// 4K display ends up with a window too small for its own contents. Capped to
/// most of the monitor so a large scale on a modest screen still opens
/// something that fits, panels and decoration included.
fn default_window_size(
    scale: f64,
    monitor: Option<&gdk::Monitor>,
    saved: (Option<i32>, Option<i32>),
) -> (i32, i32) {
    // Sixteen by nine, and a good deal larger than it was. The old size was
    // 1100x700 - close to 11:7, and so a shape no film is - which left the
    // media page holding a column of empty air down the sides of its artwork
    // before anybody had touched a window edge.
    const BASE_WIDTH: f64 = 1600.0;
    const BASE_HEIGHT: f64 = 900.0;
    const MAX_FRACTION: f64 = 0.9;

    // Where it was left, if it was left anywhere. Held to the same fraction of
    // the screen as the default below: a size remembered from a larger monitor
    // would otherwise open off the edge of a smaller one.
    let (mut width, mut height) = match saved {
        (Some(width), Some(height)) if width > 0 && height > 0 => (width as f64, height as f64),
        _ => (BASE_WIDTH * scale, BASE_HEIGHT * scale),
    };
    if let Some(monitor) = monitor {
        let geometry = monitor.geometry();
        width = width.min(geometry.width() as f64 * MAX_FRACTION);
        height = height.min(geometry.height() as f64 * MAX_FRACTION);
    }
    (width.round() as i32, height.round() as i32)
}

/// Registers the provider the interface's sizes are loaded into. Kept so the
/// sizes can be replaced later without stacking up providers, which is what
/// makes re-scaling on a different monitor possible.
fn install_styles() -> gtk::CssProvider {
    let provider = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    provider
}

fn style_css(scale: f64) -> String {
    let px = |base: f64| (base * scale).round() as i32;

    format!(
        "
        /* The font TinePlayer ships, so its own text is the same on every
           platform rather than three different system faces.

           Naming it matters for more than looks. Without this the interface
           asks for the platform's default font and ours is only ever reached
           as a fallback, one character at a time - which is what left Cyrillic
           on macOS with gaps between the letters, each one resolved separately
           from whatever happened to cover it. Named, the whole line comes from
           one face with its own metrics.

           Every script face is named too, and that is not belt and braces.
           Listed only as \"TinePlayer Sans\", the others are reachable solely
           as fallback, and fallback prefers whatever the machine already has:
           Arabic and Armenian came out as three different system faces on
           three platforms, while Telugu and Bengali were consistent purely
           because nobody else had them. Naming them puts ours first.

           The generic sans-serif at the end is what draws anything ours does
           not carry: file names, device names and track titles, in scripts
           nobody can predict. The list is written out rather than generated,
           so it has to be updated when a script is added - which
           packaging/fonts/build-fonts.py will refuse to build without. */
        window, .tp-menu, .tp-controls {{
            font-family:
                \"TinePlayer Sans\",
                \"TinePlayer Sans Arabic\", \"TinePlayer Sans Armenian\",
                \"TinePlayer Sans Bengali\", \"TinePlayer Sans Cjk\",
                \"TinePlayer Sans Devanagari\", \"TinePlayer Sans Georgian\",
                \"TinePlayer Sans Gurmukhi\", \"TinePlayer Sans Hangul\",
                \"TinePlayer Sans Hebrew\", \"TinePlayer Sans Malayalam\",
                \"TinePlayer Sans Symbols\",
                \"TinePlayer Sans Tamil\", \"TinePlayer Sans Telugu\",
                \"TinePlayer Sans Thai\",
                sans-serif;
        }}
        .tp-title {{
            font-size: {title}px;
            font-weight: bold;
            opacity: 0.75;
            letter-spacing: {tracking}px;
        }}
        .tp-row {{ font-size: {row}px; padding: {pad_v}px {pad_h}px; }}
        .tp-value {{ opacity: 0.7; }}
        /* Sized with the rest of the interface: the theme's default switch is
           drawn for a mouse at a desk, and is a smudge from a sofa.

           Monochrome rather than the theme's accent, which is the same blue as
           the row highlight and disappeared into it. On is read from the fill
           being solid, not from its hue, so it competes with nothing. Off the
           full foreground colour in both, which was the loudest thing on a
           screen of settings most people set once, but not by the same
           amount: the dark theme needs the fill near white to read as on at
           all, where the light one wants a good deal less than black.
           Literal
           colors picked from the theme here rather than `@theme_fg_color`, for
           the reason the cancel button gives. */
        .tp-row switch {{
            min-width: {switch_w}px;
            min-height: {switch_h}px;
            border-radius: {switch_h}px;
            background-color: {trough};
            background-image: none;
            border-color: transparent;
        }}
        .tp-row switch > slider {{
            min-width: {slider}px;
            min-height: {slider}px;
            border-radius: {switch_h}px;
            /* The same knob as a slider carries, for the same reason. Only
               while the switch is off: checked, the knob sits on the lit fill
               and needs to be the dark one below. */
            background-color: {knob};
        }}
        .tp-row switch:checked {{
            background-color: {fill};
            border-color: {fill};
        }}
        .tp-chevron {{ font-size: {row}px; opacity: 0.5; }}
        .tp-hint {{ font-size: {hint}px; opacity: 0.7; }}
        /* The one screen made of paragraphs. Looser than a row of settings,
           since it is read rather than scanned. */
        .tp-about {{ font-size: {hint}px; opacity: 0.8; }}
        .tp-about-heading {{
            font-size: {row}px;
            font-weight: bold;
            margin-top: {pad_v}px;
        }}
        /* Every button in the interface: one size, one padding, one corner.
           The corner matches a menu row's, so a button and the rows it sits
           over read as parts of one page. */
        .tp-button {{
            font-size: {row}px;
            padding: {pad_v}px {pad_h}px;
            border-radius: {radius}px;
        }}
        /* The one action every screen is pointing at.

           Deliberately *not* the blue the selected row is drawn in, which is
           what it was first. Two things were wrong with that. A blue button
           sitting directly above a blue selected row read as two halves of
           one control rather than as an action and a choice; and when the
           button took focus it had nothing left to say so with, because it
           was already wearing the color that means this one is selected. A
           different hue gives the focus ring something to be seen against,
           and lets blue go on meaning one thing throughout.

           Blue, and the clash is resolved from the other side: what changed
           is the *focus* color, which is now a neutral rather than the same
           accent. Blue means an action throughout, and white means where you
           currently are. Literal, and with the
           gradient cleared: the theme paints
           buttons with one, and a flat color underneath it comes out as a tint
           of whatever the theme wanted rather than as this color. */
        .tp-action {{
            font-weight: bold;
            background-image: none;
            background-color: {play_fill};
            color: {play_ink};
            border-color: transparent;
        }}
        .tp-action:hover {{ background-color: {play_hover}; }}
        /* Focus said loudly, because this is read from a distance and these
           buttons no longer sit in a list whose highlight does the saying. A
           ring around the button rather than a change of fill: the fill is
           what the button *is*, and swapping one green for another is a
           difference nobody can see across a room. */
        /* Drawn as a shadow rather than an outline. `outline` is what a focus
           ring is normally, and GTK parsed it here without complaint and drew
           nothing - so it is not something to spend an afternoon on when a
           spread shadow does the same job and demonstrably works. */

        /* The corner marks had no focus state at all, which on a screen meant
           to be driven by a gamepad from a sofa means arrowing onto one and
           having nothing tell you. Same ring, drawn round the icon. */
        .tp-gear:focus {{
            background-color: rgba(128, 128, 128, 0.22);
            box-shadow: 0 0 0 {focus_ring}px {focus};
        }}
        /* Nothing to press it about: a disabled action keeps its shape and
           loses its insistence, rather than staying the loudest thing on a
           page it cannot act on. */
        .tp-action:disabled {{
            background-color: {trough};
            color: inherit;
            opacity: 0.5;
        }}
        /* Restart, once Resume has taken the words. Square rather than merely
           narrow, so it reads as the mark's button rather than as a button
           whose label went missing. */
        .tp-action-icon {{ padding: {pad_v}px {pad_v}px; min-width: {play_icon}px; }}
        /* Half again the height, on the media page's pair alone. They are the
           one thing the page is for, and on a television they are pressed from
           across a room - so they are worth more than the height a line of
           text happens to need. Declared after `.tp-action-icon` so the
           restart button takes this padding rather than that one. */
        .tp-tall {{ padding-top: {tall_v}px; padding-bottom: {tall_v}px; }}
        /* The media page.

           The ground the whole page is drawn on, and - through `color` - the
           ground the backdrop screen-blends against. See src/backdrop.rs for
           why the background arrives as a foreground property: a widget
           cannot read its own CSS background from inside `snapshot`, and
           every other color in this application is declared here rather than
           in Rust. Literal, for the reason the highlight is literal. */
        .tp-backdrop {{ color: {page_bg}; }}
        /* The rows sit on the artwork, so everything between them and it has
           to get out of the way. A GtkListBox and a GtkScrolledWindow both
           paint the theme's view background by default, which came out as an
           opaque slab over the backdrop in the shape of the list. */
        /* Transparent only where something is meant to show through: the
           media page, which has the film's backdrop behind it, and a selector,
           which draws its own panel. Everywhere else a list keeps the theme's
           own background, which is what sets it apart from the page around it.
           
           Written unscoped to begin with, and that took the ground out from
           under every list in the application - the browser's two columns
           merged into the page behind them, and there was no longer anything
           to say where one ended. */
        .tp-media .tp-menu, .tp-media .tp-menu > row,
        .tp-selector .tp-menu, .tp-selector .tp-menu > row {{
            background-color: transparent;
        }}
        .tp-media scrolledwindow, .tp-media viewport,
        .tp-selector scrolledwindow, .tp-selector viewport {{
            background-color: transparent;
        }}
        /* The two marks in the corner are affordances rather than actions:
           no fill and no border until the pointer is on them, so they carry
           no weight beside the button the page is actually pointing at. */
        .tp-gear {{
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
        }}
        /* No color, and none needed. The gear was a symbolic icon, which GTK
           recolors from the foreground - including dimming it when the window
           loses focus, while the drawn mark beside it did not, so the pair came
           apart every time the window went to the back. It is a drawn mark
           itself now, in the same ink, so the two behave alike without being
           told to. */
        .tp-gear:hover {{ background-color: rgba(128, 128, 128, 0.22); }}
        /* No focus ring on the playback controls. Every button there already
           says where the cursor is by filling with the accent - see
           `.tp-selected` - so the shared ring was a second mark for the same
           fact, drawn inside the fill and reading as an outline around it. The
           menus keep theirs, where there is no fill to say it instead. */
        .tp-transport-button:focus {{ box-shadow: none; }}
        /* The frame the poster sits in, which is also what is seen when there
           is no poster. A flat panel a shade off the page rather than an
           outline: at a distance a thin border on a dark ground disappears,
           and the shape is what says a picture belongs here. */
        .tp-poster {{
            background-color: {panel};
            border-radius: {radius}px;
        }}
        /* The film's name. The largest thing on the page by a good margin,
           because from across a room it is the one thing being checked. */
        .tp-film-title {{
            font-size: {film_title}px;
            font-weight: bold;
        }}
        .tp-film-facts {{ font-size: {film_facts}px; opacity: 0.65; }}
        .tp-film-plot {{ font-size: {film_plot}px; opacity: 0.92; }}
        /* The label-and-reading lines: under the poster, and the languages
           above the rows. The label's own dimming is set per-span in the
           markup, so this carries only the size. */
        .tp-fact {{ font-size: {fact}px; }}
        /* The name of a reading rather than the reading itself, dimmed so a
           column of these scans as values with labels rather than as a block
           of text of one weight. */
        .tp-fact-name {{ opacity: 0.6; }}
        .tp-empty-prompt {{ font-size: {row}px; opacity: 0.7; }}
        /* Backing out, on every screen that offers it. A literal red for the
           same reason the highlight is literal: a theme name that does not
           exist makes the whole declaration fail to parse. */
        .tp-danger {{
            background-image: none;
            background-color: #c01c28;
            color: #ffffff;
        }}
        .tp-danger:hover {{ background-color: #a51d2d; }}
        /* Beside a main action rather than being one: smaller type and far
           less padding than the buttons it sits with, so it reads as a way to
           reach something else rather than as the thing to press. */
        .tp-secondary {{ font-size: {small}px; padding: {tight_v}px {tight_h}px; }}
        .tp-menu > row {{ border-radius: {radius}px; }}
        /* The ground the rows sit on. Black at a fraction rather than a
           lighter grey: it has to read as a panel over whatever backdrop the
           film brought, and a tint that darkens works over every one of them
           where a fixed colour only works over some. */
        .tp-menu-panel {{
            background-color: rgba(0, 0, 0, 0.2);
            border-radius: {panel_radius}px;
            padding: {panel_pad}px;
        }}
        /* Gray rather than a theme color, so it lifts off the background in
           both light and dark without needing two rules. */
        .tp-menu > row:hover {{ background-color: rgba(128, 128, 128, 0.18); }}
        .tp-menu:focus-within > row:selected:hover {{ background-color: {focus_row}; }}
        .tp-menu > row.tp-section-start {{ margin-top: {section}px; }}
        /* A group heading. Quiet on purpose: smaller than a row and dimmed,
           so it labels what is under it without competing with it. Indented to
           `pad_h` so it starts exactly where the row labels below it do. */
        .tp-group {{
            font-size: {group}px;
            font-weight: bold;
            opacity: 0.55;
            margin: {group_top}px {pad_h}px {group_gap}px {pad_h}px;
        }}
        .tp-group-first {{ margin-top: {group_first_top}px; }}
        /* A selector opened over the page. `contents` is the node GTK puts
           inside a popover; styling the popover itself leaves the theme's own
           background drawn underneath. */
        /* Smaller than a row on the page behind it. A selector is a list of
           variations on one value rather than a set of destinations, and at
           the page's own size it reads as a second menu that has landed on
           top of the first. */
        .tp-selector .tp-row {{
            font-size: {selector_row}px;
            padding: {selector_row_pad_v}px {selector_row_pad_h}px;
        }}
        .tp-selector separator {{
            margin: {rule_gap}px 0;
            background-color: rgba(255, 255, 255, 0.14);
        }}
        .tp-selector > contents {{
            background-color: {selector_bg};
            border-radius: {panel_radius}px;
            padding: {panel_pad}px;
            box-shadow: 0 {shadow_drop}px {shadow_blur}px rgba(0, 0, 0, 0.55);
        }}
        /* Which row is in force, as opposed to which row the cursor is on.
           Two different facts that a list has only one highlight for, and
           conflating them is actively misleading in the places column: moving
           the cursor there would appear to change the folder being shown.

           A bar down the leading edge rather than a fill, so it reads as
           'you are here' beside the focus rather than competing with it.
           Drawn with an inset shadow rather than a border so that marking a
           row does not shift its text. */
        .tp-menu > row.tp-current {{
            box-shadow: inset {mark}px 0 0 0 {highlight};
        }}
        /* Belongs to the row above it: indented so the group reads as one
           thing without every label having to name the output again. */
        .tp-menu > row.tp-subrow {{ margin-left: {subrow}px; }}
        /* A selection is only shown while the list it belongs to holds the
           focus. A list keeps its selected row either way, so that returning
           to it lands where you left - but showing that on a list you are
           not on reads as a second cursor, and with two lists side by side
           it is genuinely unclear which one an arrow key would move.

           The cost is that stepping down to the buttons leaves the list with
           nothing marked. That is the right trade: the buttons show their own
           focus, so there is still exactly one thing highlighted on screen. */
        .tp-menu:focus-within > row:selected {{
            background-image: none;
            background-color: {focus_row};
            color: {on_focus};
        }}
        .tp-menu:focus-within > row:selected .tp-value,
        .tp-menu:focus-within > row:selected .tp-chevron {{
            color: {on_focus};
            opacity: 0.85;
        }}
        /* A ring rather than a fill. Recoloring a focused button changes what
           it looks like it does - a Cancel that turns blue reads as the one
           to press - and beside another button the pair stop looking like
           peers. An inset shadow rather than a border so nothing shifts, and
           rather than an outline so it follows the rounded corners. */
        button:focus {{
            box-shadow: 0 0 0 {focus_ring}px {focus};
        }}
        /* Chrome-less until pointed at, but the arrow itself stays visible
           so the way back is always apparent. */
        /* One fixed footprint for whatever leads the header, the back arrow
           or the application mark. Without it the two screens allocate
           different widths and everything after them moves. */
        .tp-leading {{
            min-width: {leading}px;
            min-height: {leading}px;
            padding: 0px;
        }}
        /* Fixed too, so a header of buttons is no taller than one holding a
           plain label and the list below starts in the same place. */
        .tp-header {{ min-height: {leading}px; }}
        .tp-back {{
            padding: 0px;
            min-width: 0px;
            min-height: 0px;
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
            opacity: 0.6;
        }}
        .tp-back:hover {{
            background-color: rgba(128, 128, 128, 0.25);
            opacity: 1;
        }}
        .tp-back:focus {{ opacity: 1; }}
        /* Laid over the picture, so it sets its own colors rather than
           inheriting theme ones that may be light. */
        .tp-controls {{
            background-color: rgba(0, 0, 0, 0.75);
            padding: {pad_v}px {pad_h}px;
        }}
        /* The buttons sit under the timeline rather than beside it, so the
           row a controller is moving along is unambiguous. */
        .tp-buttons {{ padding: 0px; }}
        /* Tabular figures, so the digits are all one width. A proportional
           1 is narrower than a 0, which makes a running clock twitch even
           when the number of characters does not change. */
        .tp-time {{
            font-size: {hint}px;
            color: #ffffff;
            font-feature-settings: \"tnum\" 1;
        }}
        .tp-transport {{ -gtk-icon-size: {icon}px; color: #ffffff; }}
        /* Play, drawn bigger than what sits around it. */
        .tp-transport-main {{ -gtk-icon-size: {icon_main}px; }}
        /* Flat over the picture: the strip already reads as a control bar,
           and button chrome on top of video looks like a mistake. */
        .tp-transport-button {{
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
            min-height: 0px;
            min-width: 0px;
            padding: 0px {crumb_pad}px;
        }}
        .tp-transport-button:hover {{ background-color: rgba(255, 255, 255, 0.15); }}
        /* A control that is there but not doing anything: the sync button
           while an output's delay is switched off. Dimmed rather than hidden,
           since it is what turns the delay back on. */
        .tp-off {{ opacity: 0.35; }}
        /* Where a controller is, drawn boldly enough to be found from across a
           room rather than as the hairline a focus ring would give. */
        .tp-selected {{
            background-color: {highlight};
            border-radius: {radius}px;
        }}
        /* Darker than the strip it sits on, so it reads as a panel laid over
           the bar rather than as more of the bar. */
        .tp-volume-panel {{
            background-color: rgba(0, 0, 0, 0.75);
            border-radius: {radius}px;
            padding: {crumb_pad}px;
            margin-bottom: {crumb_pad}px;
            margin-right: {pad_h}px;
        }}
        /* Padded so the selection mark has room around a row rather than
           sitting tight against the words. */
        .tp-volume-panel > box {{
            padding: {crumb_pad}px;
            border-radius: {radius}px;
        }}
        .tp-volume-panel label {{ color: #ffffff; }}
        /* The same size as the transport icons, which are drawn to be read
           from a sofa rather than a desk. A button built from an icon name
           has no image to class, so the size is set on the descendant. */
        .tp-volume-panel button image {{
            -gtk-icon-size: {icon}px;
            color: #ffffff;
        }}
        /* The handle, not the whole bar: filling the trough drew over the
           very thing that says where playback is. */
        .tp-progress.tp-selected {{ background-color: transparent; }}
        .tp-progress.tp-selected slider {{
            background-color: {knob};
            outline: {outline}px solid {highlight};
            outline-offset: {outline}px;
            min-width: {handle}px;
            min-height: {handle}px;
        }}
        /* Faded while subtitles are off and solid while they are on, so the
           button reports the state as well as offering to change it. Opacity
           rather than color: the mark is an image, which a color cannot
           tint. */
        /* Darkens the menu the browser opens over, so the panel reads as
           being in front of it rather than beside it. */
        .tp-scrim {{ background-color: rgba(0, 0, 0, 0.55); }}
        /* Inset from the window edges, so the dimmed menu shows around all
           four sides and the panel looks like a window over it. Literal
           colors rather than theme names, for the reason given by the
           highlight color below. */
        .tp-modal {{
            background-color: #1e1e1e;
            border: 1px solid rgba(255, 255, 255, 0.14);
            border-radius: {radius}px;
            margin: {modal}px;
            padding: {modal_pad}px;
        }}
        /* Taller than a stock entry: this is the one thing on its panel, and
           it is read from the same distance as everything else. */
        .tp-path {{ font-size: {row}px; padding: {pad_v}px {pad_h}px; }}
        .tp-subtitles-button {{ opacity: 0.45; }}
        .tp-subtitles-on {{ opacity: 1; }}
        .tp-subtitles-button:disabled {{ opacity: 0.2; }}
        .tp-progress {{ min-height: {bar}px; }}
        .tp-progress progress {{ background-color: {highlight}; }}
        /* The alignment panel's bar, thicker than the playback scrubber: it
           is the only thing moving on that screen, and it is read from across
           a room rather than aimed at with a pointer. Its own class, so the
           scrubber and the settings sliders keep the weight they were given.
           The height has to sit on both nodes - a GtkProgressBar draws the
           fill inside the trough, and raising only one leaves a thick bar with
           a thin fill rattling around in it. */
        .tp-align-bar, .tp-align-bar trough, .tp-align-bar progress {{
            min-height: {align_bar}px;
            border-radius: {align_bar_radius}px;
        }}
        /* Styled in full rather than by borrowing `tp-bar`, whose dim fill is
           meant for a slider with a handle on it to point at. There is no
           handle here, so the fill is the whole of what is being read and it
           takes the highlight colour. `background-image: none` first, or the
           theme's gradient sits over any colour set under it. */
        .tp-align-bar trough {{
            background-color: {trough};
            background-image: none;
        }}
        .tp-align-bar progress {{
            background-color: {highlight};
            background-image: none;
        }}
        /* Settings bars, drawn to be found rather than to be tasteful. The
           theme's own colours put a faint handle on a faint trough, which on
           a dark background is a bar that has to be looked for.

           Three steps apart, so the parts stay told from each other: the
           handle brightest, the part behind it dimmer, the rest dimmer again.
           Deliberately not the highlight colour, which is what a selected row
           is painted with - a blue bar on a blue row is the one case where
           the theme's choice vanishes completely. */
        .tp-bar trough {{ background-color: {trough}; background-image: none; }}
        .tp-bar trough > highlight, .tp-bar progress {{
            background-color: {fill};
            background-image: none;
        }}
        /* `background-image: none` first, or none of the colour below shows:
           the theme paints handles and troughs with a gradient image, which
           sits over any background colour set under it. The same trap the
           transport buttons work around. */
        .tp-bar slider, .tp-row switch > slider {{
            background-image: none;
            background-color: {knob};
            box-shadow: none;
            /* A ring against the knob's own brightness, so one knob colour
               reads both on the dim trough and on the lit fill it travels
               onto - sliders and switches alike. */
            border: {edge}px solid {knob_edge};
        }}
        .tp-bar slider {{
            min-width: {handle}px;
            min-height: {handle}px;
        }}
        /* An output that is silenced or a delay not being applied: the row
           still says what it is set to, quietly. */
        .tp-bar:disabled trough > highlight,
        .tp-bar:disabled progress {{ background-color: {trough}; }}
        /* Reads as a path rather than a row of buttons, until one takes
           focus and the shared button:focus rule highlights it. */
        .tp-crumb {{
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
            min-height: 0px;
            min-width: 0px;
            padding: 2px {crumb_pad}px;
            font-size: {title}px;
            font-weight: bold;
            opacity: 0.75;
        }}
        .tp-crumb:focus {{ opacity: 1; }}
        .tp-crumb-separator {{ font-size: {title}px; opacity: 0.4; }}
        /* Kept small enough that the header stays the height every other
           screen's header is. */
        .tp-browse {{
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
            min-height: 0px;
            min-width: 0px;
            padding: 2px {crumb_pad}px;
            opacity: 0.6;
        }}
        .tp-browse:hover {{ opacity: 1; }}
        .tp-browse image {{ -gtk-icon-size: {back_icon}px; }}
        /* A new version is waiting. A dot rather than a count or a word: it
           says only that something is here, which is all it knows, and it
           reads at the distance this interface is built for.

           Drawn in the accent colour on the button that opens Settings, and
           on the row that names the version. The button's mark goes as soon
           as the row has been reached; the row keeps its own. */
        /* The dot is placed inside the gradient rather than with
           background-position, which GTK will not take two values for: it
           rejects the whole declaration as junk at the end of a value, and
           falls back to the top left corner. Windows tolerated it and
           macOS did
           not, so it looked correct on the machine it was written on and
           wrong everywhere else. The size comes from the colour stops for the
           same reason - fewer properties, fewer things to be refused. */
        .tp-badge {{
            background-image: radial-gradient(circle at 88% 14%,
                {highlight} 0, {highlight} {badge_r}px, transparent {badge_r}px);
        }}
        .tp-badge-row {{
            background-image: radial-gradient(circle at {badge_left}px 50%,
                {highlight} 0, {highlight} {badge_r}px, transparent {badge_r}px);
            padding-left: {badge_indent}px;
        }}
        /* The selection highlight is this same blue, so a blue dot on the
           selected row is a blue dot on blue. It has to change colour for the
           one moment it matters most - the row is selected the instant it is
           reached. */
        .tp-menu > row.tp-badge-row:selected {{
            background-image: radial-gradient(circle at {badge_left}px 50%,
                {on_highlight} 0, {on_highlight} {badge_r}px, transparent {badge_r}px);
        }}
        .tp-gear {{ padding: {pad_v}px {pad_h}px; }}
        /* Only where it sits beside the tall pair, and the same height as it.
           `tall_pad` across is what `tall_v` down was before the height came
           back ten percent - it is kept because the widths were to stay put,
           which leaves these marginally wider than tall rather than square. */
        .tp-gear.tp-tall {{ padding: {tall_v}px {tall_pad}px; }}
        .tp-row-icon {{ -gtk-icon-size: {row_icon}px; opacity: 0.65; }}
        .tp-back image {{ -gtk-icon-size: {back_icon}px; }}
        .{video} {{ background-color: black; }}
        ",
        title = px(20.0),
        tracking = px(2.0).max(1),
        // Scaled like everything else: a dot sized for a monitor is invisible
        // on a television across a room.
        // Big enough to read from a sofa, which is the distance this whole
        // interface is sized for. The first attempt was five pixels and
        // looked like a rendering artefact.
        badge_r = px(7.0).max(5),
        badge_left = px(14.0),
        badge_indent = px(24.0),
        // What reads against the selection highlight rather than into it.
        on_highlight = "#ffffff",
        row = px(21.0),
        hint = px(20.0),
        small = px(17.0),
        tight_v = px(7.0),
        tight_h = px(10.0),
        pad_v = px(9.0),
        pad_h = px(18.0),
        radius = px(8.0),
        // Larger than a row's corner, in proportion to the box it rounds. At
        // the row radius a panel this size reads as a rectangle with the
        // corners knocked off.
        panel_radius = px(16.0),
        panel_pad = px(8.0),
        outline = px(2.0).max(1),
        handle = px(18.0),
        // Bright on dark; on light a white knob held in by its ring rather
        // than a dark disc, which read as heavy against a white row and
        // heavier still in a column of them.
        knob = "#dcdcdc",
        fill = "#b9b9b9",
        trough = "rgba(255, 255, 255, 0.13)",
        knob_edge = "rgba(0, 0, 0, 0.55)",
        edge = px(1.0).max(1),
        switch_w = px(64.0),
        switch_h = px(32.0),
        slider = px(26.0),
        section = px(28.0),
        group = px(16.0),
        group_top = px(24.0),
        group_gap = px(4.0),
        group_first_top = px(10.0),
        // About three quarters of the page's row. Clearly subordinate to the
        // menu behind it, and still a size anyone can read from a sofa - which
        // half size was not, on the one list in the interface made of
        // near-identical strings where a misread picks the wrong track.
        rule_gap = px(6.0),
        selector_row = px(17.0),
        selector_row_pad_v = px(7.0),
        selector_row_pad_h = px(14.0),
        shadow_drop = px(4.0),
        shadow_blur = px(18.0),
        subrow = px(28.0),
        mark = px(4.0),
        // A shade larger than `ICON_PX`, which is what every other icon in
        // the interface uses. The gear sits beside the fullscreen mark, and
        // that mark is a picture with clear space drawn into it - so at the
        // same nominal size the gear's own glyph came out visibly the smaller
        // of the two. Matched by eye rather than by number, because what has
        // to agree is the drawn marks and not the boxes around them.
        icon = px(ICON_PX + 3.5),
        icon_main = px(38.4),
        crumb_pad = px(6.0),
        leading = px(38.0),
        back_icon = px(22.0),
        row_icon = px(18.0),
        bar = px(6.0),
        align_bar = px(14.0),
        align_bar_radius = px(7.0),
        // A literal color rather than a theme name: GTK's named colors
        // differ between themes and libadwaita, and an undefined one makes
        // the whole declaration fail to parse - which silently leaves the
        // highlighted row unreadable. Both foreground and background are
        // set for the same reason: overriding only the background left the
        // theme's white selection text on a pale color.
        modal = px(48.0),
        modal_pad = px(16.0),
        highlight = "#3584e4",
        film_title = px(48.0),
        film_facts = px(24.0),
        film_plot = px(22.0),
        fact = px(20.0),
        // Half again as tall as the type inside it would otherwise leave it,
        // less ten percent. Vertical and horizontal are separate numbers
        // because that ten percent came off the height alone: the marks in the
        // button bar keep the width that had them square.
        tall_v = px(17.0),
        tall_pad = px(20.0),
        play_icon = px(46.0),
        focus_ring = px(3.0).max(2),
        // The blue the play and restart buttons are drawn in - the same accent
        // as everything else the application colors deliberately.
        play_fill = "#3584e4",
        play_hover = "#4a90e8",
        play_ink = "#ffffff",
        // What shows where you are: the selected row, and the ring round a
        // focused button.
        //
        // **Not the accent, and that is the point.** With blue meaning both
        // "this is the action" and "this is where you are", a blue button
        // sitting above a blue selected row read as one continuous thing, and
        // a focused button had nothing left to distinguish itself with. The
        // highest-contrast neutral against the page says "here" without
        // competing with any color that means something - white on the dark
        // theme, and near-black on the light one, where white would be
        // invisible against a near-white page.
        focus = "#ffffff",
        // The same white, backed off, for the row the cursor is on. Only the
        // row: the ring on a focused button stays at full strength, being a
        // thin outline that has nothing like the area to spare.
        focus_row = "rgba(255, 255, 255, 0.7)",
        on_focus = "#1c1c1c",
        // The ink both corner marks share. The fullscreen image is drawn in
        // it as a picture; the gear is told to match.
        // The page's own ground. Matched to what each theme's window is
        // already drawn in, since the backdrop paints over the whole page and
        // a mismatch would show as a rectangle behind the content.
        page_bg = "#242424",
        // Darker than the page and all but opaque. It sits over artwork here
        // and will sit over a moving picture later, and neither is a ground
        // anyone can read a list against.
        selector_bg = "rgba(26, 26, 26, 0.98)",
        // The poster's frame, a shade off the page in whichever direction
        // there is room to go.
        panel = "rgba(255, 255, 255, 0.07)",
        video = crate::player::VIDEO_CSS_CLASS,
    )
}

#[cfg(test)]
mod notices {
    use super::*;

    /// The real file, since that is what ships and what the transform has to
    /// cope with. A table that still has its pipes in it is the failure this
    /// is watching for: it reads as punctuation rather than as a list.
    #[test]
    fn the_shipped_notices_read_as_text() {
        let blocks = notices_blocks(include_str!("../THIRD-PARTY.md"));
        assert!(!blocks.is_empty(), "nothing was produced");

        let mut headings = Vec::new();
        for block in &blocks {
            match block {
                Notice::Heading(text) => headings.push(text.as_str()),
                Notice::Text(text) => {
                    assert!(!text.contains('|'), "table pipe left in: {text:?}");
                    assert!(!text.contains("**"), "bold marker left in: {text:?}");
                    assert!(!text.starts_with('>'), "quote marker left in: {text:?}");
                    assert!(text.trim() != "[!NOTE]", "note marker left in");
                }
            }
        }

        for wanted in ["Fonts", "Native libraries", "Rust dependencies"] {
            assert!(
                headings.contains(&wanted),
                "no {wanted:?} heading in {headings:?}"
            );
        }
    }

    /// A crate row keeps all three of its cells, joined rather than dropped.
    #[test]
    fn a_crate_row_keeps_its_columns() {
        let blocks = notices_blocks(
            "## Rust dependencies\n\n| Crate | Version | License |\n|---|---|---|\n| serde | 1.0.229 | MIT OR Apache-2.0 |\n",
        );
        let rows: Vec<&String> = blocks
            .iter()
            .filter_map(|block| match block {
                Notice::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert!(
            rows.iter().any(|row| row.contains("serde")
                && row.contains("1.0.229")
                && row.contains("MIT OR Apache-2.0")),
            "the crate row lost a column: {rows:?}"
        );
        // The rule under the header carries no words and should be gone.
        assert!(!rows.iter().any(|row| row.contains("---")), "{rows:?}");
    }

    /// Paragraphs wrapped in the file wrap to the window instead.
    #[test]
    fn wrapped_prose_is_rejoined() {
        let blocks = notices_blocks("one line\nand its continuation\n\na second paragraph\n");
        let texts: Vec<&String> = blocks
            .iter()
            .filter_map(|block| match block {
                Notice::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert_eq!(texts[0], "one line and its continuation");
    }
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
        let all: Vec<Item> = Category::ALL
            .iter()
            .flat_map(|category| category.items())
            .map(|(_, item)| item)
            .collect();
        for item in &all {
            let count = all.iter().filter(|other| *other == item).count();
            assert_eq!(count, 1, "an item appears {count} times");
        }
        // Written out rather than derived, so adding a setting and forgetting
        // to place it fails here instead of at a glance. Twenty-six rather
        // than the twenty-one variants of `Item`: the five an output has are
        // placed once for each output.
        assert_eq!(all.len(), 26);
    }

    /// The version sits under the switch that decides whether anything is
    /// said about newer ones. Read the other way round it is a status with no
    /// stated relationship to the control above it.
    #[test]
    fn the_version_follows_the_update_switch() {
        let general: Vec<Item> = Category::General
            .items()
            .into_iter()
            .map(|(_, item)| item)
            .collect();
        let switch = general.iter().position(|item| *item == Item::Updates);
        let status = general.iter().position(|item| *item == Item::UpdateStatus);
        assert_eq!(status, switch.map(|at| at + 1));
    }

    /// Clear Data destroys something, and was asked to sit at the end of
    /// General rather than among the everyday toggles.
    #[test]
    fn clearing_data_comes_last() {
        let general = Category::General.items();
        assert_eq!(general.last().map(|(_, item)| *item), Some(Item::ClearData));
    }

    /// A row carries a switch or a bar or neither, and the two that carry
    /// both - the pair whose bar can be turned off - are deliberate. What must
    /// not happen is a row claiming a switch it was never built with, since
    /// activating it would then do nothing at all.
    #[test]
    fn every_switch_row_has_something_to_switch() {
        for (_, item) in Category::ALL.iter().flat_map(|category| category.items()) {
            if item.has_switch() {
                assert!(
                    item.setting().is_none(),
                    "a row cannot both open a chooser and hold a switch"
                );
            }
        }
    }
}
