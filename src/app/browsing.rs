//! Choosing a video: the built-in browser, the native chooser, and pasting a URL.

use super::*;

impl App {
    /// Notes the screen a modal is about to cover.
    ///
    /// Only the screens that are not themselves modals, so that one modal
    /// replacing another leaves the pair's origin alone. A modal recorded as
    /// its own origin is a trap: backing out of it returns to itself, and
    /// nothing closes it.
    pub(super) fn remember_origin(&self) {
        let screen = *self.screen.borrow();
        if matches!(screen, Screen::Menu | Screen::VideoSource) {
            self.origin.set(screen);
        }
    }

    /// Back to whatever the modal was opened over.
    pub(super) fn return_to_origin(self: &Rc<Self>) {
        match self.origin.get() {
            Screen::VideoSource => self.choose_video(),
            _ => self.show_menu(),
        }
    }

    /// Floats a page over the main menu, dimmed and unresponsive behind it.
    ///
    /// The menu is rebuilt rather than kept aside, because every screen here
    /// replaces the window's child outright and there is no earlier page still
    /// around to reuse. Building a second one is cheap next to what it buys:
    /// the browser reads as something opened over the menu instead of as
    /// another step deeper into it.
    /// A dialog over the screen behind it, held to one width.
    ///
    /// Every panel that states something and asks a question goes through
    /// here, so they are all the same measure however long their words are.
    /// Without a ceiling each one is as wide as its own longest sentence
    /// wants to be, which on a 3440px monitor is a single line across the
    /// whole screen - and two dialogs in a row are then visibly two different
    /// shapes for no reason a viewer could name.
    ///
    /// The cap is a `Column` rather than a size request, for the reason
    /// `src/column.rs` sets out at length: a size request is a minimum, so a
    /// panel whose natural width exceeds it widens anyway. The panel keeps the
    /// modal styling and the `Column` around it stays invisible, or the
    /// background would be drawn across the full width instead of behind the
    /// words.
    pub(super) fn dialog(self: &Rc<Self>, page: &gtk::Box) -> gtk::Overlay {
        page.add_css_class("tp-modal");
        self.modal_around(&self.dialog_column(page))
    }

    /// The width ceiling on its own, for the two panels that fill the window
    /// rather than floating over a screen: closing the player is asked before
    /// there is anything to float over, and a fatal error has nothing left
    /// behind it worth showing.
    pub(super) fn dialog_column(&self, page: &gtk::Box) -> crate::column::Column {
        let most = (DIALOG_MAX_UNITS * self.scale.get()).round() as i32;
        crate::column::Column::around(page, most)
    }

    pub(super) fn modal(self: &Rc<Self>, page: &gtk::Box) -> gtk::Overlay {
        page.add_css_class("tp-modal");
        self.modal_around(page)
    }

    /// The scrim and the screen behind it, around whatever is being floated.
    fn modal_around(self: &Rc<Self>, content: &impl IsA<gtk::Widget>) -> gtk::Overlay {
        // Whatever is on screen right now, so the modal opens over the screen
        // it was actually opened from rather than always over the main menu.
        //
        // One modal replacing another hands back the page *behind* it instead
        // of the modal itself, or the dimming would stack up a layer deeper
        // every time.
        //
        // Nothing behind it is drawn as nothing. A menu built to stand in for
        // the screen behind was what this did before there was a real one to
        // use, and a rebuilt menu is not the screen it claims to be: it shows
        // the main menu behind a dialog opened from somewhere else entirely.
        // The window has a child from the first screen onwards, so what is
        // left here is the moment before that.
        // Only a *modal's* overlay is unwrapped, which is what the marker
        // class is for. The media page is an overlay too - artwork behind,
        // page in front - and taking its child handed back the bare backdrop
        // and threw the page away, so the browser opened over a film's
        // wallpaper with nothing on it.
        let modal_stack = |child: &gtk::Widget| {
            child
                .downcast_ref::<gtk::Overlay>()
                .is_some_and(|overlay| overlay.has_css_class(MODAL_STACK))
        };
        let backdrop: gtk::Widget = match self.window.child() {
            Some(child) if modal_stack(&child) => {
                let overlay = child.downcast::<gtk::Overlay>().expect("checked above");
                let under = overlay.child();
                overlay.set_child(None::<&gtk::Widget>);
                under.unwrap_or_else(|| empty_backdrop().upcast())
            }
            Some(child) => {
                self.window.set_child(None::<&gtk::Widget>);
                child
            }
            None => empty_backdrop().upcast(),
        };
        // Not just visually behind: an insensitive page cannot take focus, so
        // neither tab nor the gamepad can reach what is underneath.
        backdrop.set_sensitive(false);

        let scrim = gtk::Box::builder().css_classes(["tp-scrim"]).build();

        let overlay = gtk::Overlay::new();
        overlay.add_css_class(MODAL_STACK);
        overlay.set_child(Some(&backdrop));
        overlay.add_overlay(&scrim);
        overlay.add_overlay(content);
        overlay
    }

    /// A panel for the one thing browsing folders cannot reach: an address.
    ///
    /// Its own screen rather than a field in the browser, because a text field
    /// among the folders is a trap for a controller, which can neither type
    /// into one nor easily get out of it. Behind a row, it is only ever
    /// entered on purpose, and there is room to say what may be pasted.
    pub(super) fn show_paste_uri(self: &Rc<Self>) {
        // Built by hand rather than from the list page every other screen
        // uses: that one leads with a header and a list, and here both would
        // be empty space above the only thing on the panel.
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(28)
            .valign(gtk::Align::Center)
            .margin_top(48)
            .margin_bottom(48)
            .margin_start(56)
            .margin_end(56)
            .build();

        let heading = heading_label(&tr!("Open a URL"));
        heading.set_halign(gtk::Align::Center);
        page.append(&heading);

        let blurb = gtk::Label::builder()
            .label(
                tr!("Enter an address to a video file, such as a link from a media server, a local file path, or a network path.").as_ref(),
            )
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&blurb);

        let field = gtk::Entry::new();
        field.add_css_class("tp-path");
        field.set_placeholder_text(Some("http://…"));
        gtk::prelude::EditableExt::set_alignment(&field, 0.5);
        field.set_hexpand(true);
        page.append(&field);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        let open = gtk::Button::with_label(&tr!("Open"));
        open.add_css_class("tp-button");
        open.add_css_class("tp-action");
        // Nothing to open until there is something in the field, and an empty
        // one would only fail slowly against a source that does not exist.
        open.set_sensitive(false);
        {
            let open = open.clone();
            field.connect_changed(move |field| {
                open.set_sensitive(!field.text().trim().is_empty());
            });
        }
        buttons.append(&cancel);
        buttons.append(&open);
        page.append(&buttons);

        {
            let app = self.clone();
            let field = field.clone();
            open.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.open_typed_path(&field.text());
            });
        }
        {
            let app = self.clone();
            field.connect_activate(move |field| {
                if !field.text().trim().is_empty() {
                    app.open_typed_path(&field.text());
                }
            });
        }
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.remember_origin();
        // Its own tab order: the field, then the two buttons. Without stops
        // of its own there is nothing for Tab to move between, and the Open
        // button cannot be reached without a pointer.
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&field);
        self.add_nav_stop(&cancel);
        self.add_nav_stop(&open);
        *self.screen.borrow_mut() = Screen::PasteUri;
        self.window.set_child(Some(&self.modal(&page)));
        // The field wants the caret from the moment it opens: this screen
        // exists to be typed into.
        field.grab_focus();

        // Filled in for you when the clipboard already holds something this
        // panel could open, and selected so typing replaces it. Better than a
        // Paste button: a controller cannot reach one, and a button says
        // nothing about whether pressing it would help.
        {
            let field = field.clone();
            gtk::prelude::WidgetExt::display(&self.window)
                .clipboard()
                .read_text_async(gtk::gio::Cancellable::NONE, move |text| {
                    let Ok(Some(text)) = text else { return };
                    let text = text.trim();
                    if looks_openable(text) {
                        field.set_text(text);
                        field.select_region(0, -1);
                    }
                });
        }
    }

    /// Opens whatever was typed into the paste panel.
    ///
    /// A folder browses to it, so typing a path is another way to navigate.
    /// Anything else is handed to [`Source`], which is what decides whether a
    /// string is a file or a URL, so this cannot disagree with what the
    /// command line accepts.
    fn open_typed_path(self: &Rc<Self>, text: &str) {
        let text = text.trim();
        let as_path = std::path::Path::new(text);
        if as_path.is_dir() {
            self.show_browser(as_path, None);
            return;
        }

        self.show_opening(Source::parse(text));
    }

    /// Waits for a source to answer, with something on screen that says so.
    ///
    /// Reading a remote source is not quick and can fail slowly: an address
    /// nothing answers at takes the discoverer's full ten seconds. Doing that
    /// on the main thread froze the whole window, which reads as a crash
    /// rather than as waiting, so the probe runs on a thread of its own.
    fn show_opening(self: &Rc<Self>, source: Source) {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(28)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .margin_top(48)
            .margin_bottom(48)
            .margin_start(56)
            .margin_end(56)
            .build();

        // A floor rather than a fixed size: with only a spinner and a short
        // address on it, the panel would otherwise shrink to something much
        // narrower than the one it replaces, and the swap would read as the
        // window jumping about.
        page.set_size_request((560.0 * self.scale.get()).round() as i32, -1);

        let spinner = gtk::Spinner::new();
        spinner.set_size_request(
            (48.0 * self.scale.get()).round() as i32,
            (48.0 * self.scale.get()).round() as i32,
        );
        spinner.start();
        page.append(&spinner);
        page.append(&heading_label(&tr!("Opening")));

        // The launcher's title where there is one, and the file name
        // otherwise. Nothing beside the file has been read yet at this point -
        // that is what the spinner is waiting for - so this is as much as can
        // be known, and for an add-on stream it is the difference between a
        // name and an opaque id.
        let opening = match self.launcher_title() {
            title if !title.is_empty() => title,
            _ => source.label(),
        };
        let what = gtk::Label::builder()
            .label(&opening)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&what);

        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        cancel.set_halign(gtk::Align::Center);
        page.append(&cancel);
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.show_paste_uri());
        }

        *self.screen.borrow_mut() = Screen::Opening;
        self.window.set_child(Some(&self.modal(&page)));
        self.set_nav(None, &[], &[]);
        cancel.grab_focus();

        // A plain channel polled from the main loop, rather than anything
        // asynchronous: the probe returns once, and the result has to be
        // applied on this thread because everything it touches is `Rc`.
        let (sender, receiver) = std::sync::mpsc::channel();
        let probing = source.clone();
        std::thread::spawn(move || {
            let _ = sender.send(crate::probe::probe_media(&probing));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                // The thread is gone without an answer, which leaves nothing
                // to report and no reason to keep looking.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            // Cancelled, or moved on some other way, while it was working.
            if *app.screen.borrow() != Screen::Opening {
                return glib::ControlFlow::Break;
            }

            match result.and_then(|media| app.apply_media(&source, media)) {
                Ok(()) => app.show_menu(),
                Err(e) => {
                    log::error!("Couldn't read {}: {e}", source.uri());
                    app.forget_file();
                    app.show_source_error(&source, &e, false);
                }
            }
            glib::ControlFlow::Break
        });
    }

    /// The built-in browser: another list screen, so it navigates exactly
    /// like the menus and needs no pointer.
    ///
    /// `select` names the folder just stepped out of, which is then the row
    /// focus lands on. Going up otherwise dumps you at the top of a long
    /// list with no sense of where you were.
    /// The screen for choosing a video: folders, and the videos in them.
    pub(super) fn show_browser(
        self: &Rc<Self>,
        directory: &std::path::Path,
        select: Option<&std::path::Path>,
    ) {
        // The same screen chooses a video and a separate soundtrack for one,
        // differing only in what it lists and what activating a row does.
        // Which of the two is in hand is held on the application rather than
        // passed down, because stepping into a folder re-enters here and would
        // otherwise forget what was being looked for.
        let mode = match self.errand.get() {
            Errand::Audio(_) => Browse::Audio,
            Errand::Subtitle => Browse::Subtitles,
            Errand::Video => Browse::Videos,
        };
        let directory = crate::browser::rooted(directory);
        let page = self.browser_page(&directory, mode);
        let entries = browser_entries(&directory, mode);

        // The two things done with a selection, together in the middle, in
        // the order every other pair in the application uses: the way out
        // first, then the action. Opening the system browser stays off to one
        // side, being a way out of this screen rather than a use of it.
        let choices = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing((24.0 * self.scale.get()).round() as i32)
            .build();
        choices.append(&page.cancel);
        choices.append(&page.open);

        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&page.browse));
        footer.set_center_widget(Some(&choices));
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
                    Some(path) if path.is_dir() => app.show_browser(path, None),
                    // A soundtrack for the video already chosen, rather than a
                    // video: it replaces whatever track that output was on and
                    // hands straight back to the menu, where the row now names
                    // the file.
                    Some(path) if app.errand.get() == Errand::Subtitle => {
                        app.set_subtitle_file(path);
                        app.show_menu();
                    }
                    Some(path) if matches!(app.errand.get(), Errand::Audio(_)) => {
                        app.set_audio_file(path);
                        app.show_menu();
                    }
                    // Through the same screen a URL opens through, rather
                    // than reading the file here and moving when it is done.
                    // Probing is not instant - it starts a GStreamer
                    // discoverer, and a file on a network share can take a
                    // second or two - and doing it on this thread left the
                    // browser standing there with the row lit, looking like
                    // the press had been missed. This puts the spinner up
                    // first and reads the file behind it.
                    Some(path) => app.show_opening(Source::File(path.to_path_buf())),
                    // Up. Only offered when there is somewhere above to go:
                    // at the top of the tree the column to the left is how
                    // you reach anywhere else.
                    None => {
                        if let Some(parent) = here.parent() {
                            app.show_browser(parent, Some(&here));
                        }
                    }
                }
            });
        }
        {
            let app = self.clone();
            page.cancel.connect_clicked(move |_| app.go_back());
        }
        // The button does what a double click does, by asking the list to
        // activate the row rather than repeating what activation means. One
        // description of what opening a row is, in the handler above.
        {
            let list = page.list.clone();
            page.open.connect_clicked(move |_| {
                if let Some(row) = list.selected_row() {
                    list.emit_by_name::<()>("row-activated", &[&row]);
                }
            });
        }
        // Off unless a file is selected. Not a folder, which a double click
        // or Enter still steps into - the button is for choosing the thing
        // this screen exists to choose, and a folder is not it. Not the way
        // up, and not the notice a folder with nothing in it shows, which is
        // a row like any other to GTK.
        {
            let open = page.open.clone();
            let openable: Vec<bool> = entries.iter().map(|entry| entry.openable).collect();
            page.list.connect_row_selected(move |_, row| {
                let selected = row
                    .map(|row| row.index() as usize)
                    .and_then(|index| openable.get(index).copied())
                    .unwrap_or(false);
                open.set_sensitive(selected);
            });
        }

        {
            let mut config = self.config.borrow_mut();
            config.last_folder = Some(directory.to_path_buf());
            let _ = config.save();
        }

        // The trail alone now that the arrow has gone: left from the current
        // folder simply walks back up it.
        // Typing a letter jumps to the first name that begins with it, which
        // is how a folder of two hundred films is reached without holding an
        // arrow key. Attached here rather than to every list: the browser is
        // the one screen whose rows are named by something other than us, and
        // so the one where a name cannot be predicted.
        {
            let labels: Vec<String> = entries
                .iter()
                .map(|entry| entry.label.trim().to_lowercase())
                .collect();
            let list = page.list.clone();
            let app = self.clone();
            // What was typed last, so a repeat of it can be told from a new
            // letter. Held by the controller rather than the application: it
            // belongs to this listing and is meaningless once it is gone.
            let last: RefCell<Option<String>> = RefCell::new(None);
            let controller = gtk::EventControllerKey::new();
            controller.connect_key_pressed(move |_, key, _, state| {
                // Nothing with a modifier on it: those are shortcuts, and
                // Ctrl+C on a browser row should stay Ctrl+C. Shift is let
                // through, being how a capital arrives.
                if state.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::META_MASK,
                ) {
                    return glib::Propagation::Proceed;
                }
                let Some(typed) = key.to_unicode().filter(|c| c.is_alphanumeric()) else {
                    return glib::Propagation::Proceed;
                };
                let typed = typed.to_lowercase().to_string();
                // The same letter again walks on to the next name that starts
                // with it, wrapping at the end; a different letter starts from
                // the top. Without that, a folder holding a dozen films
                // beginning with "The" would answer every press with the same
                // row and look as though the key had done nothing.
                let again = last.borrow().as_deref() == Some(typed.as_str());
                *last.borrow_mut() = Some(typed.clone());
                let from = match again {
                    true => list
                        .selected_row()
                        .map_or(0, |row| row.index() as usize + 1),
                    false => 0,
                };
                let matching = |offset: usize| {
                    let index = (from + offset) % labels.len().max(1);
                    labels
                        .get(index)
                        .filter(|label| label.starts_with(&typed))
                        .map(|_| index)
                };
                let Some(index) = (0..labels.len()).find_map(matching) else {
                    // Nothing starts with it. Swallowed all the same, so a
                    // stray letter cannot fall through to whatever else on the
                    // screen might answer it.
                    return glib::Propagation::Stop;
                };
                if let Some(row) = list.row_at_index(index as i32) {
                    app.sounds.borrow().click();
                    list.select_row(Some(&row));
                    settle_on(&row);
                }
                glib::Propagation::Stop
            });
            page.list.add_controller(controller);
        }

        self.wire_navigation(
            &page.list,
            &page.crumbs,
            &[page.cancel.clone(), page.open.clone()],
        );
        self.remember_origin();
        *self.screen.borrow_mut() = Screen::Browser;
        self.window.set_child(Some(&self.modal(&page.page)));

        let opening = select
            .and_then(|wanted| {
                entries
                    .iter()
                    .position(|entry| entry.path.as_deref() == Some(wanted))
            })
            // Otherwise the first real entry, skipping the rows that only
            // lead somewhere else: up, and the empty-folder notice.
            .or_else(|| entries.iter().position(|entry| entry.path.is_some()))
            // Nothing to open: the way up, rather than the line saying so.
            .unwrap_or(0) as i32;
        if let Some(row) = page.list.row_at_index(opening) {
            page.list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// The scaffolding every browsing screen is built on.
    ///
    /// One page for two jobs - choosing a video, and choosing a folder to set
    /// Kodi up in - because they are the same screen with different rows in
    /// it. Built separately they drifted: the same trail, places column and
    /// system-browser button written twice, so a change to how browsing looks
    /// had to be made in both and was once made in only one.
    ///
    /// What differs is left to the caller: what the footer holds, what a row
    /// does when it is chosen, and where the cursor starts.
    pub(super) fn browser_page(
        self: &Rc<Self>,
        directory: &std::path::Path,
        mode: Browse,
    ) -> BrowserPage {
        let (crumbs, crumb_buttons) = self.breadcrumbs(directory, mode.folders_only());

        let (page, list, _back, slot) = list_page_with(&crumbs, false, self.scale.get());
        // The arrow's slot holds a fixed width for every screen to line up
        // against. With no arrow in it, that is just a gap before the trail.
        slot.set_visible(false);
        // Keeps its place marked while the keyboard is over in the places
        // column, since that is the row you are put back on when you come
        // out of it - see `.tp-resting` in the stylesheet.
        list.add_css_class("tp-resting");
        self.add_places_column(&page, directory, mode.folders_only(), &crumb_buttons);
        self.follow_focus(&list);

        // Along the foot with the way out, rather than tucked into the header:
        // both are things done with the browser rather than places inside it.
        // Still not focusable, and last: it exists for a pointer, and the
        // dialog it opens cannot be driven by a controller anyway.
        let browse_face = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        // Larger than the lettering beside it: at this size the icon is what
        // the eye finds first, and the words only confirm it.
        // The same folder the rows are drawn with, so the button that opens
        // another browser is marked with what it opens - smaller than in a
        // row, where it stands alone against a name; here it sits beside a
        // line of text on a button and should not outweigh it.
        let browse_icon = RowIcon::Folder.image_at(BUTTON_FOLDER_PX, self.scale.get());
        browse_face.append(&browse_icon);
        browse_face.append(&gtk::Label::new(Some(tr!("Open System Browser").as_ref())));
        let browse = gtk::Button::builder().child(&browse_face).build();
        browse.add_css_class("tp-button");
        browse.add_css_class("tp-secondary");
        browse.set_can_focus(false);
        browse.set_valign(gtk::Align::Start);
        {
            let app = self.clone();
            // Wherever the listing behind it has reached. Handing over at the
            // top of the tree, or wherever the system dialog last was, means
            // walking back down a path already walked.
            let here = directory.to_path_buf();
            browse.connect_clicked(move |_| match mode {
                Browse::Videos => app.open_file_chooser(&here),
                Browse::Folders => app.choose_kodi_folder_natively(&here),
                // The same dialog, filtered to whatever is being looked for:
                // it reads which errand it is on for itself.
                Browse::Audio | Browse::Subtitles => app.open_file_chooser(&here),
            });
        }

        // What a click used to do on its own. A single click selects now, so
        // there has to be something a pointer can press to act on what it
        // selected - a double click is the shortcut, not the only way.
        let open = gtk::Button::with_label(&tr!("Open"));
        open.add_css_class("tp-button");
        open.add_css_class("tp-action");
        // Nothing is selected until the list is filled, and a row that opens
        // nothing leaves it off again. See `follow_open`.
        open.set_sensitive(false);

        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");

        // A click selects; it takes a second one to open. Set here rather than
        // on each screen, so the file browser and the folder chooser cannot
        // come to disagree about what a click does.
        //
        // The keyboard is untouched by it. GtkListBox emits `row-activated` on
        // a double click and on Enter either way, and Enter here goes through
        // `activate_focused`, which emits it by hand - so every handler is
        // reached exactly as it was.
        list.set_activate_on_single_click(false);

        BrowserPage {
            page,
            list,
            crumbs: crumb_buttons,
            browse,
            open,
            cancel,
        }
    }

    /// The current directory as a row of buttons, one per level, so any
    /// ancestor is a single press away rather than several trips through Up.
    ///
    /// Capped at the last few levels: a deep path would otherwise run off the
    /// side, and the leading button stands in for everything trimmed away.
    /// `folders` decides which browser a crumb reopens. Without it, stepping
    /// up the trail from the folder browser lands in the video browser, which
    /// is the same shape of screen doing an entirely different job.
    fn breadcrumbs(
        self: &Rc<Self>,
        directory: &std::path::Path,
        folders: bool,
    ) -> (gtk::Box, Vec<gtk::Button>) {
        use std::path::{Component, PathBuf};

        // Each level paired with the path that reaches it.
        let mut levels: Vec<(String, PathBuf)> = Vec::new();
        let mut walked = PathBuf::new();
        for component in directory.components() {
            match component {
                Component::Prefix(prefix) => {
                    walked.push(prefix.as_os_str());
                    // Rooted right here, because `H:` on its own does not mean
                    // the top of that drive: it means wherever that drive was
                    // last left, which is a relative path. Browsing to one
                    // works, since reading it still finds the right folder,
                    // but every entry under it is relative too and no URI can
                    // be made from those.
                    walked.push(std::path::MAIN_SEPARATOR_STR);
                    levels.push((
                        prefix.as_os_str().to_string_lossy().to_string(),
                        walked.clone(),
                    ));
                }
                Component::RootDir => {
                    if levels.is_empty() {
                        walked.push(std::path::MAIN_SEPARATOR_STR);
                        levels.push(("/".to_string(), walked.clone()));
                    }
                }
                Component::Normal(name) => {
                    walked.push(name);
                    levels.push((name.to_string_lossy().to_string(), walked.clone()));
                }
                _ => {}
            }
        }

        const SHOWN: usize = 4;
        let mut trimmed = Vec::new();
        if levels.len() > SHOWN {
            let hidden = levels.len() - SHOWN;
            // Leads to the level just above the first one still shown.
            trimmed.push(("…".to_string(), levels[hidden - 1].1.clone()));
            trimmed.extend_from_slice(&levels[hidden..]);
        } else {
            trimmed = levels;
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .hexpand(true)
            .build();
        let mut buttons = Vec::new();

        for (position, (label, target)) in trimmed.iter().enumerate() {
            if position > 0 {
                let separator = gtk::Label::new(Some("›"));
                separator.add_css_class("tp-crumb-separator");
                row.append(&separator);
            }

            let button = gtk::Button::with_label(label);
            button.add_css_class("tp-crumb");
            {
                let app = self.clone();
                let target = target.clone();
                let here = directory.to_path_buf();
                button.connect_clicked(move |_| {
                    app.sounds.borrow().click();
                    if folders {
                        app.show_kodi_folder(&target);
                        return;
                    }
                    // Selecting the folder you are already in should settle
                    // focus back on the listing rather than rebuild nothing.
                    let select = (target != here).then(|| here.clone());
                    app.show_browser(&target, select.as_deref());
                });
            }
            row.append(&button);
            buttons.push(button);
        }

        (row, buttons)
    }

    /// Where a video comes from: a folder on this machine, or an address.
    ///
    /// A step of its own rather than opening the browser straight away,
    /// because the two are not the same kind of thing. Walking folders finds
    /// what is here; an address reaches what is not, and no amount of
    /// browsing would ever lead to it.
    pub(super) fn choose_video(self: &Rc<Self>) {
        let scale = self.scale.get();
        let (panel, browse, address, connect, cancel) = self.choose_source_panel(scale, true);
        let cancel = cancel.expect("asked for with a cancel button");

        // A floor rather than a fixed size, the way the Opening panel has one:
        // three buttons and a line of text would otherwise make a panel much
        // narrower than the page behind it, and the swap would read as the
        // window jumping about.
        panel.set_size_request((560.0 * scale).round() as i32, -1);

        {
            let app = self.clone();
            browse.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.browse_for_file();
            });
        }
        {
            let app = self.clone();
            address.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_paste_uri();
            });
        }
        if let Some(connect) = connect.as_ref() {
            let app = self.clone();
            connect.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.start_jellyfin_connect(ConnectFrom::Menu);
            });
        }
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_menu();
            });
        }

        // The same words the empty screen shows, floated over the film rather
        // than replacing it: what is loaded is still loaded, and backing out
        // returns to it.
        self.remember_origin();
        let mut stops = vec![cancel.clone(), browse.clone(), address];
        stops.extend(connect);
        self.set_nav(None, &[], &stops);
        *self.screen.borrow_mut() = Screen::VideoSource;
        self.window.set_child(Some(&self.modal(&panel)));
        browse.grab_focus();
    }

    /// Opens the file browser where browsing last stopped.
    ///
    /// Always the built-in browser. Guessing from the last input was
    /// unpredictable: the same button opened different things depending on
    /// what you had touched. The system dialog is still reachable, from a
    /// pointer-only button in the footer.
    pub(super) fn browse_for_file(self: &Rc<Self>) {
        // Whatever errand the browser was last on, this one is a video.
        self.errand.set(Errand::Video);
        self.open_browser();
    }

    /// The same browser, looking for a soundtrack to put on one output.
    ///
    /// Starts where the video is rather than where browsing left off: a
    /// separate audio track is usually downloaded to sit beside the film, and
    /// when it is not, the film's folder is still a better place to start from
    /// than wherever a video was last chosen.
    pub(super) fn browse_for_audio(self: &Rc<Self>, role: Role) {
        self.errand.set(Errand::Audio(role));
        let beside = self
            .file
            .borrow()
            .as_ref()
            .and_then(|file| file.local().and_then(|path| path.parent()))
            .map(|folder| folder.to_path_buf());
        match beside {
            Some(folder) => self.show_browser(&folder, None),
            None => self.open_browser(),
        }
    }

    pub(super) fn open_browser(self: &Rc<Self>) {
        let (remembered, last_video) = {
            let config = self.config.borrow();
            (config.last_folder.clone(), config.last_video.clone())
        };
        let start = crate::browser::start_location(remembered.as_deref(), last_video.as_deref());
        self.show_browser(&start, None);
    }
}
