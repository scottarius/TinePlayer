use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gstreamer::prelude::DeviceExt;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::config::Config;
use crate::devices::list_audio_output_devices;
use crate::player::Playback;
use crate::sound::Sounds;
use crate::probe::{probe_audio_tracks, AudioTrack};

/// Which setting a chooser screen is editing. The menu drills into one of
/// these and returns once a choice is made.
#[derive(Clone, Copy, PartialEq)]
enum Setting {
    PrimaryDevice,
    PrimaryTrack,
    SecondaryDevice,
    SecondaryTrack,
}

/// Menu rows that begin a new group: the primary pair and the secondary
/// pair each get separating space above them.
const SECTION_STARTS: [i32; 2] = [1, 3];

/// Tracked so Escape can mean "go back one level" rather than one fixed
/// action: out of playback, out of a chooser, or out of the application.
#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Menu,
    Chooser,
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
    playback: RefCell<Option<Rc<Playback>>>,
    screen: RefCell<Screen>,
    /// Restored when returning from a chooser, so the menu comes back with
    /// the row you left from still highlighted.
    menu_row: RefCell<i32>,
    sounds: RefCell<Sounds>,
    restart: bool,
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
        apply_styles(config.ui_scale);
        suppress_error_bell();
        let sounds = Sounds::new(config.sounds, config.primary_sink.clone());

        let window = gtk::ApplicationWindow::builder()
            .application(gtk_app)
            .title("TinePlayer")
            .default_width(1100)
            .default_height(700)
            .build();

        let app = Rc::new(App {
            window: window.clone(),
            config: RefCell::new(config),
            file: RefCell::new(None),
            tracks: RefCell::new(Vec::new()),
            primary_track: RefCell::new(None),
            secondary_track: RefCell::new(None),
            playback: RefCell::new(None),
            screen: RefCell::new(Screen::Menu),
            menu_row: RefCell::new(0),
            sounds: RefCell::new(sounds),
            restart,
        });

        // Playback has to be torn down before the window goes away, so the
        // resume position is written and the audio devices are released.
        {
            let app = app.clone();
            window.connect_close_request(move |_| {
                app.stop_playback();
                glib::Propagation::Proceed
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
                app.start_playback();
            }
            _ => app.show_menu(),
        }

        window.present();
    }

    fn install_key_handling(self: &Rc<Self>) {
        let controller = gtk::EventControllerKey::new();
        let app = self.clone();
        controller.connect_key_pressed(move |_, key, _, _| {
            let playing = app.playback.borrow().is_some();
            match key {
                // Only claimed during playback — the menus need Space for
                // activating whatever row has focus.
                gdk::Key::space if playing => {
                    if let Some(playback) = app.playback.borrow().as_ref() {
                        playback.toggle_pause();
                    }
                    glib::Propagation::Stop
                }
                // Always goes back one level, so it never quits by surprise
                // from somewhere the user was only browsing.
                gdk::Key::Escape => {
                    // Copied out first: the handlers below take the same
                    // cell mutably, and holding the read borrow across them
                    // panics.
                    let screen = *app.screen.borrow();
                    match screen {
                        Screen::Playing => {
                            app.stop_playback();
                            app.show_menu();
                        }
                        Screen::Chooser | Screen::Error | Screen::ConfirmQuit => app.show_menu(),
                        Screen::Menu => app.show_confirm_quit(),
                    }
                    glib::Propagation::Stop
                }
                // Available on every screen, not just during playback: on a
                // television the menus want the whole display too.
                gdk::Key::f | gdk::Key::F => {
                    if app.window.is_fullscreen() {
                        app.window.unfullscreen();
                    } else {
                        app.window.fullscreen();
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(controller);
    }

    fn stop_playback(&self) {
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
        let rows: Vec<(String, String, bool)> = vec![
            (
                "Video".to_string(),
                file.as_ref()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                    .unwrap_or_else(|| "Choose a video…".to_string()),
                true,
            ),
            (
                "Primary Audio Device".to_string(),
                config.primary_sink.clone().unwrap_or_else(|| "Not set".to_string()),
                true,
            ),
            (
                "Primary Audio Track".to_string(),
                if has_file { describe_track(&self.primary_track.borrow()) } else { "—".to_string() },
                has_file,
            ),
            (
                "Secondary Audio Device".to_string(),
                config.secondary_sink.clone().unwrap_or_else(|| "None".to_string()),
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
        let play = gtk::Button::with_label("▶  Play");
        play.add_css_class("tp-play");
        play.set_sensitive(can_play);
        page.append(&play);

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                *app.menu_row.borrow_mut() = row.index();
                match row.index() {
                    0 => app.open_file_chooser(),
                    1 => app.show_chooser(Setting::PrimaryDevice),
                    2 => app.show_chooser(Setting::PrimaryTrack),
                    3 => app.show_chooser(Setting::SecondaryDevice),
                    4 => app.show_chooser(Setting::SecondaryTrack),
                    _ => {}
                }
            });
        }
        {
            let app = self.clone();
            play.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.start_playback();
            });
        }

        self.wire_navigation(&list, Some(&play));

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
        };
        let (page, list, back) = list_page(title, true);

        // Entries are (display text, choice). `None` means the "None"
        // option, which every list offers except the primary device — an
        // output has to exist for anything to play.
        let mut entries: Vec<(String, Option<usize>)> = Vec::new();
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                if setting == Setting::SecondaryDevice {
                    entries.push(("None".to_string(), None));
                }
                match list_audio_output_devices() {
                    Ok(devices) => {
                        for (position, device) in devices.iter().enumerate() {
                            entries.push((device.display_name().to_string(), Some(position)));
                        }
                    }
                    Err(e) => entries.push((format!("Error: {e}"), None)),
                }
            }
            Setting::PrimaryTrack | Setting::SecondaryTrack => {
                entries.push(("None".to_string(), None));
                for (position, track) in self.tracks.borrow().iter().enumerate() {
                    entries.push((describe_audio_track(track), Some(position)));
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
                app.show_menu();
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.show_menu());
        }

        self.wire_navigation(&list, None);

        *self.screen.borrow_mut() = Screen::Chooser;
        self.window.set_child(Some(&page));
        if let Some(row) = list.row_at_index(0) {
            list.select_row(Some(&row));
            row.grab_focus();
        }
    }

    /// Arrow keys don't move focus out of a ListBox, so the boundary
    /// between the list and the button below it has to be bridged by hand —
    /// otherwise the button is unreachable without a pointer. Movements
    /// that would go past either end are swallowed, which also stops GTK
    /// reporting them as failed navigation.
    fn wire_navigation(self: &Rc<Self>, list: &gtk::ListBox, footer: Option<&gtk::Button>) {
        {
            let app = self.clone();
            let list_weak = list.downgrade();
            let footer = footer.map(|b| b.downgrade());
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
                    if let Some(button) = footer.as_ref().and_then(|b| b.upgrade()) {
                        if button.is_sensitive() {
                            app.sounds.borrow().click();
                            button.grab_focus();
                        }
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

        if let Some(button) = footer {
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


    fn apply_choice(self: &Rc<Self>, setting: Setting, choice: Option<usize>) {
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                let names: Vec<String> = list_audio_output_devices()
                    .map(|devices| devices.iter().map(|d| d.display_name().to_string()).collect())
                    .unwrap_or_default();
                let picked = choice.and_then(|index| names.get(index).cloned());

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
                if setting == Setting::PrimaryDevice {
                    let (enabled, device) = {
                        let config = self.config.borrow();
                        (config.sounds, config.primary_sink.clone())
                    };
                    *self.sounds.borrow_mut() = Sounds::new(enabled, device);
                }
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

        // Matroska only: the pipeline demuxes with matroskademux, so
        // offering other containers would just produce a failure after the
        // user had already chosen one.
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Matroska video (.mkv, .webm)"));
        for pattern in ["*.mkv", "*.webm", "*.MKV", "*.WEBM"] {
            filter.add_pattern(pattern);
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
            if response == gtk::ResponseType::Accept {
                if let Some(path) = chooser.file().and_then(|f| f.path()) {
                    app.set_file(&path);
                }
            }
            held.borrow_mut().take();
            app.show_menu();
        });
        chooser.show();
    }

    /// Probes the file and preselects sensible tracks: the first for the
    /// primary output, and a different one for the secondary where the file
    /// offers a choice — which is the whole point of the application.
    fn set_file(self: &Rc<Self>, path: &std::path::Path) {
        match probe_audio_tracks(path) {
            Ok(tracks) => {
                *self.primary_track.borrow_mut() = tracks.first().map(|t| t.index);
                *self.secondary_track.borrow_mut() = tracks.get(1).map(|t| t.index);
                *self.tracks.borrow_mut() = tracks;
                *self.file.borrow_mut() = Some(path.to_path_buf());
            }
            Err(e) => {
                eprintln!("Couldn't read {}: {e}", path.display());
                *self.tracks.borrow_mut() = Vec::new();
                *self.primary_track.borrow_mut() = None;
                *self.secondary_track.borrow_mut() = None;
                *self.file.borrow_mut() = None;
            }
        }
    }

    // --- Playback ------------------------------------------------------

    fn start_playback(self: &Rc<Self>) {
        let Some(path) = self.file.borrow().clone() else {
            return;
        };
        self.stop_playback();

        let app = self.clone();
        let on_ended = move || {
            app.stop_playback();
            app.show_menu();
        };

        let result = Playback::start(
            &path,
            *self.primary_track.borrow(),
            *self.secondary_track.borrow(),
            &self.config.borrow(),
            self.restart,
            on_ended,
        );

        match result {
            Ok(playback) => {
                self.window.set_child(Some(playback.widget()));
                self.window.set_title(Some(
                    &path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                ));
                *self.playback.borrow_mut() = Some(playback);
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
        let quit = gtk::Button::with_label("Quit");
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
fn apply_styles(scale: f64) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let px = |base: f64| (base * scale).round() as i32;

    let css = format!(
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
        // A literal colour rather than a theme name: GTK's named colours
        // differ between themes and libadwaita, and an undefined one makes
        // the whole declaration fail to parse — which silently leaves the
        // highlighted row unreadable. Both foreground and background are
        // set for the same reason: overriding only the background left the
        // theme's white selection text on a pale colour.
        highlight = "#3584e4",
        video = crate::player::VIDEO_CSS_CLASS,
    );

    let provider = gtk::CssProvider::new();
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
