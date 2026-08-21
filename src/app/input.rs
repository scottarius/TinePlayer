//! Keys, media keys, the gamepad, and moving the selection around a screen.

use super::*;

impl App {
    /// What a media key means, wherever the platform reported it from: a
    /// keysym on Linux, a `WM_APPCOMMAND` on Windows. Says whether it was
    /// used, which Windows needs in order to decide whether to pass the key
    /// on to whatever else would have played.
    ///
    /// With a video chosen but not started, play begins it: the media page is
    /// where somebody arrives before pressing anything, and a play key that
    /// does nothing there reads as a broken key rather than as a deliberate
    /// silence. Everything else needs a film already running, and says so by
    /// declining the key so it can go to whatever else would have played.
    pub(super) fn handle_media(self: &Rc<Self>, command: crate::media_keys::Command) -> bool {
        use crate::media_keys::Command;

        // Read and released before anything below can borrow it again.
        let playing = self
            .playback
            .borrow()
            .as_ref()
            .map(|playback| playback.is_playing());

        let Some(is_playing) = playing else {
            // Nothing is loaded to start, or there is nowhere to play it.
            let ready = self.file.borrow().is_some() && self.config.borrow().primary_sink.is_some();
            return match command {
                Command::Play | Command::PlayPause if ready => {
                    self.start_playback(false);
                    true
                }
                _ => false,
            };
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
    pub(super) fn go_back(self: &Rc<Self>) {
        // Copied out first: the handlers below take the same cell mutably,
        // and holding the read borrow across them panics.
        let screen = *self.screen.borrow();
        match screen {
            Screen::Playing => self.leave_playback(),
            Screen::Confirm | Screen::Notices => self.show_settings(),
            // Everything Kodi opens is opened from the Kodi pane and
            // returns straight to it. Each is one panel over that pane rather
            // than a step in a sequence, so there is no part-answered state to
            // step back into: on a confirmation this is the same as pressing
            // Cancel, which is what Escape should mean on a panel whose other
            // button says Cancel.
            Screen::KodiConfirm
            | Screen::KodiFolder
            | Screen::KodiPermission
            | Screen::KodiError => self.return_to_kodi_settings(),
            // The same, for the pane beside it. Backing out of a waiting code
            // abandons the pairing rather than pausing it: the polling stops
            // because this screen is no longer showing, and the code the
            // server issued is left to expire on its own.
            Screen::JellyfinConnect | Screen::JellyfinPanel => self.leave_jellyfin_connect(),
            // Nothing to go back to when the video we were started for could
            // not be opened.
            Screen::Error if self.error_is_fatal.get() => self.window.close(),
            Screen::Opening => self.show_paste_uri(),
            // Leaving the middle step abandons the measurement rather than
            // stepping back into the track list: the thread cannot be stopped,
            // but its answer is dropped, and nothing has been written.
            Screen::PasteUri
            | Screen::Browser
            | Screen::Shortcuts
            | Screen::AlignChoose
            | Screen::AlignProgress
            | Screen::AlignResult => self.return_to_origin(),
            // Out of the settings and back to the categories, and only then
            // out of the screen. Two steps because it is entered in two.
            Screen::Settings if self.in_settings_pane.get() => self.hold_settings_categories(),
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
    pub(super) fn start_tick(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        // Ticks since Kodi was last told where playback had reached. Counted
        // here rather than given a timer of its own so that it stops when
        // playback does, without anything extra to tear down.
        let mut since_report = 0u32;
        // What was last published to the system as the running time. Playback
        // can begin before GStreamer has worked the duration out, and a
        // now-playing entry claiming a film is zero seconds long stays wrong
        // until something else happens to republish it. Kept beside
        // `since_report` as closure state for the same reason: it belongs to
        // this timer and stops when it does.
        let mut published_duration = 0f64;
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

                    // Only when it changes, which is normally once a film.
                    // The position is deliberately not pushed on the tick:
                    // the system extrapolates it from the rate, so it stays
                    // right on its own between the moments that do change it.
                    let duration = playback
                        .duration()
                        .map(|time| time.nseconds() as f64 / 1e9)
                        .unwrap_or(0.0);
                    if duration != published_duration {
                        published_duration = duration;
                        app.publish_now_playing();
                    }

                    since_report += 1;
                    // Every 30 seconds, so that a player killed outright still
                    // leaves Kodi's library close to where you actually got to.
                    if since_report >= 300 {
                        since_report = 0;
                        playback.report_to_kodi();
                    }

                    // Jellyfin more often, because a phone watching this
                    // session shows the position as it moves rather than only
                    // after the fact. Ten seconds is what its own clients
                    // send, and it is one small request.
                    let told = app.jellyfin_reported.get() + 1;
                    app.jellyfin_reported.set(told);
                    if told >= 100 {
                        app.jellyfin_reported.set(0);
                        app.report_to_jellyfin(JellyfinMoment::Progress);
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
    pub(super) fn wake_controls(&self) {
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
    pub(super) fn enter_controls(self: &Rc<Self>) {
        use crate::controls::Row;
        let Some(controls) = self.controls.borrow().clone() else {
            return;
        };
        match controls.row() {
            Row::None => controls.set_row(Row::Buttons),
            Row::Buttons => controls.set_row(Row::Timeline),
            Row::Timeline => {}
            // The menu opens upward out of its button, so up climbs its rows
            // and stops at the top of them.
            Row::Volume => controls.move_output(-1),
            // A chooser opens the same way, so up climbs it likewise.
            Row::Audio => controls.move_audio(-1),
            Row::Subtitles => controls.move_subtitle(-1),
        }
    }

    /// Escape, and the gamepad's B: back out by one step, whatever the
    /// outermost thing on screen happens to be.
    ///
    /// The ladder, from the top: the key list, then an open chooser, then the
    /// strip itself, and only once all of that is gone does it leave the film.
    /// Each press is rid of the last thing that appeared, which is what "back"
    /// means everywhere else in the application.
    ///
    /// **Leaving the film is the last rung and never a shortcut past the
    /// others.** It used to be the first: Escape went straight out whatever the
    /// strip was doing, reasoning that Down already put the strip away and
    /// spending Escape on it too would make leaving a film two presses. That is
    /// the wrong way round in practice. The strip is on screen because you just
    /// did something, so the key for getting out of what you are in should get
    /// you out of *that* - and a film ended by a stray Escape costs far more
    /// than a second press does.
    pub(super) fn back_out(self: &Rc<Self>) {
        use crate::controls::Row;
        // The key list first, wherever it is: it is laid over everything else
        // and has to be got rid of before "back" can mean anything else.
        let controls = self.controls.borrow().clone();
        if let Some(controls) = controls
            && controls.close_shortcuts()
        {
            return;
        }
        // Cloned out rather than acted on through the borrow, the way every
        // other caller here does it: `set_row` runs the strip's own handlers,
        // and holding the cell open across them is how a re-entrant borrow
        // panic gets written.
        let Some(controls) = self.controls.borrow().clone() else {
            // Not playing at all, so the strip is not the thing being backed
            // out of - the screen is.
            self.go_back();
            return;
        };
        match controls.row() {
            // Back to the buttons rather than off the strip entirely, so the
            // icon the chooser came out of is highlighted and can be seen to
            // have changed - the same landing choosing a row gives.
            Row::Volume | Row::Audio | Row::Subtitles => controls.set_row(Row::Buttons),
            // The strip is being driven, so letting go of it is the step.
            Row::Buttons | Row::Timeline => controls.set_row(Row::None),
            // Nothing is being driven, but the strip may still be up from a
            // moved pointer or a seek, and that is what is on screen to lose.
            Row::None if controls.is_showing() => controls.hide(),
            // Nothing left over the film. Now it means the film.
            Row::None => self.go_back(),
        }
    }

    /// Down: back to the buttons from the timeline, then let the strip go.
    pub(super) fn leave_controls(self: &Rc<Self>) {
        use crate::controls::Row;
        let Some(controls) = self.controls.borrow().clone() else {
            return;
        };
        match controls.row() {
            Row::Timeline => controls.set_row(Row::Buttons),
            Row::Buttons => controls.set_row(Row::None),
            // Down the rows of the menu, and off the bottom of it back to the
            // speaker the menu came out of.
            Row::Volume => {
                if controls.at_last_output() {
                    controls.set_row(Row::Buttons);
                } else {
                    controls.move_output(1);
                }
            }
            // Down the soundtracks, and off the bottom back to the icon the
            // chooser came out of.
            Row::Audio => {
                if controls.at_last_audio() {
                    controls.set_row(Row::Buttons);
                } else {
                    controls.move_audio(1);
                }
            }
            // Down the list of subtitles, and off the bottom of it back to
            // the icon the chooser came out of.
            Row::Subtitles => {
                if controls.at_last_subtitle() {
                    controls.set_row(Row::Buttons);
                } else {
                    controls.move_subtitle(1);
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
    pub(super) fn strip_takes_enter(&self) -> bool {
        self.playback.borrow().is_some()
            && self
                .controls
                .borrow()
                .as_ref()
                .is_some_and(|controls| controls.takes_activation())
    }

    pub(super) fn press_activate(self: &Rc<Self>) {
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
        self.hold_started.set(holds.is_some());
        match holds {
            Some(hold) => controls.press_hold(hold),
            None => controls.activate_focused(),
        }
    }

    /// Letting go of a held button. Does the ordinary thing unless the hold
    /// already did something else.
    pub(super) fn release_activate(self: &Rc<Self>) {
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
        if self.hold_started.replace(false) && controls.release_hold() {
            controls.activate_focused();
        }
    }

    /// Writes the configuration out a second after the last volume change,
    /// rather than on each one. The level itself takes effect immediately;
    /// this is only about remembering it.
    /// Tells a controller what the sound is doing now, rather than leaving it
    /// to the next scheduled report.
    ///
    /// Those go every ten seconds, which is right for a position that a phone
    /// can interpolate between and wrong for a level: moving the main level in
    /// the room left the slider on somebody's phone showing the old value for
    /// most of a minute, which reads as a remote that has lost the player
    /// rather than one that is a moment behind. Reported by Scott, 2026-08-14.
    ///
    /// The same debounce the configuration write uses, and for the same reason,
    /// with a shorter wait because this one is about what somebody is watching
    /// happen on a second screen.
    pub(super) fn report_sound_soon(self: &Rc<Self>) {
        if self.sound_report_pending.replace(true) {
            return;
        }
        let app = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            app.sound_report_pending.set(false);
            // The scheduled report starts its ten seconds again from here, so a
            // drag does not leave one following a moment behind it.
            app.jellyfin_reported.set(0);
            app.report_to_jellyfin(JellyfinMoment::Progress);
        });
    }

    pub(super) fn save_volume_soon(self: &Rc<Self>) {
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
    pub(super) fn controls_left_right(self: &Rc<Self>, direction: isize) {
        use crate::controls::Row;
        let row = self
            .controls
            .borrow()
            .as_ref()
            .map(|controls| controls.row())
            .unwrap_or(Row::None);
        // Swallowed rather than passed on: there is nowhere sideways to go in
        // a list, and seeking the film out from under an open chooser would be
        // worse than doing nothing.
        if matches!(row, Row::Subtitles | Row::Audio) {
            return;
        }
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
    pub(super) fn scrub(self: &Rc<Self>, seconds: f64) {
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
    pub(super) fn end_scrub(&self) {
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

    pub(super) fn toggle_fullscreen(&self) {
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

    /// Takes the pointer off the screen once something else is driving.
    ///
    /// Fullscreen only. A window sits on a desktop the pointer belongs to as
    /// much as to us - there is a title bar above it and other windows behind -
    /// and one that vanishes while crossing an application is one somebody
    /// then has to go hunting for. Fullscreen is the case where this is all
    /// there is, and a pointer left over the menu is just something on screen.
    ///
    /// The playback strip does the same for itself against the picture; this
    /// is the menus, which had no reason to think about the pointer until they
    /// filled a television.
    pub(super) fn hide_pointer(&self) {
        if self.window.is_fullscreen() {
            self.window.set_cursor_from_name(Some("none"));
        }
    }

    /// And puts it back the moment it moves, which is the only signal that
    /// somebody has picked the mouse up again.
    pub(super) fn show_pointer(&self) {
        self.window.set_cursor(None);
    }

    /// Records what the gamepad should be moving through. Screens built from
    /// buttons alone pass `None`, and fall back to GTK's directional focus.
    pub(super) fn set_nav(
        &self,
        list: Option<&gtk::ListBox>,
        header: &[gtk::Button],
        footer: &[gtk::Button],
    ) {
        // Every screen goes through here, which makes it the one place that
        // can be sure a screen with selectable text is no longer the one on
        // display. A screen that has some sets it again afterwards.
        *self.copy_root.borrow_mut() = None;
        *self.nav_list.borrow_mut() = list.cloned();
        *self.nav_header.borrow_mut() = header.to_vec();
        // Cleared here so it belongs to one screen only: a page that wants Up
        // to land somewhere particular says so after wiring its navigation.
        *self.nav_header_entry.borrow_mut() = None;
        *self.nav_middle.borrow_mut() = Vec::new();
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
    pub(super) fn last_header(header: &[gtk::Button]) -> Option<&gtk::Button> {
        header.iter().rev().find(|button| button.is_sensitive())
    }

    pub(super) fn handle_action(self: &Rc<Self>, action: crate::gamepad::Action) {
        use crate::gamepad::Action;
        self.hide_pointer();
        match action {
            Action::Shortcuts => self.toggle_shortcuts(),
            // While the list is up it is the only thing on screen worth
            // driving, so the D-pad scrolls it rather than working the strip
            // underneath - which is hidden anyway.
            Action::Up | Action::Down if self.shortcuts_showing() => {
                let delta = if action == Action::Up { -1 } else { 1 };
                if let Some(controls) = self.controls.borrow().clone() {
                    controls.scroll_shortcuts(delta);
                }
            }
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
            // The bars answer these where there is one, and nothing else on
            // that screen does - see the key handler for why silence matters.
            Action::Left if self.on_settings() => {
                self.settings_slider(-1);
            }
            Action::Right if self.on_settings() => {
                self.settings_slider(1);
            }
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
                self.toggle_pause();
                self.wake_controls();
            }
            Action::Activate => self.activate_focused(),
            // Nothing is playing, so Start is what the page's Play button is:
            // the way to it without crossing the page to press it. It did
            // nothing at all here, which on the one screen a pad is most
            // likely to be picked up on read as a dead button.
            Action::PlayPause => {
                self.play_from_page();
            }
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
            //
            // Through the same ladder Escape takes rather than a copy of half
            // of it, which is what this was - it took the strip down whole,
            // open chooser and all, where Escape stepped out of the chooser
            // first. One press, one rung, on both.
            Action::Back => self.back_out(),
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
    pub(super) fn move_selection(self: &Rc<Self>, delta: i32) {
        if self.scroll_about(delta) {
            return;
        }
        // Cloned out before anything can rebuild the screen underneath us.
        let list = self.nav_list.borrow().clone();
        let footer = self.nav_footer.borrow().clone();
        let header = self.nav_header.borrow().clone();
        let middle = self.nav_middle.borrow().clone();

        let Some(list) = list else {
            // A screen of buttons and no rows. Between the two rows by name,
            // since a directional search cannot reliably get from one to the
            // other when they are not above one another on the page.
            let focused = |buttons: &[gtk::Button]| buttons.iter().any(|button| button.has_focus());
            // Three rows where there is a middle one, and the middle is
            // stepped over rather than stopped at when it is empty - which is
            // every screen but the empty page.
            let landing = match delta {
                _ if delta > 0 && focused(&header) => middle.first().or(footer.first()),
                _ if delta > 0 && focused(&middle) => footer.first(),
                _ if delta < 0 && focused(&footer) => middle.first().or(header.first()),
                _ if delta < 0 && focused(&middle) => header.first(),
                _ => None,
            };
            if let Some(button) = landing {
                self.sounds.borrow().click();
                button.grab_focus();
                return;
            }
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
    pub(super) fn activate_focused(self: &Rc<Self>) {
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

    /// Tells the system what is playing, where the system cares.
    ///
    /// Called when the answer changes rather than on the tick: what is
    /// published is a position and a rate, and macOS extrapolates between
    /// them, so a film left playing stays correct without being told again.
    /// The four moments that do change it are playback starting, pausing,
    /// seeking, and stopping.
    ///
    /// Silent everywhere but macOS. See `media_keys::NowPlaying` for why it
    /// is not merely cosmetic there: it is what decides who receives a media
    /// key at all.
    pub(super) fn publish_now_playing(self: &Rc<Self>) {
        // Queued behind the main loop rather than done here.
        //
        // Telling the system what is playing means writing the poster to disk
        // and several calls into another process, and `begin_playback` reaches
        // this by way of `stop_playback` - so all of that was running in the
        // middle of building the pipeline. On 2026-08-13 that was enough to
        // hang TinePlayer outright: the main thread ended up blocked inside
        // `gst_pad_push_event`, waiting on a lock a streaming thread held,
        // with the delay this introduced landing squarely in the window where
        // that race is possible.
        //
        // Nothing here needs to be immediate. The panel wants to know within a
        // moment, and the main loop is idle a moment later by definition.
        if self.now_playing_queued.replace(true) {
            return;
        }
        let app = self.clone();
        glib::idle_add_local_once(move || {
            app.now_playing_queued.set(false);
            app.send_now_playing();
        });
    }

    /// Gathers what is playing and hands it to the platform.
    fn send_now_playing(&self) {
        // Nothing chosen at all: the panel goes away rather than sitting
        // there empty. An empty one is worse than none, because the name it
        // shows when it has no title is the application's own identifier.
        if self.file.borrow().is_none() {
            crate::media_keys::set_now_playing(None);
            return;
        }
        let seconds = |time: Option<gstreamer::ClockTime>| {
            time.map(|time| time.nseconds() as f64 / 1e9).unwrap_or(0.0)
        };
        // A video chosen but not started is published too, stopped rather than
        // absent, so the panel names what is about to be watched and its play
        // button has something to do. Without it the media page showed a panel
        // with no title at all.
        let playback = self.playback.borrow();
        let (duration_s, elapsed_s, playing) = match playback.as_ref() {
            Some(playback) => (
                seconds(playback.duration()),
                seconds(playback.position()),
                playback.is_playing(),
            ),
            None => (self.details.borrow().duration_s, 0.0, false),
        };
        drop(playback);
        crate::media_keys::set_now_playing(Some(crate::media_keys::NowPlaying {
            // The same title as the titlebar and the media page, from the one
            // chain that resolves it.
            title: self.file_label().unwrap_or_default(),
            duration_s,
            elapsed_s,
            playing,
            // The poster the page found, cloned rather than borrowed: this
            // runs a handful of times per film, and the alternative is a
            // lifetime threaded through a platform boundary for nothing.
            artwork: self.details.borrow().poster.clone(),
        }));
    }
}
