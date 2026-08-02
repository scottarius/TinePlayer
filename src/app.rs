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
    InterfaceScale,
    PrimaryLanguage,
    SecondaryLanguage,
    SubtitleLanguage,
    SubtitleSize,
    SubtitleFont,
}

/// What a slider on the settings screen is setting. All of them work in
/// percentages, which is what makes one set of arithmetic serve the lot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slider {
    /// The level for one output, by role.
    Volume(&'static str),
    ResumeThreshold,
    WatchedThreshold,
}

impl Slider {
    /// How far one press moves it. Levels move in fives, being a rough
    /// setting anyone can hear; the thresholds move by one, since the useful
    /// range of each is narrow enough that fives would be three choices.
    fn step(self) -> f64 {
        match self {
            Slider::Volume(_) => 5.0,
            _ => 1.0,
        }
    }

    fn range(self) -> std::ops::RangeInclusive<f64> {
        match self {
            Slider::Volume(_) => 0.0..=100.0,
            // Below one per cent is indistinguishable from starting over, and
            // past a quarter of a film nothing would ever be resumable.
            Slider::ResumeThreshold => 1.0..=25.0,
            // Anything under half is not watching it, and a hundred means
            // sitting through the credits to be counted.
            Slider::WatchedThreshold => 50.0..=100.0,
        }
    }
}

/// Rows of the settings screen, in the order they appear.
/// Rows a page jump covers, roughly a screenful at the default size. What
/// makes a folder of a hundred films navigable without a hundred presses.
const PAGE_ROWS: i32 = 8;

const SETTINGS_ROWS: usize = 19;
/// Rows that begin a group: each output, then subtitles, then what is
/// remembered between runs, then the housekeeping at the bottom.
const SETTINGS_SECTIONS: [i32; 5] = [3, 7, 11, 14, 17];
/// Rows that belong to the row named above them, drawn indented so the group
/// reads as settings of that one thing rather than as more of their own.
/// Indentation is what lets them be called just "Preferred Language" instead
/// of repeating "Primary" and "Secondary" in every label.
const SETTINGS_SUBROWS: [i32; 8] = [4, 5, 6, 8, 9, 10, 12, 13];

/// Sizes offered for subtitles. The middle of the range is the default; the
/// ends are deliberately wide, since what reads well from a sofa and what
/// reads well at a desk are genuinely different.
const SUBTITLE_SIZES: [u32; 8] = [8, 10, 12, 14, 16, 18, 20, 24];

/// Font families offered in the menu. Generic names Pango always resolves
/// rather than an enumeration of everything installed, which would run to
/// hundreds of rows. `subtitle_font` in the config takes any description.
const SUBTITLE_FONTS: [&str; 5] = ["Sans Bold", "Sans", "Serif Bold", "Serif", "Monospace Bold"];

/// Fixed interface scales offered alongside automatic detection.
const UI_SCALES: [f64; 6] = [1.0, 1.25, 1.5, 2.0, 2.5, 3.0];

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
}

/// Everything the menu can act on. Devices persist to the config file;
/// the file and track choices last for the session.
pub struct App {
    window: gtk::ApplicationWindow,
    /// Holds the display awake while a film is playing. See [`crate::awake`].
    awake: crate::awake::KeepAwake,
    config: RefCell<Config>,
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
            settings_sliders: RefCell::new(Vec::new()),
            about_scroll: RefCell::new(None),
            copy_root: RefCell::new(None),
            kodi_draft: RefCell::new(None),
            settings_switches: RefCell::new(Vec::new()),
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
        if app.config.borrow().ui_scale.is_none() {
            let weak = Rc::downgrade(&app);
            window.connect_realize(move |window| {
                let Some(app) = weak.upgrade() else { return };
                let Some(monitor) = appearance::monitor_for_window(window) else {
                    return;
                };
                let actual = appearance::scale_for(&monitor);
                if actual != app.scale.get() {
                    app.restyle(actual);
                }
            });
        }

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

        let unopenable = match &file {
            Some(source) => app.set_file(source).err().map(|e| (source.clone(), e)),
            None => None,
        };

        // Track choices from the command line are applied whether or not
        // playback is starting. Without --play they simply arrive already
        // made, so the menu opens on them and they can be checked before
        // pressing Play.
        if let Some(preset) = preset.as_ref()
            && app.file.borrow().is_some()
        {
            let resolve = |spec: Option<&str>| -> Option<u32> {
                let spec = spec?;
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
            *app.primary_track.borrow_mut() = resolve(preset.primary.as_deref());
            *app.secondary_track.borrow_mut() = resolve(preset.secondary.as_deref());

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
                // Backing out of the strip before backing out of playback: a
                // press that quit the film while somebody was working through
                // the buttons would be a nasty surprise.
                gdk::Key::Escape if playing => {
                    let showing = app
                        .controls
                        .borrow()
                        .as_ref()
                        .is_some_and(|controls| controls.is_showing());
                    if showing {
                        if let Some(controls) = app.controls.borrow().as_ref() {
                            controls.hide();
                        }
                    } else {
                        app.go_back();
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Escape => {
                    app.go_back();
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
            Screen::Confirm | Screen::About | Screen::Kodi => self.show_settings(),
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
            Action::Left => {
                if !self.settings_slider(-1) {
                    self.window.child_focus(gtk::DirectionType::Left);
                }
            }
            Action::Right => {
                if !self.settings_slider(1) {
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

        if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>().cloned()
            && let Some(list) = list
        {
            self.sounds.borrow().click();
            list.select_row(Some(&row));
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
        let now = scale.value().round();
        let moved = if direction > 0 {
            (now / step).floor() * step + step
        } else {
            (now / step).ceil() * step - step
        };
        let range = kind.range();
        let moved = moved.clamp(*range.start(), *range.end());
        scale.set_value(moved);
        self.set_slider(kind, moved, &value);
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
        self.save_volume_soon();
    }

    /// Where a slider stands now, and how that reads beside it.
    fn slider_state(&self, kind: Slider) -> (f64, String) {
        let config = self.config.borrow();
        match kind {
            Slider::Volume(role) => {
                let level = config.volume(role);
                (level * 100.0, volume_label(level, config.muted(role)))
            }
            Slider::ResumeThreshold => {
                let percent = config.resume_min_percent().round();
                (percent, format!("{percent}%"))
            }
            Slider::WatchedThreshold => {
                let percent = config.watched_percent().round();
                (percent, format!("{percent}%"))
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
                Slider::ResumeThreshold => config.resume_min_percent = Some(moved),
                Slider::WatchedThreshold => config.watched_percent = Some(moved),
            }
        }
        value.set_text(&match kind {
            Slider::Volume(_) => volume_label(moved / 100.0, false),
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
            Setting::InterfaceScale => "Interface Size",
            Setting::PrimaryLanguage => "Primary Language Preference",
            Setting::SecondaryLanguage => "Secondary Language Preference",
            Setting::SubtitleLanguage => "Subtitle Preference",
            Setting::SubtitleSize => "Subtitle Size",
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
            Setting::InterfaceScale => {
                current = self
                    .config
                    .borrow()
                    .ui_scale
                    .and_then(|scale| UI_SCALES.iter().position(|offered| *offered == scale));
                entries.push(("Automatic".to_string(), None));
                for (position, scale) in UI_SCALES.iter().enumerate() {
                    entries.push((format!("{scale}x"), Some(position)));
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
            Setting::SubtitleSize => {
                let chosen = self
                    .config
                    .borrow()
                    .subtitle_size
                    .unwrap_or(crate::pipeline::DEFAULT_SUBTITLE_SIZE);
                current = SUBTITLE_SIZES.iter().position(|size| *size == chosen);
                for (position, size) in SUBTITLE_SIZES.iter().enumerate() {
                    let note = if *size == crate::pipeline::DEFAULT_SUBTITLE_SIZE {
                        "  (default)"
                    } else {
                        ""
                    };
                    entries.push((format!("{size}{note}"), Some(position)));
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
        {
            let app = self.clone();
            let list_weak = list.downgrade();
            let footer: Vec<glib::WeakRef<gtk::Button>> =
                footer.iter().map(|b| b.downgrade()).collect();
            let header_up: Vec<glib::WeakRef<gtk::Button>> =
                header.iter().map(|b| b.downgrade()).collect();
            let controller = gtk::EventControllerKey::new();
            controller.connect_key_pressed(move |_, key, _, _| {
                let Some(list) = list_weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if key != gdk::Key::Down && key != gdk::Key::Up {
                    return glib::Propagation::Proceed;
                }

                let last = last_row_index(&list);
                let current = list.selected_row().map(|r| r.index());

                if key == gdk::Key::Down && current == Some(last) {
                    let buttons: Vec<gtk::Button> =
                        footer.iter().filter_map(|b| b.upgrade()).collect();
                    if let Some(button) = App::first_footer(&buttons) {
                        app.sounds.borrow().click();
                        button.grab_focus();
                    }
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Up && current == Some(0) {
                    let buttons: Vec<gtk::Button> =
                        header_up.iter().filter_map(|b| b.upgrade()).collect();
                    // The rightmost, which is the folder you are in: moving
                    // left from there walks back up the tree.
                    if let Some(button) = App::last_header(&buttons) {
                        app.sounds.borrow().click();
                        button.grab_focus();
                    }
                    return glib::Propagation::Stop;
                }

                app.sounds.borrow().click();
                glib::Propagation::Proceed
            });
            list.add_controller(controller);
        }

        // Down from any header button returns to the top of the list.
        for button in header {
            let app = self.clone();
            let list_weak = list.downgrade();
            let controller = gtk::EventControllerKey::new();
            controller.connect_key_pressed(move |_, key, _, _| {
                if key != gdk::Key::Down {
                    return glib::Propagation::Proceed;
                }
                let Some(list) = list_weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if let Some(row) = list.row_at_index(0) {
                    app.sounds.borrow().click();
                    list.select_row(Some(&row));
                    settle_on(&row);
                }
                glib::Propagation::Stop
            });
            button.add_controller(controller);
        }

        // Wired on every button, so Up returns to the list from whichever
        // one happens to hold focus.
        for button in footer {
            let app = self.clone();
            let list_weak = list.downgrade();
            let controller = gtk::EventControllerKey::new();
            controller.connect_key_pressed(move |_, key, _, _| {
                if key != gdk::Key::Up {
                    return glib::Propagation::Proceed;
                }
                let Some(list) = list_weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if let Some(row) = list.row_at_index(last_row_index(&list)) {
                    app.sounds.borrow().click();
                    list.select_row(Some(&row));
                    row.grab_focus();
                }
                glib::Propagation::Stop
            });
            button.add_controller(controller);
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
            Setting::InterfaceScale => {
                let picked = choice.and_then(|index| UI_SCALES.get(index).copied());
                {
                    let mut config = self.config.borrow_mut();
                    config.ui_scale = picked;
                    let _ = config.save();
                }
                // Automatic means measuring the display again rather than
                // keeping whatever was last in force.
                let scale = picked.unwrap_or_else(|| {
                    appearance::monitor_for_window(&self.window)
                        .as_ref()
                        .map(appearance::scale_for)
                        .unwrap_or(1.0)
                });
                self.restyle(scale);
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
            Setting::SubtitleSize => {
                let mut config = self.config.borrow_mut();
                config.subtitle_size = choice.and_then(|index| SUBTITLE_SIZES.get(index).copied());
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

    fn open_file_chooser(self: &Rc<Self>) {
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

    /// The built-in browser: another list screen, so it navigates exactly
    /// like the menus and needs no pointer.
    ///
    /// `select` names the folder just stepped out of, which is then the row
    /// focus lands on. Going up otherwise dumps you at the top of a long
    /// list with no sense of where you were.
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
        *self.screen.borrow_mut() = Screen::PasteUri;
        self.window.set_child(Some(&self.modal(&page)));
        // Nothing here moves a selection, and the field wants the caret from
        // the moment it opens: this screen exists to be typed into.
        self.set_nav(None, &[], &[]);
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

    fn show_browser(
        self: &Rc<Self>,
        directory: &std::path::Path,
        select: Option<&std::path::Path>,
    ) {
        // Guards against a relative folder reaching here from anywhere at
        // all, including a `last_folder` saved before this was fixed.
        let directory = &crate::browser::rooted(directory);
        let (crumbs, crumb_buttons) = self.breadcrumbs(directory, false);

        let (page, list, _back, slot) = list_page_with(&crumbs, false);
        // The arrow's slot holds a fixed width for every screen to line up
        // against. With no arrow in it, that is just a gap before the trail.
        slot.set_visible(false);
        self.add_places_column(&page, directory, false, &crumb_buttons);
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
            browse.connect_clicked(move |_| app.open_file_chooser());
        }
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.add_css_class("tp-cancel");
        // A center box rather than a row: the way out belongs in the middle
        // wherever the other button happens to end up, and a plain row would
        // center the pair together instead.
        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&browse));
        footer.set_center_widget(Some(&cancel));
        page.append(&footer);

        // Entries, the icon that leads them, and the path they open. `None`
        // steps up a level.
        let mut rows: Vec<(String, &str, Option<std::path::PathBuf>)> = Vec::new();
        let parent = directory.parent().map(|p| p.to_path_buf());
        if parent.is_some() {
            // Two dots rather than the word: it is what a file listing has
            // always called the folder above, and it needs no translating.
            rows.push(("..".to_string(), "folder-symbolic", None));
        }
        for entry in crate::browser::read(directory) {
            // A play mark rather than a generic video one: that icon is not
            // in this theme and fell back to the missing-image glyph, which
            // reads as a warning about the file itself.
            let icon = if entry.is_dir {
                "folder-symbolic"
            } else {
                "media-playback-start-symbolic"
            };
            rows.push((entry.label.clone(), icon, Some(entry.path)));
        }
        if rows.is_empty() {
            rows.push((
                "Nothing here".to_string(),
                "dialog-information-symbolic",
                None,
            ));
        }

        for (label, icon, _) in &rows {
            // Two dots read aloud as nothing at all, being punctuation. What
            // it does is worth saying, and where it goes even more so.
            let spoken = if label == ".." {
                match parent.as_deref().and_then(|path| path.file_name()) {
                    Some(name) => format!("Up to {}", name.to_string_lossy()),
                    None => "Up to the list of drives".to_string(),
                }
            } else {
                label.clone()
            };
            append_named(&list, &browser_row(icon, label), &spoken);
        }

        {
            let app = self.clone();
            let rows = rows.clone();
            let here = directory.to_path_buf();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let Some((_, _, target)) = rows.get(row.index() as usize) else {
                    return;
                };
                match target {
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
            cancel.connect_clicked(move |_| app.go_back());
        }

        {
            let mut config = self.config.borrow_mut();
            config.last_folder = Some(directory.to_path_buf());
            let _ = config.save();
        }

        // The trail alone now that the arrow has gone: left from the current
        // folder simply walks back up it.
        self.wire_navigation(&list, &crumb_buttons, std::slice::from_ref(&cancel));
        self.remember_origin();
        *self.screen.borrow_mut() = Screen::Browser;
        self.window.set_child(Some(&self.modal(&page)));

        let opening = select
            .and_then(|wanted| {
                rows.iter()
                    .position(|(_, _, path)| path.as_deref() == Some(wanted))
            })
            // Otherwise the first real entry, skipping the rows that only
            // lead somewhere else: paste, up, and the empty-folder notice.
            .or_else(|| rows.iter().position(|(_, _, path)| path.is_some()))
            .unwrap_or(if rows.len() > 1 { 1 } else { 0 }) as i32;
        if let Some(row) = list.row_at_index(opening) {
            list.select_row(Some(&row));
            settle_on(&row);
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

    /// Always the built-in browser.
    ///
    /// Guessing from the last input was unpredictable: the same button opened
    /// different things depending on what you had touched. The system dialog
    /// is still reachable, from a pointer-only button in the footer.
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
                    crate::kodi_setup::summary(&crate::kodi_setup::find_all(&config.kodi_paths)),
                    // Always reachable: with nothing configured, this is where
                    // configuring starts.
                    true,
                ),
                ("About".to_string(), String::new(), true),
            ]
        };
        debug_assert_eq!(rows.len(), SETTINGS_ROWS);

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
        for (index, label, on) in [
            (2, "Navigation Sounds", self.config.borrow().sounds),
            (
                5,
                "Prefer Audio Description",
                self.config.borrow().primary_audio_description,
            ),
            (
                9,
                "Prefer Audio Description",
                self.config.borrow().secondary_audio_description,
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
            (6, Slider::Volume("primary"), "Volume"),
            (10, Slider::Volume("secondary"), "Volume"),
            (14, Slider::ResumeThreshold, "Resume Threshold"),
            (15, Slider::WatchedThreshold, "Watched Threshold"),
        ] {
            let (now, reading) = self.slider_state(kind);
            let (widget, scale, value) =
                slider_row(label, slider_width, kind.range(), now, &reading);
            if let Some(row) = list.row_at_index(index) {
                row.set_child(Some(&widget));
            }
            {
                let app = self.clone();
                let value = value.clone();
                scale.connect_change_value(move |_, _, moved| {
                    app.set_slider(kind, moved, &value);
                    glib::Propagation::Proceed
                });
            }
            self.settings_sliders
                .borrow_mut()
                .push((index, kind, scale, value));
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

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                // Remembered so returning from a chooser lands back on the
                // row it was opened from, as the main menu does.
                *app.settings_row.borrow_mut() = row.index();
                match row.index() {
                    0 => app.open_setting(Setting::Theme),
                    1 => app.open_setting(Setting::InterfaceScale),
                    2 => app.toggle_sounds(),
                    3 => app.open_setting(Setting::PrimaryDevice),
                    4 => app.open_setting(Setting::PrimaryLanguage),
                    5 => app.toggle_audio_description(true),
                    6 => app.toggle_settings_mute(6),
                    7 => app.open_setting(Setting::SecondaryDevice),
                    8 => app.open_setting(Setting::SecondaryLanguage),
                    9 => app.toggle_audio_description(false),
                    10 => app.toggle_settings_mute(10),
                    11 => app.open_setting(Setting::SubtitleLanguage),
                    12 => app.open_setting(Setting::SubtitleSize),
                    13 => app.open_setting(Setting::SubtitleFont),
                    16 => app.confirm_clear_data(),
                    17 => app.show_kodi(),
                    18 => app.show_about(),
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
        self.set_settings_switch(if primary { 5 } else { 9 }, on);
    }

    /// Moves the switch on a settings row to match what it now reports.
    fn set_settings_switch(&self, index: i32, on: bool) {
        if let Some((_, switch)) = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == index)
        {
            switch.set_active(on);
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
        self.set_settings_switch(2, enabled);
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
        body.append(&about_link(
            "Report issues or check for updates at",
            "https://github.com/scottarius/TinePlayer",
            "https://github.com/scottarius/TinePlayer",
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
            "Remove",
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
        Some((scroller, list))
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
    fn show_kodi_folder(self: &Rc<Self>, directory: &std::path::Path) {
        let directory = crate::browser::rooted(directory);
        let (crumbs, crumb_buttons) = self.breadcrumbs(&directory, true);
        let (page, list, _back, slot) = list_page_with(&crumbs, false);
        // With no arrow in it, the slot is just a gap before the trail.
        slot.set_visible(false);
        self.add_places_column(&page, &directory, true, &crumb_buttons);
        self.follow_focus(&list);

        // Folders only. A userdata folder is a folder, and listing the files
        // inside it would be a list of things that cannot be chosen.
        let folders: Vec<crate::browser::Entry> = crate::browser::read(&directory)
            .into_iter()
            .filter(|entry| entry.is_dir)
            .collect();

        // The system chooser as well, as the video browser offers: a pointer
        // and a keyboard can go faster through a dialog they already know.
        // Not focusable, and a folder chooser rather than a file one.
        let browse_face = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
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
            browse.connect_clicked(move |_| app.choose_kodi_folder_natively());
        }

        let choose = gtk::Button::with_label("Choose");
        choose.add_css_class("tp-button");
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.add_css_class("tp-cancel");
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        buttons.append(&cancel);
        buttons.append(&choose);
        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&browse));
        footer.set_center_widget(Some(&buttons));
        page.append(&footer);

        // Only when there is somewhere above to go. At the root of a drive
        // the way to another one is the column to the left, not a row that
        // leads out of the listing entirely.
        let up = directory.parent().is_some();
        if up {
            append_named(
                &list,
                &browser_row("folder-symbolic", ".."),
                "Up one folder",
            );
        }
        for entry in &folders {
            append_named(
                &list,
                &browser_row("folder-symbolic", &entry.label),
                &entry.label,
            );
        }

        {
            let app = self.clone();
            let directory = directory.clone();
            let paths: Vec<std::path::PathBuf> =
                folders.iter().map(|entry| entry.path.clone()).collect();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let index = row.index() as usize;
                if up && index == 0 {
                    if let Some(parent) = directory.parent() {
                        app.show_kodi_folder(parent);
                    }
                    return;
                }
                let offset = if up { 1 } else { 0 };
                if let Some(path) = paths.get(index - offset) {
                    app.show_kodi_folder(path);
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
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_kodi_choose();
            });
        }

        self.wire_navigation(&list, &crumb_buttons, &[choose.clone(), cancel.clone()]);
        *self.screen.borrow_mut() = Screen::KodiFolder;
        self.window.set_child(Some(&self.modal(&page)));
        if let Some(row) = list.row_at_index(0) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// The system's own folder chooser, for anyone who would rather use it.
    fn choose_kodi_folder_natively(self: &Rc<Self>) {
        let chooser = gtk::FileChooserNative::new(
            Some("Choose Kodi's userdata folder"),
            Some(&self.window),
            gtk::FileChooserAction::SelectFolder,
            Some("Choose"),
            Some("Cancel"),
        );
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

        // Marked with whatever this Kodi is already set to, so re-running the
        // setup shows what is in force rather than a default.
        let current = self
            .draft_userdata()
            .map(|userdata| {
                crate::kodi_setup::reads_as_playing(&userdata.join("playercorefactory.xml"))
            })
            .unwrap_or(false);
        if let Some(row) = list.row_at_index(if current { 0 } else { 1 }) {
            row.add_css_class("tp-current");
        }

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
            move || app.show_kodi_how()
        };
        let app = self.clone();
        self.show_kodi_dialog(
            "One thing to do yourself",
            &lines,
            "Continue",
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
            move || app.show_kodi_how()
        };
        let app = self.clone();
        self.show_kodi_dialog(
            "Confirm Configuration",
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
            "Configure",
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
        confirm_label: &str,
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
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.add_css_class("tp-cancel");
        let confirm = gtk::Button::with_label(confirm_label);
        confirm.add_css_class("tp-button");
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
/// Drawn for this application rather than taken from the icon theme: the
/// bundled theme has 157 icons and none of them mean fullscreen. The nearest,
/// `window-maximize-symbolic`, is a small square that reads as "maximize".
///
/// Drawn twice in each direction, once in each theme's foreground color,
/// because an embedded image cannot be recoloured the way a symbolic icon is.
/// A single compromise gray read poorly against both.
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

/// Appends a row to a list and gives it a name.
///
/// Focus lands on the row GTK wraps around the widget, not on the labels
/// inside it, and GTK derives a name from a child label but not from a
/// grandchild. A row built as a box of two labels therefore had no name, and
/// a screen reader announced it as "3 of 6" and nothing more.
fn append_named(list: &gtk::ListBox, child: &impl IsA<gtk::Widget>, name: &str) {
    list.append(child);
    if let Some(row) = child.as_ref().parent().and_downcast::<gtk::ListBoxRow>() {
        name_it(&row, name);
    }
}

/// How a settings row reads aloud: the setting, then what it is set to.
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

/// Where a stored language code sits in the offered list.
/// How a level reads in the settings menu. A silenced output says so rather
/// than showing the level it will return to, which is what the panel during
/// playback does too.
fn volume_label(level: f64, muted: bool) -> String {
    if muted {
        "Muted".to_string()
    } else {
        format!("{}%", (level * 100.0).round() as u32)
    }
}

/// A settings row carrying a switch rather than the word "On" or "Yes".
///
/// The switch is a readout, not a control: it cannot be clicked or focused,
/// and the row it sits in is what gets activated. That keeps one way of
/// working the menu - move to a row, press it - rather than a second target
/// inside the row that only a pointer could reach.
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
    switch.set_can_target(false);
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
fn slider_row(
    label: &str,
    width: i32,
    range: std::ops::RangeInclusive<f64>,
    now: f64,
    reading: &str,
) -> (gtk::Box, gtk::Scale, gtk::Label) {
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
    name_it(&scale, label);
    row.append(&scale);

    // Fixed width, so the bar beside it does not shift as the reading goes
    // from "Muted" to "5%" and back.
    let value = gtk::Label::new(Some(reading));
    value.add_css_class("tp-value");
    value.set_xalign(1.0);
    value.set_width_chars(6);
    row.append(&value);

    (row, scale, value)
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

/// A line ending in a link that opens in the machine's browser. The address
/// is shown as written rather than hidden behind words, since on a screen
/// nobody can click there is still a use in being able to read it out.
fn about_link(lead: &str, href: &str, shown: &str) -> gtk::Label {
    let label = about_text("");
    // The address on its own line rather than run on from the sentence: an
    // address is a thing to be read character by character, and one wrapped
    // mid-way through a paragraph is hard to pick back out of it.
    label.set_markup(&format!(
        "{}\n<a href=\"{}\">{}</a>",
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

/// A browser row: an icon from the desktop's own set, then the name.
///
/// Icons rather than emoji, because emoji depend on a color font being
/// installed. The Pi has none, so a folder character rendered as an empty box
/// with the codepoint inside it.
fn browser_row(icon: &str, text: &str) -> gtk::Box {
    // The padding goes on the row rather than the label, so it applies
    // before the icon as well as around the text.
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(16)
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
        }}
        .tp-row switch > slider {{
            min-width: {slider}px;
            min-height: {slider}px;
            border-radius: {switch_h}px;
        }}
        .tp-row switch:checked {{
            background-color: {switch_on};
            border-color: {switch_on};
        }}
        .tp-row switch:checked > slider {{ background-color: {switch_knob}; }}
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
            background-color: #ffffff;
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
        .tp-gear {{ padding: {pad_v}px {pad_h}px; }}
        .tp-gear image {{ -gtk-icon-size: {icon}px; }}
        .tp-row-icon {{ -gtk-icon-size: {row_icon}px; opacity: 0.65; }}
        .tp-back image {{ -gtk-icon-size: {back_icon}px; }}
        .{video} {{ background-color: black; }}
        ",
        title = px(20.0),
        tracking = px(2.0).max(1),
        row = px(26.0),
        hint = px(20.0),
        small = px(17.0),
        tight_v = px(7.0),
        tight_h = px(10.0),
        pad_v = px(16.0),
        pad_h = px(24.0),
        radius = px(8.0),
        outline = px(2.0).max(1),
        handle = px(18.0),
        switch_on = if dark { "#dcdcdc" } else { "#707070" },
        switch_knob = if dark { "#1c1c1c" } else { "#ffffff" },
        switch_w = px(64.0),
        switch_h = px(32.0),
        slider = px(26.0),
        section = px(28.0),
        subrow = px(28.0),
        mark = px(4.0),
        icon = px(24.0),
        icon_main = px(38.4),
        crumb_pad = px(6.0),
        leading = px(38.0),
        back_icon = px(22.0),
        row_icon = px(22.0),
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
