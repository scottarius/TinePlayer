use std::cell::{Cell, RefCell};
use std::path::PathBuf;
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
    FileBrowser,
}

/// Rows of the settings screen, in the order they appear.
/// Rows a page jump covers, roughly a screenful at the default size. What
/// makes a folder of a hundred films navigable without a hundred presses.
const PAGE_ROWS: i32 = 8;

const SETTINGS_ROWS: usize = 14;
/// Rows that begin a group: audio, subtitles, then the housekeeping at the
/// bottom.
const SETTINGS_SECTIONS: [i32; 3] = [4, 8, 11];

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
    Chooser,
    Confirm,
    ConfirmQuit,
    Error,
    Playing,
}

/// Everything the menu can act on. Devices persist to the config file;
/// the file and track choices last for the session.
pub struct App {
    window: gtk::ApplicationWindow,
    config: RefCell<Config>,
    file: RefCell<Option<PathBuf>>,
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
    controls: RefCell<Option<Rc<Controls>>>,
    /// Whether the open chooser was reached from the settings screen, so
    /// that finishing with it returns where it came from.
    from_settings: Cell<bool>,
    /// Whether a controller was the last thing to act, which decides the
    /// picker in automatic mode.
    gamepad_last: Cell<bool>,
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
}

impl App {
    pub fn build(
        gtk_app: &gtk::Application,
        config: Config,
        file: Option<PathBuf>,
        preset_tracks: Option<(Option<u32>, Option<u32>)>,
        restart: bool,
        fullscreen: bool,
    ) {
        appearance::apply_theme(config.theme);
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
            controls: RefCell::new(None),
            from_settings: Cell::new(false),
            gamepad_last: Cell::new(false),
            styles: styles.clone(),
            scale: Cell::new(scale),
            scrub_generation: Cell::new(0),
            scrub_seen: Cell::new(None),
            tick: RefCell::new(None),
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

        if let Some(path) = file {
            app.set_file(&path);
        }

        // Command-line track choices go straight to playback, but only when
        // there's actually somewhere to play them.
        let ready = app.config.borrow().primary_sink.is_some();
        match preset_tracks {
            Some((primary, secondary)) if ready && app.file.borrow().is_some() => {
                let resolve = |choice: Option<u32>| -> Option<u32> {
                    let tracks = app.tracks.borrow();
                    choice
                        .filter(|n| *n > 0)
                        .and_then(|n| tracks.get((n - 1) as usize))
                        .map(|t| t.index)
                };
                *app.primary_track.borrow_mut() = resolve(primary);
                *app.secondary_track.borrow_mut() = resolve(secondary);
                app.start_playback(app.restart);
            }
            _ => app.show_menu(),
        }

        window.present();
    }

    fn install_key_handling(self: &Rc<Self>) {
        let controller = gtk::EventControllerKey::new();
        let app = self.clone();
        controller.connect_key_pressed(move |_, key, _, _| {
            app.gamepad_last.set(false);
            let playing = app.playback.borrow().is_some();
            match key {
                // Only claimed during playback — the menus need Space for
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
        {
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
                app.set_file(&path);
                app.show_menu();
                true
            });
            self.window.add_controller(drop);
        }

        // Any pointer movement means someone is at the machine rather than
        // on a sofa, which is what automatic picker mode keys off.
        {
            let app = self.clone();
            let pointer = gtk::EventControllerMotion::new();
            pointer.connect_motion(move |_, _, _| app.gamepad_last.set(false));
            self.window.add_controller(pointer);
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
            Screen::Browser | Screen::Settings | Screen::Error | Screen::ConfirmQuit => {
                self.show_menu()
            }
            Screen::Menu => self.show_confirm_quit(),
        }
    }

    /// Refreshes the controls readout twice a second: often enough that the
    /// clock never looks stuck, rare enough to be free.
    fn start_tick(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
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
    fn set_nav(&self, list: Option<&gtk::ListBox>, footer: &[gtk::Button]) {
        *self.nav_list.borrow_mut() = list.cloned();
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

    fn handle_action(self: &Rc<Self>, action: crate::gamepad::Action) {
        use crate::gamepad::Action;
        self.gamepad_last.set(true);
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
        }
    }

    /// Moves the selection one row, obeying the same boundary rules the
    /// keyboard does: the footer button sits below the last row, and the top
    /// of the list is a hard stop rather than wrapping.
    fn move_selection(self: &Rc<Self>, delta: i32) {
        // Cloned out before anything can rebuild the screen underneath us.
        let list = self.nav_list.borrow().clone();
        let footer = self.nav_footer.borrow().clone();

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

    fn stop_playback(&self) {
        if let Some(tick) = self.tick.borrow_mut().take() {
            tick.remove();
        }
        if let Some(controls) = self.controls.borrow_mut().take() {
            controls.cancel();
        }
        if let Some(playback) = self.playback.borrow_mut().take() {
            playback.stop();
        }
        self.window.set_title(Some("TinePlayer"));
    }

    // --- Menu ----------------------------------------------------------

    fn show_menu(self: &Rc<Self>) {
        let (page, list, _back) = list_page("Playback Options", false);

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

        let has_file = file.is_some();
        let has_secondary = config.secondary_sink.is_some();
        let mut rows: Vec<(String, String, bool)> = vec![
            (
                "Video".to_string(),
                file.as_ref()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                    .unwrap_or_else(|| "Choose a video…".to_string()),
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
        let resume_at = file
            .as_deref()
            .and_then(crate::config::load_resume)
            .and_then(|resume| resume.resume_position());
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

        let gear = gtk::Button::from_icon_name("emblem-system-symbolic");
        gear.add_css_class("tp-gear");
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
        footer.push(gear);

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
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

        self.wire_navigation(&list, &footer);

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
            Setting::PrimaryLanguage => "Primary Language",
            Setting::SecondaryLanguage => "Secondary Language",
            Setting::SubtitleLanguage => "Subtitle Language",
            Setting::SubtitleSize => "Subtitle Size",
            Setting::SubtitleFont => "Subtitle Font",
            Setting::FileBrowser => "File Browser",
        };
        let (page, list, back) = list_page(title, true);

        // Entries are (display text, choice). `None` means the "None"
        // option, which every list offers except the primary device — an
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
                for (position, name) in ["Follow the desktop", "Light", "Dark"]
                    .into_iter()
                    .enumerate()
                {
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
                // Not "None": an output with no preference still plays
                // something, it just takes whatever comes first.
                entries.push(("No preference".to_string(), None));
                for (position, (_, name, _)) in crate::languages::LANGUAGES.iter().enumerate() {
                    entries.push((name.to_string(), Some(position)));
                }
            }
            Setting::SubtitleLanguage => {
                current = language_position(self.config.borrow().subtitle_language.as_deref());
                entries.push(("None".to_string(), None));
                for (position, (_, name, _)) in crate::languages::LANGUAGES.iter().enumerate() {
                    entries.push((name.to_string(), Some(position)));
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
            Setting::FileBrowser => {
                current = Some(match self.config.borrow().file_browser {
                    crate::config::BrowserMode::Automatic => 0,
                    crate::config::BrowserMode::System => 1,
                    crate::config::BrowserMode::BuiltIn => 2,
                });
                for (position, name) in ["Automatic", "System dialog", "Built-in"]
                    .into_iter()
                    .enumerate()
                {
                    entries.push((name.to_string(), Some(position)));
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
                app.apply_choice(setting, *choice);
                app.leave_chooser();
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.leave_chooser());
        }

        self.wire_navigation(&list, &[]);

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
    /// between the list and the button below it has to be bridged by hand —
    /// otherwise the button is unreachable without a pointer. Movements
    /// that would go past either end are swallowed, which also stops GTK
    /// reporting them as failed navigation.
    fn wire_navigation(self: &Rc<Self>, list: &gtk::ListBox, footer: &[gtk::Button]) {
        self.set_nav(Some(list), footer);
        {
            let app = self.clone();
            let list_weak = list.downgrade();
            let footer: Vec<glib::WeakRef<gtk::Button>> =
                footer.iter().map(|b| b.downgrade()).collect();
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
                    return glib::Propagation::Stop;
                }

                app.sounds.borrow().click();
                glib::Propagation::Proceed
            });
            list.add_controller(controller);
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
        let Some(path) = self.file.borrow().clone() else {
            return;
        };
        crate::config::save_tracks(
            &path,
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

    fn apply_choice(self: &Rc<Self>, setting: Setting, choice: Option<usize>) {
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
                        if picked.is_none() {
                            return;
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
                appearance::apply_theme(theme);
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
            Setting::PrimaryLanguage | Setting::SecondaryLanguage | Setting::SubtitleLanguage => {
                let picked = choice
                    .and_then(|index| crate::languages::LANGUAGES.get(index))
                    .map(|(code, _, _)| code.to_string());
                let mut config = self.config.borrow_mut();
                match setting {
                    Setting::PrimaryLanguage => config.primary_language = picked,
                    Setting::SecondaryLanguage => config.secondary_language = picked,
                    _ => config.subtitle_language = picked,
                }
                let _ = config.save();
            }
            Setting::FileBrowser => {
                let mut config = self.config.borrow_mut();
                config.file_browser = match choice {
                    Some(1) => crate::config::BrowserMode::System,
                    Some(2) => crate::config::BrowserMode::BuiltIn,
                    _ => crate::config::BrowserMode::Automatic,
                };
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
        // Held by the closure so the dialog outlives this function; a
        // dropped FileChooserNative closes before the user can answer.
        let held = RefCell::new(Some(chooser.clone()));
        chooser.connect_response(move |chooser, response| {
            if response == gtk::ResponseType::Accept
                && let Some(path) = chooser.file().and_then(|f| f.path())
            {
                app.set_file(&path);
            }
            held.borrow_mut().take();
            app.show_menu();
        });
        chooser.show();
    }

    /// Probes the file and chooses tracks for it.
    ///
    /// A file played before comes back with the tracks it was played with;
    /// otherwise the first track goes to the primary output and a different
    /// one to the secondary, which is the whole point of the application.
    fn set_file(self: &Rc<Self>, path: &std::path::Path) {
        let media = match crate::probe::probe_media(path) {
            Ok(media) => media,
            Err(e) => {
                eprintln!("Couldn't read {}: {e}", path.display());
                *self.tracks.borrow_mut() = Vec::new();
                *self.subtitle_options.borrow_mut() = Vec::new();
                *self.primary_track.borrow_mut() = None;
                *self.secondary_track.borrow_mut() = None;
                *self.subtitle.borrow_mut() = None;
                *self.file.borrow_mut() = None;
                return;
            }
        };
        let tracks = media.audio;
        let options = crate::subtitles::options(path, &media.subtitles);

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

        let saved = crate::config::load_resume(path).and_then(|resume| resume.tracks);
        let (primary, secondary) = match saved.clone() {
            // A saved None is a real choice ("no audio on that output"), so a
            // saved pair is taken as it stands rather than filled in.
            Some(choice) => (choice.primary, choice.secondary),
            // Otherwise the preferred languages decide, falling back to the
            // old behaviour of the first track and a different one.
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
            None => subtitle_language.as_deref().and_then(|code| {
                options
                    .iter()
                    .find(|option| crate::languages::matches(option.label(), code))
                    .map(|option| option.choice())
            }),
        };
        *self.subtitle.borrow_mut() =
            subtitle.filter(|choice| options.iter().any(|option| option.choice() == *choice));
        *self.subtitle_options.borrow_mut() = options;
        *self.tracks.borrow_mut() = tracks;
        *self.file.borrow_mut() = Some(path.to_path_buf());

        let mut config = self.config.borrow_mut();
        config.last_video = Some(path.to_path_buf());
        let _ = config.save();
    }

    // --- Browsing ------------------------------------------------------

    /// The built-in browser: another list screen, so it navigates exactly
    /// like the menus and needs no pointer.
    ///
    /// `select` names the folder just stepped out of, which is then the row
    /// focus lands on. Going up otherwise dumps you at the top of a long
    /// list with no sense of where you were.
    fn show_browser(
        self: &Rc<Self>,
        directory: &std::path::Path,
        select: Option<&std::path::Path>,
    ) {
        let (page, list, back) = list_page(
            &directory
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| directory.to_string_lossy().to_string()),
            true,
        );

        // Entries and the paths they lead to. `None` steps up a level.
        let mut rows: Vec<(String, Option<std::path::PathBuf>)> = Vec::new();
        let parent = directory.parent().map(|p| p.to_path_buf());
        if parent.is_some() || !crate::browser::roots().is_empty() {
            rows.push(("⬆  Up".to_string(), None));
        }
        for entry in crate::browser::read(directory) {
            let label = if entry.is_dir {
                format!("📁  {}", entry.label)
            } else {
                entry.label.clone()
            };
            rows.push((label, Some(entry.path)));
        }
        if rows.is_empty() {
            rows.push(("Nothing here".to_string(), None));
        }

        for (label, _) in &rows {
            list.append(&chooser_row(label));
        }

        {
            let app = self.clone();
            let rows = rows.clone();
            let here = directory.to_path_buf();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let Some((_, target)) = rows.get(row.index() as usize) else {
                    return;
                };
                match target {
                    Some(path) if path.is_dir() => app.show_browser(path, None),
                    Some(path) => {
                        app.set_file(path);
                        app.show_menu();
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
            back.connect_clicked(move |_| app.show_menu());
        }

        {
            let mut config = self.config.borrow_mut();
            config.last_folder = Some(directory.to_path_buf());
            let _ = config.save();
        }

        self.wire_navigation(&list, &[]);
        *self.screen.borrow_mut() = Screen::Browser;
        self.window.set_child(Some(&page));

        let opening = select
            .and_then(|wanted| {
                rows.iter()
                    .position(|(_, path)| path.as_deref() == Some(wanted))
            })
            // Otherwise the first real entry rather than the Up row.
            .unwrap_or(if rows.len() > 1 { 1 } else { 0 }) as i32;
        if let Some(row) = list.row_at_index(opening) {
            list.select_row(Some(&row));
            row.grab_focus();
        }
    }

    /// The drive list, which only Windows has anything above the root to
    /// show.
    fn show_roots(self: &Rc<Self>) {
        let roots = crate::browser::roots();
        if roots.is_empty() {
            return;
        }
        let (page, list, back) = list_page("Drives", true);
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

        self.wire_navigation(&list, &[]);
        *self.screen.borrow_mut() = Screen::Browser;
        self.window.set_child(Some(&page));
        if let Some(row) = list.row_at_index(0) {
            list.select_row(Some(&row));
            row.grab_focus();
        }
    }

    /// Opens whichever picker suits how the request arrived.
    fn choose_video(self: &Rc<Self>) {
        let mode = self.config.borrow().file_browser;
        let built_in = match mode {
            crate::config::BrowserMode::BuiltIn => true,
            crate::config::BrowserMode::System => false,
            crate::config::BrowserMode::Automatic => self.gamepad_last.get(),
        };
        if built_in {
            let (remembered, last_video) = {
                let config = self.config.borrow();
                (config.last_folder.clone(), config.last_video.clone())
            };
            let start =
                crate::browser::start_location(remembered.as_deref(), last_video.as_deref());
            self.show_browser(&start, None);
        } else {
            self.open_file_chooser();
        }
    }

    // --- Settings ------------------------------------------------------

    /// Everything that applies to the application rather than to the video
    /// currently loaded. Reached from the gear in the footer.
    fn show_settings(self: &Rc<Self>) {
        let (page, list, back) = list_page("Settings", true);

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
                        crate::config::Theme::Auto => "Follow the desktop".to_string(),
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
                    "File Browser".to_string(),
                    match config.file_browser {
                        crate::config::BrowserMode::Automatic => "Automatic".to_string(),
                        crate::config::BrowserMode::System => "System dialog".to_string(),
                        crate::config::BrowserMode::BuiltIn => "Built-in".to_string(),
                    },
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
                    "Primary Language".to_string(),
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
                    "Secondary Language".to_string(),
                    language(&config.secondary_language, "Second track"),
                    true,
                ),
                (
                    "Subtitle Language".to_string(),
                    language(&config.subtitle_language, "None"),
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
                    3 => app.open_setting(Setting::FileBrowser),
                    4 => app.open_setting(Setting::PrimaryDevice),
                    5 => app.open_setting(Setting::PrimaryLanguage),
                    6 => app.open_setting(Setting::SecondaryDevice),
                    7 => app.open_setting(Setting::SecondaryLanguage),
                    8 => app.open_setting(Setting::SubtitleLanguage),
                    9 => app.open_setting(Setting::SubtitleSize),
                    10 => app.open_setting(Setting::SubtitleFont),
                    11 => app.confirm_clear_data(),
                    _ => {}
                }
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.show_menu());
        }

        self.wire_navigation(&list, &[]);
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

        self.set_nav(None, &[]);
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
        crate::config::save_tracks(&path, primary, secondary, subtitle.clone());

        let app = self.clone();
        let on_ended = move || {
            app.stop_playback();
            app.show_menu();
        };

        let result = Playback::start(
            &path,
            primary,
            secondary,
            subtitle.as_ref(),
            &self.config.borrow(),
            restart,
            on_ended,
        );

        match result {
            Ok(playback) => {
                let controls = Controls::new(playback.widget());
                self.window.set_child(Some(controls.widget()));
                controls.update(&playback);
                controls.flash(false);
                *self.controls.borrow_mut() = Some(controls);
                self.start_tick();
                self.window.set_title(Some(
                    &path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                ));
                *self.playback.borrow_mut() = Some(playback);
                // Nothing to move a selection through here.
                self.set_nav(None, &[]);
                *self.screen.borrow_mut() = Screen::Playing;
            }
            Err(e) => self.show_error(&format!("Couldn't play that file.\n\n{e}")),
        }
    }

    /// Centred rather than top-aligned, and a full screen rather than a
    /// modal dialog: it has to be readable at the same distance as
    /// everything else and navigable without a pointer.
    fn show_confirm_quit(self: &Rc<Self>) {
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
        self.set_nav(None, &[]);
        *self.screen.borrow_mut() = Screen::ConfirmQuit;
        self.window.set_child(Some(&page));
        // Cancel takes focus so a reflexive second Enter doesn't quit.
        cancel.grab_focus();
    }

    fn show_error(self: &Rc<Self>, message: &str) {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();

        page.append(&heading_label("Something went wrong"));

        let label = gtk::Label::new(Some(message));
        label.add_css_class("tp-hint");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        label.set_max_width_chars(48);
        page.append(&label);

        let back = gtk::Button::with_label("Back");
        back.add_css_class("tp-button");
        back.set_halign(gtk::Align::Center);
        page.append(&back);

        let app = self.clone();
        back.connect_clicked(move |_| app.show_menu());

        // Nothing to move a selection through here.
        self.set_nav(None, &[]);
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
fn list_page(title: &str, show_back: bool) -> (gtk::Box, gtk::ListBox, gtk::Button) {
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
        .build();
    let back = back_button();
    if !show_back {
        // Kept in the layout so it still occupies its space, but invisible
        // and skipped by focus.
        back.set_opacity(0.0);
        back.set_sensitive(false);
        back.set_can_focus(false);
    }
    header.append(&back);

    let heading = heading_label(title);
    heading.set_xalign(0.0);
    header.append(&heading);
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

    (page, list, back)
}

/// Uppercased here rather than with the `text-transform` CSS property,
/// which needs a newer GTK than this project's baseline.
fn heading_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(&text.to_uppercase()));
    label.add_css_class("tp-title");
    label
}

fn back_button() -> gtk::Button {
    // An icon rather than a text glyph: a "‹" character sits off the
    // vertical centre because it's positioned by font metrics rather than
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

fn chooser_row(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-row");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

/// GTK rings the system bell when a keyboard move can't go anywhere — at
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
        .tp-menu > row {{ border-radius: {radius}px; }}
        .tp-menu > row.tp-section-start {{ margin-top: {section}px; }}
        /* The selected row keeps the same colours whether or not the list
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
           background-color set underneath it. The label is coloured
           separately because themes set button text colour directly. */
        button:focus {{
            background-image: none;
            background-color: {highlight};
        }}
        button:focus label {{ color: #ffffff; }}
        /* Chrome-less until pointed at, but the arrow itself stays visible
           so the way back is always apparent. */
        .tp-back {{
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
        /* Laid over the picture, so it sets its own colours rather than
           inheriting theme ones that may be light. */
        .tp-controls {{
            background-color: rgba(0, 0, 0, 0.75);
            padding: {pad_v}px {pad_h}px;
        }}
        .tp-time {{ font-size: {hint}px; color: #ffffff; }}
        .tp-transport {{ -gtk-icon-size: {icon}px; color: #ffffff; }}
        .tp-progress {{ min-height: {bar}px; }}
        .tp-progress progress {{ background-color: {highlight}; }}
        .tp-gear {{ padding: {pad_v}px {pad_h}px; }}
        .tp-gear image {{ -gtk-icon-size: {icon}px; }}
        .{video} {{ background-color: black; }}
        ",
        title = px(20.0),
        tracking = px(2.0).max(1),
        row = px(26.0),
        hint = px(20.0),
        pad_v = px(16.0),
        pad_h = px(24.0),
        radius = px(8.0),
        section = px(28.0),
        icon = px(24.0),
        bar = px(6.0),
        // A literal colour rather than a theme name: GTK's named colours
        // differ between themes and libadwaita, and an undefined one makes
        // the whole declaration fail to parse — which silently leaves the
        // highlighted row unreadable. Both foreground and background are
        // set for the same reason: overriding only the background left the
        // theme's white selection text on a pale colour.
        highlight = "#3584e4",
        video = crate::player::VIDEO_CSS_CLASS,
    )
}
