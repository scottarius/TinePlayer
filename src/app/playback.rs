//! Starting a video, and the screens shown while it runs or when it fails.

use super::*;

impl App {
    /// Shows the black video surface, then starts playback a frame later.
    ///
    /// Building the pipeline and seeking to a resume position both happen on
    /// this thread, so nothing repaints until they finish. Swapping the window
    /// first and letting one frame through means the menu disappears the
    /// instant Play is pressed, and the wait happens against black - which is
    /// what a video starting looks like anyway. Accurate seeking made this
    /// worth doing: it decodes forward to the exact position, and on a long
    /// film that is visible.
    pub(super) fn start_playback(self: &Rc<Self>, restart: bool) {
        if self.file.borrow().is_none() {
            return;
        }

        let waiting = gtk::Box::builder()
            .css_classes([crate::player::VIDEO_CSS_CLASS])
            .hexpand(true)
            .vexpand(true)
            .build();
        self.window.set_child(Some(&waiting));

        // Playback begins on a frame that has actually been drawn.
        //
        // This used to wait 16 milliseconds and hope - one frame at 60Hz - and
        // that held from a button press, where GTK is already mid-way through
        // dispatching an event and will draw shortly after. It did not hold
        // when the play came from a media key, which arrives on an idle
        // callback: the pipeline was built against a surface that had never
        // been presented, and on an AV1 video being resumed the D3D12 decoder
        // deadlocked in `gst_video_decoder_finish_frame` while holding the pad
        // lock the seek's flush needed. TinePlayer stopped drawing entirely,
        // and only from that entry point, and only on the first play of a
        // session. Found 2026-08-13 with a debugger on the hung process.
        //
        // The tick callback is GTK's own answer to "after the next frame", so
        // there is nothing to tune and nothing to be unlucky with.
        let started = Rc::new(Cell::new(false));
        {
            let app = self.clone();
            let started = started.clone();
            waiting.add_tick_callback(move |_, _| {
                if !started.replace(true) {
                    // Queued rather than run here. A tick callback fires in
                    // the frame clock's update phase, which is before the
                    // frame is painted - building the pipeline in it left the
                    // surface still unpresented and deadlocked in the same
                    // place, with the menu visibly still on screen. An idle
                    // queued from here runs once that whole frame, paint
                    // included, has finished.
                    let app = app.clone();
                    glib::idle_add_local_once(move || app.begin_playback(restart));
                }
                glib::ControlFlow::Break
            });
        }
        // And a way out for a window that is never drawn at all - minimized,
        // or hidden behind something full screen - where no frame arrives and
        // the tick callback would never run. Waiting forever there would be a
        // worse bug than the one above.
        {
            let app = self.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                if !started.replace(true) {
                    app.begin_playback(restart);
                }
            });
        }
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
                self.subtitle_by_hand.get(),
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

        // Worked out here rather than in the pipeline, because locating one
        // can need the server address and access token, which are ours to
        // know. A subtitle that cannot be found gives up the subtitle and not
        // the film: it is the least of what somebody pressed play for.
        let located = match self.locate_subtitle(&path, subtitle.as_ref()) {
            Ok(located) => located,
            Err(e) => {
                log::error!("{e}");
                None
            }
        };
        // Either there is something to switch to, or something already chosen.
        // The second half is not redundant: `--play` goes straight past the
        // page that fills the list in, so a subtitle named on the command line
        // would otherwise resolve correctly and then have no overlay to be
        // drawn by.
        let offers_subtitles = !self.subtitle_options.borrow().is_empty() || located.is_some();

        let result = Playback::start(
            &path,
            primary_audio.as_ref(),
            secondary_audio.as_ref(),
            located.as_ref(),
            offers_subtitles,
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
                    self.external,
                );
                controls.set_levels(&levels);
                // The pipeline built each output's level from that output's own
                // setting, which is all the configuration told it. The main
                // level is applied here, once, before a frame has played - the
                // same shape as the alignment baseline below it.
                controls.set_main_level(self.config.borrow().main_volume());
                for (role, _) in &outputs {
                    playback.set_volume(role, self.effective(self.config.borrow().volume(role)));
                }
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
                        // Through the one function that knows about the main
                        // level, rather than sent straight to the sink from
                        // here. What an output plays at is its own level times
                        // the main level, and a second place doing that
                        // arithmetic is how the two come to disagree - the same
                        // lesson `push_offset` is already the answer to.
                        //
                        // Given the level rather than reading it back out of
                        // the configuration, because a level that is not being
                        // kept never reaches the configuration at all.
                        app.push_volume_at(role, level);
                        app.push_mute(role, muted);
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

                    // The main level moves both outputs, so both are pushed
                    // again rather than one being singled out. Kept like a
                    // level and unlike a hush: somebody chose it, and a film
                    // that started at half volume because of last week is a
                    // setting, where a film that started silent would be a bug.
                    // Silencing everything is a layer over the outputs rather
                    // than a change to them, so what comes back is only whether
                    // the layer is on. Each output is then pushed at its own
                    // state underneath it, which is what it goes on showing.
                    let app = self.clone();
                    controls.connect_hush(move |hushed| {
                        app.hushed.set(hushed);
                        for role in ["primary", "secondary"] {
                            let muted = app.config.borrow().muted(role);
                            app.push_mute(role, muted);
                        }
                        app.report_sound_soon();
                    });

                    let app = self.clone();
                    controls.connect_main(move |level| {
                        app.config.borrow_mut().set_main_volume(level);
                        for role in ["primary", "secondary"] {
                            app.push_volume(role);
                        }
                        app.save_volume_soon();
                        app.report_sound_soon();
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
                        app.toggle_pause();
                        app.wake_controls();
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_fullscreen(move || app.toggle_fullscreen());
                }
                {
                    // Holding the icon, which shows or hides what is already
                    // chosen. Tapping it opens the chooser instead.
                    let app = self.clone();
                    controls.connect_subtitles(move || app.toggle_subtitles());
                }
                {
                    let app = self.clone();
                    controls.connect_subtitle_chosen(move |entry| app.choose_subtitle(entry));
                }
                {
                    let app = self.clone();
                    controls.connect_audio_chosen(move |role, entry| app.choose_audio(role, entry));
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
                    controls.connect_back(move || app.leave_playback());
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
                                    app.publish_now_playing();
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
                                app.publish_now_playing();
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
                self.show_subtitle_state(&playback, &controls);
                self.push_audio_entries(&playback, &controls);
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
                self.publish_now_playing();
                self.jellyfin_reported.set(0);
                self.report_to_jellyfin(JellyfinMoment::Started);
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
            Err(e) => self.show_error(
                &tr!("Couldn't play that file.\n\n{reason}", reason = e),
                false,
            ),
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
    pub(super) fn show_confirm_quit(self: &Rc<Self>) {
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

        page.append(&heading_label(&tr!("Close the Player?")));

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();

        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        let quit = gtk::Button::with_label(&tr!("Close"));
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
        self.window.set_child(Some(&self.dialog_column(&page)));
        // Cancel takes focus so a reflexive second Enter doesn't quit.
        cancel.grab_focus();
    }

    pub(super) fn show_error(self: &Rc<Self>, message: &str, fatal: bool) {
        self.error_is_fatal.set(fatal);

        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(32)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Center)
            .margin_start(48)
            .margin_end(48)
            .build();

        page.append(&heading_label(&tr!("Something went wrong")));

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
        let back = gtk::Button::with_label(&if fatal { tr!("Close") } else { tr!("Back") });
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
        self.window.set_child(Some(&self.dialog_column(&page)));
        back.grab_focus();
    }
}
