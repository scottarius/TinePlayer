//! The settings screen: its categories, its rows, and what each row does when it is used.

use super::*;

impl App {
    /// Everything that applies to the application rather than to the video
    /// currently loaded. Reached from the gear in the footer.
    /// What a settings row is called.
    fn item_label(&self, item: Item) -> String {
        match item {
            Item::InterfaceScale => tr!("Interface Size").into_owned(),
            Item::InterfaceLanguage => tr!("Interface Language").into_owned(),
            Item::Sounds => tr!("Navigation Sounds").into_owned(),
            Item::StartFullscreen => tr!("Always Start Fullscreen").into_owned(),
            Item::ReadMetadata => tr!("Read Metadata Beside Files").into_owned(),
            Item::ShowBackdrop => tr!("Show Backdrop Artwork").into_owned(),
            Item::ResumeThreshold => tr!("Resume Threshold").into_owned(),
            Item::WatchedThreshold => tr!("Watched Threshold").into_owned(),
            Item::Updates => tr!("Check for updates").into_owned(),
            Item::UpdateStatus => self.version_label(),
            Item::ClearData => tr!("Clear Saved Playback Data").into_owned(),
            Item::Device(_) => tr!("Output Device").into_owned(),
            Item::Language(_) => tr!("Preferred Language").into_owned(),
            Item::Description(_) => tr!("Prefer Audio Description").into_owned(),
            Item::Volume(_) => tr!("Volume").into_owned(),
            Item::Sync(_) => tr!("Audio Sync").into_owned(),
            Item::SubtitlePreference => tr!("Subtitle Preference").into_owned(),
            Item::SubtitleSize => tr!("Subtitle Size").into_owned(),
            Item::SubtitleFont => tr!("Subtitle Font").into_owned(),
            Item::KodiType(_) => tr!("Configure As").into_owned(),
            Item::KodiHandover(_) => tr!("When Kodi Opens TinePlayer").into_owned(),
            Item::KodiPermission(_) => tr!("Sandbox Permission").into_owned(),
            Item::KodiNone => tr!("No Kodi installations were found on this system").into_owned(),
            // Named for what it actually wants. "Add a Kodi Folder" asked for
            // the wrong thing: Kodi's own folder is not where
            // playercorefactory.xml goes, and choosing it lands one level
            // above the folder that is.
            Item::KodiAdd => tr!("Add User Data Folder").into_owned(),
            Item::JellyfinConnect => tr!("Connect to Jellyfin").into_owned(),
            Item::JellyfinDisconnect => match self.jellyfin_server_label() {
                // Named rather than positional, so a translator can put the
                // server where their own grammar wants it.
                Some(server) => tr!("Disconnect from {server}", server = server).into_owned(),
                None => tr!("Disconnect").into_owned(),
            },
            Item::Notices => tr!("Third-Party Notices").into_owned(),
        }
    }

    /// What it reads against the label. Empty for the rows that carry a
    /// switch or a bar, which show their state in the control itself, and for
    /// the ones that only open something.
    fn item_value(&self, item: Item) -> String {
        let config = self.config.borrow();
        match item {
            Item::Device(role) => {
                let sink = match role {
                    Role::Primary => config.primary_sink.clone(),
                    Role::Secondary => config.secondary_sink.clone(),
                };
                // Both carry a context. "None" here is an output device that
                // was deliberately left off, which several languages spell
                // differently from "None" meaning no subtitles - and a
                // translator handed the bare word cannot tell which is which.
                sink.unwrap_or_else(|| match role {
                    Role::Primary => trc!("audio output device", "Not set").into_owned(),
                    Role::Secondary => trc!("audio output device", "None").into_owned(),
                })
            }
            Item::Language(role) => {
                let (code, unset) = match role {
                    Role::Primary => (&config.primary_language, tr!("First track")),
                    Role::Secondary => (&config.secondary_language, tr!("Second track")),
                };
                match code {
                    Some(code) => crate::languages::display_name(code),
                    None => unset.into_owned(),
                }
            }
            Item::InterfaceLanguage => {
                // What is *set*, not what is in force. Those differ on a
                // machine set to a language with no catalog, and this row is
                // the setting rather than a report - "Use the system language"
                // is the honest reading of an unset preference even where the
                // system's language is one nothing answers to.
                let chosen = config.language.clone();
                drop(config);
                crate::i18n::offered()
                    .into_iter()
                    .find(|offered| offered.code == chosen)
                    .map(|offered| offered.label)
                    // A config naming a language this build has no catalog
                    // for. The code itself says more than a blank would.
                    .or(chosen)
                    .unwrap_or_default()
            }
            Item::SubtitlePreference => {
                crate::subtitles::describe(config.subtitle_language.as_deref())
            }
            Item::SubtitleFont => config
                .subtitle_font
                .clone()
                .unwrap_or_else(|| crate::pipeline::DEFAULT_SUBTITLE_FONT.to_string()),
            Item::KodiType(index) => {
                drop(config);
                self.with_kodi(index, |setup| {
                    // What it cannot do outranks what it is set to. A Snap is
                    // never set to anything, and saying "Not configured"
                    // invites somebody to try.
                    match setup.confinement.supported() {
                        false => tr!("Not supported").into_owned(),
                        true => setup.state.describe().into_owned(),
                    }
                })
            }
            Item::KodiHandover(index) => {
                drop(config);
                self.with_kodi(index, |setup| {
                    handover()[usize::from(setup.play)].to_string()
                })
            }
            // Not a claim that it has been granted, which nothing here checks.
            // The row opens the instructions, and this says there are some.
            Item::KodiPermission(_) => tr!("Action needed").into_owned(),
            Item::UpdateStatus => {
                drop(config);
                self.version_status()
            }
            _ => String::new(),
        }
    }

    /// Whether the switch on this row is on, for the rows that have one.
    fn item_switch(&self, item: Item) -> Option<bool> {
        let config = self.config.borrow();
        Some(match item {
            // On means the size is worked out from the screen, which is the
            // one switch here that turns the bar beside it off rather than on.
            Item::InterfaceScale => config.ui_scale.is_none(),
            Item::Sounds => config.sounds,
            Item::StartFullscreen => config.fullscreen,
            Item::ReadMetadata => config.read_metadata,
            Item::ShowBackdrop => config.show_backdrop,
            Item::Description(Role::Primary) => config.primary_audio_description,
            Item::Description(Role::Secondary) => config.secondary_audio_description,
            Item::Volume(role) => !config.muted(role.key()),
            Item::Sync(role) => config.offset_on(role.key()),
            Item::Updates => config.check_for_updates,
            _ => return None,
        })
    }

    /// A line under the row explaining what it does, for the settings whose
    /// names do not say it.
    ///
    /// Most do not have one, and that is the point: a note under every row is
    /// a wall of text nobody reads, and the ones that matter stop standing
    /// out. These are the settings whose effect is invisible until it happens,
    /// or whose name is a term of art.
    /// `Cow` rather than `&'static str`, which is what it was: a translated
    /// string is built at run time and cannot be static. Every const table and
    /// `&'static str` return in this file carrying interface text has the same
    /// problem, and this is the first of them to be converted - see
    /// `src/i18n.rs` for what the rest of that pass involves.
    fn item_description(&self, item: Item) -> Option<Cow<'static, str>> {
        Some(match item {
            // No note. The row is called Interface Language and its value is
            // the language it is set to, which says the whole of it - and the
            // note it used to carry only repeated the name.
            Item::ReadMetadata => {
                tr!(
                    "Find and read metadata beside video files like .nfo and images often provided by media libraries."
                )
            }
            Item::ShowBackdrop => {
                tr!("If backdrop artwork is found, display it behind the video details.")
            }
            Item::ResumeThreshold => {
                tr!(
                    "How much of a video should be viewed before offering the choice to resume a previously watched video."
                )
            }
            Item::WatchedThreshold => {
                tr!("How much of a video should be viewed to consider it as watched.")
            }
            Item::Language(_) => tr!("Attempt to auto-select a language track for the output."),
            Item::Description(_) => {
                tr!("Attempt to auto-select an Audio Description track for the output.")
            }
            Item::Sync(_) => {
                tr!(
                    "Adjust the audio sync for the output. Useful for countering latency with bluetooth speakers and headphones."
                )
            }
            Item::SubtitlePreference => tr!("Attempt to auto-select subtitles when available."),
            Item::ClearData => {
                tr!("Delete remembered video preferences, track choices, and resume positions.")
            }
            Item::KodiPermission(_) => {
                tr!("This Kodi installation is sandboxed and needs permission to start TinePlayer.")
            }
            // Says which folder, because the obvious guess is the wrong one:
            // Kodi's user data lives apart from Kodi itself, and it does not
            // exist until Kodi has been run once.
            Item::KodiAdd => {
                tr!(
                    "For a Kodi installation in a non-standard location, such as a portable install. Its user data folder is the one holding guisettings.xml, not the folder Kodi itself is installed in."
                )
            }
            // Says what will happen, because the answer is unusual enough to
            // be worth knowing before pressing it: no password is ever typed
            // into TinePlayer, which is the whole reason this is a code and
            // not a login form.
            Item::JellyfinConnect => {
                // TRANSLATORS: "Quick Connect" is Jellyfin's own name for this
                // feature. Use whatever Jellyfin's translation into your language
                // calls it, rather than translating it afresh - this text sends
                // somebody to find it in their own Jellyfin app, and a different
                // wording sends them looking for something that is not there.
                tr!("Find and connect to a Jellyfin server using Quick Connect.")
            }
            Item::JellyfinDisconnect => {
                tr!(
                    "Removes the access token stored on this machine and signs this device out of the server."
                )
            }
            _ => return None,
        })
    }

    /// The note drawn under a row: its explanation, and for one row a link
    /// beside it.
    fn item_note(self: &Rc<Self>, item: Item, scale: f64) -> Option<gtk::Widget> {
        let text = row_note(&self.item_description(item)?, scale);

        // Where the data this clears actually lives, openable rather than
        // printed. A path read off a television is a path nobody is going to
        // type, and the folder is the thing wanted anyway - to take a copy of
        // it before pressing the row above, or to see that it is really gone
        // afterwards.
        //
        // The data folder rather than the config one: they are not the same
        // place, and this row does not touch settings.
        //
        // A Kodi's own folder is offered the same way, but under its group
        // heading rather than on a row - see `GroupNote`.
        if item != Item::ClearData {
            return Some(text.upcast());
        }
        let Some(folder) = crate::config::positions_path()
            .parent()
            .map(|folder| folder.to_path_buf())
        else {
            return Some(text.upcast());
        };
        let sentence = text.text().to_string();
        // On the same line as the sentence it belongs to, rather than under
        // it: two lines of small print under one row reads as a paragraph.
        text.set_markup(&format!(
            "{}  <a href=\"{}\">{}</a>",
            glib::markup_escape_text(&sentence),
            glib::markup_escape_text(&gtk::gio::File::for_path(&folder).uri()),
            glib::markup_escape_text(&tr!("Open user data folder")),
        ));
        // Reported rather than swallowed: a link that does nothing looks like
        // a link that was pressed wrongly.
        {
            let folder = folder.clone();
            text.connect_activate_link(move |_, _| {
                show_folder(&folder);
                glib::Propagation::Stop
            });
        }

        Some(text.upcast())
    }

    /// Whether the row can be worked at all.
    ///
    /// The rule in every case here is the same: a control over something that
    /// does not exist yet is worse than no control, because it invites a
    /// choice and then does nothing with it. With nothing read from beside the
    /// file there is no artwork to draw. With TinePlayer not registered in a
    /// Kodi there is no entry for a handover setting to be part of, and no
    /// reason to grant that Kodi permission to start us. And an installation
    /// that cannot start an external player at all can be set to nothing.
    fn item_enabled(&self, item: Item) -> bool {
        match item {
            Item::ShowBackdrop => self.config.borrow().read_metadata,
            Item::KodiType(index) => self.with_kodi(index, |setup| setup.confinement.supported()),
            Item::KodiHandover(index) | Item::KodiPermission(index) => self
                .with_kodi(index, |setup| {
                    setup.confinement.supported() && setup.is_configured()
                }),
            // There to be read. Landing on it would be landing on a sentence.
            Item::KodiNone => false,
            _ => true,
        }
    }

    /// What to call the paired server on screen: its own name where it gave
    /// one, and its address otherwise. `None` when there is no pairing at all.
    pub(super) fn jellyfin_server_label(&self) -> Option<String> {
        self.jellyfin_pairing
            .borrow()
            .as_ref()
            .map(crate::jellyfin::Pairing::label)
    }

    /// Which run of the programme this is: "Season 2".
    ///
    /// The library's own wording wins where it gave one, because a number
    /// cannot say everything a name can - season zero is "Specials", and
    /// reading it back as "Season 0" would be wrong in the one case anybody
    /// would notice. Empty for a film, and for an episode whose source said
    /// neither.
    pub(super) fn season_label(&self) -> String {
        let details = self.details.borrow();
        if !details.season_name.is_empty() {
            return details.season_name.clone();
        }
        match details.episode {
            Some((season, _)) => format!("Season {season}"),
            None => String::new(),
        }
    }

    /// Whether there is a token to cast with, as the pane last read it.
    pub(super) fn jellyfin_connected(&self) -> bool {
        self.jellyfin_pairing
            .borrow()
            .as_ref()
            .is_some_and(crate::jellyfin::Pairing::is_connected)
    }

    /// Whether the Jellyfin pane is what is on screen right now.
    ///
    /// Asked by the two things that can finish long after they were started -
    /// a token going stale, and a server being told about a disconnection - so
    /// that neither redraws a screen nobody is looking at or throws a panel
    /// over a film. The screen is copied out rather than tested in place,
    /// which is the rule `go_back` records: a caller acting on the answer takes
    /// the same cell mutably.
    pub(super) fn showing_jellyfin_pane(&self) -> bool {
        let screen = *self.screen.borrow();
        screen == Screen::Settings && self.settings_category.get() == Category::Jellyfin
    }

    /// Which of the two shapes the Jellyfin pane takes.
    fn jellyfin_pane(&self) -> JellyfinPane {
        match self.jellyfin_connected() {
            true => JellyfinPane::Connected,
            false => JellyfinPane::NotConnected,
        }
    }

    /// What the Jellyfin heading says under itself.
    ///
    /// What the feature is, since a pane nobody has set up says nothing else
    /// about why it is there - and what pairing leaves behind, which is the
    /// one thing about this worth stating outright. Obfuscating a credential
    /// TinePlayer can read unattended would be theatre; saying where it is is
    /// not.
    fn jellyfin_group_note(&self) -> GroupNote {
        // Which server, and as whom - the two facts the rows used to spend
        // themselves on. Stated rather than offered, since neither is a thing
        // to press: the way to change either is to disconnect and connect
        // again, which is the row underneath.
        let connected = {
            let pairing = self.jellyfin_pairing.borrow();
            pairing.as_ref().map(|pairing| {
                let who = pairing
                    .account
                    .as_ref()
                    .map(|account| account.user_name.clone())
                    .filter(|name| !name.is_empty());
                match who {
                    Some(who) => tr!(
                        "Connected to {server} as {who}.",
                        server = pairing.label(),
                        who = who
                    )
                    .into_owned(),
                    None => tr!("Connected to {server}.", server = pairing.label()).into_owned(),
                }
            })
        };
        GroupNote {
            sentence: match self.jellyfin_pane() {
                JellyfinPane::NotConnected => tr!(
                    "Connect a Jellyfin server to cast videos to TinePlayer from the Jellyfin app in a browser, phone, or tablet."
                )
                .into_owned(),
                JellyfinPane::Connected => tr!(
                    "{connected} Videos can be cast to TinePlayer from a Jellyfin app in a browser, phone, or tablet.",
                    connected = connected.unwrap_or_default(),
                )
                .into_owned(),
            },
            // Named rather than opened. The folder holds the token, and a
            // settings screen that offers to show somebody their own
            // credential in a file manager is offering the wrong thing.
            folder: None,
        }
    }

    /// What one installation's group heading says under itself: which file it
    /// is, and either why it cannot be used or the thing true of every Kodi
    /// and invisible until it bites - it reads that file once, at startup, so
    /// a change made here does nothing until it restarts.
    fn kodi_group_note(&self, index: usize) -> Option<GroupNote> {
        self.with_kodi_setup(index, |setup| GroupNote {
            // An installation that cannot be used says why instead. Nothing
            // will be modified there, so promising that it will would be the
            // one sentence on the screen that is not true.
            sentence: setup
                .confinement
                .unsupported_reason()
                .unwrap_or_else(|| {
                    tr!(
                        "This installation's playercorefactory.xml will be modified. Restart Kodi for changes to take effect."
                    )
                })
                .into_owned(),
            folder: Some(setup.userdata().to_path_buf()),
        })
    }

    /// One installation out of the list the pane was built from, by its place
    /// in it, with a default for the moment the list has moved on from under a
    /// row that was built against it.
    pub(super) fn with_kodi<T: Default>(
        &self,
        index: usize,
        read: impl FnOnce(&crate::kodi_setup::Setup) -> T,
    ) -> T {
        self.with_kodi_setup(index, read).unwrap_or_default()
    }

    /// The same, for callers that need to tell "no such installation" apart
    /// from whatever the answer would have been.
    pub(super) fn with_kodi_setup<T>(
        &self,
        index: usize,
        read: impl FnOnce(&crate::kodi_setup::Setup) -> T,
    ) -> Option<T> {
        self.kodi_setups.borrow().get(index).map(read)
    }

    /// Opens the settings screen from outside it, at the categories.
    ///
    /// Coming back from a chooser or from About calls `show_settings` directly
    /// and keeps whichever half of the screen the keyboard was in; arriving
    /// from the menu starts where the screen starts.
    pub(super) fn enter_settings(self: &Rc<Self>) {
        self.in_settings_pane.set(false);
        self.show_settings();
    }

    /// Settings, as a column of categories and the rows of whichever one is
    /// chosen.
    ///
    /// One flat list of twenty-three rows before this, which is how it came to
    /// hold two rows called Volume and two called Audio Sync with nothing but
    /// their position to tell them apart.
    pub(super) fn show_settings(self: &Rc<Self>) {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;
        let (page, list, back, slot) = list_page(&tr!("Settings"), true, self.scale.get());

        // What the window has to spend. The monitor stands in before the
        // window has been given a size.
        let window_width = match self.window.width() {
            0 => appearance::monitor_for_window(&self.window)
                .map(|monitor| monitor.geometry().width())
                .unwrap_or(1920),
            width => width,
        };
        // A fifth of it, so a bar is a consistent share of the screen whether
        // that is a laptop or a television.
        let slider_width = window_width / 5;

        // The logo at the trailing end of the header, which is otherwise dead
        // space beside the heading.
        //
        // Added here rather than in `list_page_with`, which the file browser
        // shares: that screen's header carries a breadcrumb trail along the
        // same row, and a logo would be competing with the one thing on it
        // that has to be read.
        //
        // Drawn at every window size. A header is a box and a box does not
        // overlap what is in it: with the screen narrow enough for the two to
        // meet, what happens is the heading and the logo are pushed together,
        // not one over the other.
        if let Some(header) = slot.parent().and_downcast::<gtk::Box>() {
            let lockup = lockup_image(HORIZONTAL_LOCKUP, SETTINGS_LOCKUP * scale);
            // Takes the room left over and sits at the end of it, so the
            // heading keeps its place at the start of the row rather than
            // being centered by what follows it.
            lockup.set_hexpand(true);
            lockup.set_halign(gtk::Align::End);
            // Held off the edge by the radius of the panel beneath it, so it
            // lines up with where that corner turns rather than with a corner
            // the panel does not actually have.
            lockup.set_margin_end(px(PANEL_RADIUS));
            header.append(&lockup);
        }

        // The right-hand pane, rebuilt in place when the category changes
        // rather than by rebuilding the screen: the cursor is in the column on
        // the left at that moment, and rebuilding around it would take it away.
        // The list comes out of its scroller so a block of text can sit above
        // it inside the same one, which is what makes the two scroll together.
        // Taken out first: `gtk_box_append` refuses a widget that still has a
        // parent, and says so only in a log nobody is reading.
        let scroller = list
            .parent()
            .and_then(|viewport| viewport.parent())
            .and_downcast::<gtk::ScrolledWindow>();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        if let Some(scroller) = scroller.as_ref() {
            scroller.set_child(None::<&gtk::Widget>);
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .build();
            column.append(&body);
            column.append(&list);
            scroller.set_child(Some(&column));
            // What the arrows move when there is text rather than rows to move
            // through - see `reading_about`, which decides when that is.
            *self.about_scroll.borrow_mut() = Some(scroller.vadjustment());
        }

        let fill: Rc<Fill> = {
            let list = list.clone();
            let body = body.clone();
            Rc::new(move |app: &Rc<Self>| {
                // What this category says for itself, before its rows.
                while let Some(child) = body.first_child() {
                    body.remove(&child);
                }
                *app.settings_body.borrow_mut() = match app.settings_category.get() {
                    Category::About => {
                        let text = app.about_body();
                        body.append(&text);
                        Some(text)
                    }
                    _ => None,
                };
                // Where Ctrl+A and Ctrl+C look for text. Set here as well as
                // in `settings_stage`, because choosing a category refills the
                // pane without going through it - so About selected from the
                // column had a body on screen and nothing pointing at it.
                *app.copy_root.borrow_mut() =
                    app.settings_body.borrow().clone().map(|body| body.upcast());
                while let Some(row) = list.row_at_index(0) {
                    list.remove(&row);
                }
                app.settings_switches.borrow_mut().clear();
                app.settings_sliders.borrow_mut().clear();

                // Found once per build of the pane, not once per row: it walks
                // the disk looking for Kodi, and every label, value and note on
                // those rows is read back out of this. Every installation now,
                // not only the configured ones - discovering them was the
                // wizard's first screen, and a pane that lists them needs no
                // such screen.
                if app.settings_category.get() == Category::Kodi {
                    *app.kodi_setups.borrow_mut() = app.known_kodis();
                }
                // Re-read for the same reason, and it is the more important of
                // the two: the token in that file can be revoked from a
                // dashboard on the other side of the house, and this pane is
                // where somebody comes to find out that it was.
                if app.settings_category.get() == Category::Jellyfin {
                    *app.jellyfin_pairing.borrow_mut() = crate::jellyfin::load();
                }
                let panes: Vec<KodiPane> = app
                    .kodi_setups
                    .borrow()
                    .iter()
                    .map(|setup| KodiPane {
                        heading: setup.label().to_uppercase(),
                        confinement: setup.confinement,
                    })
                    .collect();
                let entries = app
                    .settings_category
                    .get()
                    .items(&panes, app.jellyfin_pane());
                *app.pane_items.borrow_mut() = entries.iter().map(|(_, item)| *item).collect();

                for (index, (_, item)) in entries.iter().enumerate() {
                    let item = *item;
                    let label = app.item_label(item);
                    let enabled = app.item_enabled(item);

                    // Three kinds of row, and which one it is belongs to the
                    // item rather than to where it sits.
                    let widget = match (item.slider(), app.item_switch(item)) {
                        (Some(kind), on) => {
                            let (now, reading) = app.slider_state(kind);
                            let (widget, bar, value, switch) =
                                slider_row(&label, slider_width, kind.range(), now, &reading, on);
                            if kind == Slider::Scale {
                                let by_hand = app.config.borrow().ui_scale.is_some();
                                bar.set_sensitive(by_hand);
                                value.set_sensitive(by_hand);
                            }
                            app.wire_slider(kind, &bar, &value);
                            if let Some(switch) = switch {
                                app.settings_switches.borrow_mut().push((item, switch));
                            }
                            app.settings_sliders
                                .borrow_mut()
                                .push((item, kind, bar, value));
                            widget
                        }
                        (None, Some(on)) => {
                            let (widget, switch) = switch_row(&label, on);
                            switch.set_sensitive(enabled);
                            app.settings_switches.borrow_mut().push((item, switch));
                            widget
                        }
                        (None, None) => menu_row(&label, &app.item_value(item), enabled),
                    };

                    // The note goes inside the row rather than under it as a
                    // row of its own, which is what keeps it out of the way of
                    // everything: it cannot be selected, cannot be arrowed on
                    // to, and does not shift the numbering the pane is read by.
                    let widget = match app.item_note(item, scale) {
                        Some(note) => {
                            let stack = gtk::Box::builder()
                                .orientation(gtk::Orientation::Vertical)
                                .build();
                            stack.append(&widget);
                            stack.append(&note);
                            stack.upcast::<gtk::Widget>()
                        }
                        None => widget.upcast::<gtk::Widget>(),
                    };

                    let name = row_name(&label, &app.item_value(item));
                    append_named(&list, &widget, &name);
                    let Some(row) = list.row_at_index(index as i32) else {
                        continue;
                    };
                    row.set_sensitive(enabled);
                    if item == Item::UpdateStatus {
                        app.watch_update_row(&row);
                    }
                }

                // Each switch reports its own presses, now that it takes them
                // rather than letting them fall through to the row. Guarded
                // against the moves made from here when the same setting is
                // worked another way.
                for (item, switch) in app.settings_switches.borrow().iter() {
                    let app = app.clone();
                    let item = *item;
                    switch.connect_state_set(move |_, _| {
                        if !app.settling_switch.get() {
                            app.sounds.borrow().click();
                            app.apply_switch_item(item);
                        }
                        glib::Propagation::Proceed
                    });
                }

                // A heading above the row that opens a group, by the same
                // mechanism the media page uses: headers are not rows, so they
                // cannot be landed on.
                let headings: Vec<Option<String>> = entries
                    .iter()
                    .map(|(heading, _)| heading.as_ref().map(|text| text.to_string()))
                    .collect();
                // What each heading says under itself, by the row it sits
                // above. Only a Kodi group has one, and it belongs to the
                // installation rather than to the row that opens it - which is
                // why it is here and not a note on Player Type, where it read
                // as an explanation of that one setting.
                let notes: Vec<Option<GroupNote>> = entries
                    .iter()
                    .map(|(heading, item)| match (heading, item) {
                        (Some(_), Item::KodiType(index)) => app.kodi_group_note(*index),
                        (Some(_), Item::JellyfinConnect | Item::JellyfinDisconnect) => {
                            Some(app.jellyfin_group_note())
                        }
                        _ => None,
                    })
                    .collect();
                list.set_header_func(move |row, _| {
                    let index = row.index();
                    match headings.get(index as usize).and_then(Option::as_deref) {
                        Some(heading) => row.set_header(Some(&group_header(
                            heading,
                            notes.get(index as usize).and_then(Option::as_ref),
                            scale,
                            index == 0,
                        ))),
                        None => row.set_header(None::<&gtk::Widget>),
                    }
                });
                app.refresh_version_row();
            })
        };
        fill(self);

        // The categories, down the left.
        let (categories_scroller, categories) = scrolling_list();
        // Which keeps its place marked after the keyboard has left it - see
        // `.tp-resting` in the stylesheet for which lists do that and why.
        categories.add_css_class("tp-resting");
        categories_scroller.set_size_request(px(CATEGORY_WIDTH), -1);
        for category in Category::ALL {
            append_named(
                &categories,
                &menu_row(&category.title(), "", true),
                &category.title(),
            );
        }
        if let Some(row) = Category::ALL
            .iter()
            .position(|category| *category == self.settings_category.get())
            .and_then(|index| categories.row_at_index(index as i32))
        {
            categories.select_row(Some(&row));
        }
        // Immediately, on the selection moving, rather than on the row being
        // activated: this is a column of what is being looked at, not a list of
        // things to do, and having to press a category to see it is a step that
        // says nothing.
        {
            let app = self.clone();
            let fill = fill.clone();
            categories.connect_row_selected(move |_, row| {
                let Some(category) = row
                    .map(|row| row.index() as usize)
                    .and_then(|index| Category::ALL.get(index).copied())
                else {
                    return;
                };
                if category == app.settings_category.get() {
                    return;
                }
                app.settings_category.set(category);
                // The remembered row belongs to the category it was in.
                *app.settings_row.borrow_mut() = 0;
                fill(&app);
            });
        }

        // Both panes on grounds of their own, the way the media page's rows
        // are: two lists side by side on a bare page have nothing to say where
        // either one ends.
        let Some(listing) = page.last_child() else {
            return;
        };
        page.remove(&listing);
        let columns = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(16.0))
            .vexpand(true)
            .build();
        for (pane, expand, ground) in [
            (
                categories_scroller.clone().upcast::<gtk::Widget>(),
                false,
                "tp-bare",
            ),
            (listing.clone(), true, "tp-menu-panel"),
        ] {
            let panel = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .hexpand(expand)
                .css_classes([ground])
                .build();
            panel.append(&pane);
            columns.append(&panel);
        }
        page.append(&columns);

        // Watched in the capture phase, so a press is known about before
        // anything else handles it. Cleared on the way out rather than on
        // release, because the row is activated in between - and a press that
        // never activates a row must not leave the next key press looking like
        // a click.
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

        *self.settings_list.borrow_mut() = Some(list.clone());

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
                let Some(item) = app.item_at(row.index()) else {
                    return;
                };
                // A switch is worked by pressing the switch, not by clicking
                // the row it sits on: the row is a wide target, and hitting it
                // on the way past should not change a setting. Enter on the
                // selected row still does, which arrives here with nothing
                // having been clicked.
                if app.clicked_row.replace(false) && item.has_switch() {
                    return;
                }
                // A switch row is answered by the switch, which plays its own
                // click when it moves. Playing one here too would double it.
                if !item.has_switch() {
                    app.sounds.borrow().click();
                }
                // Remembered so returning from a chooser lands back on the row
                // it was opened from, as the main menu does.
                *app.settings_row.borrow_mut() = row.index();
                app.activate_item(item, row);
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.show_menu());
        }

        // Enter hands the keyboard to the settings beside the category.
        {
            let app = self.clone();
            categories.connect_row_activated(move |_, _| {
                app.sounds.borrow().click();
                app.hold_settings_pane();
            });
        }

        // Both lists are wired for the arrows, and which of them the arrows
        // are actually driving is settled below by `set_nav`. Deliberately not
        // `nav_side_list`, which is how the browser puts its drives column in
        // the order beside its listing: that is what makes left and right step
        // between two lists, and left and right are spoken for here.
        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        self.wire_arrows(categories.upcast_ref());
        announce_selection(&categories);
        *self.settings_categories.borrow_mut() = Some(categories.clone());

        // Tab moves the focus without going through either handler above, so
        // each pane says so for itself when the focus arrives. Without this the
        // arrows carried on driving the pane that was left behind.
        for (widget, pane) in [(categories.clone(), false), (list.clone(), true)] {
            let app = self.clone();
            let controller = gtk::EventControllerFocus::new();
            controller.connect_enter(move |_| {
                if *app.screen.borrow() != Screen::Settings {
                    return;
                }
                if app.in_settings_pane.get() != pane {
                    app.settings_stage(pane);
                }
                if pane {
                    app.select_focused_row();
                }
            });
            widget.add_controller(controller);
        }

        *self.screen.borrow_mut() = Screen::Settings;
        self.window.set_child(Some(&page));
        // Back where it was left. Coming out of a chooser returns to the row
        // that opened it, which is in the pane; arriving fresh starts in the
        // categories.
        match self.in_settings_pane.get() {
            true => self.hold_settings_pane(),
            false => self.hold_settings_categories(),
        }
    }

    /// Whether the settings screen is the one on display.
    pub(super) fn on_settings(&self) -> bool {
        *self.screen.borrow() == Screen::Settings
    }

    /// Says which of the two panes the arrows are driving, without moving the
    /// focus itself.
    ///
    /// Split from the two below because the focus can arrive on its own: Tab
    /// steps between the panes, and the pane it lands on has to start taking
    /// the arrow keys without being asked to grab a focus it already has.
    ///
    /// Both lists stay in the tab order either way, which is what Tab moves
    /// through. That is also why left and right are kept away from
    /// `move_between_lists`, which walks the very same list of stops: it is the
    /// tab order and the left-right order at once everywhere else, and here
    /// those two need different answers.
    fn settings_stage(&self, pane: bool) {
        let (Some(list), Some(categories)) = (
            self.settings_list.borrow().clone(),
            self.settings_categories.borrow().clone(),
        ) else {
            return;
        };
        let Some(back) = self.nav_header.borrow().first().cloned() else {
            return;
        };
        self.in_settings_pane.set(pane);
        match pane {
            true => self.set_nav(Some(&list), std::slice::from_ref(&back), &[]),
            false => self.set_nav(Some(&categories), std::slice::from_ref(&back), &[]),
        }
        // After `set_nav`, which clears it: that is how a screen without
        // selectable text is sure of not leaving the last one's behind.
        *self.copy_root.borrow_mut() = self
            .settings_body
            .borrow()
            .clone()
            .map(|body| body.upcast());
        // Rewritten after `set_nav`, which builds the order from the one list
        // it was given. Tab should reach both, in the order they are read.
        *self.nav_stops.borrow_mut() = vec![back.upcast(), categories.upcast(), list.upcast()];
    }

    /// Gives the keyboard to the settings themselves.
    fn hold_settings_pane(self: &Rc<Self>) {
        let Some(list) = self.settings_list.borrow().clone() else {
            return;
        };
        // Nothing to step into: a category with no rows would take the keys
        // and answer nothing, and Escape would be the only way out.
        if list.row_at_index(0).is_none() {
            return;
        }
        self.settings_stage(true);
        let remembered = (*self.settings_row.borrow()).min(last_row_index(&list));
        if let Some(row) = list.row_at_index(remembered) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Gives it back to the column of categories.
    pub(super) fn hold_settings_categories(self: &Rc<Self>) {
        let Some(categories) = self.settings_categories.borrow().clone() else {
            return;
        };
        self.settings_stage(false);
        if let Some(row) = Category::ALL
            .iter()
            .position(|category| *category == self.settings_category.get())
            .and_then(|index| categories.row_at_index(index as i32))
        {
            categories.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Selects the row the focus has just landed in.
    ///
    /// A switch or a bar takes the focus when it is clicked, which carries it
    /// into the pane without going through the arrow keys - and the list's own
    /// arrival handler answers a list with nothing selected by selecting its
    /// first row. Clicking a switch two thirds of the way down therefore lit
    /// the row at the top. The row under the pointer is the one meant.
    fn select_focused_row(&self) {
        let Some(list) = self.settings_list.borrow().clone() else {
            return;
        };
        let Some(mut widget) = gtk::prelude::GtkWindowExt::focus(&self.window) else {
            return;
        };
        // Up from whatever took the focus to the row holding it, which may be
        // a switch inside a box inside the row.
        loop {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.parent().as_ref() == Some(list.upcast_ref::<gtk::Widget>())
            {
                list.select_row(Some(row));
                *self.settings_row.borrow_mut() = row.index();
                return;
            }
            match widget.parent() {
                Some(parent) => widget = parent,
                None => return,
            }
        }
    }

    /// Which setting a row in the right-hand pane is.
    pub(super) fn item_at(&self, index: i32) -> Option<Item> {
        self.pane_items.borrow().get(index as usize).copied()
    }

    /// Takes the mark off the settings button once the version row is reached.
    ///
    /// Arriving on it is the moment somebody has been told, and pressing it
    /// should not be required to stop being nagged about something already
    /// seen. Attached whether or not there is anything new, since a check
    /// finishing while this screen is open can make there be.
    fn watch_update_row(self: &Rc<Self>, row: &gtk::ListBoxRow) {
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

    /// What a row does when it is chosen.
    fn activate_item(self: &Rc<Self>, item: Item, row: &gtk::ListBoxRow) {
        if let Some(setting) = item.setting() {
            self.show_selector(setting, row);
            return;
        }
        if item.has_switch() {
            self.work_switch_item(item);
            return;
        }
        match item {
            Item::ClearData => self.confirm_clear_data(),
            // Home, every time. Kodi's userdata lives under it on every
            // platform, and where the video browser was last says nothing
            // about where Kodi keeps its settings.
            Item::KodiAdd => self.show_kodi_folder(&crate::browser::home()),
            Item::KodiPermission(index) => self.show_kodi_permission(index),
            Item::JellyfinConnect => self.start_jellyfin_connect(ConnectFrom::Settings),
            Item::JellyfinDisconnect => self.confirm_jellyfin_disconnect(),
            Item::Notices => self.show_notices(),
            Item::UpdateStatus => self.open_release_page(),
            _ => {}
        }
    }

    /// Wires a bar to the setting it moves.
    fn wire_slider(self: &Rc<Self>, kind: Slider, bar: &gtk::Scale, value: &gtk::Label) {
        {
            let app = self.clone();
            let value = value.clone();
            bar.connect_change_value(move |_, scroll, moved| {
                app.set_slider(kind, moved, &value);
                if kind == Slider::Scale {
                    // A drag reports Jump, over and over, while the pointer
                    // holds the bar. Anything else - a step, a page, a scroll
                    // wheel - is finished by the time it arrives and can be
                    // drawn straight away.
                    if scroll == gtk::ScrollType::Jump {
                        app.wanted_scale.set(Some(moved));
                    } else {
                        app.apply_scale(moved);
                    }
                }
                glib::Propagation::Proceed
            });
        }
        // Let go of, and only then redrawn. Watched rather than handled, so the
        // bar keeps its own grip on the pointer while it is being dragged.
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
            bar.add_controller(watcher);
        }
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
                Item::Description(Role::Primary)
            } else {
                Item::Description(Role::Secondary)
            },
            on,
        );
    }

    /// Moves the switch on a settings row to match what it now reports.
    pub(super) fn set_settings_switch(&self, item: Item, on: bool) {
        self.settling_switch.set(true);
        if let Some((_, switch)) = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == item)
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
    fn work_switch_item(self: &Rc<Self>, item: Item) {
        let switch = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == item)
            .map(|(_, switch)| switch.clone());
        match switch {
            // Its own handler carries on from here, as it does for a click.
            Some(switch) => {
                switch.activate();
            }
            None => self.apply_switch_item(item),
        }
    }

    /// What a switch row actually changes, once something has asked for it.
    fn apply_switch_item(self: &Rc<Self>, item: Item) {
        match item {
            Item::InterfaceScale => self.toggle_automatic_scale(),
            Item::Sounds => self.toggle_sounds(),
            Item::StartFullscreen => self.toggle_start_fullscreen(),
            Item::ReadMetadata => self.toggle_read_metadata(),
            Item::ShowBackdrop => self.toggle_show_backdrop(),
            Item::Description(role) => self.toggle_audio_description(role == Role::Primary),
            Item::Volume(_) => self.toggle_settings_mute(item),
            Item::Sync(_) => self.toggle_settings_offset(item),
            Item::Updates => self.toggle_update_checks(),
            _ => {}
        }
    }

    /// Turns "open fullscreen" on or off.
    ///
    /// Only this changes it. Pressing F11 or the fullscreen mark is about the
    /// session in hand and leaves this alone - see [`App::toggle_fullscreen`].
    fn toggle_start_fullscreen(self: &Rc<Self>) {
        let mut config = self.config.borrow_mut();
        config.fullscreen = !config.fullscreen;
        let _ = config.save();
    }

    /// Turns the reading of sidecars and artwork beside a video on or off.
    ///
    /// The page is rebuilt afterwards, since what it can show has changed -
    /// and the backdrop row with it, which is only workable while this is on.
    fn toggle_read_metadata(self: &Rc<Self>) {
        {
            let mut config = self.config.borrow_mut();
            config.read_metadata = !config.read_metadata;
            let _ = config.save();
        }
        self.reread_details();
        // The one row this governs, redrawn where it stands.
        //
        // Rebuilding the whole screen was what this did, and it moved the
        // cursor every time: a switch is worked without activating its row, so
        // the remembered row is whatever was last activated, and coming back
        // in lands on that instead of on the switch just pressed.
        self.refresh_backdrop_row();
    }

    /// Turns the backdrop row on or off to match whether there is anything
    /// to draw, without disturbing the screen around it.
    fn refresh_backdrop_row(&self) {
        let enabled = self.config.borrow().read_metadata;
        if let Some((_, switch)) = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(item, _)| *item == Item::ShowBackdrop)
        {
            switch.set_sensitive(enabled);
        }
        let Some(index) = self
            .pane_items
            .borrow()
            .iter()
            .position(|item| *item == Item::ShowBackdrop)
        else {
            return;
        };
        let list = self.settings_list.borrow().clone();
        if let Some(row) = list.and_then(|list| list.row_at_index(index as i32)) {
            row.set_sensitive(enabled);
        }
    }

    /// Turns the film's fanart behind the media page on or off.
    fn toggle_show_backdrop(self: &Rc<Self>) {
        {
            let mut config = self.config.borrow_mut();
            config.show_backdrop = !config.show_backdrop;
            let _ = config.save();
        }
        self.reread_details();
    }

    /// Reads what is beside the file again, after a setting changed what may
    /// be read at all.
    ///
    /// Nothing to do without a file: the answer is about a video, and the
    /// next one loaded will be read under whatever the setting now says.
    fn reread_details(self: &Rc<Self>) {
        let Some(source) = self.file.borrow().clone() else {
            return;
        };
        let beside = {
            let config = self.config.borrow();
            crate::metadata::Beside {
                metadata: config.read_metadata,
                backdrop: config.show_backdrop,
            }
        };
        let media = crate::probe::Media {
            audio: Vec::new(),
            subtitles: Vec::new(),
            duration_ns: 0,
            video: self.details.borrow().video.clone(),
            tags: Default::default(),
        };
        let mut details = crate::metadata::resolve(&source, &media, beside, &self.launcher_title());
        // The parts that came from the container rather than from beside the
        // file are already known and are not re-probed for a toggle.
        let held = self.details.borrow();
        details.duration_s = held.duration_s;
        details.container = held.container.clone();
        drop(held);
        *self.details.borrow_mut() = details;
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        *self.series_art.borrow_mut() = None;
        self.art_generation.set(self.art_generation.get() + 1);
        self.start_art_load();
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
        self.set_settings_switch(Item::Updates, on);
        self.refresh_version_row();
    }

    /// The version this is, on the left of its row.
    fn version_label(&self) -> String {
        tr!(
            "Current Version: v{version}",
            version = env!("CARGO_PKG_VERSION")
        )
        .into_owned()
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
            Some((version, _)) => tr!(
                "Update available: v{version}",
                version = version.trim_start_matches(['v', 'V'])
            )
            .into_owned(),
            None => tr!("Up to date").into_owned(),
        }
    }

    /// Redraws the row naming the version, in place.
    ///
    /// In place rather than by rebuilding the screen: turning the check on or
    /// off changes two words, and rebuilding for it threw the whole page away
    /// and drew it again - which flickers and moves every row under whatever
    /// was pointing at one.
    fn refresh_version_row(&self) {
        // Found by asking which row is the version one, rather than by a fixed
        // number: it is only in the pane at all when General is the category
        // being shown.
        let Some(index) = self
            .pane_items
            .borrow()
            .iter()
            .position(|item| *item == Item::UpdateStatus)
        else {
            return;
        };
        let list = self.settings_list.borrow().clone();
        let Some(row) = list.and_then(|list| list.row_at_index(index as i32)) else {
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
    pub(super) fn check_for_updates(self: &Rc<Self>, now: bool) {
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
    pub(super) fn draw_update_badge(&self) {
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
        self.set_settings_switch(Item::Sounds, enabled);
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
            .find(|(row, ..)| *row == Item::InterfaceScale)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        if let Some((kind, scale, value)) = found {
            let (now, reading) = self.slider_state(kind);
            scale.set_value(now);
            value.set_text(&reading);
            scale.set_sensitive(!now_automatic);
            value.set_sensitive(!now_automatic);
        }
        self.set_settings_switch(Item::InterfaceScale, now_automatic);
    }

    /// Redraws the interface at the size the bar is now at.
    pub(super) fn apply_scale(self: &Rc<Self>, steps: f64) {
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
    pub(super) fn follow_automatic_scale(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
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
    fn restyle(self: &Rc<Self>, scale: f64) {
        self.scale.set(scale);
        self.styles.load_from_data(&style_css(scale));

        // The stylesheet is only half of a size. Everything drawn rather than
        // styled takes its size in Rust at the moment the page is built - the
        // poster, the marks on the buttons, every margin, the width the page
        // is held to - and none of that moves when the stylesheet is
        // reloaded. Restyling alone therefore left the two halves disagreeing:
        // type at the new size inside a page laid out for the old one.
        //
        // It shows worst where the change is largest. A 4K television picks
        // 2x, so a page built at 1x and restyled kept a half-size poster and
        // half-size margins under full-size text, and the whole composition
        // sat in the top of the screen with the bottom third empty.
        //
        // Rebuilding is cheap here and this happens on a monitor change or a
        // fullscreen toggle, not on a drag.
        if *self.screen.borrow() == Screen::Menu {
            let app = self.clone();
            glib::idle_add_local_once(move || {
                if *app.screen.borrow() == Screen::Menu {
                    app.show_menu();
                }
            });
        }
    }
}
