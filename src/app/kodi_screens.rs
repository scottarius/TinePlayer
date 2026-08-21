//! The screens for pointing TinePlayer at a Kodi installation and writing its launcher.

use super::*;

impl App {
    /// Puts the settings screen back with the Kodi category showing.
    ///
    /// Where a confirmation, an error, or the folder browser comes back to.
    /// Rebuilding is what re-reads every Kodi from disk, so the rows state
    /// what is in the files rather than what was asked for.
    pub(super) fn return_to_kodi_settings(self: &Rc<Self>) {
        self.settings_category.set(Category::Kodi);
        self.in_settings_pane.set(true);
        self.show_settings();
    }

    /// A fresh reading of one installation, taken at the moment it is about to
    /// be written to rather than out of the list the pane was built from -
    /// which may be minutes old and describes a file that anything else on the
    /// machine is free to have changed since.
    pub(super) fn kodi_at(&self, index: usize) -> Option<crate::kodi_setup::Setup> {
        let userdata = self.with_kodi_setup(index, |setup| setup.userdata().to_path_buf())?;
        Some(crate::kodi_setup::setup_at(userdata))
    }

    /// Setting the Player Type row, which is the only row that registers
    /// TinePlayer with Kodi or takes it back out.
    ///
    /// Two of the three answers are asked about first, and for opposite
    /// reasons. **Removal** is asked about because it undoes something. **The
    /// first setting of any file** is asked about because until it happens the
    /// file is entirely somebody else's: they may have players and comments in
    /// there that we are about to edit around, and being told which file is
    /// being changed and what is being kept is the least this can do. After
    /// that, changing Optional to Default is editing our own entry, and asking
    /// again would be asking permission to change a setting on the settings
    /// screen.
    ///
    /// This is what the five-screen wizard came down to. Everything it
    /// collected - which Kodi, what type, what handover, whether to back up -
    /// is a row on this pane or a rule with an obvious answer.
    pub(super) fn choose_kodi_type(self: &Rc<Self>, index: usize, choice: Option<usize>) -> bool {
        use crate::kodi_setup::Registration;

        let (Some(chosen), Some(setup)) = (choice, self.kodi_at(index)) else {
            return false;
        };
        let Some(want) = Registration::ALL.get(chosen).copied() else {
            return false;
        };
        // Nothing asked for, so nothing done, and above all nothing asked.
        if want == setup.state {
            return false;
        }

        // Kept whatever is being set, so that changing type does not silently
        // change what Kodi does when it hands a video over.
        let play = setup.play;
        let label = setup.label();

        if want == Registration::Absent {
            let app = self.clone();
            self.confirm_kodi(
                &tr!("Remove Configuration?"),
                &[&tr!(
                    "TinePlayer will be removed as an external player from {label}.",
                    label = label
                )],
                Confirm {
                    label: "Remove",
                    destructive: true,
                },
                move || {
                    let Some(setup) = app.kodi_at(index) else {
                        return app.return_to_kodi_settings();
                    };
                    let userdata = setup.userdata().to_path_buf();
                    if app.write_kodi(&setup, Registration::Absent, None, play) {
                        // Before the pane is drawn again, or it would be drawn
                        // from a list this is about to shorten. A folder named
                        // by hand is only worth remembering while something is
                        // set up in it.
                        app.forget_kodi_path(&userdata);
                        app.return_to_kodi_settings();
                    }
                },
            );
            return true;
        }

        if setup.is_configured() {
            // Our own entry, rewritten. Nothing here is anybody else's.
            if self.write_kodi(&setup, want, None, play) {
                self.return_to_kodi_settings();
            }
            // Answered either way: the pane has been put back, or an error
            // panel is up over it and must not be drawn over.
            return true;
        }

        // The first time TinePlayer touches this file. The backup is settled
        // here rather than at write time so the name cannot drift: computing
        // it twice would give two names a second apart.
        let backup = setup
            .backup_by_default()
            .then(|| crate::kodi_setup::backup_path(&setup.file));

        let app = self.clone();
        self.confirm_kodi(
            &tr!("Configure {label}?", label = label),
            &[&tr!(
                "Are you sure you want to edit this installation's playercorefactory.xml file?"
            )],
            Confirm {
                label: "Configure",
                destructive: false,
            },
            move || {
                let Some(setup) = app.kodi_at(index) else {
                    return app.return_to_kodi_settings();
                };
                if app.write_kodi(&setup, want, backup.as_deref(), play) {
                    app.return_to_kodi_settings();
                }
            },
        );
        true
    }

    /// The one place anything is written to a Kodi, and the one place a
    /// failure to is reported. Answers whether it was written.
    ///
    /// Deliberately does not put the pane back itself. Two callers have
    /// something to do between the write and the rebuild - removal has a
    /// remembered folder to forget - and a rebuild in here would draw the pane
    /// from state that was about to change. On a failure the error panel is up
    /// and the answer is false, which is what stops a caller drawing the pane
    /// over the top of it.
    #[must_use]
    pub(super) fn write_kodi(
        self: &Rc<Self>,
        setup: &crate::kodi_setup::Setup,
        want: crate::kodi_setup::Registration,
        backup: Option<&std::path::Path>,
        play: bool,
    ) -> bool {
        match crate::kodi_setup::apply(setup, want, backup, play) {
            Ok(()) => true,
            Err(e) => {
                self.show_kodi_error(&e, {
                    let app = self.clone();
                    move || app.return_to_kodi_settings()
                });
                false
            }
        }
    }

    /// Takes a folder somebody browsed to, once it has been checked.
    ///
    /// A folder that does not look like Kodi's user data is refused rather
    /// than taken, because writing to the wrong one fails silently: the rows
    /// would read as configured, Kodi would carry on playing videos itself,
    /// and nothing anywhere would say why.
    ///
    /// Dismissing goes back to the browser at the folder that was refused,
    /// which is what "choose another folder" needs to be able to mean.
    fn take_kodi_folder(self: &Rc<Self>, chosen: std::path::PathBuf) {
        let userdata = crate::kodi_setup::userdata_from(chosen);
        if crate::kodi_setup::looks_like_userdata(&userdata) {
            return self.remember_kodi_path(userdata);
        }

        let app = self.clone();
        let refused = userdata.clone();
        self.kodi_notice(
            &tr!("This does not look like Kodi's user data folder"),
            &[
                &userdata.display().to_string(),
                &tr!("A user data folder usually holds guisettings.xml and a Database folder."),
                &tr!("Please choose another folder."),
            ],
            move || app.show_kodi_folder(&refused),
        );
    }

    /// Keeps track of a folder somebody named by hand, so it heads a group of
    /// its own on the pane like anything found by itself.
    ///
    /// Written down as soon as it is named rather than once something has been
    /// set up in it, which is when the wizard used to do it. The pane is built
    /// from the installations known, so a folder that is not written down is a
    /// folder that vanishes on the way back from the browser - and there would
    /// be nothing to set up in.
    ///
    /// One TinePlayer already finds by itself is not written down, since it
    /// would be found twice and listed once anyway.
    fn remember_kodi_path(self: &Rc<Self>, userdata: std::path::PathBuf) {
        let found = crate::kodi_setup::find_all(&[])
            .iter()
            .any(|setup| setup.userdata() == userdata);
        if !found {
            let mut config = self.config.borrow_mut();
            if !config.kodi_paths.contains(&userdata) {
                config.kodi_paths.push(userdata);
                let _ = config.save();
            }
        }
        self.return_to_kodi_settings();
    }

    /// Stops keeping track of a folder somebody named by hand, once nothing is
    /// set up in it. One that TinePlayer finds by itself is not forgotten,
    /// because it was never remembered: it will be found again next time.
    fn forget_kodi_path(self: &Rc<Self>, userdata: &std::path::Path) {
        let mut config = self.config.borrow_mut();
        if config.kodi_paths.iter().any(|path| path == userdata) {
            config.kodi_paths.retain(|path| path != userdata);
            let _ = config.save();
        }
    }

    /// Every Kodi on this machine, including any folder named by hand that we
    /// are still keeping track of.
    pub(super) fn known_kodis(&self) -> Vec<crate::kodi_setup::Setup> {
        let extra = self.config.borrow().kodi_paths.clone();
        crate::kodi_setup::find_all(&extra)
    }

    /// The places column that sits to the left of a browser's listing.
    ///
    /// Home, the drives or filesystem, and whatever is mounted - all at once
    /// rather than on a separate screen reached by stepping off the top of
    /// the tree. Moving between the two lists is left and right, which the
    /// keyboard and the gamepad both do by ordinary directional focus.
    ///
    /// `folders` says which browser a drive reopens, the same way the
    /// breadcrumbs do.
    fn places_column(
        self: &Rc<Self>,
        current: &std::path::Path,
        folders: bool,
    ) -> Option<(gtk::ScrolledWindow, gtk::ListBox)> {
        let roots = crate::browser::places();
        if roots.is_empty() {
            return None;
        }

        let list = gtk::ListBox::new();
        list.add_css_class("tp-menu");
        list.set_selection_mode(gtk::SelectionMode::Browse);
        list.set_activate_on_single_click(true);

        // Which place the listing is inside, so the column says where you are
        // as well as where you could go. The longest match wins: a volume
        // under /mnt is a better answer than the filesystem root that also
        // contains it.
        let here = crate::browser::rooted(current);
        let mut selected: Option<(i32, usize)> = None;
        for (index, entry) in roots.iter().enumerate() {
            append_named(&list, &chooser_row(&entry.label), &entry.label);
            if here.starts_with(&entry.path) {
                let depth = entry.path.components().count();
                if selected.is_none_or(|(_, best)| depth > best) {
                    selected = Some((index as i32, depth));
                }
            }
        }
        let selected = selected.map(|(index, _)| index);
        if let Some(row) = selected.and_then(|index| list.row_at_index(index)) {
            // Marked as the one in force. No selection to go with it: the
            // screen opens with the cursor in the listing, and a column that
            // is not being driven should not be showing a cursor.
            row.add_css_class("tp-current");
        }

        {
            let app = self.clone();
            let paths: Vec<std::path::PathBuf> = roots.iter().map(|e| e.path.clone()).collect();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                if let Some(path) = paths.get(row.index() as usize) {
                    if folders {
                        app.show_kodi_folder(path);
                    } else {
                        app.show_browser(path, None);
                    }
                }
            });
        }
        self.follow_focus(&list);

        // The column keeps no cursor between visits. Where you are is already
        // marked, so a selection left behind after the keyboard moved into the
        // listing said nothing beyond "this list has been visited" - and said
        // it in the theme's own selection color, which nothing else on the
        // screen is drawn in.
        //
        // Coming back lands on the place in force rather than resuming
        // wherever the cursor was left, because this column is a statement of
        // where the listing is, not a position of its own to hold.
        {
            let controller = gtk::EventControllerFocus::new();
            // Weak both ways: the controller is added to the list it watches.
            let entering = list.downgrade();
            controller.connect_enter(move |_| {
                let Some(list) = entering.upgrade() else {
                    return;
                };
                // The first row when nothing here contains the listing, so the
                // arrows always have somewhere to start from.
                let landing = selected
                    .and_then(|index| list.row_at_index(index))
                    .or_else(|| list.row_at_index(0));
                list.select_row(landing.as_ref());
            });
            let leaving = list.downgrade();
            controller.connect_leave(move |_| {
                if let Some(list) = leaving.upgrade() {
                    list.select_row(None::<&gtk::ListBoxRow>);
                }
            });
            list.add_controller(controller);
        }

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .width_request((220.0 * self.scale.get()).round() as i32)
            .child(&list)
            .build();
        scroller.set_focusable(false);
        list.set_focusable(true);
        Some((scroller, list))
    }

    /// Sends this widget's up and down keys through `move_selection`, which
    /// knows where the focus is and what each boundary should do.
    ///
    /// Needed on anything that can hold focus beside a list, now that rows
    /// cannot: GtkListBox moves its cursor by moving focus between rows, and
    /// with nothing able to take it that does nothing at all. Capture phase,
    /// so this runs before the list's own bindings swallow the key.
    pub(super) fn wire_arrows(self: &Rc<Self>, widget: &gtk::Widget) {
        let app = self.clone();
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        controller.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Up => {
                app.move_selection(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                app.move_selection(1);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        widget.add_controller(controller);
    }

    /// Puts a widget into the tab order at the end.
    pub(super) fn add_nav_stop(&self, widget: &impl IsA<gtk::Widget>) {
        self.nav_stops.borrow_mut().push(widget.clone().upcast());
    }

    /// Puts a row of buttons between the header and the footer.
    ///
    /// Called after [`set_nav`], which clears it. Up and down then step
    /// header, middle, footer rather than stepping over the middle - which is
    /// what a button drawn on its own line below two others looks like it
    /// should do, and what it did not do when it was merely the third entry of
    /// the header row.
    ///
    /// [`set_nav`]: Self::set_nav
    pub(super) fn set_nav_middle(&self, middle: &[gtk::Button]) {
        *self.nav_middle.borrow_mut() = middle.to_vec();
        // Into the tab order in the place it is on the page: after the header
        // it sits under, before the footer in the corner.
        let after = self.nav_header.borrow().len();
        let mut stops = self.nav_stops.borrow_mut();
        for (offset, button) in middle.iter().enumerate() {
            stops.insert(after + offset, button.clone().upcast());
        }
    }

    /// Moves to the next or previous thing on this screen worth stopping on.
    ///
    /// Returns whether it did, so a screen with no stops of its own - a text
    /// panel, say - falls back to GTK's own handling rather than trapping the
    /// key.
    pub(super) fn move_focus_stop(self: &Rc<Self>, delta: isize) -> bool {
        let stops = self.nav_stops.borrow().clone();
        if stops.is_empty() {
            return false;
        }
        let focused = gtk::prelude::GtkWindowExt::focus(&self.window);
        // Which stop the focus is in, rather than which stop it is: focus on
        // a button inside a stop still counts as being there.
        let at = focused.and_then(|widget| {
            stops.iter().position(|stop| {
                *stop == widget || stop.is_ancestor(&widget) || widget.is_ancestor(stop)
            })
        });
        let next = match at {
            Some(at) => (at as isize + delta).rem_euclid(stops.len() as isize) as usize,
            // Nowhere in particular yet: forwards starts at the beginning,
            // backwards at the end.
            None if delta > 0 => 0,
            None => stops.len() - 1,
        };
        if let Some(stop) = stops.get(next) {
            self.sounds.borrow().click();
            stop.grab_focus();
        }
        true
    }

    /// Moves between two lists sitting side by side, and does nothing
    /// anywhere else: left and right are for the panes of the browser, not a
    /// second way to reach the buttons.
    pub(super) fn move_between_lists(self: &Rc<Self>, delta: isize) -> bool {
        // Not on the settings screen, whose two lists are in the tab order
        // together and are stepped between with Enter and Escape. Left and
        // right there belong to the bars on the rows.
        if *self.screen.borrow() == Screen::Settings {
            return false;
        }
        let stops = self.nav_stops.borrow().clone();
        let Some(focused) = gtk::prelude::GtkWindowExt::focus(&self.window) else {
            return false;
        };
        let Some(at) = stops.iter().position(|stop| {
            *stop == focused || stop.is_ancestor(&focused) || focused.is_ancestor(stop)
        }) else {
            return false;
        };
        if !stops[at].is::<gtk::ListBox>() {
            return false;
        }
        let next = at as isize + delta;
        if next < 0 || next as usize >= stops.len() {
            return false;
        }
        let next = &stops[next as usize];
        if !next.is::<gtk::ListBox>() {
            return false;
        }
        self.sounds.borrow().click();
        next.grab_focus();
        true
    }

    /// Makes a list the one the gamepad drives whenever it holds the focus.
    ///
    /// The navigation machinery knows about a single list at a time, which is
    /// all any other screen needs. With two side by side, which one is "the"
    /// list has to follow the focus, or the gamepad keeps driving whichever
    /// was wired last however far the viewer has moved away from it.
    pub(super) fn follow_focus(self: &Rc<Self>, list: &gtk::ListBox) {
        let app = self.clone();
        let controller = gtk::EventControllerFocus::new();
        {
            let list = list.clone();
            controller.connect_enter(move |_| {
                *app.nav_list.borrow_mut() = Some(list.clone());
            });
        }
        list.add_controller(controller);
    }

    /// Puts a browser's listing beside its drive column.
    ///
    /// `list_page_with` has already put the listing in the page; this takes
    /// it back out and rebuilds that row with the drives to its left.
    pub(super) fn add_places_column(
        self: &Rc<Self>,
        page: &gtk::Box,
        current: &std::path::Path,
        folders: bool,
        header: &[gtk::Button],
    ) {
        let Some(listing) = page.last_child() else {
            return;
        };
        let Some((places, list)) = self.places_column(current, folders) else {
            return;
        };
        page.remove(&listing);

        // The column takes the width it asked for and the listing takes the
        // rest. Without this the listing is given its minimum, which for a
        // list of names is very little, and the folders end up in a ribbon
        // down one side of the screen.
        places.set_hexpand(false);
        listing.set_hexpand(true);

        // Handed to set_nav, which puts it in the order ahead of the listing
        // it sits left of, and driven by the same keys once it has focus.
        *self.nav_side_list.borrow_mut() = Some(list.clone());
        self.wire_arrows(list.upcast_ref());
        // Its own, since wire_navigation only ever sees a screen's main list.
        announce_selection(&list);

        // Up from the top of the column reaches the trail above it, the same
        // way it does from the listing.
        {
            let app = self.clone();
            let header: Vec<glib::WeakRef<gtk::Button>> =
                header.iter().map(|button| button.downgrade()).collect();
            let controller = gtk::EventControllerKey::new();
            // Weak, since the controller is added to the very list it watches
            // and holding a strong reference would keep the pair alive.
            let watched = list.downgrade();
            controller.connect_key_pressed(move |_, key, _, _| {
                let Some(list) = watched.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if key != gdk::Key::Up || list.selected_row().map(|row| row.index()) != Some(0) {
                    return glib::Propagation::Proceed;
                }
                let buttons: Vec<gtk::Button> = header
                    .iter()
                    .filter_map(|button| button.upgrade())
                    .collect();
                if let Some(button) = App::last_header(&buttons) {
                    app.sounds.borrow().click();
                    button.grab_focus();
                }
                glib::Propagation::Stop
            });
            list.add_controller(controller);
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .vexpand(true)
            .build();
        row.append(&places);
        row.append(&listing);
        page.append(&row);
    }

    /// Browsing for Kodi's userdata folder, in TinePlayer's own browser.
    ///
    /// The system's folder chooser would do the job, but not from a sofa: it
    /// is a desktop dialog that a gamepad cannot drive and that draws itself
    /// at desktop sizes on a television. This is the same browser used for
    /// finding a video, showing only folders, with choosing the current one
    /// on a button beside the way out.
    ///
    /// Deliberately a sibling of `show_browser` rather than a mode of it.
    /// That one carries a paste row, video entries, a remembered location and
    /// an origin to return to, none of which belong here, and threading a
    /// purpose through all of it would put the video browser at risk for the
    /// sake of a screen that shares only its shape.
    /// The screen for choosing the folder Kodi keeps its settings in.
    pub(super) fn show_kodi_folder(self: &Rc<Self>, directory: &std::path::Path) {
        let directory = crate::browser::rooted(directory);
        let page = self.browser_page(&directory, Browse::Folders);
        let entries = browser_entries(&directory, Browse::Folders);

        // What this browser is for, said on it. It is reached by a trail of
        // folder names and nothing else, so without this the only statement of
        // what to look for was on the row that opened it, a screen ago - and
        // the wrong answer here is one that fails silently later.
        let prompt = row_note(
            tr!("Choose Kodi's user data folder - the one holding guisettings.xml.").as_ref(),
            self.scale.get(),
        );
        prompt.set_halign(gtk::Align::Center);
        page.page.append(&prompt);

        let choose = gtk::Button::with_label(&tr!("Choose This Folder"));
        choose.add_css_class("tp-button");
        choose.add_css_class("tp-action");
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        buttons.append(&page.cancel);
        buttons.append(&choose);
        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&page.browse));
        footer.set_center_widget(Some(&buttons));
        page.page.append(&footer);

        fill_browser_list(&page.list, &entries, self.scale.get());

        {
            let app = self.clone();
            let entries = entries.clone();
            let here = directory.clone();
            page.list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let Some(entry) = entries.get(row.index() as usize) else {
                    return;
                };
                match &entry.path {
                    Some(path) => app.show_kodi_folder(path),
                    None => {
                        if let Some(parent) = here.parent() {
                            app.show_kodi_folder(parent);
                        }
                    }
                }
            });
        }
        {
            let app = self.clone();
            let directory = directory.clone();
            choose.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.take_kodi_folder(directory.clone());
            });
        }
        {
            let app = self.clone();
            page.cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_kodi_settings();
            });
        }

        // Same order they are laid out in, or moving between them runs
        // backwards against what is on screen.
        self.wire_navigation(
            &page.list,
            &page.crumbs,
            &[page.cancel.clone(), choose.clone()],
        );
        *self.screen.borrow_mut() = Screen::KodiFolder;
        self.window.set_child(Some(&self.modal(&page.page)));
        if let Some(row) = page.list.row_at_index(0) {
            page.list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// The system's own folder chooser, for anyone who would rather use it.
    pub(super) fn choose_kodi_folder_natively(self: &Rc<Self>, start: &std::path::Path) {
        let chooser = gtk::FileChooserNative::new(
            Some(tr!("Choose Kodi's user data folder").as_ref()),
            Some(&self.window),
            gtk::FileChooserAction::SelectFolder,
            Some("Choose"),
            Some("Cancel"),
        );
        open_at(&chooser, start);
        let app = self.clone();
        // Held by the closure so the dialog outlives this function; a dropped
        // FileChooserNative closes before the user can answer. Same handling
        // as the video chooser.
        let held = RefCell::new(Some(chooser.clone()));
        chooser.connect_response(move |chooser, response| {
            let chosen = (response == gtk::ResponseType::Accept)
                .then(|| chooser.file().and_then(|file| file.path()))
                .flatten();
            held.borrow_mut().take();
            if let Some(folder) = chosen {
                app.take_kodi_folder(folder);
            }
        });
        chooser.show();
    }

    /// Something went wrong, said plainly, with a way back to where it was
    /// worth trying from.
    fn show_kodi_error(self: &Rc<Self>, message: &str, back: impl Fn() + 'static) {
        self.kodi_notice(&tr!("Configuration Error"), &[message], back);
    }

    /// A panel that states something and offers only to be dismissed.
    ///
    /// Distinct from [`confirm_kodi`], which asks a question and therefore has
    /// two answers. Nothing here is being decided: the one button is a way on
    /// from something already settled, so it says OK rather than naming an
    /// action nobody is taking.
    ///
    /// [`confirm_kodi`]: Self::confirm_kodi
    fn kodi_notice(self: &Rc<Self>, title: &str, lines: &[&str], back: impl Fn() + 'static) {
        let page = wizard_page(title);
        for line in lines {
            page.append(&wizard_text(line, false));
        }

        let ok = gtk::Button::with_label(&tr!("OK"));
        ok.add_css_class("tp-button");
        ok.set_halign(gtk::Align::Center);
        page.append(&ok);
        {
            let app = self.clone();
            ok.connect_clicked(move |_| {
                app.sounds.borrow().click();
                back();
            });
        }

        self.set_nav(None, std::slice::from_ref(&ok), &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::KodiError;
        self.window.set_child(Some(&self.dialog(&page)));
        ok.grab_focus();
    }

    /// A panel that states something and asks whether to go ahead.
    ///
    /// Cancel always returns to the Kodi pane, because that is the one
    /// place any of this is opened from now. It used to take a destination:
    /// backing out of the wizard's summary went to the screen the answer had
    /// been given on, so it could be changed rather than the whole sequence
    /// restarted. With the answers on rows there is nothing to restart, and
    /// the row is on the pane behind this panel.
    fn confirm_kodi(
        self: &Rc<Self>,
        title: &str,
        lines: &[&str],
        confirm: Confirm<'_>,
        action: impl Fn() + 'static,
    ) {
        let page = wizard_page(title);
        for line in lines {
            // A command is the one thing somebody has to reproduce exactly,
            // so it is set apart and wraps by character rather than by word.
            let command = line.starts_with("flatpak ");
            page.append(&wizard_text(line, command));
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        // Backing out is never the hazard, so Cancel is never the red one.
        // It used to be: red was put on whichever button was left over, so a
        // confirmation of something harmless painted the way out as the
        // dangerous choice.
        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        let destructive = confirm.destructive;
        let confirm = gtk::Button::with_label(confirm.label);
        confirm.add_css_class("tp-button");
        confirm.add_css_class(match destructive {
            true => "tp-danger",
            false => "tp-action",
        });
        row.append(&cancel);
        row.append(&confirm);
        page.append(&row);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_kodi_settings();
            });
        }
        {
            let app = self.clone();
            confirm.connect_clicked(move |_| {
                app.sounds.borrow().click();
                action();
            });
        }

        self.set_nav(None, &[cancel.clone(), confirm.clone()], &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::KodiConfirm;
        self.window.set_child(Some(&self.dialog(&page)));
        // Cancel, so a reflexive second press changes nothing.
        cancel.grab_focus();
    }

    /// The permission a Flatpak Kodi needs before it can start TinePlayer at
    /// all, as something to read and run rather than something done quietly on
    /// somebody's behalf.
    ///
    /// This was a step in the wizard, which meant everyone who had a Flatpak
    /// Kodi met it exactly once, at the moment they were busy setting the
    /// thing up, and never again. It is a row now: still there the next day,
    /// when the film did not play and the question is why.
    ///
    /// Granting it lets Kodi run *any* command on the machine, which is a real
    /// widening of what an installed application can do, so the panel says so
    /// and TinePlayer never runs it.
    pub(super) fn show_kodi_permission(self: &Rc<Self>, index: usize) {
        let Some(manual) = self
            .with_kodi_setup(index, |setup| setup.confinement)
            .and_then(crate::kodi_setup::manual_step)
        else {
            return;
        };

        let page = wizard_page(&manual.what);
        page.append(&wizard_text(&manual.why, false));
        if let Some(command) = manual.command {
            page.append(&wizard_text(&tr!("Run this once, in a terminal:"), false));
            page.append(&wizard_text(command, true));
        }
        page.append(&wizard_text(&manual.cost, false));
        if let Some(undo) = manual.undo {
            page.append(&wizard_text(&tr!("To undo it:"), false));
            page.append(&wizard_text(undo, true));
        }

        let ok = gtk::Button::with_label(&tr!("Done"));
        ok.add_css_class("tp-button");
        ok.add_css_class("tp-action");
        ok.set_halign(gtk::Align::Center);
        page.append(&ok);
        {
            let app = self.clone();
            ok.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_kodi_settings();
            });
        }

        self.set_nav(None, std::slice::from_ref(&ok), &[]);
        // Ctrl+C reaches the command, which is the whole point of the panel.
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::KodiPermission;
        self.window.set_child(Some(&self.dialog(&page)));
        ok.grab_focus();
    }
}
