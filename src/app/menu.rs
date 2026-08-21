//! The top-level page, and the switch between having a video and not.

use super::*;

impl App {
    /// Builds the screen the application sits on, without installing it.
    ///
    /// Two shapes behind one entry point, because everything that shows the
    /// menu wants whichever is right rather than having to ask first. With no
    /// video there is nothing to configure and nothing to play, so the page is
    /// an invitation to choose one. With a video it is a page about that
    /// video, and the choices sit under what they are choices about.
    ///
    /// Split out so the browser can raise the same page behind itself as a
    /// backdrop, which is what makes it read as a window opening over the
    /// menu rather than as another screen replacing it.
    fn build_menu_page(self: &Rc<Self>) -> (gtk::Widget, Option<gtk::ListBox>) {
        // What a resize compares against to decide whether rebuilding this
        // page would change anything - recorded here, for every menu page,
        // rather than where the poster is built.
        //
        // Only the media page has a poster, so recording it there left the
        // empty page's figure at whatever it happened to be, which never
        // matched and so always answered "yes, rebuild". The page was then
        // rebuilt every quarter second for as long as it was on screen, and
        // the surface layout that followed each rebuild scheduled the next.
        //
        // It was close to invisible, because the page it kept rebuilding looks
        // the same each time - but the pointer's idea of what is under it does
        // not survive the widget being destroyed and made again, so hovering a
        // button only lit it while the mouse was moving, and a click only
        // landed if it happened to arrive between two rebuilds.
        self.built_poster.set(self.poster_height(self.scale.get()));
        if self.file.borrow().is_none() {
            return (self.build_empty_page().upcast(), None);
        }
        let (page, list) = self.build_media_page();
        (page.upcast(), Some(list))
    }

    /// The page about the video that is loaded: what it is, above how it is
    /// about to be played.
    fn build_media_page(self: &Rc<Self>) -> (gtk::Overlay, gtk::ListBox) {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;

        // Everything sits in one column, held to 16:9 by `hold_safe_area` so
        // that a wide window widens the artwork behind rather than the text on
        // top. A plot line three thousand pixels across is not a page anyone
        // reads, and a row whose value drifts that far from its label stops
        // reading as one row.
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(16.0))
            .margin_top(px(30.0))
            // Matched to the sides rather than to the top. The panel now runs
            // to the bottom of the page, so this margin is a visible edge
            // along it, and at 26 it read as a thinner border than the 34 down
            // either side.
            .margin_bottom(px(34.0))
            .margin_start(px(34.0))
            .margin_end(px(34.0))
            // Filled, not centered. The centering is `Column`'s job, and a
            // box that also centers itself shrinks to its natural width
            // inside the column it was just given - which is what truncated
            // every row value on a file with a short plot.
            .css_classes(["tp-media"])
            .build();

        // The poster keeps to the left for the height of the page; everything
        // else runs down the column beside it.
        let columns = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(32.0))
            .vexpand(true)
            .build();
        columns.append(&self.poster_column(scale));

        let main = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(6.0))
            .hexpand(true)
            .build();
        columns.append(&main);
        content.append(&columns);

        let (scroller, list) = scrolling_list();
        name_it(&list, &tr!("Playback Options"));

        // The film's details sit still. Only the rows scroll, so the poster,
        // the title and the buttons stay where they are however long the list
        // gets - and the list scrolls under them rather than the page moving
        // as a whole.
        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(6.0))
            .valign(gtk::Align::Start)
            .build();
        for widget in self.heading_block(scale) {
            info.append(&widget);
        }
        main.append(&info);

        let file = self.file.borrow().clone();
        let config = self.config.borrow();
        let tracks = self.tracks.borrow();

        // Asked before the rows are built, not after: this is what fetches
        // Kodi's title as well as its resume point, and a row built ahead of
        // it would show the file name until something rebuilt the screen.
        let resume_at = self.resume_position();

        let has_file = file.is_some();
        let has_secondary = config.secondary_sink.is_some();

        // The rows, and the group each one opens - `None` for a row that
        // continues the group above it.
        //
        // Kept as a second list rather than a fifth element on the tuple so
        // that `alignment_row` can go on returning a row and nothing else. The
        // two are pushed together every time, which is what keeps them in step.
        let mut rows: Vec<(String, String, bool, MenuAction)> = Vec::new();
        // Owned rather than borrowed, since a translated heading is built
        // at the moment it is asked for and outlives nothing.
        let mut groups: Vec<Option<String>> = Vec::new();
        let mut push = |group: Option<String>, row: Option<(String, String, bool, MenuAction)>| {
            if let Some(row) = row {
                groups.push(group);
                rows.push(row);
            }
        };

        // Which output, said once at the top of the group, rather than on the
        // front of all three rows under it. "First Output" and "Second Output"
        // rather than primary and secondary: the ordinal is the whole of what
        // distinguishes them to anyone watching, and Primary/Secondary is the
        // vocabulary of the code and the config file.
        push(
            Some(tr!("FIRST OUTPUT").into_owned()),
            Some((
                tr!("Output Device").into_owned(),
                config
                    .primary_sink
                    .clone()
                    .unwrap_or_else(|| trc!("audio output device", "Not set").into_owned()),
                true,
                MenuAction::Device(Role::Primary),
            )),
        );
        push(
            None,
            Some((
                tr!("Audio Track").into_owned(),
                if has_file {
                    self.describe_audio(Role::Primary)
                } else {
                    "—".to_string()
                },
                has_file,
                MenuAction::Track(Role::Primary),
            )),
        );
        push(None, self.alignment_row(Role::Primary));

        push(
            Some(tr!("SECOND OUTPUT").into_owned()),
            Some((
                tr!("Output Device").into_owned(),
                config
                    .secondary_sink
                    .clone()
                    .unwrap_or_else(|| trc!("audio output device", "None").into_owned()),
                true,
                MenuAction::Device(Role::Secondary),
            )),
        );
        push(
            None,
            Some((
                tr!("Audio Track").into_owned(),
                if has_file && has_secondary {
                    self.describe_audio(Role::Secondary)
                } else {
                    "—".to_string()
                },
                has_file && has_secondary,
                MenuAction::Track(Role::Secondary),
            )),
        );
        if has_secondary {
            push(None, self.alignment_row(Role::Secondary));
        }

        // Its own group rather than sitting with the audio pair: the subtitle
        // language is an independent choice, and may be a third language again
        // or a repeat of either soundtrack.
        push(
            Some(tr!("SUBTITLES").into_owned()),
            Some((
                tr!("Language").into_owned(),
                self.describe_subtitle(),
                has_file,
                MenuAction::Subtitles,
            )),
        );

        let can_play = has_file && config.primary_sink.is_some();
        drop(tracks);
        drop(config);

        // What each row is called to anyone who cannot see the list. The group
        // heading is read once at the top of a group and does not survive into
        // a row announced on its own, so the name carries it: "Audio Track" is
        // two rows on this page and "First output, Audio Track" is one.
        //
        // Worked out here, where both lists are still in hand, and in title
        // case rather than the heading's capitals - a screen reader given
        // "FIRST OUTPUT" may spell it.
        let mut heading = String::new();
        let names: Vec<String> = rows
            .iter()
            .zip(&groups)
            .map(|((label, value, _, _), group)| {
                if let Some(group) = group {
                    heading = title_case(group);
                }
                row_name(&format!("{heading}, {label}"), value)
            })
            .collect();

        for ((label, value, enabled, _), name) in rows.iter().zip(&names) {
            append_named(&list, &menu_row(label, value, *enabled), name);
        }

        // A heading above the row that opens a group, and nothing above the
        // rest. Headings are not rows: they sit outside the selection model
        // and outside the focus chain, so they are unselectable and skipped by
        // the arrow keys without anything having to arrange it.
        //
        // That is also why the indent under them is gone. It said "this
        // belongs to the output above"; the heading says it for all three rows
        // at once, and says which output.
        //
        // It has to be done through this function rather than by setting the
        // header on each row directly, which is the obvious way and does
        // nothing: `set_header` only stores the widget on the row, and the
        // list parents and draws it from inside its header function - which
        // returns immediately when none is set. The headings were built, held
        // and never mounted.
        list.set_header_func(move |row, _before| {
            let index = row.index();
            match groups
                .get(index as usize)
                .and_then(|group| group.as_deref())
            {
                Some(group) => row.set_header(Some(&group_heading(group, scale, index == 0))),
                None => row.set_header(None::<&gtk::Widget>),
            }
        });

        let resumable = resume_at.is_some();

        // Between the film and the choices rather than under both. Playing is
        // what the page is for, so it sits where the eye arrives after
        // reading what the film is - and the rows below become what they
        // actually are, the settings you may want to change first rather than
        // a list to get past. Generous room above and below, so it reads as a
        // division of the page rather than as another row.
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .margin_top(px(34.0))
            .margin_bottom(px(34.0))
            .build();
        // Everything in this row packs to the left, over the rows it acts on:
        // playing, starting over, and then the two marks. Nothing expands, so
        // there is no gap pushing the marks to the far end - they read as the
        // rest of one row of controls rather than as a separate corner.
        let plays = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::Start)
            .build();
        let mut play_buttons: Vec<gtk::Button> = Vec::new();

        // Resuming is the common case for a part-watched film, so it takes
        // the first position and the focus. Starting over is deliberate
        // enough to be worth its own button rather than a hidden modifier -
        // but not enough to be worth a word beside it, so once there are two
        // the second keeps only its mark. It is the same button either way;
        // what changes is how much room it argues for.
        let play = gtk::Button::new();
        play.set_child(Some(&marked_face(
            play_image(scale),
            &match resume_at {
                Some(position) => format!(
                    "  {}",
                    tr!(
                        "Resume ({position})",
                        position = crate::controls::format_time(
                            gstreamer::ClockTime::from_nseconds(position)
                        )
                    )
                ),
                None => format!("  {}", tr!("Play")),
            },
        )));
        // The face is two labels, so the button has no text of its own for a
        // screen reader to read off. Named outright instead.
        name_it(
            &play,
            &match resume_at {
                Some(position) => tr!(
                    "Resume at {position}",
                    position =
                        crate::controls::format_time(gstreamer::ClockTime::from_nseconds(position))
                )
                .into_owned(),
                None => tr!("Play").into_owned(),
            },
        );
        play.add_css_class("tp-button");
        play.add_css_class("tp-action");
        play.add_css_class("tp-tall");
        play.set_sensitive(can_play);
        plays.append(&play);
        play_buttons.push(play);

        if resume_at.is_some() {
            let restart = gtk::Button::new();
            restart.set_child(Some(&marked_face(restart_image(scale), "")));
            restart.add_css_class("tp-button");
            restart.add_css_class("tp-action");
            restart.add_css_class("tp-action-icon");
            restart.add_css_class("tp-tall");
            restart.set_sensitive(can_play);
            // The word is gone from the face, so it has to be somewhere: a
            // tooltip for a pointer, and a name for a screen reader, which
            // would otherwise announce the glyph or nothing at all.
            restart.set_tooltip_text(Some(tr!("Start from the beginning").as_ref()));
            name_it(&restart, &tr!("Restart"));
            plays.append(&restart);
            play_buttons.push(restart);
        }
        buttons.append(&plays);

        let (fullscreen, gear) = self.corner_buttons();
        let open = self.browse_button();
        // Square, and as tall as the play button beside them. The marks are
        // built the same way on the empty page, where there is no tall button
        // to match, so this is asked for here rather than where they are made.
        for mark in [Some(&open), Some(&gear), fullscreen.as_ref()]
            .into_iter()
            .flatten()
        {
            mark.add_css_class("tp-tall");
        }
        // A little clear air between the pair that plays the film and the
        // marks that do not, so the row reads as two groups rather than a run
        // of equal buttons.
        open.set_margin_start(px(16.0));
        // Left out under a launcher: something else chose the film and is
        // waiting for this playback of it, so there is nothing to choose here.
        if !self.external {
            buttons.append(&open);
        }
        buttons.append(&gear);
        if let Some(fullscreen) = fullscreen.as_ref() {
            buttons.append(fullscreen);
        }
        let close = self.close_button();
        close.add_css_class("tp-tall");
        buttons.append(&close);

        // The page in order: what the film is, what to do about it, and then
        // the choices - which are the only part that scrolls.
        main.append(&buttons);
        // The rows sit in a panel of their own rather than loose on the page.
        // It runs to the bottom because the scroller inside it expands, which
        // is also what turns the space left below the last row into part of
        // the panel instead of a band of nothing.
        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .css_classes(["tp-menu-panel"])
            .build();
        panel.append(&scroller);
        main.append(&panel);

        // A header now rather than a footer, because that is where they sit:
        // Up from the first row reaches them, and Down from them returns.
        // Ordered as they appear, so left and right walk along the row.
        let mut header = play_buttons.clone();
        if !self.external {
            header.push(open);
        }
        header.push(gear);
        header.extend(fullscreen);
        header.push(close);

        {
            let app = self.clone();
            let actions: Vec<MenuAction> = rows.iter().map(|(_, _, _, action)| *action).collect();
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
                match actions.get(row.index() as usize) {
                    Some(MenuAction::Device(Role::Primary)) => {
                        app.show_selector(Setting::PrimaryDevice, row)
                    }
                    Some(MenuAction::Track(Role::Primary)) => {
                        app.show_selector(Setting::PrimaryTrack, row)
                    }
                    Some(MenuAction::Device(Role::Secondary)) => {
                        app.show_selector(Setting::SecondaryDevice, row)
                    }
                    Some(MenuAction::Track(Role::Secondary)) => {
                        app.show_selector(Setting::SecondaryTrack, row)
                    }
                    Some(MenuAction::Align(role)) => app.show_align(*role),
                    Some(MenuAction::Subtitles) => app.show_selector(Setting::Subtitles, row),
                    None => {}
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

        self.wire_navigation(&list, &header, &[]);
        // Up from the top row lands on Play rather than on the far end of the
        // row, which is Settings. Playing is what the page is for, and it is
        // also what someone arrowing upwards off the list is reaching for.
        *self.nav_header_entry.borrow_mut() = header.first().cloned();
        (self.behind_artwork(&content), list)
    }

    /// Puts a page in front of the backdrop, and holds it to its column.
    ///
    /// Both screens go through here, so a page with no artwork still gets the
    /// same ground and the same width as one with it - which is what keeps the
    /// two from being two designs.
    pub(super) fn behind_artwork(self: &Rc<Self>, content: &gtk::Box) -> gtk::Overlay {
        let backdrop = crate::artwork::Artwork::backdrop();
        let texture = self.backdrop_art.borrow().clone();
        let arrived = texture.is_some() && self.fade_art.get();
        backdrop.set_texture(texture);
        if arrived {
            fade_in(&backdrop);
        }
        *self.backdrop_widget.borrow_mut() = Some(backdrop.clone());

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&backdrop));
        // The backdrop fills the window; the page inside stops widening once
        // lines get too long to read. See src/column.rs for why that is a
        // widget rather than something set on this box.
        let most = (PAGE_MAX_UNITS * self.scale.get()).round() as i32;
        overlay.add_overlay(&crate::column::Column::around(content, most));
        overlay
    }

    pub(super) fn show_menu(self: &Rc<Self>) {
        let (page, list) = self.build_menu_page();

        *self.screen.borrow_mut() = Screen::Menu;
        self.window.set_child(Some(&page));

        // The empty page has no rows to land on: its two buttons are the
        // whole of it, and `build_empty_page` has already focused one.
        let Some(list) = list else { return };
        // Selected as well as focused: focus alone doesn't mark a row
        // selected, which left the list opening with nothing highlighted
        // until the first arrow key.
        let remembered = (*self.menu_row.borrow()).min(last_row_index(&list));
        if let Some(row) = list.row_at_index(remembered) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }
}
