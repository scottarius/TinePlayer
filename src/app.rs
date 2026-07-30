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

/// Rows of the settings screen, in the order they appear.
/// Rows a page jump covers, roughly a screenful at the default size. What
/// makes a folder of a hundred films navigable without a hundred presses.
const PAGE_ROWS: i32 = 8;

const SETTINGS_ROWS: usize = 13;
/// Rows that begin a group: audio, subtitles, then the housekeeping at the
/// bottom.
const SETTINGS_SECTIONS: [i32; 3] = [3, 7, 10];

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
    ConfirmQuit,
    Error,
    Playing,
}

/// Choices given on the command line, which skip the menu entirely.
#[derive(Clone)]
pub struct Preset {
    /// Numbered as `--list-tracks` prints them, so 1 is the first track and
    /// 0 means none.
    pub primary: Option<u32>,
    pub secondary: Option<u32>,
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
    /// Something else chose the video and is waiting for this playback.
    pub external: bool,
    /// That something else is Kodi, which can also be talked to.
    pub kodi: bool,
}

/// Everything the menu can act on. Devices persist to the config file;
/// the file and track choices last for the session.
pub struct App {
    window: gtk::ApplicationWindow,
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
    /// Whether the error on screen ended the session: a video named on the
    /// command line that could not be opened leaves nothing to go back to, so
    /// its button closes the player. Every other error returns to the menu.
    error_is_fatal: Cell<bool>,
    /// What Kodi says it is playing through us: its title, database id, resume
    /// point, and the path to report progress against. Fetched once at startup,
    /// because it cannot change while we are the player. `None` when Kodi was
    /// not involved or did not answer, which is not an error.
    kodi_item: RefCell<Option<crate::kodi::Item>>,
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
            external,
            kodi,
        } = launch;
        let dark = appearance::apply_theme(config.theme);
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
            error_is_fatal: Cell::new(false),
            kodi_item: RefCell::new(None),
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

        // Command-line track choices go straight to playback, but only when
        // there's actually somewhere to play them.
        let ready = app.config.borrow().primary_sink.is_some();
        match preset {
            Some(preset) if ready && app.file.borrow().is_some() => {
                let resolve = |choice: Option<u32>| -> Option<u32> {
                    let tracks = app.tracks.borrow();
                    choice
                        .filter(|n| *n > 0)
                        .and_then(|n| tracks.get((n - 1) as usize))
                        .map(|t| t.index)
                };
                *app.primary_track.borrow_mut() = resolve(preset.primary);
                *app.secondary_track.borrow_mut() = resolve(preset.secondary);

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
                app.start_playback(app.restart);
            }
            // Nothing to choose from if the video could not be read, so the
            // reason is shown instead of an empty menu.
            //
            // The video comes first when both went wrong: it is what someone
            // asked for, and settings that failed to load can be seen for
            // themselves in the menu behind.
            _ => match (&unopenable, &config_problem) {
                (Some((source, error)), _) => app.show_source_error(source, error, true),
                // Not fatal: Back lands in the menu, which is where the
                // settings would be put right.
                (None, Some(problem)) => app.show_error(problem, false),
                (None, None) => app.show_menu(),
            },
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
                    }
                    app.wake_controls();
                    glib::Propagation::Stop
                }
                // Only during playback: elsewhere the arrows belong to the
                // menus, where left and right mean nothing.
                gdk::Key::Left if playing => {
                    app.scrub(-crate::player::STEP_SECONDS);
                    glib::Propagation::Stop
                }
                gdk::Key::Right if playing => {
                    app.scrub(crate::player::STEP_SECONDS);
                    glib::Propagation::Stop
                }
                // Always goes back one level, so it never quits by surprise
                // from somewhere the user was only browsing.
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
                if matches!(key, gdk::Key::Left | gdk::Key::Right) {
                    app.end_scrub();
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
            Screen::Playing => {
                self.stop_playback();
                self.show_menu();
            }
            Screen::Chooser => self.leave_chooser(),
            Screen::Confirm => self.show_settings(),
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

    /// Refreshes the controls readout twice a second: often enough that the
    /// clock never looks stuck, rare enough to be free.
    fn start_tick(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        // Ticks since Kodi was last told where playback had reached. Counted
        // here rather than given a timer of its own so that it stops when
        // playback does, without anything extra to tear down.
        let mut since_report = 0u32;
        let source = glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
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
                    if since_report >= 60 {
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

    /// Begins or continues a scrub. Nothing moves until the ticker decides
    /// this is a hold; a tap resolves to a single step when released.
    fn scrub(self: &Rc<Self>, seconds: f64) {
        let playback = self.playback.borrow().clone();
        let Some(playback) = playback else { return };

        let already = playback.is_scrubbing();
        playback.scrub_input(seconds);
        self.scrub_seen.set(Some(std::time::Instant::now()));
        self.wake_controls();
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
            app.wake_controls();
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
        self.wake_controls();
    }

    fn toggle_fullscreen(&self) {
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
            Action::Up | Action::Down if self.playback.borrow().is_some() => self.wake_controls(),
            Action::Up => self.move_selection(-1),
            Action::Down => self.move_selection(1),
            Action::Left if self.playback.borrow().is_some() => {
                self.scrub(-crate::player::STEP_SECONDS)
            }
            Action::Right if self.playback.borrow().is_some() => {
                self.scrub(crate::player::STEP_SECONDS)
            }
            Action::Left => {
                self.window.child_focus(gtk::DirectionType::Left);
            }
            Action::Right => {
                self.window.child_focus(gtk::DirectionType::Right);
            }
            // During playback the lower face button is the obvious place for
            // play/pause, and there is nothing else on screen to activate.
            Action::Activate | Action::PlayPause if self.playback.borrow().is_some() => {
                if let Some(playback) = self.playback.borrow().as_ref() {
                    playback.toggle_pause();
                }
                self.wake_controls();
            }
            Action::Activate => self.activate_focused(),
            Action::PlayPause => {}
            Action::DirectionReleased => self.end_scrub(),
            Action::PageUp => self.move_selection(-PAGE_ROWS),
            Action::PageDown => self.move_selection(PAGE_ROWS),
            Action::Back => self.go_back(),
            Action::Fullscreen => self.toggle_fullscreen(),
            // Ignored outside playback, matching the keyboard: there is
            // nothing to turn off from a menu.
            Action::Subtitles => self.toggle_subtitles(),
        }
    }

    /// Moves the selection one row, obeying the same boundary rules the
    /// keyboard does: the footer button sits below the last row, and the top
    /// of the list is a hard stop rather than wrapping.
    fn move_selection(self: &Rc<Self>, delta: i32) {
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
                row.grab_focus();
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
        if let Some(controls) = self.controls.borrow().as_ref() {
            controls.set_subtitles(playback.has_subtitles(), showing);
        }
        self.wake_controls();
    }

    fn stop_playback(&self) {
        self.finish_playback(false);
    }

    /// Tears playback down, saving or clearing the resume position as it goes.
    ///
    /// `wait_for_kodi` holds on until the last progress report has actually
    /// reached Kodi. That only matters when the process is about to end, since
    /// the report goes out on a detached thread and exiting would take it
    /// along; everywhere else it would be a stall for nothing.
    fn finish_playback(&self, wait_for_kodi: bool) {
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
        if let Some(item) = self.kodi_item.borrow().as_ref() {
            return item.resume_ns;
        }
        let key = self.storage_key()?;
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
            list.append(&menu_row(label, value, *enabled));
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
        buttons.append(&fullscreen);
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
            row.grab_focus();
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
                    entries.push((option.label().to_string(), Some(position)));
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
                for (position, (_, name, _)) in crate::languages::LANGUAGES.iter().enumerate() {
                    entries.push((name.to_string(), Some(position)));
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
                            .position(|(code, _, _)| *code == setting)
                            .map(|position| modes + position)
                    });
                for (position, (_, label)) in crate::subtitles::MODES.iter().enumerate() {
                    entries.push((label.to_string(), Some(position)));
                }
                for (position, (_, name, _)) in crate::languages::LANGUAGES.iter().enumerate() {
                    entries.push((name.to_string(), Some(modes + position)));
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
            list.append(&chooser_row(text));
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
            list.select_row(Some(&row));
            row.grab_focus();
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
                    row.grab_focus();
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
            .map(|option| option.label().to_string())
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
                    .map(|(code, _, _)| code.to_string());
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

        let (primary_language, secondary_language, subtitle_language) = {
            let config = self.config.borrow();
            (
                config.primary_language.clone(),
                config.secondary_language.clone(),
                config.subtitle_language.clone(),
            )
        };
        // First track in the preferred language, if one was named.
        let by_language = |preferred: &Option<String>| -> Option<u32> {
            let code = preferred.as_deref()?;
            tracks
                .iter()
                .find(|track| crate::languages::matches(&track.language, code))
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
                by_language(&primary_language).or_else(|| tracks.first().map(|t| t.index)),
                by_language(&secondary_language).or_else(|| tracks.get(1).map(|t| t.index)),
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
        let (crumbs, crumb_buttons) = self.breadcrumbs(directory);

        let (page, list, _back, slot) = list_page_with(&crumbs, false);
        // The arrow's slot holds a fixed width for every screen to line up
        // against. With no arrow in it, that is just a gap before the trail.
        slot.set_visible(false);

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
        if parent.is_some() || !crate::browser::roots().is_empty() {
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
            list.append(&browser_row(icon, label));
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
                    // Up: to the parent, or to the drive list when there is
                    // nothing above this.
                    None => match here.parent() {
                        Some(parent) => app.show_browser(parent, Some(&here)),
                        None => app.show_roots(),
                    },
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
            row.grab_focus();
        }
    }

    /// The current directory as a row of buttons, one per level, so any
    /// ancestor is a single press away rather than several trips through Up.
    ///
    /// Capped at the last few levels: a deep path would otherwise run off the
    /// side, and the leading button stands in for everything trimmed away.
    fn breadcrumbs(self: &Rc<Self>, directory: &std::path::Path) -> (gtk::Box, Vec<gtk::Button>) {
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

    /// The drive list, which only Windows has anything above the root to
    /// show.
    fn show_roots(self: &Rc<Self>) {
        let roots = crate::browser::roots();
        if roots.is_empty() {
            return;
        }
        let (page, list, back, _header) = list_page("Drives", true);
        for entry in &roots {
            list.append(&chooser_row(&entry.label));
        }

        {
            let app = self.clone();
            let paths: Vec<std::path::PathBuf> = roots.iter().map(|e| e.path.clone()).collect();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                if let Some(path) = paths.get(row.index() as usize) {
                    app.show_browser(path, None);
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.show_menu());
        }

        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        *self.screen.borrow_mut() = Screen::Browser;
        self.window.set_child(Some(&page));
        if let Some(row) = list.row_at_index(0) {
            list.select_row(Some(&row));
            row.grab_focus();
        }
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
            list.append(&menu_row(label, value, true));
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
            row.grab_focus();
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
                    "Primary Language Preference".to_string(),
                    language(&config.primary_language, "First track"),
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
                    "Secondary Language Preference".to_string(),
                    language(&config.secondary_language, "Second track"),
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
                ("Clear Saved Playback Data".to_string(), String::new(), true),
                (
                    "Version".to_string(),
                    env!("CARGO_PKG_VERSION").to_string(),
                    false,
                ),
                (
                    "GStreamer".to_string(),
                    gstreamer::version_string().to_string(),
                    false,
                ),
            ]
        };
        debug_assert_eq!(rows.len(), SETTINGS_ROWS);

        for (label, value, enabled) in &rows {
            list.append(&menu_row(label, value, *enabled));
        }
        for index in SETTINGS_SECTIONS {
            if let Some(row) = list.row_at_index(index) {
                row.add_css_class("tp-section-start");
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
                    5 => app.open_setting(Setting::SecondaryDevice),
                    6 => app.open_setting(Setting::SecondaryLanguage),
                    7 => app.open_setting(Setting::SubtitleLanguage),
                    8 => app.open_setting(Setting::SubtitleSize),
                    9 => app.open_setting(Setting::SubtitleFont),
                    10 => app.confirm_clear_data(),
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
            row.grab_focus();
        }
    }

    /// Opens a chooser and remembers to come back here rather than to the
    /// main menu.
    fn open_setting(self: &Rc<Self>, setting: Setting) {
        self.from_settings.set(true);
        self.show_chooser(setting);
    }

    fn toggle_sounds(self: &Rc<Self>) {
        let (enabled, device) = {
            let mut config = self.config.borrow_mut();
            config.sounds = !config.sounds;
            let _ = config.save();
            (config.sounds, config.primary_sink.clone())
        };
        *self.sounds.borrow_mut() = Sounds::new(enabled, device);
        self.show_settings();
    }

    /// Re-renders every size in the interface at a new scale.
    fn restyle(&self, scale: f64) {
        self.scale.set(scale);
        self.styles.load_from_data(&style_css(scale));
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

    fn start_playback(self: &Rc<Self>, restart: bool) {
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

        let result = Playback::start(
            &path,
            primary,
            secondary,
            subtitle.as_ref(),
            &self.config.borrow(),
            // "Restart" means start from the beginning whoever is asking, so
            // it beats both our saved position and Kodi's.
            (!restart).then(|| self.resume_position()).flatten(),
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
                let controls = Controls::new(
                    playback.widget(),
                    self.scale.get(),
                    self.dark.get(),
                    self.window.is_fullscreen(),
                );
                {
                    let app = self.clone();
                    controls.connect_play_pause(move || {
                        if let Some(playback) = app.playback.borrow().as_ref() {
                            playback.toggle_pause();
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
                controls.set_subtitles(playback.has_subtitles(), playback.subtitles_showing());
                {
                    let app = self.clone();
                    controls.connect_double_click(move || app.toggle_fullscreen());
                }
                {
                    let app = self.clone();
                    controls.connect_motion(move || app.wake_controls());
                }
                {
                    let app = self.clone();
                    controls.connect_seek(move |fraction| {
                        let playback = app.playback.borrow().clone();
                        let Some(playback) = playback else { return };
                        let Some(duration) = playback.duration() else {
                            return;
                        };
                        playback.seek_to(gstreamer::ClockTime::from_nseconds(
                            (duration.nseconds() as f64 * fraction) as u64,
                        ));
                        app.wake_controls();
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
                self.window.set_child(Some(controls.widget()));
                controls.update(&playback);
                controls.flash(false);
                *self.controls.borrow_mut() = Some(controls);
                self.start_tick();
                self.window
                    .set_title(Some(&self.file_label().unwrap_or_default()));
                *self.playback.borrow_mut() = Some(playback);
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
    list_page_with(&heading, show_back)
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
    const LOGO: &[u8] = include_bytes!("../data/tineplayer.png");

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
    const ICON: &[u8] = include_bytes!("../data/subtitles.png");

    let image = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON)) {
        image.set_paintable(Some(&texture));
    }
    image.set_pixel_size((26.0 * scale).round() as i32);
    image
}

pub fn fullscreen_image(fullscreen: bool, scale: f64, dark: bool) -> gtk::Image {
    const ENTER_LIGHT: &[u8] = include_bytes!("../data/fullscreen-light.png");
    const ENTER_DARK: &[u8] = include_bytes!("../data/fullscreen-dark.png");
    const LEAVE_LIGHT: &[u8] = include_bytes!("../data/restore-light.png");
    const LEAVE_DARK: &[u8] = include_bytes!("../data/restore-dark.png");

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

fn back_button() -> gtk::Button {
    // An icon rather than a text glyph: a "‹" character sits off the
    // vertical center because it's positioned by font metrics rather than
    // by the icon's own bounding box.
    let button = gtk::Button::from_icon_name("go-previous-symbolic");
    button.add_css_class("tp-back");
    button.set_valign(gtk::Align::Center);
    button
}

/// Where a stored language code sits in the offered list.
fn language_position(code: Option<&str>) -> Option<usize> {
    let code = code?;
    crate::languages::LANGUAGES
        .iter()
        .position(|(stored, _, _)| *stored == code)
}

fn last_row_index(list: &gtk::ListBox) -> i32 {
    let mut last = 0;
    while list.row_at_index(last + 1).is_some() {
        last += 1;
    }
    last
}

fn describe_audio_track(track: &AudioTrack) -> String {
    let mut text = format!("{} — {} {}ch", track.language, track.codec, track.channels);
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

fn style_css(scale: f64) -> String {
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
        .tp-chevron {{ font-size: {row}px; opacity: 0.5; }}
        .tp-hint {{ font-size: {hint}px; opacity: 0.7; }}
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
        .tp-menu > row:selected:hover {{ background-color: {highlight}; }}
        .tp-menu > row.tp-section-start {{ margin-top: {section}px; }}
        /* The selected row keeps the same colors whether or not the list
           holds focus, and is simply dimmed when it doesn't. Fading the
           whole row rather than clearing its background keeps the position
           visible while the Play button or another window is active, and
           dims text and background together so the contrast between them
           survives. */
        .tp-menu > row:selected {{
            background-image: none;
            background-color: {highlight};
            color: #ffffff;
            opacity: 0.45;
        }}
        .tp-menu > row:selected .tp-value,
        .tp-menu > row:selected .tp-chevron {{
            color: #ffffff;
            opacity: 0.85;
        }}
        .tp-menu:focus-within > row:selected {{ opacity: 1; }}
        /* background-image has to be cleared too: themes draw button
           backgrounds with a gradient image, which covers any
           background-color set underneath it. The label is colored
           separately because themes set button text color directly. */
        button:focus {{
            background-image: none;
            background-color: {highlight};
        }}
        button:focus label {{ color: #ffffff; }}
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
        .tp-time {{ font-size: {hint}px; color: #ffffff; }}
        .tp-transport {{ -gtk-icon-size: {icon}px; color: #ffffff; }}
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
        section = px(28.0),
        icon = px(24.0),
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
