//! The popover lists behind each settings row, and what a choice from one does.

use super::*;

impl App {
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
    pub(super) fn scan_devices_soon(self: &Rc<Self>, then: impl Fn(&Rc<Self>) + 'static) {
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
            // Written down whenever it changes, because "no devices are
            // offered" and "the second output is silent" are both questions
            // about this list, and neither can be answered without seeing what
            // the machine actually reported.
            match names.is_empty() {
                true => log::info!("Audio outputs: none found"),
                false => log::info!("Audio outputs: {}", names.join(", ")),
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
        let mut dividers: Vec<(usize, Option<String>)> = Vec::new();
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                if setting == Setting::SecondaryDevice {
                    entries.push((trc!("audio output device", "None").into_owned(), None));
                    // A rule under it. "None" here means "play nothing on a
                    // second output", which is a different kind of answer to
                    // the hardware listed below it - and the only list where
                    // this one is offered at all.
                    dividers.push((1, None));
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
                    entries.push((tr!("Searching for outputs...").into_owned(), None));
                }
                for (position, name) in devices.iter().enumerate() {
                    if configured.as_deref() == Some(name.as_str()) {
                        current = Some(position);
                    }
                    entries.push((name.clone(), Some(position)));
                }
            }
            Setting::Subtitles => {
                entries.push((trc!("subtitle track", "None").into_owned(), None));
                // Under "None", and again above the row that leaves the film
                // to go looking on disk. What sits between is what the file
                // itself offers, and the two either side of it are answers of
                // a different kind.
                dividers.push((1, None));
                let chosen = self.subtitle.borrow().clone();
                for (position, option) in self.subtitle_options.borrow().iter().enumerate() {
                    if chosen.as_ref() == Some(&option.choice()) {
                        current = Some(position);
                    }
                    entries.push((crate::subtitles::row_native(option), Some(position)));
                }
                // Last, after everything the video came with, the same way the
                // track lists offer one: a subtitle file from somewhere else
                // is the answer when what is wanted is not beside the film.
                dividers.push((entries.len(), None));
                entries.push((
                    tr!("Browse...").into_owned(),
                    Some(self.subtitle_options.borrow().len()),
                ));
            }
            Setting::PrimaryTrack | Setting::SecondaryTrack => {
                entries.push((trc!("audio track", "None").into_owned(), None));
                dividers.push((1, None));
                let role = if setting == Setting::PrimaryTrack {
                    Role::Primary
                } else {
                    Role::Secondary
                };
                let chosen = *self.track_for(role).borrow();
                let file = self.file_for(role).borrow().clone();
                let found = self.audio_files.borrow();
                // Which of the files beside the video this output has been
                // given, where it has been given one of them. A file chosen by
                // hand from elsewhere is none of them and gets the row at the
                // bottom instead, so nothing is ever listed twice.
                let beside = file
                    .as_ref()
                    .and_then(|file| file.local())
                    .and_then(|path| found.iter().position(|audio| audio.path == path));
                for (position, track) in self.tracks.borrow().iter().enumerate() {
                    if file.is_none() && chosen == Some(track.index) {
                        current = Some(position);
                    }
                    entries.push((describe_audio_track(track), Some(position)));
                }
                // "None" is not one of them, and the choices below count from
                // the end of the tracks.
                let tracks = entries.len() - 1;
                // Then the separate soundtracks sitting beside the film, found
                // by the same convention as the subtitle files beside it: a
                // described or dubbed track downloaded next to a video is the
                // commonest thing there is to want here, and nobody should
                // have to go looking on disk for a file already in the folder.
                if !found.is_empty() {
                    dividers.push((entries.len(), Some(tr!("AUDIO FILES").into_owned())));
                }
                // The rows say only what they are. Every one of them used to
                // begin "Audio File:", which put the same three words down the
                // whole group where the heading says it once - and pushed what
                // tells one file from another towards the end of a row that
                // ellipsizes, on the one screen this is read across a room
                // from.
                for (position, audio) in found.iter().enumerate() {
                    if beside == Some(position) {
                        current = Some(tracks + position);
                    }
                    entries.push((audio.label(), Some(tracks + position)));
                }
                // Last, after everything the film came with and everything
                // sitting beside it: a file from somewhere else entirely,
                // which is the answer when it is neither.
                let elsewhere = tracks + found.len();
                dividers.push((entries.len(), None));
                match file.as_ref().filter(|_| beside.is_none()) {
                    Some(file) => {
                        current = Some(elsewhere);
                        entries.push((file.label(), Some(elsewhere)));
                    }
                    None => entries.push((tr!("Browse...").into_owned(), Some(elsewhere))),
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
                dividers.push((1, None));
                // Worded exactly as the settings row shows it when unset, so
                // the list and the value it came from agree.
                entries.push((
                    if setting == Setting::PrimaryLanguage {
                        tr!("First track").into_owned()
                    } else {
                        tr!("Second track").into_owned()
                    },
                    None,
                ));
                for position in crate::languages::display_order() {
                    let code = crate::languages::LANGUAGES[position].0;
                    entries.push((crate::languages::display_name(code), Some(position)));
                }
            }
            Setting::SubtitleKind => {
                // Five entries, no languages: this half asks what to show and
                // the row below asks whose language to show it in.
                let setting = self
                    .config
                    .borrow()
                    .subtitle_kind
                    .clone()
                    .unwrap_or_else(|| crate::subtitles::DEFAULT_KIND.to_string());
                current = crate::subtitles::KINDS
                    .iter()
                    .position(|value| *value == setting);
                // Under "None", which is the one entry that turns the whole
                // thing off rather than choosing among kinds.
                dividers.push((1, None));
                for (position, value) in crate::subtitles::KINDS.iter().enumerate() {
                    let label = crate::subtitles::kind_label(value)
                        .map(std::borrow::Cow::into_owned)
                        .unwrap_or_else(|| (*value).to_string());
                    entries.push((label, Some(position)));
                }
            }
            Setting::SubtitleLanguage => {
                // Following an output first, then the languages, in one list:
                // they answer the same question, and following an output is
                // the answer most people want - it tracks whatever is actually
                // being heard, file by file, where naming a language is a
                // guess that holds until it does not.
                let places = crate::subtitles::PLACES.len();
                let setting = self
                    .config
                    .borrow()
                    .subtitle_language
                    .clone()
                    .unwrap_or_else(|| crate::subtitles::DEFAULT_PLACE.to_string());
                current = crate::subtitles::PLACES
                    .iter()
                    .position(|value| *value == setting)
                    .or_else(|| {
                        crate::languages::LANGUAGES
                            .iter()
                            .position(|(code, _, _, _)| *code == setting)
                            .map(|position| places + position)
                    });
                dividers.push((places, None));
                for (position, value) in crate::subtitles::PLACES.iter().enumerate() {
                    let label = crate::subtitles::place_label(value)
                        .map(std::borrow::Cow::into_owned)
                        .unwrap_or_else(|| (*value).to_string());
                    entries.push((label, Some(position)));
                }
                for position in crate::languages::display_order() {
                    let code = crate::languages::LANGUAGES[position].0;
                    entries.push((
                        crate::languages::display_name(code),
                        Some(places + position),
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
            Setting::InterfaceLanguage => {
                let chosen = self.config.borrow().language.clone();
                let languages = crate::i18n::offered();
                current = languages.iter().position(|offered| offered.code == chosen);
                // Below "Use the system language", which is not a language but
                // the answer almost everybody wants and so sits above the list
                // rather than in it.
                dividers.push((1, None));
                for (position, offered) in languages.into_iter().enumerate() {
                    entries.push((offered.label, Some(position)));
                }
            }
            Setting::KodiType(index) => {
                use crate::kodi_setup::Registration;
                let Some((state, configured)) =
                    self.with_kodi_setup(index, |setup| (setup.state, setup.is_configured()))
                else {
                    return Choices {
                        entries,
                        current,
                        dividers,
                    };
                };
                current = Registration::ALL.iter().position(|option| *option == state);
                for (position, option) in Registration::ALL.iter().enumerate() {
                    entries.push((option.choice(configured).to_string(), Some(position)));
                }
                // A rule above removal, and only when it is a removal. The
                // other two entries are states to be in and this one is a
                // thing to do, which is the same reason the secondary device
                // list rules off its "None".
                if configured {
                    dividers.push((Registration::ALL.len() - 1, None));
                }
            }
            Setting::KodiHandover(index) => {
                let plays = self.with_kodi(index, |setup| setup.play);
                current = Some(usize::from(plays));
                for (position, choice) in handover().into_iter().enumerate() {
                    entries.push((choice.into_owned(), Some(position)));
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
    pub(super) fn show_selector(self: &Rc<Self>, setting: Setting, anchor: &gtk::ListBoxRow) {
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
                    entry.set_xalign(appearance::text_end());
                    entry.set_justify(appearance::text_justify());
                    append_named(&list, &entry, text);
                }
                // Opened on whatever is already in force. Grabbing focus
                // scrolls it into view, which is what a long list needs.
                let opening = fresh
                    .iter()
                    .position(|(_, choice)| *choice == current)
                    .unwrap_or(0) as i32;
                *entries.borrow_mut() = fresh;
                // A rule above the entries that begin a group, or a heading
                // where the group is named. A header rather than a row of its
                // own, for the reason the media page's group headings give:
                // headers sit outside the selection model and the focus chain,
                // so neither can be landed on. Set on every fill, since the
                // rows they describe are rebuilt each time.
                let scale = app.scale.get();
                list.set_header_func(move |row, _| {
                    let group = dividers
                        .iter()
                        .find(|(at, _)| *at == row.index() as usize)
                        .map(|(_, caption)| caption);
                    match group {
                        Some(Some(caption)) => {
                            // Never the first row, so it always takes its top
                            // margin: a named group opens below rows rather
                            // than at the top of the list.
                            let heading = group_heading(caption, scale, false);
                            // Against the same edge as the rows it labels,
                            // which in a popover is the far one - see the note
                            // on the rows above. The page's own headings start
                            // where their rows start, and so does this.
                            heading.set_xalign(appearance::text_end());
                            row.set_header(Some(&heading))
                        }
                        Some(None) => {
                            row.set_header(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)))
                        }
                        None => row.set_header(None::<&gtk::Widget>),
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
        aim_at_value(&popover, anchor, content_width + px(SELECTOR_PAD) * 2);

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

    pub(super) fn wire_navigation(
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
    pub(super) fn remember_tracks(&self) {
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
    pub(super) fn saved_path(&self, role: Role) -> Option<std::path::PathBuf> {
        self.file_for(role)
            .borrow()
            .as_ref()
            .and_then(|file| file.local().map(|path| path.to_path_buf()))
    }

    /// What the menu shows against the Subtitles row.
    pub(super) fn describe_subtitle(&self) -> String {
        let Some(chosen) = self.subtitle.borrow().clone() else {
            return trc!("subtitle track", "None").into_owned();
        };
        self.subtitle_options
            .borrow()
            .iter()
            .find(|option| option.choice() == chosen)
            .map(crate::subtitles::row_native)
            .unwrap_or_else(|| trc!("subtitle track", "None").into_owned())
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
                        log::error!("Failed to save config: {e}");
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
            Setting::SubtitleKind => {
                let picked = choice.map(|index| crate::subtitles::KINDS[index].to_string());
                let mut config = self.config.borrow_mut();
                config.subtitle_kind = picked;
                let _ = config.save();
            }
            Setting::SubtitleLanguage => {
                let places = crate::subtitles::PLACES.len();
                let picked = choice.map(|index| match index.checked_sub(places) {
                    Some(language) => crate::languages::LANGUAGES[language].0.to_string(),
                    None => crate::subtitles::PLACES[index].to_string(),
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
            Setting::InterfaceLanguage => {
                let picked = choice
                    .and_then(|index| crate::i18n::offered().into_iter().nth(index))
                    .and_then(|offered| offered.code);
                {
                    let mut config = self.config.borrow_mut();
                    config.language = picked.clone();
                    let _ = config.save();
                }
                // Put in force straight away rather than on the next start.
                // The caller rebuilds this pane the moment this returns - the
                // same route a choice of output device takes - so the row that
                // was just used, and every row around it, comes back in the
                // new language. Nothing else on screen is older than that:
                // the playback controls are built per film, and Settings
                // cannot be reached while one is playing.
                crate::i18n::set_language(picked.as_deref());
            }
            Setting::KodiType(index) => return self.choose_kodi_type(index, choice),
            Setting::KodiHandover(index) => {
                let (Some(chosen), Some(setup)) = (choice, self.kodi_at(index)) else {
                    return false;
                };
                // Everything else about the entry stays as it is: this rewrites
                // our own element with one argument different. No confirmation
                // and no backup - by the time this row can be worked at all,
                // the entry being edited is one we wrote.
                let state = setup.state;
                if self.write_kodi(&setup, state, None, chosen == 1) {
                    self.return_to_kodi_settings();
                }
                return true;
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
                self.subtitle_by_hand.set(true);
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
                let found = self.audio_files.borrow().len();
                // The row past the last of them all is the one that goes
                // looking on disk, and opens a screen instead of settling
                // anything here.
                if choice == Some(count + found) {
                    self.browse_for_audio(role);
                    return true;
                }
                // A soundtrack found beside the video, taken up exactly as one
                // chosen by hand is - because that is what it is, with the
                // looking already done.
                if let Some(position) = choice.filter(|choice| *choice >= count) {
                    let path = self
                        .audio_files
                        .borrow()
                        .get(position - count)
                        .map(|audio| audio.path.clone());
                    if let Some(path) = path {
                        self.use_audio_file(role, &path);
                    }
                    return false;
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
}
