//! The keyboard: the shortcut list it can put on screen, the handler, and the actions only it reaches.

use super::*;

impl App {
    /// Whether the key list is over the picture right now.
    pub(super) fn shortcuts_showing(&self) -> bool {
        self.controls
            .borrow()
            .as_ref()
            .is_some_and(|controls| controls.shortcuts_open())
    }

    /// Shows the key list, or puts it away again.
    ///
    /// Two ways of showing one list, because there is no single way that works
    /// on both sides: during playback it is an overlay over the picture, since
    /// a page would replace the window's child and take the video widget with
    /// it, and everywhere else it is an ordinary page.
    pub(super) fn toggle_shortcuts(self: &Rc<Self>) {
        if let Some(controls) = self.controls.borrow().clone() {
            controls.toggle_shortcuts(self.scale.get());
            return;
        }
        if *self.screen.borrow() == Screen::Shortcuts {
            self.return_to_origin();
        } else {
            self.show_shortcuts();
        }
    }

    /// The key list as a page, for the menus.
    fn show_shortcuts(self: &Rc<Self>) {
        let px = |base: f64| (base * self.scale.get()).round() as i32;
        // Centered on both axes, like every other panel that floats over a
        // screen. Without the vertical half the box takes the whole window,
        // and the dialog around it grows with it however short the list is -
        // which reads as a panel that failed to size itself.
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(20.0))
            .valign(gtk::Align::Center)
            .margin_top(px(28.0))
            .margin_bottom(px(28.0))
            .margin_start(px(32.0))
            .margin_end(px(32.0))
            .build();
        page.append(&heading_label(&tr!("Keys and Buttons")));

        // Scrolled for the same reason the notices are: on a small window, or
        // at a large interface scale, the list is taller than the screen.
        //
        // Not expanded, unlike the notices: two hundred crates are always
        // longer than the window and a fixed share of it reads as a page,
        // where a dozen keys are not, and a panel stretched to the window
        // around them reads as a panel that failed to size itself.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .vexpand(false)
            .valign(gtk::Align::Center)
            .child(&crate::shortcuts::page(self.scale.get()))
            .build();
        scroller.set_focusable(false);
        let height = (self.window.height() as f64 * NOTICES_SHARE).round() as i32;
        scroller.set_max_content_height(height.max(px(320.0)));
        page.set_halign(gtk::Align::Center);
        page.append(&scroller);

        let close = gtk::Button::with_label(&tr!("Close"));
        close.add_css_class("tp-button");
        close.set_halign(gtk::Align::Center);
        page.append(&close);
        {
            let app = self.clone();
            close.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_origin();
            });
        }

        self.remember_origin();
        // Nothing to step through, so up and down scroll it - the same
        // arrangement the notices use.
        self.set_nav(None, std::slice::from_ref(&close), &[]);
        *self.about_scroll.borrow_mut() = Some(scroller.vadjustment());
        *self.screen.borrow_mut() = Screen::Shortcuts;
        self.window.set_child(Some(&self.modal(&page)));
        close.grab_focus();
    }

    /// The level over every output, moved by a key rather than by the panel.
    /// Cloned out of the cell before it is used, the way every other caller
    /// here does it, since what it reaches takes the same borrows.
    fn nudge_main(self: &Rc<Self>, delta: isize) {
        if let Some(controls) = self.controls.borrow().clone() {
            controls.nudge_main(delta);
        }
    }

    pub(super) fn install_key_handling(self: &Rc<Self>) {
        let controller = gtk::EventControllerKey::new();
        let primary = primary_mask();
        let app = self.clone();
        controller.connect_key_pressed(move |_, key, _, state| {
            app.hide_pointer();
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
                // The key list, which is the one binding that has to work
                // wherever you are - including on the screen where you have
                // forgotten what any of the others do.
                gdk::Key::F1 => {
                    app.toggle_shortcuts();
                    glib::Propagation::Stop
                }
                // The level over both outputs, on the keys every other player
                // uses for it. `equal` as well as `plus` because they are the
                // same key and nobody holds Shift to turn a film up.
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add if playing => {
                    app.nudge_main(1);
                    glib::Propagation::Stop
                }
                gdk::Key::minus | gdk::Key::KP_Subtract if playing => {
                    app.nudge_main(-1);
                    glib::Propagation::Stop
                }
                // In the menus they belong to a slider if one is selected,
                // and to nothing otherwise.
                gdk::Key::Left if app.settings_slider(-1) => glib::Propagation::Stop,
                gdk::Key::Right if app.settings_slider(1) => glib::Propagation::Stop,
                // And nothing at all otherwise, anywhere on that screen.
                //
                // Left unhandled the key falls through to GTK's own
                // directional search, which finds whichever pane is to the
                // side and moves the focus into it - stepping between the two
                // by a route that Enter and Escape were meant to replace. A
                // row with no bar on it has nothing for these keys to do.
                gdk::Key::Left | gdk::Key::Right if app.on_settings() => glib::Propagation::Stop,
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
                // One step back per press, down the ladder in `back_out`: a
                // chooser, then the strip, then the film. The same rung the
                // gamepad's B takes, through the same code, because the key
                // list on `F1` promises they are the same thing.
                gdk::Key::Escape => {
                    app.back_out();
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
                // The one letter claimed *outside* playback, and the reason
                // Space is not: Space belongs to whatever row has focus, so a
                // media page where it played the film would stop opening the
                // chooser somebody had arrowed onto. P is what Kodi binds Play
                // to, and Kodi is the launcher most of this application's
                // viewers arrive through.
                //
                // `play_from_page` is what decides, and says no everywhere the
                // letter is worth more as a letter - which is what hands the
                // key back to the browser's type-ahead.
                gdk::Key::p | gdk::Key::P if !playing && app.play_from_page() => {
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
                // Steps each output through the file's audio tracks while it
                // plays. A shortcut ahead of the real thing, which is a chooser
                // per output on the control strip.
                gdk::Key::a | gdk::Key::A if playing => {
                    app.cycle_audio("primary");
                    glib::Propagation::Stop
                }
                gdk::Key::s | gdk::Key::S if playing => {
                    app.cycle_audio("secondary");
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

    /// Play or resume from the media page, for the two shortcuts that stand
    /// in for its Play button: `P` and the pad's Start.
    ///
    /// Answers whether it did anything, which is what lets the key be claimed
    /// only where it means this - press `P` in the file browser and it is
    /// still a letter to jump by.
    ///
    /// **Only from the media page, and only when nothing is open over it.**
    /// Every other screen has a use for the letter or nothing to play: the
    /// browser and the choosers jump by it, and a selector open over the page
    /// is a question being answered, not a page waiting to be played from. A
    /// popover is found by looking up from whatever has the focus, there being
    /// no register of the open one - and `take_nav` moving the arrows into it
    /// says nothing about the keys this handler still sees.
    ///
    /// Resuming rather than restarting, which is what the button it stands for
    /// does: `start_playback(false)` keeps the saved position, and Restart is
    /// a button of its own for the other answer.
    pub(super) fn play_from_page(self: &Rc<Self>) -> bool {
        if *self.screen.borrow() != Screen::Menu || self.playback.borrow().is_some() {
            return false;
        }
        // Spelled out because `focus` is on two traits the window implements
        // and neither is the obvious one.
        let mut widget = gtk::prelude::GtkWindowExt::focus(&self.window);
        while let Some(current) = widget {
            if current.is::<gtk::Popover>() {
                return false;
            }
            widget = current.parent();
        }
        // The same readiness the media key asks about, through the same path:
        // there is one answer to "can this play now" and one place that gives
        // it.
        self.handle_media(crate::media_keys::Command::Play)
    }

    /// Pause or resume, keeping the display-awake hold in step with it.
    /// Everything that pauses goes through here.
    ///
    /// The button on the controls and the gamepad used to call the pipeline
    /// directly, each repeating the two lines below. That was harmless until
    /// there were three of them and one had something extra to do: pausing
    /// from the screen left the system's now-playing widget still showing a
    /// pause button, and pressing it did nothing, while the media key on the
    /// keyboard - which did come through here - worked.
    pub(super) fn toggle_pause(self: &Rc<Self>) {
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.toggle_pause();
            self.awake.set(playback.is_playing());
        }
        self.publish_now_playing();
    }
}
