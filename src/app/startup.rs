//! Building the window and everything hung off it, once, at launch.

use super::*;

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
            now_playing_queued: Cell::new(false),
            fade_art: Cell::new(false),
            backdrop_widget: RefCell::new(None),
            poster_frame: RefCell::new(None),
            series_frame: RefCell::new(None),
            poster_art: RefCell::new(None),
            backdrop_art: RefCell::new(None),
            series_art: RefCell::new(None),
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
            audio_files: RefCell::new(Vec::new()),
            errand: Cell::new(Errand::Video),
            primary_baseline: Cell::new(0.0),
            secondary_baseline: Cell::new(0.0),
            duration_s: Cell::new(0.0),
            primary_track: RefCell::new(None),
            secondary_track: RefCell::new(None),
            subtitle_options: RefCell::new(Vec::new()),
            subtitle: RefCell::new(None),
            subtitle_by_hand: Cell::new(false),
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
            in_settings_pane: Cell::new(false),
            pane_items: RefCell::new(Vec::new()),
            about_scroll: RefCell::new(None),
            copy_root: RefCell::new(None),
            settings_switches: RefCell::new(Vec::new()),
            settings_list: RefCell::new(None),
            settings_categories: RefCell::new(None),
            settings_body: RefCell::new(None),
            kodi_setups: RefCell::new(Vec::new()),
            clicked_row: Cell::new(false),
            settling_switch: Cell::new(false),
            key_held: Cell::new(false),
            hold_started: Cell::new(false),
            releases: Cell::new(0),
            wanted_scale: Cell::new(None),
            nav_footer: RefCell::new(Vec::new()),
            nav_header: RefCell::new(Vec::new()),
            nav_middle: RefCell::new(Vec::new()),
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
            jellyfin: RefCell::new(None),
            jellyfin_pairing: RefCell::new(crate::jellyfin::load()),
            jellyfin_attempt: Cell::new(0),
            connect_from: Cell::new(ConnectFrom::Settings),
            jellyfin_item: RefCell::new(None),
            jellyfin_session: RefCell::new(None),
            jellyfin_reported: Cell::new(0),
            jellyfin_play_session: RefCell::new(String::new()),
            session_resume: RefCell::new(None),
            subtitles_hidden: Cell::new(false),
            volume_save_pending: Cell::new(false),
            hushed: Cell::new(false),
            sound_report_pending: Cell::new(false),
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

        {
            let weak = Rc::downgrade(&app);
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(move |_, _, _| {
                if let Some(app) = weak.upgrade() {
                    app.show_pointer();
                }
            });
            window.add_controller(motion);
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
            // Resolves a spec and puts the answer where that output reads it:
            // a track index, or a path for a soundtrack beside the video.
            // Both, because one number can now mean either - the chooser lists
            // them as one run and so does `--list-tracks`.
            let apply =
                |spec: &str, track: &RefCell<Option<u32>>, file: &RefCell<Option<Source>>| {
                    match crate::probe::resolve_audio(
                        spec,
                        &app.tracks.borrow(),
                        &app.audio_files.borrow(),
                    ) {
                        Ok(crate::probe::AudioChoice::Silent) => *track.borrow_mut() = None,
                        Ok(crate::probe::AudioChoice::Track(index)) => {
                            *track.borrow_mut() = Some(index)
                        }
                        Ok(crate::probe::AudioChoice::File(path)) => {
                            *file.borrow_mut() = Some(Source::File(path))
                        }
                        // Reported rather than obeyed silently, the same way a
                        // subtitle that cannot be resolved is: playing the
                        // wrong track is not what was asked for either.
                        Err(e) => eprintln!("{e}"),
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
                    None => apply(spec, &app.primary_track, &app.primary_file),
                }
            }
            if let Some(spec) = preset.secondary.as_deref() {
                match as_file(spec) {
                    Some(file) => *app.secondary_file.borrow_mut() = Some(file),
                    None => apply(spec, &app.secondary_track, &app.secondary_file),
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

        // Reaches the paired Jellyfin server, if there is one. Everything it
        // does is allowed to fail quietly: a server that is off is not a
        // reason for a video player to say anything on the way up.
        app.start_jellyfin();
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
                    app.enter_settings();
                }
            });
        }
        gtk_app.add_action(&settings);
        bind_accels(gtk_app, "app.settings", &["comma"]);
    }
}
