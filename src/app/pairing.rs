//! Signing in to a Jellyfin server, and signing out of one again.

use super::*;

impl App {
    /// Puts the settings screen back with the Jellyfin category showing.
    ///
    /// The counterpart to [`return_to_kodi_settings`], and it does the same
    /// job: rebuilding is what re-reads the pairing file, so the rows state
    /// what is stored rather than what was asked for.
    ///
    /// [`return_to_kodi_settings`]: Self::return_to_kodi_settings
    fn return_to_jellyfin_settings(self: &Rc<Self>) {
        // Any code still waiting is abandoned by leaving, so nothing arriving
        // late can pair a server the viewer has walked away from.
        self.jellyfin_attempt.set(self.jellyfin_attempt.get() + 1);
        self.settings_category.set(Category::Jellyfin);
        self.in_settings_pane.set(true);
        self.show_settings();
    }

    /// Opens the connection flow, remembering where it was opened from.
    ///
    /// One dialog for the whole of it rather than a row per question. Pairing
    /// is a single errand somebody does once, and splitting it across a
    /// settings pane made three rows out of two facts - which server, and the
    /// code that proves it is yours.
    pub(super) fn start_jellyfin_connect(self: &Rc<Self>, from: ConnectFrom) {
        self.connect_from.set(from);
        self.show_jellyfin_address();
    }

    /// Leaves the flow, by finishing it or backing out of it.
    ///
    /// Back to whichever screen opened it. Always returning to Settings would
    /// strand somebody who started from the empty page and never went there.
    pub(super) fn leave_jellyfin_connect(self: &Rc<Self>) {
        // Anything still polling belongs to an attempt that is now over.
        self.jellyfin_attempt.set(self.jellyfin_attempt.get() + 1);
        match self.connect_from.get() {
            ConnectFrom::Settings => self.return_to_jellyfin_settings(),
            ConnectFrom::Menu => self.show_menu(),
        }
    }

    /// The first half of the dialog: which server.
    ///
    /// It looks for one while the field sits there, and fills it in with
    /// whatever answers - so on the machine this is built for, a box wired to a
    /// television and driven by a remote, the address need never be typed at
    /// all. The field stays editable because a server reached across a VPN or
    /// on another subnet will never answer a broadcast, and its owner knows the
    /// address perfectly well.
    ///
    /// **What is found fills the field rather than becoming a list to choose
    /// from.** A list would be a second question on a panel that has one, and
    /// on a home network the answer is one server.
    ///
    /// **The field is not there while it looks.** An address box offered and
    /// then filled in underneath somebody is an invitation to start typing
    /// into something that is about to change; waiting, with a spinner and a
    /// sentence saying so, asks nothing of anybody for the two seconds it
    /// takes. It appears with the answer, whether or not there was one.
    fn show_jellyfin_address(self: &Rc<Self>) {
        /// Long enough for a server to answer twice over, short enough to sit
        /// through. Jellyfin replies in milliseconds; the wait is for one that
        /// is busy or asleep, not for the network.
        const LOOK_FOR: std::time::Duration = std::time::Duration::from_secs(2);
        /// What the waiting half of the panel is held to, so that the field
        /// arriving changes what the panel says rather than how big it is.
        /// The same floor the Opening panel keeps for the same reason.
        const BODY_MIN: f64 = 132.0;

        let scale = self.scale.get();
        let page = wizard_page(&tr!("Connect to Jellyfin"));

        // Both states live in here, which is what gives the panel one height:
        // a spinner and a sentence while it looks, a sentence and a field
        // afterwards.
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .valign(gtk::Align::Center)
            .build();
        body.set_size_request(-1, (BODY_MIN * scale).round() as i32);
        page.append(&body);

        let spinner = gtk::Spinner::new();
        let spin = (48.0 * scale).round() as i32;
        spinner.set_size_request(spin, spin);
        spinner.set_halign(gtk::Align::Center);
        spinner.start();
        body.append(&spinner);

        let hint = wizard_text(&tr!("Looking for a server on this network..."), false);
        body.append(&hint);

        let field = gtk::Entry::new();
        field.add_css_class("tp-path");
        field.set_placeholder_text(Some("http://jellyfin.local:8096"));
        gtk::prelude::EditableExt::set_alignment(&field, 0.5);
        field.set_hexpand(true);
        // Whatever was here before, which is the address of a server this
        // installation has been paired with and may be pairing with again.
        let known = self
            .jellyfin_pairing
            .borrow()
            .as_ref()
            .map(|pairing| pairing.server.clone())
            .unwrap_or_default();
        field.set_text(&known);
        // Hidden rather than merely empty: a widget that is not visible takes
        // no space, which is what leaves the sentence centred in the body
        // above it.
        field.set_visible(false);
        body.append(&field);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        let connect = gtk::Button::with_label(&tr!("Connect"));
        connect.add_css_class("tp-button");
        connect.add_css_class("tp-action");
        connect.set_sensitive(!field.text().trim().is_empty());
        {
            let connect = connect.clone();
            field.connect_changed(move |field| {
                connect.set_sensitive(!field.text().trim().is_empty());
            });
        }
        buttons.append(&cancel);
        buttons.append(&connect);
        page.append(&buttons);

        let start = {
            let app = self.clone();
            let field = field.clone();
            move || {
                let typed = field.text();
                if typed.trim().is_empty() {
                    return;
                }
                app.sounds.borrow().click();
                app.begin_quick_connect(&typed);
            }
        };
        {
            let start = start.clone();
            connect.connect_clicked(move |_| start());
        }
        {
            let start = start.clone();
            field.connect_activate(move |_| start());
        }
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.leave_jellyfin_connect();
            });
        }

        // Its own tab order. The field is deliberately not a stop yet: it is
        // hidden, and a stop that cannot be seen is a place the focus
        // disappears into. It is added when it appears.
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&cancel);
        self.add_nav_stop(&connect);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::JellyfinPanel;
        // `dialog` rather than `modal`, so this is the same measure as every
        // other panel that states something and asks a question. Left
        // uncapped, a panel is as wide as its own longest sentence wants to
        // be - which on a wide monitor is most of the screen, and makes the
        // two halves of this one flow visibly different shapes.
        self.window.set_child(Some(&self.dialog(&page)));
        // Cancel while there is nothing else to be on. The field takes the
        // focus the moment it appears, which is the two-second mark.
        cancel.grab_focus();

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(crate::jellyfin::discover(LOOK_FOR));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            let found = match receiver.try_recv() {
                Ok(found) => found,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            // Gone: cancelled, or already past this step. A panel taken out of
            // the window has no root, which is cheaper to ask than remembering
            // which screen replaced it.
            if page.root().is_none() {
                return glib::ControlFlow::Break;
            }
            // Whatever it found, the looking is over: the spinner goes and
            // the field takes its place, as a stop and as the focus.
            spinner.stop();
            spinner.set_visible(false);
            field.set_visible(true);
            app.add_nav_stop(&field);
            field.grab_focus();
            field.select_region(0, -1);
            match found.first() {
                Some(server) => {
                    hint.set_text(&tr!(
                        "Found {server} on this network.",
                        server = server.name
                    ));
                    // Only if nobody has typed since. Overwriting an address
                    // somebody is part way through entering would be the worst
                    // thing this could do with its answer.
                    if field.text() == known {
                        field.set_text(&server.address);
                    }
                }
                None => hint.set_text(
                    tr!("No server answered on this network. Enter an address manually.").as_ref(),
                ),
            }
            glib::ControlFlow::Break
        });
    }

    /// Writes down the address, then asks the server to start a pairing.
    ///
    /// A scheme is added when there is none, because "hoth:8096" is what
    /// somebody types and every request made with it would fail with nothing on
    /// screen to say why. Plain HTTP is what a Jellyfin server on a home
    /// network answers to; anybody reaching one over the internet types the
    /// https themselves.
    fn begin_quick_connect(self: &Rc<Self>, typed: &str) {
        let typed = typed.trim();
        let address = match typed.contains("://") {
            true => typed.to_string(),
            false => format!("http://{typed}"),
        };

        let pairing = match self.jellyfin_pairing.borrow().clone() {
            Some(mut pairing) => {
                pairing.set_server(&address);
                pairing
            }
            None => crate::jellyfin::Pairing::new(&address),
        };
        if let Err(e) = crate::jellyfin::save(&pairing) {
            return self.jellyfin_notice(&tr!("Could Not Save"), &[&e]);
        }
        *self.jellyfin_pairing.borrow_mut() = Some(pairing);
        self.show_jellyfin_code();
    }

    /// The second half of the dialog: the code, and waiting for it.
    ///
    /// A code rather than a login form, and that is not a matter of taste: this
    /// runs on a television, where typing a password with a remote is
    /// miserable, and it means no password is ever typed into TinePlayer at
    /// all. The viewer approves it in a Jellyfin app they are already signed
    /// in to.
    ///
    /// One thread does the whole of it - asking for the code, then polling
    /// until somebody approves it - and reports each step back to the main loop
    /// through a channel, which is the same shape everything else here uses to
    /// talk to a server without the interface stopping.
    fn show_jellyfin_code(self: &Rc<Self>) {
        /// How often to ask whether the code has been approved. Often enough
        /// that pressing approve on a phone and looking up at the television
        /// shows it done, rarely enough not to be a request a second for the
        /// several minutes somebody may take to find their phone.
        const ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
        /// Five minutes of asking. Jellyfin expires a code of its own accord
        /// around then, so waiting longer only produces a code that cannot
        /// work and a screen that does not say so.
        const TRIES: usize = 150;

        let Some(pairing) = self.jellyfin_pairing.borrow().clone() else {
            return;
        };
        if pairing.server.is_empty() {
            return;
        }

        let attempt = self.jellyfin_attempt.get() + 1;
        self.jellyfin_attempt.set(attempt);

        // Named for what it is rather than repeating the step before it. This
        // half of the dialog is Jellyfin's own Quick Connect, and the words on
        // screen match what the viewer is about to go looking for in their
        // Jellyfin app - which is the menu item called Quick Connect.
        // TRANSLATORS: "Quick Connect" is Jellyfin's own name for this
        // feature. Use whatever Jellyfin's translation into your language
        // calls it, rather than translating it afresh - this text sends
        // somebody to find it in their own Jellyfin app, and a different
        // wording sends them looking for something that is not there.
        let page = wizard_page(&tr!("Quick Connect"));
        // Filled in once the server answers. Empty rather than absent, so the
        // panel does not change shape under the eye when the code arrives.
        let code = gtk::Label::new(None);
        code.add_css_class("tp-code");
        code.set_selectable(true);
        code.set_can_focus(false);
        page.append(&code);
        // TRANSLATORS: "Quick Connect" is Jellyfin's own name for this
        // feature. Use whatever Jellyfin's translation into your language
        // calls it, rather than translating it afresh.
        let status = wizard_text(&tr!("Obtaining Quick Connect code..."), false);
        page.append(&status);

        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        cancel.set_halign(gtk::Align::Center);
        page.append(&cancel);
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.leave_jellyfin_connect();
            });
        }

        self.set_nav(None, std::slice::from_ref(&cancel), &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::JellyfinConnect;
        // The same cap as the step before it, so pressing Connect changes what
        // the panel says rather than how big it is.
        self.window.set_child(Some(&self.dialog(&page)));
        cancel.grab_focus();

        let server = pairing.server.clone();
        let device_id = pairing.device_id.clone();
        let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let asking = alive.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Some(name) = crate::jellyfin::server_name(&server)
                && sender.send(QuickConnect::Named(name)).is_err()
            {
                return;
            }
            let pending = match crate::jellyfin::quick_connect_start(&server, &device_id) {
                Ok(pending) => pending,
                Err(e) => {
                    let _ = sender.send(QuickConnect::Failed(e.to_string()));
                    return;
                }
            };
            if sender
                .send(QuickConnect::Code(pending.code.clone()))
                .is_err()
            {
                return;
            }
            for _ in 0..TRIES {
                // Checked before the request rather than after it, so
                // cancelling stops the asking rather than stopping one round
                // later.
                if !asking.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                match crate::jellyfin::quick_connect_poll(&server, &device_id, &pending) {
                    Ok(Some(account)) => {
                        let _ = sender.send(QuickConnect::Done(Box::new(account)));
                        return;
                    }
                    // Nobody has approved it yet, which is the ordinary answer
                    // while somebody finds their phone.
                    Ok(None) => {}
                    Err(e) => {
                        let _ = sender.send(QuickConnect::Failed(e.to_string()));
                        return;
                    }
                }
                std::thread::sleep(ASK_EVERY);
            }
            let _ = sender.send(QuickConnect::Failed(
                tr!("The code was not approved in time.").into_owned(),
            ));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            // Left behind by a panel that has been closed, or by a second
            // attempt started over the top of this one. Either way this one is
            // over, and the thread is told so it stops asking.
            if app.jellyfin_attempt.get() != attempt {
                alive.store(false, std::sync::atomic::Ordering::Relaxed);
                return glib::ControlFlow::Break;
            }
            let step = match receiver.try_recv() {
                Ok(step) => step,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            match step {
                // Held rather than shown: this panel is about the code. It is
                // written down when the pairing is saved, and it is what every
                // screen afterwards calls this server.
                QuickConnect::Named(name) => {
                    if let Some(pairing) = app.jellyfin_pairing.borrow_mut().as_mut() {
                        pairing.name = Some(name);
                    }
                    glib::ControlFlow::Continue
                }
                QuickConnect::Code(shown) => {
                    code.set_text(&shown);
                    status.set_text(
                        // TRANSLATORS: "Quick Connect" is Jellyfin's own name for this
                        // feature. Use whatever Jellyfin's translation into your language
                        // calls it, rather than translating it afresh.
                        tr!("In a Jellyfin app you are signed in to, open Quick Connect from the user menu and enter this code.").as_ref(),
                    );
                    glib::ControlFlow::Continue
                }
                QuickConnect::Done(account) => {
                    app.jellyfin_paired(*account);
                    glib::ControlFlow::Break
                }
                QuickConnect::Failed(why) => {
                    app.jellyfin_notice(&tr!("Could Not Connect"), &[&why]);
                    glib::ControlFlow::Break
                }
            }
        });
    }

    /// Somebody approved the code. Writes the token down and goes on the air.
    ///
    /// Connecting straight away rather than at the next start: this is the
    /// moment the viewer is watching to see whether it worked, and a cast
    /// target that appears on their phone only after a restart looks like one
    /// that did not.
    fn jellyfin_paired(self: &Rc<Self>, account: crate::jellyfin::Account) {
        let Some(mut pairing) = self.jellyfin_pairing.borrow().clone() else {
            return;
        };
        pairing.account = Some(account);
        if let Err(e) = crate::jellyfin::save(&pairing) {
            return self.jellyfin_notice(&tr!("Could Not Save"), &[&e]);
        }
        *self.jellyfin_pairing.borrow_mut() = Some(pairing);
        self.start_jellyfin();
        self.leave_jellyfin_connect();
    }

    /// Asked before disconnecting, because it throws a pairing away.
    pub(super) fn confirm_jellyfin_disconnect(self: &Rc<Self>) {
        let server = self
            .jellyfin_pairing
            .borrow()
            .as_ref()
            .map(crate::jellyfin::Pairing::label)
            .unwrap_or_default();

        let app = self.clone();
        self.confirm_jellyfin(
            &tr!("Disconnect from Jellyfin?"),
            &[&tr!(
                "TinePlayer will no longer appear as a player in {server}.",
                server = server
            )],
            Confirm {
                label: "Disconnect",
                destructive: true,
            },
            move || app.disconnect_jellyfin(),
        );
    }

    /// Ends the pairing here and, as far as it can, at the server too.
    ///
    /// The server is told on a worker thread while the local file goes at
    /// once. Waiting on it would mean a settings screen that hangs for as long
    /// as a switched-off server takes to time out, for a message the viewer has
    /// already decided the answer to - and what they asked for is to stop being
    /// paired, which is true the moment the token is gone from this machine.
    fn disconnect_jellyfin(self: &Rc<Self>) {
        let client = self
            .jellyfin_pairing
            .borrow()
            .as_ref()
            .and_then(crate::jellyfin::Client::new);
        if let Some(client) = client {
            let app = self.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(client.disconnect());
            });
            glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                match receiver.try_recv() {
                    Ok(Ok(())) => {}
                    // Said rather than swallowed, and said where it can be
                    // acted on. This is now only reached when the server could
                    // not be reached at all - a single logout either revokes
                    // the token and removes the device or does neither - so the
                    // pairing really is still live over there, and the viewer
                    // is the only one who can end it. Only over the pane it was
                    // asked from: a panel arriving over a film minutes later
                    // would be a worse fault than the one it reports.
                    Ok(Err(e)) => {
                        log::error!("Jellyfin was not told about the disconnection: {e}");
                        if app.showing_jellyfin_pane() {
                            app.jellyfin_notice(
                                &tr!("Disconnected Here Only"),
                                &[
                                    &tr!("The access token stored on this machine has been removed."),
                                    // TRANSLATORS: "Devices" is the name of a page in Jellyfin's own
                                    // dashboard. Use whatever Jellyfin's translation calls that page.
                                    &tr!("The server could not be reached. Remove TinePlayer under Devices in the Jellyfin dashboard to revoke access."),
                                    &e.to_string(),
                                ],
                            );
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                }
                glib::ControlFlow::Break
            });
        }

        *self.jellyfin.borrow_mut() = None;
        // Dropping it closes the socket, which is what takes TinePlayer off
        // everybody's phone.
        *self.jellyfin_session.borrow_mut() = None;
        if let Err(e) = crate::jellyfin::remove() {
            log::error!("Couldn't remove the Jellyfin pairing: {e}");
        }
        *self.jellyfin_pairing.borrow_mut() = None;
        self.return_to_jellyfin_settings();
    }

    /// A panel stating something the Jellyfin pane has to say, with the one
    /// way on from it.
    fn jellyfin_notice(self: &Rc<Self>, title: &str, lines: &[&str]) {
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
                app.return_to_jellyfin_settings();
            });
        }

        self.set_nav(None, std::slice::from_ref(&ok), &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::JellyfinPanel;
        self.window.set_child(Some(&self.dialog(&page)));
        ok.grab_focus();
    }

    /// The same question shape the Kodi pane asks, returning to this pane
    /// instead.
    fn confirm_jellyfin(
        self: &Rc<Self>,
        title: &str,
        lines: &[&str],
        confirm: Confirm<'_>,
        action: impl Fn() + 'static,
    ) {
        let page = wizard_page(title);
        for line in lines {
            page.append(&wizard_text(line, false));
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
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
                app.return_to_jellyfin_settings();
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
        *self.screen.borrow_mut() = Screen::JellyfinPanel;
        self.window.set_child(Some(&self.dialog(&page)));
        // Cancel, so a reflexive second press changes nothing.
        cancel.grab_focus();
    }

    pub(super) fn confirm_clear_data(self: &Rc<Self>) {
        let app = self.clone();
        self.show_confirm(
            &tr!("Clear all saved playback data?"),
            &tr!("Clear"),
            move || {
                if let Err(e) = crate::config::clear_all_resume() {
                    log::error!("{e}");
                }
                // The loaded file keeps its choices for this session; only
                // what was written down is gone.
                app.show_settings();
            },
        );
    }

    /// A yes-or-no panel over the screen that asked the question.
    ///
    /// Over it rather than in place of it, which is what it used to be: a
    /// question about something on the screen behind should leave that screen
    /// where it is, and answering it should put nothing back together.
    ///
    /// The confirming button is destructive, because this panel is. It exists
    /// for one question - whether to throw away what has been remembered - and
    /// a red button on a question that only ever destroys something is the
    /// application's own rule rather than a decision taken here.
    fn show_confirm(
        self: &Rc<Self>,
        message: &str,
        confirm_label: &str,
        action: impl Fn() + 'static,
    ) {
        let px = |base: f64| (base * self.scale.get()).round() as i32;
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(28.0))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(px(36.0))
            .margin_bottom(px(36.0))
            .margin_start(px(44.0))
            .margin_end(px(44.0))
            .build();
        let heading = heading_label(message);
        heading.set_halign(gtk::Align::Center);
        page.append(&heading);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        let confirm = gtk::Button::with_label(confirm_label);
        confirm.add_css_class("tp-button");
        confirm.add_css_class("tp-danger");
        buttons.append(&cancel);
        buttons.append(&confirm);
        page.append(&buttons);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }
        {
            let app = self.clone();
            confirm.connect_clicked(move |_| {
                app.sounds.borrow().click();
                action();
            });
        }

        self.set_nav(None, &[], &[]);
        *self.screen.borrow_mut() = Screen::Confirm;
        self.window.set_child(Some(&self.dialog(&page)));
        // Cancel takes focus, so a reflexive second press doesn't destroy
        // anything.
        cancel.grab_focus();
    }
}
