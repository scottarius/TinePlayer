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

/// Which setting a chooser screen is editing. The menu drills into one of
/// these and returns once a choice is made.
#[derive(Clone, Copy, PartialEq)]
enum Setting {
    PrimaryDevice,
    PrimaryTrack,
    SecondaryDevice,
    SecondaryTrack,
    Subtitles,
    Theme,
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

/// The rows the settings screen always has. The version row below the
/// update switch is extra, and only there while the switch is on.
const SETTINGS_ROWS: usize = 24;

/// Every row of the settings screen, in the order it is built.
///
/// Named rather than written as numbers at each use. The screen is coupled to
/// these in three separate places - the list that builds the rows, the sliders
/// attached to particular ones, and the match that acts on an activation - and
/// nothing catches a mismatch: the compiler is satisfied either way, the row
/// count still adds up, and the symptom is a row that quietly opens the wrong
/// screen. Inserting a row means changing one constant rather than renumbering
/// every literal after it.
const ROW_THEME: i32 = 0;
const ROW_INTERFACE_SCALE: i32 = 1;
const ROW_SOUNDS: i32 = 2;
const ROW_PRIMARY_DEVICE: i32 = 3;
const ROW_PRIMARY_LANGUAGE: i32 = 4;
const ROW_PRIMARY_DESCRIPTION: i32 = 5;
const ROW_PRIMARY_VOLUME: i32 = 6;
const ROW_PRIMARY_SYNC: i32 = 7;
const ROW_SECONDARY_DEVICE: i32 = 8;
const ROW_SECONDARY_LANGUAGE: i32 = 9;
const ROW_SECONDARY_DESCRIPTION: i32 = 10;
const ROW_SECONDARY_VOLUME: i32 = 11;
const ROW_SECONDARY_SYNC: i32 = 12;
const ROW_SUBTITLE_LANGUAGE: i32 = 13;
const ROW_SUBTITLE_SIZE: i32 = 14;
const ROW_SUBTITLE_FONT: i32 = 15;
const ROW_RESUME_THRESHOLD: i32 = 16;
const ROW_WATCHED_THRESHOLD: i32 = 17;
const ROW_CLEAR_DATA: i32 = 18;
const ROW_KODI: i32 = 19;
const ROW_ABOUT: i32 = 20;
const ROW_NOTICES: i32 = 21;
/// Where the update switch sits, and the row naming a new version under it.
const UPDATE_SWITCH_ROW: i32 = 22;
/// The version this is, and what the check made of it. Always built, unlike
/// the check itself, which can be turned off.
const UPDATE_STATUS_ROW: i32 = 23;

/// Every row, in the order they are built, which is what the constants above
/// are positions in. Inserting a row means renumbering everything below it,
/// and a number that does not get renumbered puts a control on the wrong row
/// rather than failing - which is how the secondary output's switch came to
/// be built over its Preferred Language row.
const SETTINGS_ORDER: [i32; SETTINGS_ROWS] = [
    ROW_THEME,
    ROW_INTERFACE_SCALE,
    ROW_SOUNDS,
    ROW_PRIMARY_DEVICE,
    ROW_PRIMARY_LANGUAGE,
    ROW_PRIMARY_DESCRIPTION,
    ROW_PRIMARY_VOLUME,
    ROW_PRIMARY_SYNC,
    ROW_SECONDARY_DEVICE,
    ROW_SECONDARY_LANGUAGE,
    ROW_SECONDARY_DESCRIPTION,
    ROW_SECONDARY_VOLUME,
    ROW_SECONDARY_SYNC,
    ROW_SUBTITLE_LANGUAGE,
    ROW_SUBTITLE_SIZE,
    ROW_SUBTITLE_FONT,
    ROW_RESUME_THRESHOLD,
    ROW_WATCHED_THRESHOLD,
    ROW_CLEAR_DATA,
    ROW_KODI,
    ROW_ABOUT,
    ROW_NOTICES,
    UPDATE_SWITCH_ROW,
    UPDATE_STATUS_ROW,
];

/// Rows that begin a group: each output, then subtitles, then what is
/// remembered between runs, then the housekeeping at the bottom.
const SETTINGS_SECTIONS: [i32; 6] = [
    ROW_PRIMARY_DEVICE,
    ROW_SECONDARY_DEVICE,
    ROW_SUBTITLE_LANGUAGE,
    ROW_RESUME_THRESHOLD,
    ROW_KODI,
    ROW_ABOUT,
];
/// Rows that belong to the row named above them, drawn indented so the group
/// reads as settings of that one thing rather than as more of their own.
/// Indentation is what lets them be called just "Preferred Language" instead
/// of repeating "Primary" and "Secondary" in every label.
const SETTINGS_SUBROWS: [i32; 10] = [
    ROW_PRIMARY_LANGUAGE,
    ROW_PRIMARY_DESCRIPTION,
    ROW_PRIMARY_VOLUME,
    ROW_PRIMARY_SYNC,
    ROW_SECONDARY_LANGUAGE,
    ROW_SECONDARY_DESCRIPTION,
    ROW_SECONDARY_VOLUME,
    ROW_SECONDARY_SYNC,
    ROW_SUBTITLE_SIZE,
    ROW_SUBTITLE_FONT,
];

/// Font families offered in the menu. Generic names Pango always resolves
/// rather than an enumeration of everything installed, which would run to
/// hundreds of rows. `subtitle_font` in the config takes any description.
const SUBTITLE_FONTS: [&str; 5] = ["Sans Bold", "Sans", "Serif Bold", "Serif", "Monospace Bold"];

/// Menu rows that begin a new group: the primary pair and the secondary
/// pair each get separating space above them.
const SECTION_STARTS: [i32; 3] = [1, 3, 5];

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
    Chooser,
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
    /// The video was remembered from last time rather than asked for.
    ///
    /// It decides what happens when the file will not open. A video named on
    /// the command line that cannot be read is worth stopping for, because
    /// somebody asked for that video and nothing else will do. One picked up
    /// from `last_video` is a convenience, and a convenience that fails should
    /// get out of the way: it is forgotten and the menu opens as if the file
    /// had never been remembered.
    pub remembered: bool,
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
    tracks: RefCell<Vec<AudioTrack>>,
    primary_track: RefCell<Option<u32>>,
    secondary_track: RefCell<Option<u32>>,
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
    settings_sliders: RefCell<Vec<(i32, Slider, gtk::Scale, gtk::Label)>>,
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
    settings_switches: RefCell<Vec<(i32, gtk::Switch)>>,
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
    /// The size a drag has reached, kept until the bar is let go. Nothing
    /// while the size is not being dragged.
    wanted_scale: Cell<Option<f64>>,
    nav_footer: RefCell<Vec<gtk::Button>>,
    /// Buttons above the list, currently the browser's path trail. Up from
    /// the first row reaches them, the way Down reaches the footer.
    nav_header: RefCell<Vec<gtk::Button>>,
    controls: RefCell<Option<Rc<Controls>>>,
    /// Whether the open chooser was reached from the settings screen, so
    /// that finishing with it returns where it came from.
    from_settings: Cell<bool>,
    /// Whether the dark theme is in force, so switching away from it can be
    /// recognized.
    dark: Cell<bool>,
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
            remembered,
        } = launch;
        let dark = appearance::apply_theme(config.theme);
        suppress_error_bell();

        // Sized from the tallest monitor to begin with, since no window exists
        // yet to ask which one it is on. Corrected below once there is.
        let styles = install_styles();
        let monitor = appearance::tallest_monitor();
        let scale = appearance::resolve_scale(config.ui_scale, monitor.as_ref());
        styles.load_from_data(&style_css(scale, dark));
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

        let (width, height) = default_window_size(scale, monitor.as_ref());
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
            tracks: RefCell::new(Vec::new()),
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
            about_scroll: RefCell::new(None),
            copy_root: RefCell::new(None),
            kodi_draft: RefCell::new(None),
            settings_switches: RefCell::new(Vec::new()),
            settings_list: RefCell::new(None),
            clicked_row: Cell::new(false),
            settling_switch: Cell::new(false),
            wanted_scale: Cell::new(None),
            nav_footer: RefCell::new(Vec::new()),
            nav_header: RefCell::new(Vec::new()),
            controls: RefCell::new(None),
            from_settings: Cell::new(false),
            dark: Cell::new(dark),
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
        });
        // And again whenever the window fills the screen or stops doing so,
        // since that is what the automatic size depends on.
        let weak = Rc::downgrade(&app);
        window.connect_fullscreened_notify(move |window| {
            let Some(app) = weak.upgrade() else { return };
            app.follow_automatic_scale(window);
        });

        app.install_key_handling();

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

        let mut unopenable = match &file {
            Some(source) => app.set_file(source).err().map(|e| (source.clone(), e)),
            None => None,
        };

        // A remembered video that will not open is forgotten rather than
        // reported. Nobody asked for it this time, so an error about it is an
        // error about a decision the application made on its own - and it
        // arrived in front of a menu that then could not be reached.
        //
        // Seen on a MacBook whose last video was on a network share that was
        // not mounted: the path still existed as a stale mount point, so the
        // check in main.rs let it through, and opening it failed. Clearing it
        // here rather than only ignoring it means the next launch does not
        // meet the same wall.
        if remembered && unopenable.is_some() {
            if let Some((source, error)) = unopenable.take() {
                eprintln!("Forgetting {}: {error}", source.label());
            }
            app.config.borrow_mut().last_video = None;
            if let Err(e) = app.config.borrow().save() {
                eprintln!("Could not forget the last video: {e}");
            }
        }

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
            if let Some(spec) = preset.primary.as_deref() {
                *app.primary_track.borrow_mut() = resolve(spec);
            }
            if let Some(spec) = preset.secondary.as_deref() {
                *app.secondary_track.borrow_mut() = resolve(spec);
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
    }

    fn install_key_handling(self: &Rc<Self>) {
        let controller = gtk::EventControllerKey::new();
        let app = self.clone();
        controller.connect_key_pressed(move |_, key, _, state| {
            let playing = app.playback.borrow().is_some();
            match key {
                // Only claimed during playback - the menus need Space for
                // activating whatever row has focus.
                gdk::Key::space if playing => {
                    if let Some(playback) = app.playback.borrow().as_ref() {
                        playback.toggle_pause();
                        app.awake.set(playback.is_playing());
                    }
                    app.wake_controls();
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
                gdk::Key::Return | gdk::Key::KP_Enter if playing => {
                    let on_buttons = app
                        .controls
                        .borrow()
                        .as_ref()
                        .is_some_and(|controls| controls.takes_activation());
                    if !on_buttons {
                        return glib::Propagation::Proceed;
                    }
                    app.press_activate();
                    glib::Propagation::Stop
                }
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
                gdk::Key::f | gdk::Key::F => {
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
                    if state.contains(gdk::ModifierType::CONTROL_MASK)
                        && !app.external
                        && matches!(*app.screen.borrow(), Screen::Menu | Screen::VideoSource) =>
                {
                    app.show_paste_uri();
                    glib::Propagation::Stop
                }
                // The shortcut for copying, which GTK would otherwise only
                // deliver to whichever widget has focus - and the text on the
                // About page deliberately never takes it.
                gdk::Key::c | gdk::Key::C
                    if state.contains(gdk::ModifierType::CONTROL_MASK) && app.copy_selection() =>
                {
                    glib::Propagation::Stop
                }
                // The other half of the pair, and the shortcut every desktop
                // application uses for opening a file.
                gdk::Key::o | gdk::Key::O
                    if state.contains(gdk::ModifierType::CONTROL_MASK)
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
            controller.connect_key_released(move |_, key, _, _| {
                match key {
                    gdk::Key::Left | gdk::Key::Right => app.end_scrub(),
                    // Where the press of a held button is finally acted on,
                    // if holding it did not already mean something else.
                    gdk::Key::Return | gdk::Key::KP_Enter if app.playback.borrow().is_some() => {
                        app.release_activate()
                    }
                    _ => {}
                }
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
            Screen::Chooser => self.leave_chooser(),
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
            Screen::PasteUri | Screen::Browser => self.return_to_origin(),
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
    fn press_activate(self: &Rc<Self>) {
        let controls = self.controls.borrow().clone();
        let Some(controls) = controls else { return };
        if controls.holds_press() {
            controls.press_volume();
        } else {
            controls.activate_focused();
        }
    }

    /// Letting go of a held button. Does the ordinary thing unless the hold
    /// already did something else.
    fn release_activate(self: &Rc<Self>) {
        let controls = self.controls.borrow().clone();
        let Some(controls) = controls else { return };
        if controls.holds_press() && controls.release_volume() {
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
            self.window.fullscreen();
        } else {
            self.window.unfullscreen();
            // The pointer only hides in fullscreen, and leaving takes the
            // countdown that would have brought it back with it.
            if let Some(controls) = self.controls.borrow().as_ref() {
                controls.reveal_pointer();
            }
        }

        let mut config = self.config.borrow_mut();
        config.fullscreen = wanted;
        let _ = config.save();
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
            } else if let Some(button) = App::last_header(&header) {
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
        let Some(index) = self
            .nav_list
            .borrow()
            .as_ref()
            .and_then(|list| list.selected_row())
            .map(|row| row.index())
        else {
            return false;
        };
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == index)
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
    fn toggle_settings_mute(self: &Rc<Self>, index: i32) {
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == index)
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
        self.set_settings_switch(index, !muted);
        self.save_volume_soon();
    }

    /// Turns an output's delay on or off, keeping whatever it is set to.
    ///
    /// Off is how somebody checks whether a delay is helping: winding it to
    /// zero would answer the same question and lose the value they spent time
    /// finding.
    fn toggle_settings_offset(self: &Rc<Self>, index: i32) {
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == index)
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
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.set_offset_ms(role, self.config.borrow().applied_offset_ms(role));
        }
        scale.set_sensitive(on);
        value.set_text(&offset_label(self.config.borrow().applied_offset_ms(role)));
        value.set_sensitive(on);
        self.set_settings_switch(index, on);
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
        if let Slider::Offset(role) = kind
            && let Some(playback) = self.playback.borrow().as_ref()
        {
            playback.set_offset_ms(role, moved);
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

    /// Builds the main menu without installing it.
    ///
    /// Split out so the browser can raise the same page behind itself as a
    /// backdrop, which is what makes it read as a window opening over the
    /// menu rather than as another screen replacing it.
    fn build_menu_page(self: &Rc<Self>) -> (gtk::Box, gtk::ListBox) {
        let (page, list, back, slot) = list_page("Playback Options", false);
        // The arrow's slot is empty on this screen, so the mark takes it
        // rather than leaving a gap beside the title.
        slot.remove(&back);
        slot.append(&logo_image(self.scale.get()));

        let file = self.file.borrow().clone();
        let config = self.config.borrow();
        let tracks = self.tracks.borrow();

        let describe_track = |chosen: &Option<u32>| -> String {
            match chosen {
                None => "None".to_string(),
                Some(index) => tracks
                    .iter()
                    .find(|t| t.index == *index)
                    .map(describe_audio_track)
                    .unwrap_or_else(|| "None".to_string()),
            }
        };

        // Asked before the rows are built, not after: this is what fetches
        // Kodi's title as well as its resume point, and a row built ahead of
        // it would show the file name until something rebuilt the screen.
        let resume_at = self.resume_position();

        let has_file = file.is_some();
        let has_secondary = config.secondary_sink.is_some();
        let mut rows: Vec<(String, String, bool)> = vec![
            (
                "Video".to_string(),
                self.file_label()
                    .unwrap_or_else(|| "Choose a video…".to_string()),
                // Something else chose the video, so there is nothing to pick
                // here. The row stays, to name what is about to play.
                !self.external,
            ),
            (
                "Primary Audio Device".to_string(),
                config
                    .primary_sink
                    .clone()
                    .unwrap_or_else(|| "Not set".to_string()),
                true,
            ),
            (
                "Primary Audio Track".to_string(),
                if has_file {
                    describe_track(&self.primary_track.borrow())
                } else {
                    "—".to_string()
                },
                has_file,
            ),
            (
                "Secondary Audio Device".to_string(),
                config
                    .secondary_sink
                    .clone()
                    .unwrap_or_else(|| "None".to_string()),
                true,
            ),
            (
                "Secondary Audio Track".to_string(),
                if has_file && has_secondary {
                    describe_track(&self.secondary_track.borrow())
                } else {
                    "—".to_string()
                },
                has_file && has_secondary,
            ),
        ];
        // Its own section rather than sitting with the audio pair: the
        // subtitle language is an independent choice, and may be a third
        // language again or a repeat of either soundtrack.
        rows.push(("Subtitles".to_string(), self.describe_subtitle(), has_file));

        let can_play = has_file && config.primary_sink.is_some();
        drop(tracks);
        drop(config);

        for (label, value, enabled) in &rows {
            append_named(
                &list,
                &menu_row(label, value, *enabled),
                &row_name(label, value),
            );
        }

        // Extra space above the rows that begin a group, so the primary and
        // secondary pairs read as sections. Done with a margin on the row
        // rather than by inserting separator rows, which would be
        // focusable and interrupt keyboard navigation.
        for index in SECTION_STARTS {
            if let Some(row) = list.row_at_index(index) {
                row.add_css_class("tp-section-start");
            }
        }

        // Pinned below the scrolling list so it stays reachable however
        // long the list gets.
        let resumable = resume_at.is_some();

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        // The play buttons share the space between them; the gear keeps to
        // its own width at the end.
        let plays = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .homogeneous(true)
            .hexpand(true)
            .build();
        let mut play_buttons: Vec<gtk::Button> = Vec::new();

        // Resuming is the common case for a part-watched film, so it takes
        // the first position and the focus. Starting over is deliberate
        // enough to be worth its own button rather than a hidden modifier.
        if let Some(position) = resume_at {
            let resume = format!(
                "▶  Resume {}",
                crate::controls::format_time(gstreamer::ClockTime::from_nseconds(position))
            );
            for label in [resume.as_str(), "↻  Restart"] {
                let button = gtk::Button::with_label(label);
                button.add_css_class("tp-play");
                button.set_sensitive(can_play);
                plays.append(&button);
                play_buttons.push(button);
            }
        } else {
            let play = gtk::Button::with_label("▶  Play");
            play.add_css_class("tp-play");
            play.set_sensitive(can_play);
            plays.append(&play);
            play_buttons.push(play);
        }
        buttons.append(&plays);

        // Maximize and restore rather than the usual fullscreen pair, which
        // is absent from the icon theme on both platforms and would draw the
        // missing-image glyph.
        let fullscreen = gtk::Button::new();
        fullscreen.set_child(Some(&fullscreen_image(
            self.window.is_fullscreen(),
            self.scale.get(),
            self.dark.get(),
        )));
        fullscreen.add_css_class("tp-gear");
        // Still reachable by keyboard and controller, but a mouse click no
        // longer leaves it holding focus and lit up.
        fullscreen.set_focus_on_click(false);
        fullscreen.set_tooltip_text(Some("Toggle fullscreen"));
        name_it(&fullscreen, "Toggle fullscreen");
        // Left out entirely when fullscreen is not this viewer's to change: a
        // button that declines to do the one thing it offers is worse than no
        // button.
        if !self.locked_fullscreen {
            buttons.append(&fullscreen);
        }
        {
            let app = self.clone();
            fullscreen.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.toggle_fullscreen();
            });
        }
        {
            // Weak, so a menu rebuilt later leaves this handler harmless
            // rather than keeping the old button alive.
            let weak = fullscreen.downgrade();
            let scale = self.scale.get();
            let dark = self.dark.get();
            self.window.connect_fullscreened_notify(move |window| {
                if let Some(button) = weak.upgrade() {
                    button.set_child(Some(&fullscreen_image(window.is_fullscreen(), scale, dark)));
                }
            });
        }

        let gear = gtk::Button::from_icon_name("emblem-system-symbolic");
        gear.add_css_class("tp-gear");
        gear.set_focus_on_click(false);
        gear.set_tooltip_text(Some("Settings"));
        name_it(&gear, "Settings");
        buttons.append(&gear);
        // Rebuilt with the screen, so the list is replaced rather than added
        // to - the old buttons are gone and holding them would keep them
        // alive for nothing.
        *self.update_badges.borrow_mut() = vec![gear.clone()];
        self.draw_update_badge();
        page.append(&buttons);

        {
            let app = self.clone();
            gear.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }

        // Ordered as they sit on screen, so left and right walk along the
        // row and Down from the list lands on the first.
        let mut footer = play_buttons.clone();
        footer.push(fullscreen);
        footer.push(gear);

        {
            let app = self.clone();
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
                match row.index() {
                    0 => app.choose_video(),
                    1 => app.show_chooser(Setting::PrimaryDevice),
                    2 => app.show_chooser(Setting::PrimaryTrack),
                    3 => app.show_chooser(Setting::SecondaryDevice),
                    4 => app.show_chooser(Setting::SecondaryTrack),
                    5 => app.show_chooser(Setting::Subtitles),
                    _ => {}
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

        self.wire_navigation(&list, &[], &footer);
        (page, list)
    }

    fn show_menu(self: &Rc<Self>) {
        let (page, list) = self.build_menu_page();

        *self.screen.borrow_mut() = Screen::Menu;
        self.window.set_child(Some(&page));
        // Selected as well as focused: focus alone doesn't mark a row
        // selected, which left the list opening with nothing highlighted
        // until the first arrow key.
        let remembered = (*self.menu_row.borrow()).min(last_row_index(&list));
        if let Some(row) = list.row_at_index(remembered) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    // --- Choosers ------------------------------------------------------

    fn show_chooser(self: &Rc<Self>, setting: Setting) {
        let title = match setting {
            Setting::PrimaryDevice => "Primary Audio Device",
            Setting::PrimaryTrack => "Primary Audio Track",
            Setting::SecondaryDevice => "Secondary Audio Device",
            Setting::SecondaryTrack => "Secondary Audio Track",
            Setting::Subtitles => "Subtitles",
            Setting::Theme => "Theme",
            Setting::PrimaryLanguage => "Primary Language Preference",
            Setting::SecondaryLanguage => "Secondary Language Preference",
            Setting::SubtitleLanguage => "Subtitle Preference",
            Setting::SubtitleFont => "Subtitle Font",
        };
        let (page, list, back, _header) = list_page(title, true);

        // Entries are (display text, choice). `None` means the "None"
        // option, which every list offers except the primary device - an
        // output has to exist for anything to play.
        let mut entries: Vec<(String, Option<usize>)> = Vec::new();
        // The choice already in force, so the list opens on it rather than
        // at the top. Left as None when nothing is set, which lands on the
        // "None" row every list that has one begins with.
        let mut current: Option<usize> = None;
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                if setting == Setting::SecondaryDevice {
                    entries.push(("None".to_string(), None));
                }
                let configured = {
                    let config = self.config.borrow();
                    if setting == Setting::PrimaryDevice {
                        config.primary_sink.clone()
                    } else {
                        config.secondary_sink.clone()
                    }
                };
                match list_audio_output_devices() {
                    Ok(devices) => {
                        for (position, device) in devices.iter().enumerate() {
                            let name = device.display_name().to_string();
                            if configured.as_deref() == Some(name.as_str()) {
                                current = Some(position);
                            }
                            entries.push((name, Some(position)));
                        }
                    }
                    Err(e) => entries.push((format!("Error: {e}"), None)),
                }
            }
            Setting::Subtitles => {
                entries.push(("None".to_string(), None));
                let chosen = self.subtitle.borrow().clone();
                for (position, option) in self.subtitle_options.borrow().iter().enumerate() {
                    if chosen.as_ref() == Some(&option.choice()) {
                        current = Some(position);
                    }
                    entries.push((
                        crate::languages::describe_tag(option.label()),
                        Some(position),
                    ));
                }
            }
            Setting::PrimaryTrack | Setting::SecondaryTrack => {
                entries.push(("None".to_string(), None));
                let chosen = if setting == Setting::PrimaryTrack {
                    *self.primary_track.borrow()
                } else {
                    *self.secondary_track.borrow()
                };
                for (position, track) in self.tracks.borrow().iter().enumerate() {
                    if chosen == Some(track.index) {
                        current = Some(position);
                    }
                    entries.push((describe_audio_track(track), Some(position)));
                }
            }
            Setting::Theme => {
                current = Some(match self.config.borrow().theme {
                    crate::config::Theme::Auto => 0,
                    crate::config::Theme::Light => 1,
                    crate::config::Theme::Dark => 2,
                });
                for (position, name) in ["System", "Light", "Dark"].into_iter().enumerate() {
                    entries.push((name.to_string(), Some(position)));
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

        for (text, _) in &entries {
            append_named(&list, &chooser_row(text), text);
        }

        {
            let app = self.clone();
            let entries = entries.clone();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let Some((_, choice)) = entries.get(row.index() as usize) else {
                    return;
                };
                if !app.apply_choice(setting, *choice) {
                    app.leave_chooser();
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.leave_chooser());
        }

        // Up from the first row reaches the back arrow, so leaving is a
        // navigable step rather than only a button press.
        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);

        *self.screen.borrow_mut() = Screen::Chooser;
        self.window.set_child(Some(&page));
        // Opens on whatever is already selected, and grabbing focus scrolls
        // it into view, which matters for the language list.
        let opening = entries
            .iter()
            .position(|(_, choice)| *choice == current)
            .unwrap_or(0) as i32;
        if let Some(row) = list.row_at_index(opening) {
            // The setting in force, marked as such: scrolling a long list -
            // the languages especially - otherwise loses track of which one
            // is actually set the moment the cursor moves off it.
            row.add_css_class("tp-current");
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Arrow keys don't move focus out of a ListBox, so the boundary
    /// between the list and the button below it has to be bridged by hand -
    /// otherwise the button is unreachable without a pointer. Movements
    /// that would go past either end are swallowed, which also stops GTK
    /// reporting them as failed navigation.
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
        );
    }

    /// Returns to whichever screen the chooser was opened from.
    fn leave_chooser(self: &Rc<Self>) {
        if self.from_settings.replace(false) {
            self.show_settings();
        } else {
            self.show_menu();
        }
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
            .map(|option| crate::languages::describe_tag(option.label()))
            .unwrap_or_else(|| "None".to_string())
    }

    /// Returns whether it has already moved to another screen, in which case
    /// the caller must not navigate on top of it.
    fn apply_choice(self: &Rc<Self>, setting: Setting, choice: Option<usize>) -> bool {
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                let names: Vec<String> = list_audio_output_devices()
                    .map(|devices| {
                        devices
                            .iter()
                            .map(|d| d.display_name().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let picked = choice.and_then(|index| names.get(index).cloned());

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
                        // A secondary track without a device to play it on
                        // is meaningless, so clear it alongside.
                        if config.secondary_sink.is_none() {
                            *self.secondary_track.borrow_mut() = None;
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
                }

                if setting == Setting::PrimaryDevice {
                    let (enabled, device) = {
                        let config = self.config.borrow();
                        (config.sounds, config.primary_sink.clone())
                    };
                    *self.sounds.borrow_mut() = Sounds::new(enabled, device);
                }
            }
            Setting::Theme => {
                let theme = match choice {
                    Some(1) => crate::config::Theme::Light,
                    Some(2) => crate::config::Theme::Dark,
                    _ => crate::config::Theme::Auto,
                };
                {
                    let mut config = self.config.borrow_mut();
                    config.theme = theme;
                    let _ = config.save();
                }
                let was_dark = self.dark.get();
                let now_dark = appearance::apply_theme(theme);
                self.dark.set(now_dark);

                // GTK's Windows build will move to the dark theme but never
                // back, whatever is done to the settings. Everything worth
                // keeping is already written to disk, so restarting is
                // seamless, but it should still be asked for rather than
                // done out of the blue.
                if cfg!(target_os = "windows") && was_dark && !now_dark {
                    let app = self.clone();
                    self.show_confirm(
                        "Switching to the light theme needs a restart.\nRestart now?",
                        "Restart",
                        move || app.relaunch(),
                    );
                    return true;
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
                let tracks = self.tracks.borrow();
                let picked = choice.and_then(|index| tracks.get(index)).map(|t| t.index);
                drop(tracks);
                if setting == Setting::PrimaryTrack {
                    *self.primary_track.borrow_mut() = picked;
                } else {
                    *self.secondary_track.borrow_mut() = picked;
                }
                self.remember_tracks();
            }
        }
        false
    }

    /// Starts a fresh copy and closes this one. Playback cannot be running
    /// here, since the settings are only reachable from the menu, and the
    /// file, tracks, position and window state are all already saved.
    fn relaunch(&self) {
        match std::env::current_exe() {
            Ok(exe) => {
                if let Err(e) = std::process::Command::new(exe).spawn() {
                    eprintln!("Could not restart: {e}");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Could not find the executable to restart: {e}");
                return;
            }
        }
        self.window.close();
    }

    // --- File selection ------------------------------------------------

    fn open_file_chooser(self: &Rc<Self>, start: &std::path::Path) {
        // FileChooserNative rather than FileDialog: the latter needs GTK
        // 4.10, above this project's 4.6 baseline. It also gives the real
        // system file dialog on each platform.
        let chooser = gtk::FileChooserNative::new(
            Some("Choose a video"),
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
        filter.set_name(Some("Video files"));
        for extension in crate::browser::VIDEO_EXTENSIONS {
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
        *self.tracks.borrow_mut() = Vec::new();
        *self.subtitle_options.borrow_mut() = Vec::new();
        *self.primary_track.borrow_mut() = None;
        *self.secondary_track.borrow_mut() = None;
        *self.subtitle.borrow_mut() = None;
        *self.file.borrow_mut() = None;
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

        let tracks = media.audio;
        let options = crate::subtitles::options(source.local(), &media.subtitles);

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

        let saved = self
            .storage_key()
            .and_then(|key| crate::config::load_resume(&key))
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
        *self.subtitle.borrow_mut() =
            subtitle.filter(|choice| options.iter().any(|option| option.choice() == *choice));
        *self.subtitle_options.borrow_mut() = options;
        *self.tracks.borrow_mut() = tracks;
        *self.file.borrow_mut() = Some(source.clone());

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
        let existing = self.window.child();
        let backdrop: gtk::Widget = match existing {
            Some(child) => match child.downcast::<gtk::Overlay>() {
                Ok(overlay) => {
                    let under = overlay.child();
                    overlay.set_child(None::<&gtk::Widget>);
                    match under {
                        Some(under) => under,
                        None => self.build_menu_page().0.upcast(),
                    }
                }
                Err(child) => {
                    self.window.set_child(None::<&gtk::Widget>);
                    child
                }
            },
            None => self.build_menu_page().0.upcast(),
        };
        // Not just visually behind: an insensitive page cannot take focus, so
        // neither tab nor the gamepad can reach what is underneath.
        backdrop.set_sensitive(false);

        let scrim = gtk::Box::builder().css_classes(["tp-scrim"]).build();

        page.add_css_class("tp-modal");
        if self.dark.get() {
            page.add_css_class("tp-modal-dark");
        }

        let overlay = gtk::Overlay::new();
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
        cancel.add_css_class("tp-cancel");
        let open = gtk::Button::with_label("Open");
        open.add_css_class("tp-button");
        open.add_css_class("tp-play");
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
        cancel.add_css_class("tp-cancel");
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
        let directory = crate::browser::rooted(directory);
        let page = self.browser_page(&directory, Browse::Videos);
        let entries = browser_entries(&directory, Browse::Videos);

        // The way out alone in the middle. Choosing here is opening a video,
        // which the rows themselves do.
        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&page.browse));
        footer.set_center_widget(Some(&page.cancel));
        page.page.append(&footer);

        fill_browser_list(&page.list, &entries);

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
                    Some(path) => {
                        let source = Source::File(path.to_path_buf());
                        match app.set_file(&source) {
                            Ok(()) => app.show_menu(),
                            Err(e) => app.show_source_error(&source, &e, false),
                        }
                    }
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

        {
            let mut config = self.config.borrow_mut();
            config.last_folder = Some(directory.to_path_buf());
            let _ = config.save();
        }

        // The trail alone now that the arrow has gone: left from the current
        // folder simply walks back up it.
        self.wire_navigation(&page.list, &page.crumbs, std::slice::from_ref(&page.cancel));
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
        let browse_icon = gtk::Image::from_icon_name("folder-symbolic");
        browse_icon.set_pixel_size((24.0 * self.scale.get()).round() as i32);
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
            });
        }

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.add_css_class("tp-cancel");

        BrowserPage {
            page,
            list,
            crumbs: crumb_buttons,
            browse,
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
        let (page, list, back, _header) = list_page("Choose a Video", true);

        for (label, value) in [
            (
                "Browse for a File",
                "On this machine or a shared network drive",
            ),
            (
                "Enter a URL",
                "A link to a video, such as one from a media server",
            ),
        ] {
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
                match row.index() {
                    0 => app.browse_for_file(),
                    1 => app.show_paste_uri(),
                    _ => {}
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.show_menu());
        }

        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        *self.screen.borrow_mut() = Screen::VideoSource;
        self.window.set_child(Some(&page));
        if let Some(row) = list.row_at_index(0) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Opens the file browser where browsing last stopped.
    ///
    /// Always the built-in browser. Guessing from the last input was
    /// unpredictable: the same button opened different things depending on
    /// what you had touched. The system dialog is still reachable, from a
    /// pointer-only button in the footer.
    fn browse_for_file(self: &Rc<Self>) {
        let (remembered, last_video) = {
            let config = self.config.borrow();
            (config.last_folder.clone(), config.last_video.clone())
        };
        let start = crate::browser::start_location(remembered.as_deref(), last_video.as_deref());
        self.show_browser(&start, None);
    }

    // --- Settings ------------------------------------------------------

    /// Everything that applies to the application rather than to the video
    /// currently loaded. Reached from the gear in the footer.
    fn show_settings(self: &Rc<Self>) {
        let (page, list, back, _header) = list_page("Settings", true);

        let rows = {
            let config = self.config.borrow();
            let language = |code: &Option<String>, unset: &str| match code {
                Some(code) => crate::languages::name_for(code),
                None => unset.to_string(),
            };
            [
                (
                    "Theme".to_string(),
                    match config.theme {
                        crate::config::Theme::Auto => "System".to_string(),
                        crate::config::Theme::Light => "Light".to_string(),
                        crate::config::Theme::Dark => "Dark".to_string(),
                    },
                    true,
                ),
                (
                    "Interface Size".to_string(),
                    match config.ui_scale {
                        Some(scale) => format!("{scale}x"),
                        None => format!("Automatic ({}x)", self.scale.get()),
                    },
                    true,
                ),
                (
                    "Navigation Sounds".to_string(),
                    if config.sounds { "On" } else { "Off" }.to_string(),
                    true,
                ),
                (
                    "Primary Audio Device".to_string(),
                    config
                        .primary_sink
                        .clone()
                        .unwrap_or_else(|| "Not set".to_string()),
                    true,
                ),
                (
                    "Preferred Language".to_string(),
                    language(&config.primary_language, "First track"),
                    true,
                ),
                (
                    "Prefer Audio Description".to_string(),
                    if config.primary_audio_description {
                        "Yes"
                    } else {
                        "No"
                    }
                    .to_string(),
                    true,
                ),
                (
                    "Volume".to_string(),
                    volume_label(config.volume("primary"), config.muted("primary")),
                    true,
                ),
                (
                    "Audio Sync".to_string(),
                    offset_label(config.applied_offset_ms("primary")),
                    true,
                ),
                (
                    "Secondary Audio Device".to_string(),
                    config
                        .secondary_sink
                        .clone()
                        .unwrap_or_else(|| "None".to_string()),
                    true,
                ),
                (
                    "Preferred Language".to_string(),
                    language(&config.secondary_language, "Second track"),
                    true,
                ),
                (
                    "Prefer Audio Description".to_string(),
                    if config.secondary_audio_description {
                        "Yes"
                    } else {
                        "No"
                    }
                    .to_string(),
                    true,
                ),
                (
                    "Volume".to_string(),
                    volume_label(config.volume("secondary"), config.muted("secondary")),
                    true,
                ),
                (
                    "Audio Sync".to_string(),
                    offset_label(config.applied_offset_ms("secondary")),
                    true,
                ),
                (
                    "Subtitle Preference".to_string(),
                    crate::subtitles::describe(config.subtitle_language.as_deref()),
                    true,
                ),
                (
                    "Subtitle Size".to_string(),
                    config
                        .subtitle_size
                        .unwrap_or(crate::pipeline::DEFAULT_SUBTITLE_SIZE)
                        .to_string(),
                    true,
                ),
                (
                    "Subtitle Font".to_string(),
                    config
                        .subtitle_font
                        .clone()
                        .unwrap_or_else(|| crate::pipeline::DEFAULT_SUBTITLE_FONT.to_string()),
                    true,
                ),
                (
                    "Resume Threshold".to_string(),
                    format!("{}%", config.resume_min_percent().round()),
                    true,
                ),
                (
                    "Watched Threshold".to_string(),
                    format!("{}%", config.watched_percent().round()),
                    true,
                ),
                ("Clear Saved Playback Data".to_string(), String::new(), true),
                (
                    "Kodi".to_string(),
                    // Deliberately blank. Saying what Kodi is set to means
                    // finding every Kodi on the machine and reading its
                    // configuration file, and this row is passed by everyone
                    // who came to Settings for something else. The answer is
                    // on the screen it opens, which is where it is wanted.
                    String::new(),
                    // Always reachable: with nothing configured, this is where
                    // configuring starts.
                    true,
                ),
                ("About TinePlayer".to_string(), String::new(), true),
                ("Third Party Notices".to_string(), String::new(), true),
                (
                    "Check for updates".to_string(),
                    if config.check_for_updates {
                        "On"
                    } else {
                        "Off"
                    }
                    .to_string(),
                    true,
                ),
                (self.version_label(), self.version_status(), true),
            ]
            .to_vec()
        };
        debug_assert_eq!(rows.len(), SETTINGS_ORDER.len());

        for (label, value, enabled) in &rows {
            append_named(
                &list,
                &menu_row(label, value, *enabled),
                &row_name(label, value),
            );
        }
        // Swapped in over the ordinary rows built above, which keeps the row
        // count and the section and indent indices in one place rather than
        // splitting the list into two kinds of thing to build.
        // A fifth of what the window has, so the bar is a consistent share
        // of the screen whether that is a laptop or a television. The monitor
        // stands in before the window has been given a size.
        let slider_width = match self.window.width() {
            0 => appearance::monitor_for_window(&self.window)
                .map(|monitor| monitor.geometry().width())
                .unwrap_or(1920),
            width => width,
        } / 5;
        self.settings_switches.borrow_mut().clear();
        // By name, not by number. These were written as 2, 5 and 9, and
        // inserting the sync rows moved the secondary description row to 10
        // while the 9 stayed put - so its switch was built over Preferred
        // Language, which then had no row of its own at all.
        for (index, label, on) in [
            (ROW_SOUNDS, "Navigation Sounds", self.config.borrow().sounds),
            (
                ROW_PRIMARY_DESCRIPTION,
                "Prefer Audio Description",
                self.config.borrow().primary_audio_description,
            ),
            (
                ROW_SECONDARY_DESCRIPTION,
                "Prefer Audio Description",
                self.config.borrow().secondary_audio_description,
            ),
            (
                UPDATE_SWITCH_ROW,
                "Check for updates",
                self.config.borrow().check_for_updates,
            ),
        ] {
            let (widget, switch) = switch_row(label, on);
            if let Some(row) = list.row_at_index(index) {
                row.set_child(Some(&widget));
            }
            self.settings_switches.borrow_mut().push((index, switch));
        }

        self.settings_sliders.borrow_mut().clear();
        for (index, kind, label) in [
            (ROW_INTERFACE_SCALE, Slider::Scale, "Interface Size"),
            (ROW_SUBTITLE_SIZE, Slider::SubtitleSize, "Subtitle Size"),
            (ROW_PRIMARY_VOLUME, Slider::Volume("primary"), "Volume"),
            (ROW_PRIMARY_SYNC, Slider::Offset("primary"), "Audio Sync"),
            (ROW_SECONDARY_VOLUME, Slider::Volume("secondary"), "Volume"),
            (
                ROW_SECONDARY_SYNC,
                Slider::Offset("secondary"),
                "Audio Sync",
            ),
            (
                ROW_RESUME_THRESHOLD,
                Slider::ResumeThreshold,
                "Resume Threshold",
            ),
            (
                ROW_WATCHED_THRESHOLD,
                Slider::WatchedThreshold,
                "Watched Threshold",
            ),
        ] {
            let (now, reading) = self.slider_state(kind);
            // A switch on the two that can be turned off, and none on the
            // thresholds, which have no off - a resume threshold of "not
            // applied" is the same as zero.
            let toggle = match kind {
                Slider::Volume(role) => Some(!self.config.borrow().muted(role)),
                Slider::Offset(role) => Some(self.config.borrow().offset_on(role)),
                // On means the size is worked out from the screen, which is
                // the one switch here that turns the bar beside it off rather
                // than on.
                Slider::Scale => Some(self.config.borrow().ui_scale.is_none()),
                _ => None,
            };
            let (widget, scale, value, switch) =
                slider_row(label, slider_width, kind.range(), now, &reading, toggle);
            if let Some(row) = list.row_at_index(index) {
                row.set_child(Some(&widget));
            }
            if let Some(switch) = switch {
                self.settings_switches.borrow_mut().push((index, switch));
            }
            if kind == Slider::Scale {
                let by_hand = self.config.borrow().ui_scale.is_some();
                scale.set_sensitive(by_hand);
                value.set_sensitive(by_hand);
            }
            {
                let app = self.clone();
                let value = value.clone();
                scale.connect_change_value(move |_, scroll, moved| {
                    app.set_slider(kind, moved, &value);
                    if kind == Slider::Scale {
                        // A drag reports Jump, over and over, while the
                        // pointer holds the bar. Anything else - a step, a
                        // page, a scroll wheel - is finished by the time it
                        // arrives and can be drawn straight away.
                        if scroll == gtk::ScrollType::Jump {
                            app.wanted_scale.set(Some(moved));
                        } else {
                            app.apply_scale(moved);
                        }
                    }
                    glib::Propagation::Proceed
                });
            }
            // Let go of, and only then redrawn. Watched rather than handled,
            // so the bar keeps its own grip on the pointer while it is being
            // dragged.
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
                scale.add_controller(watcher);
            }
            self.settings_sliders
                .borrow_mut()
                .push((index, kind, scale, value));
        }

        // Each switch reports its own presses, now that it takes them rather
        // than letting them fall through to the row. Guarded against the
        // moves made from here when the same setting is worked another way.
        for (index, switch) in self.settings_switches.borrow().iter() {
            let app = self.clone();
            let index = *index;
            switch.connect_state_set(move |_, _| {
                if !app.settling_switch.get() {
                    app.sounds.borrow().click();
                    app.apply_switch_row(index);
                }
                glib::Propagation::Proceed
            });
        }

        // Watched in the capture phase, so a press is known about before
        // anything else handles it. Cleared on the way out rather than on
        // release, because the row is activated in between - and a press that
        // never activates a row must not leave the next key press looking
        // like a click.
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

        for index in SETTINGS_SECTIONS {
            if let Some(row) = list.row_at_index(index) {
                row.add_css_class("tp-section-start");
            }
        }
        for index in SETTINGS_SUBROWS {
            if let Some(row) = list.row_at_index(index) {
                row.add_css_class("tp-subrow");
            }
        }

        *self.settings_list.borrow_mut() = Some(list.clone());

        // Reaching the row is what takes the mark off the settings button:
        // arriving on it is the moment somebody has been told, and pressing
        // it should not be required to stop being nagged about something
        // already seen. Attached whether or not there is anything new, since
        // a check finishing while this screen is open can make there be -
        // acknowledging nothing is harmless.
        if let Some(row) = list.row_at_index(UPDATE_STATUS_ROW) {
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
        self.refresh_version_row();

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
                // A switch is worked by pressing the switch, not by clicking
                // the row it sits on: the row is a wide target, and hitting it
                // on the way past should not change a setting. Enter on the
                // selected row still does, which arrives here with nothing
                // having been clicked.
                if app.clicked_row.replace(false) && row_has_switch(row.index()) {
                    return;
                }
                // A switch row is answered by the switch, which plays its own
                // click when it moves. Playing one here too would double it.
                if !row_has_switch(row.index()) {
                    app.sounds.borrow().click();
                }
                // Remembered so returning from a chooser lands back on the
                // row it was opened from, as the main menu does.
                *app.settings_row.borrow_mut() = row.index();
                match row.index() {
                    ROW_THEME => app.open_setting(Setting::Theme),
                    ROW_INTERFACE_SCALE => app.work_switch_row(ROW_INTERFACE_SCALE),
                    ROW_SOUNDS => app.work_switch_row(ROW_SOUNDS),
                    ROW_PRIMARY_DEVICE => app.open_setting(Setting::PrimaryDevice),
                    ROW_PRIMARY_LANGUAGE => app.open_setting(Setting::PrimaryLanguage),
                    ROW_PRIMARY_DESCRIPTION => app.work_switch_row(ROW_PRIMARY_DESCRIPTION),
                    ROW_PRIMARY_VOLUME => app.work_switch_row(ROW_PRIMARY_VOLUME),
                    ROW_PRIMARY_SYNC => app.work_switch_row(ROW_PRIMARY_SYNC),
                    ROW_SECONDARY_DEVICE => app.open_setting(Setting::SecondaryDevice),
                    ROW_SECONDARY_LANGUAGE => app.open_setting(Setting::SecondaryLanguage),
                    ROW_SECONDARY_DESCRIPTION => app.work_switch_row(ROW_SECONDARY_DESCRIPTION),
                    ROW_SECONDARY_VOLUME => app.work_switch_row(ROW_SECONDARY_VOLUME),
                    ROW_SECONDARY_SYNC => app.work_switch_row(ROW_SECONDARY_SYNC),
                    ROW_SUBTITLE_LANGUAGE => app.open_setting(Setting::SubtitleLanguage),
                    ROW_SUBTITLE_FONT => app.open_setting(Setting::SubtitleFont),
                    ROW_CLEAR_DATA => app.confirm_clear_data(),
                    ROW_KODI => app.show_kodi(),
                    ROW_ABOUT => app.show_about(),
                    ROW_NOTICES => app.show_notices(),
                    UPDATE_SWITCH_ROW => app.work_switch_row(UPDATE_SWITCH_ROW),
                    UPDATE_STATUS_ROW => app.open_release_page(),
                    _ => {}
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.show_menu());
        }

        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        *self.screen.borrow_mut() = Screen::Settings;
        self.window.set_child(Some(&page));
        let remembered = (*self.settings_row.borrow()).min(last_row_index(&list));
        if let Some(row) = list.row_at_index(remembered) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Opens a chooser and remembers to come back here rather than to the
    /// main menu.
    fn open_setting(self: &Rc<Self>, setting: Setting) {
        self.from_settings.set(true);
        self.show_chooser(setting);
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
                ROW_PRIMARY_DESCRIPTION
            } else {
                ROW_SECONDARY_DESCRIPTION
            },
            on,
        );
    }

    /// Moves the switch on a settings row to match what it now reports.
    fn set_settings_switch(&self, index: i32, on: bool) {
        self.settling_switch.set(true);
        if let Some((_, switch)) = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == index)
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
    fn work_switch_row(self: &Rc<Self>, index: i32) {
        let switch = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == index)
            .map(|(_, switch)| switch.clone());
        match switch {
            // Its own handler carries on from here, as it does for a click.
            Some(switch) => {
                switch.activate();
            }
            None => self.apply_switch_row(index),
        }
    }

    /// What a switch row actually changes, once something has asked for it.
    fn apply_switch_row(self: &Rc<Self>, index: i32) {
        match index {
            ROW_INTERFACE_SCALE => self.toggle_automatic_scale(),
            ROW_SOUNDS => self.toggle_sounds(),
            ROW_PRIMARY_DESCRIPTION => self.toggle_audio_description(true),
            ROW_SECONDARY_DESCRIPTION => self.toggle_audio_description(false),
            ROW_PRIMARY_VOLUME | ROW_SECONDARY_VOLUME => self.toggle_settings_mute(index),
            ROW_PRIMARY_SYNC | ROW_SECONDARY_SYNC => self.toggle_settings_offset(index),
            UPDATE_SWITCH_ROW => self.toggle_update_checks(),
            _ => {}
        }
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
        self.set_settings_switch(UPDATE_SWITCH_ROW, on);
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
        let list = self.settings_list.borrow().clone();
        let Some(row) = list.and_then(|list| list.row_at_index(UPDATE_STATUS_ROW)) else {
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
        self.set_settings_switch(ROW_SOUNDS, enabled);
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
            .find(|(row, ..)| *row == ROW_INTERFACE_SCALE)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        if let Some((kind, scale, value)) = found {
            let (now, reading) = self.slider_state(kind);
            scale.set_value(now);
            value.set_text(&reading);
            scale.set_sensitive(!now_automatic);
            value.set_sensitive(!now_automatic);
        }
        self.set_settings_switch(ROW_INTERFACE_SCALE, now_automatic);
    }

    /// Redraws the interface at the size the bar is now at.
    fn apply_scale(&self, steps: f64) {
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
    fn follow_automatic_scale(&self, window: &gtk::ApplicationWindow) {
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
    fn restyle(&self, scale: f64) {
        self.scale.set(scale);
        self.styles
            .load_from_data(&style_css(scale, self.dark.get()));
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
        adjustment.set_value(moved.clamp(
            adjustment.lower(),
            (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
        ));
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

        fill_browser_list(&page.list, &entries);

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
        // The red is a warning, so it belongs on whichever button does the
        // damage. Backing out of something destructive is the safe choice and
        // should not be the one painted like a hazard.
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let destructive = confirm.destructive;
        let confirm = gtk::Button::with_label(confirm.label);
        confirm.add_css_class("tp-button");
        if destructive {
            confirm.add_css_class("tp-cancel");
        } else {
            cancel.add_css_class("tp-cancel");
        }
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
        // was never configured, whatever left the track set.
        let primary = *self.primary_track.borrow();
        let secondary = if self.config.borrow().secondary_sink.is_some() {
            *self.secondary_track.borrow()
        } else {
            None
        };
        let subtitle = self.subtitle.borrow().clone();
        if let Some(key) = self.storage_key() {
            crate::config::save_tracks(&key, primary, secondary, subtitle.clone());
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
            primary,
            secondary,
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
                    self.dark.get(),
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
                        if let Some(playback) = app.playback.borrow().as_ref() {
                            playback
                                .set_offset_ms(role, app.config.borrow().applied_offset_ms(role));
                        }
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
        quit.add_css_class("tp-cancel");
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
    page.append(&scroller);

    (page, list, back, slot)
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

    let image = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON)) {
        image.set_paintable(Some(&texture));
    }
    image.set_pixel_size((26.0 * scale).round() as i32);
    image
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
const ICON_PX: f64 = 24.0;

pub fn sync_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/sync.png");

    let image = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON)) {
        image.set_paintable(Some(&texture));
    }
    // The size the panel's other icons come out at, set here because
    // `-gtk-icon-size` sizes icon names and this is a paintable, so the CSS
    // that catches the mute button passes over this one. A pixel or two out
    // and the button beside it is a different width, which moves the start of
    // the bar and leaves the two bars different lengths.
    image.set_pixel_size((ICON_PX * scale).round() as i32);
    image
}

/// The fullscreen mark, in the direction it will take you.
///
/// Drawn for this application rather than taken from the icon theme: the
/// bundled theme has 157 icons and none of them mean fullscreen. The nearest,
/// `window-maximize-symbolic`, is a small square that reads as "maximize".
///
/// Drawn twice in each direction, once in each theme's foreground color,
/// because an embedded image cannot be recoloured the way a symbolic icon is.
/// A single compromise gray read poorly against both.
pub fn fullscreen_image(fullscreen: bool, scale: f64, dark: bool) -> gtk::Image {
    const ENTER_LIGHT: &[u8] = include_bytes!("../data/ui/fullscreen-light.png");
    const ENTER_DARK: &[u8] = include_bytes!("../data/ui/fullscreen-dark.png");
    const LEAVE_LIGHT: &[u8] = include_bytes!("../data/ui/restore-light.png");
    const LEAVE_DARK: &[u8] = include_bytes!("../data/ui/restore-dark.png");

    let bytes = match (fullscreen, dark) {
        (true, true) => LEAVE_DARK,
        (true, false) => LEAVE_LIGHT,
        (false, true) => ENTER_DARK,
        (false, false) => ENTER_LIGHT,
    };
    let image = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)) {
        image.set_paintable(Some(&texture));
    }
    image.set_pixel_size((26.0 * scale).round() as i32);
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

/// How a settings row reads aloud: the setting, then what it is set to.
/// Whether a settings row carries a switch, and so is worked by the switch
/// rather than by a click anywhere along it.
fn row_has_switch(index: i32) -> bool {
    matches!(
        index,
        ROW_INTERFACE_SCALE
            | ROW_SOUNDS
            | ROW_PRIMARY_DESCRIPTION
            | ROW_SECONDARY_DESCRIPTION
            | ROW_PRIMARY_VOLUME
            | ROW_SECONDARY_VOLUME
            | ROW_PRIMARY_SYNC
            | ROW_SECONDARY_SYNC
            | UPDATE_SWITCH_ROW
    )
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
    // The row itself, so a screen reader has a focus change to announce.
    row.grab_focus();
    // Setting the window's child maps the new page there and then, so by the
    // time a screen picks its row the row is usually mapped already and
    // waiting for the signal would be waiting forever. Only the first screen
    // of a session arrives unmapped, because the window itself is not up yet.
    if row.is_mapped() {
        after_layout(row);
    } else {
        row.connect_map(after_layout);
    }
}

/// Runs once the page has been through a layout pass, which is when a row
/// finally knows where it is and the scroller knows how much of it there is
/// to move.
fn after_layout(row: &gtk::ListBoxRow) {
    let row = row.clone();
    glib::idle_add_local_once(move || {
        row.grab_focus();
        show_row(&row);
    });
}

/// Moves the scroller so a row is on screen, a third of the way down rather
/// than jammed against an edge: a row against the top of the frame looks like
/// the first row, which is exactly the confusion being avoided.
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

    let Some((_, top)) = row.translate_coordinates(&list, 0.0, 0.0) else {
        return;
    };
    let adjustment = scroller.vadjustment();
    let page = adjustment.page_size();
    // Already on screen: leave it where it is rather than jumping the page
    // about under someone who can see the row perfectly well.
    let bottom = top + f64::from(row.height());
    if top >= adjustment.value() && bottom <= adjustment.value() + page {
        return;
    }
    let wanted = top - page / 3.0;
    adjustment.set_value(wanted.clamp(adjustment.lower(), (adjustment.upper() - page).max(0.0)));
}

/// Where a stored language code sits in the offered list.
fn language_position(code: Option<&str>) -> Option<usize> {
    let code = code?;
    crate::languages::LANGUAGES
        .iter()
        .position(|(stored, _, _, _)| *stored == code)
}

fn last_row_index(list: &gtk::ListBox) -> i32 {
    let mut last = 0;
    while list.row_at_index(last + 1).is_some() {
        last += 1;
    }
    last
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

/// A menu row: what the setting is on the left, its current value and a
/// chevron on the right.
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
    Folders,
}

impl Browse {
    /// Whether only folders are worth showing. A folder is being chosen here,
    /// so the files inside it would be a list of things that cannot be picked.
    fn folders_only(self) -> bool {
        self == Browse::Folders
    }
}

/// The parts of a browsing screen its caller still has to finish.
struct BrowserPage {
    page: gtk::Box,
    list: gtk::ListBox,
    crumbs: Vec<gtk::Button>,
    browse: gtk::Button,
    cancel: gtk::Button,
}

/// One row of a listing: what it says, what it is drawn with, where it goes,
/// and how it reads aloud. A path of `None` is the way up.
#[derive(Clone)]
struct BrowserEntry {
    label: String,
    icon: &'static str,
    path: Option<std::path::PathBuf>,
    spoken: String,
    /// Something to read rather than somewhere to go: the line saying a
    /// folder holds nothing worth listing.
    notice: bool,
}

/// Fills a listing, and leaves the notice as a line of text.
///
/// A notice drawn like an entry invites being chosen, and choosing it walked
/// back up a level - which reads as a broken listing rather than as an empty
/// folder. Centred, dimmer, without an icon, and passed over by the cursor.
fn fill_browser_list(list: &gtk::ListBox, entries: &[BrowserEntry]) {
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
            append_named(list, &browser_row(entry.icon, &entry.label), &entry.spoken);
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
            label: "..".to_string(),
            icon: "folder-symbolic",
            path: None,
            spoken: match parent.file_name() {
                Some(name) => format!("Up to {}", name.to_string_lossy()),
                None => "Up to the list of drives".to_string(),
            },
            notice: false,
        });
    }
    for entry in crate::browser::read(directory) {
        if mode.folders_only() && !entry.is_dir {
            continue;
        }
        // A play mark rather than a generic video one: that icon is not in
        // this theme and fell back to the missing-image glyph, which reads as
        // a warning about the file itself.
        let icon = if entry.is_dir {
            "folder-symbolic"
        } else {
            "media-playback-start-symbolic"
        };
        entries.push(BrowserEntry {
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
            label: "Nothing here".to_string(),
            icon: "",
            path: None,
            spoken: "Nothing here".to_string(),
            notice: true,
        });
    }
    entries
}

/// A browser row: an icon from the desktop's own set, then the name.
///
/// Icons rather than emoji, because emoji depend on a color font being
/// installed. The Pi has none, so a folder character rendered as an empty box
/// with the codepoint inside it.
fn browser_row(icon: &str, text: &str) -> gtk::Box {
    // The padding goes on the row rather than the label, so it applies
    // before the icon as well as around the text.
    //
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["tp-row"])
        .build();

    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("tp-row-icon");
    row.append(&image);

    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    row
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
fn default_window_size(scale: f64, monitor: Option<&gdk::Monitor>) -> (i32, i32) {
    const BASE_WIDTH: f64 = 1100.0;
    const BASE_HEIGHT: f64 = 700.0;
    const MAX_FRACTION: f64 = 0.9;

    let mut width = BASE_WIDTH * scale;
    let mut height = BASE_HEIGHT * scale;
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

fn style_css(scale: f64, dark: bool) -> String {
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
        .tp-button, .tp-play {{ font-size: {row}px; padding: {pad_v}px {pad_h}px; }}
        .tp-play {{ font-weight: bold; }}
        /* Backing out, on every screen that offers it. A literal red for the
           same reason the highlight is literal: a theme name that does not
           exist makes the whole declaration fail to parse. */
        .tp-cancel {{
            background-image: none;
            background-color: #c01c28;
            color: #ffffff;
        }}
        .tp-cancel:hover {{ background-color: #a51d2d; }}
        /* Beside a main action rather than being one: smaller type and far
           less padding than the buttons it sits with, so it reads as a way to
           reach something else rather than as the thing to press. */
        .tp-secondary {{ font-size: {small}px; padding: {tight_v}px {tight_h}px; }}
        .tp-menu > row {{ border-radius: {radius}px; }}
        /* Gray rather than a theme color, so it lifts off the background in
           both light and dark without needing two rules. */
        .tp-menu > row:hover {{ background-color: rgba(128, 128, 128, 0.18); }}
        .tp-menu:focus-within > row:selected:hover {{ background-color: {highlight}; }}
        .tp-menu > row.tp-section-start {{ margin-top: {section}px; }}
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
            background-color: {highlight};
            color: #ffffff;
        }}
        .tp-menu:focus-within > row:selected .tp-value,
        .tp-menu:focus-within > row:selected .tp-chevron {{
            color: #ffffff;
            opacity: 0.85;
        }}
        /* A ring rather than a fill. Recoloring a focused button changes what
           it looks like it does - a Cancel that turns blue reads as the one
           to press - and beside another button the pair stop looking like
           peers. An inset shadow rather than a border so nothing shifts, and
           rather than an outline so it follows the rounded corners. */
        button:focus {{
            box-shadow: inset 0 0 0 {mark}px {highlight};
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
        .tp-back:focus {{
            background-image: none;
            background-color: {highlight};
            opacity: 1;
        }}
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
            background-color: #fafafa;
            border: 1px solid rgba(0, 0, 0, 0.2);
            border-radius: {radius}px;
            margin: {modal}px;
            padding: {modal_pad}px;
        }}
        .tp-modal-dark {{
            background-color: #1e1e1e;
            border-color: rgba(255, 255, 255, 0.14);
        }}
        /* Taller than a stock entry: this is the one thing on its panel, and
           it is read from the same distance as everything else. */
        .tp-path {{ font-size: {row}px; padding: {pad_v}px {pad_h}px; }}
        .tp-subtitles-button {{ opacity: 0.45; }}
        .tp-subtitles-on {{ opacity: 1; }}
        .tp-subtitles-button:disabled {{ opacity: 0.2; }}
        .tp-progress {{ min-height: {bar}px; }}
        .tp-progress progress {{ background-color: {highlight}; }}
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
        .tp-gear image {{ -gtk-icon-size: {icon}px; }}
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
        outline = px(2.0).max(1),
        handle = px(18.0),
        // Bright on dark; on light a white knob held in by its ring rather
        // than a dark disc, which read as heavy against a white row and
        // heavier still in a column of them.
        knob = if dark { "#dcdcdc" } else { "#ffffff" },
        fill = if dark { "#b9b9b9" } else { "#8e8e8e" },
        trough = if dark {
            "rgba(255, 255, 255, 0.13)"
        } else {
            "rgba(0, 0, 0, 0.11)"
        },
        // On light the ring is what makes a white knob visible at all, so
        // it carries the contrast the knob itself no longer does.
        knob_edge = if dark {
            "rgba(0, 0, 0, 0.55)"
        } else {
            "rgba(0, 0, 0, 0.38)"
        },
        edge = px(1.0).max(1),
        switch_w = px(64.0),
        switch_h = px(32.0),
        slider = px(26.0),
        section = px(28.0),
        subrow = px(28.0),
        mark = px(4.0),
        icon = px(ICON_PX),
        icon_main = px(38.4),
        crumb_pad = px(6.0),
        leading = px(38.0),
        back_icon = px(22.0),
        row_icon = px(18.0),
        bar = px(6.0),
        // A literal color rather than a theme name: GTK's named colors
        // differ between themes and libadwaita, and an undefined one makes
        // the whole declaration fail to parse - which silently leaves the
        // highlighted row unreadable. Both foreground and background are
        // set for the same reason: overriding only the background left the
        // theme's white selection text on a pale color.
        modal = px(48.0),
        modal_pad = px(16.0),
        highlight = "#3584e4",
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

    /// One row, one position. A row constant that is duplicated or skipped
    /// means two controls built onto the same row and another left as a plain
    /// line of text - which is what a stale number did to Preferred Language,
    /// and it looked like a missing setting rather than like a bug.
    #[test]
    fn every_row_has_one_position() {
        let mut positions = SETTINGS_ORDER;
        positions.sort_unstable();
        let expected: Vec<i32> = (0..SETTINGS_ROWS as i32).collect();
        assert_eq!(positions.to_vec(), expected);
    }

    /// The version sits last, under the switch that decides whether anything
    /// is said about newer ones.
    #[test]
    fn the_version_row_comes_last_and_follows_the_switch() {
        assert_eq!(UPDATE_STATUS_ROW, SETTINGS_ROWS as i32 - 1);
        assert_eq!(UPDATE_SWITCH_ROW, UPDATE_STATUS_ROW - 1);
    }

    /// Headings and the rows indented under them are drawn differently, so a
    /// row named as both would be asking for two contradictory things.
    #[test]
    fn no_row_is_both_a_heading_and_indented_under_one() {
        for row in SETTINGS_SECTIONS {
            assert!(
                !SETTINGS_SUBROWS.contains(&row),
                "row {row} is both a section and a subrow"
            );
        }
        for row in SETTINGS_SECTIONS.iter().chain(SETTINGS_SUBROWS.iter()) {
            assert!(SETTINGS_ORDER.contains(row), "row {row} is not built");
        }
    }
}
