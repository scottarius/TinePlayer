//! The About screen, its third-party notices, and reading them with a keyboard.

use super::*;

impl App {
    /// What is running, who wrote what it is built on, and under what terms.
    ///
    /// Prose rather than the two version rows this replaced: the versions were
    /// only ever there to be read out when something went wrong, and the
    /// licenses of the work TinePlayer is built on ask to be acknowledged
    /// somewhere a person can find them. A packaged application with no About
    /// page has nowhere to put either.
    /// What About says, as a block of widgets rather than a screen.
    ///
    /// It was a screen reached from a row, which put the version, the license
    /// and where the settings file lives two steps and a page transition away
    /// from a viewer looking for exactly those things. The About category
    /// shows this directly, with the notices below it as the one row.
    pub(super) fn about_body(self: &Rc<Self>) -> gtk::Box {
        let px = |base: f64| (base * self.scale.get()).round() as i32;
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(12.0))
            // Room of its own inside the panel. Prose against the edge of a
            // box reads as something that overflowed into it, where the rows
            // below have their own padding and look placed.
            .margin_top(px(ABOUT_INSET))
            .margin_bottom(px(ABOUT_INSET))
            .margin_start(px(ABOUT_INSET))
            .margin_end(px(ABOUT_INSET))
            .build();

        // The name and the version, and no mark beside it: the lockup in the
        // header above says which player this is, a couple of inches away and
        // on every category rather than only this one. Two logos on one screen
        // is one of them repeating the other.
        //
        // The version stays here rather than moving into the lockup, which
        // carries a name and cannot carry a number.
        let name = about_heading(&format!("TinePlayer {}", env!("CARGO_PKG_VERSION")));
        name.add_css_class("tp-about-title");
        body.append(&name);

        // What it is, before what it is made of. Everything else on this page
        // assumes you already know, which is no use to somebody who has
        // inherited the machine it is installed on.
        body.append(&about_heading(&tr!(
            "Watch together, hear your own soundtrack"
        )));
        body.append(&about_text(
            tr!("A player that allows people to watch videos together while hearing separate soundtracks.").as_ref(),
        ));

        body.append(&about_text(
            tr!("Free software under the MIT License, Copyright (c) 2026 Scott Bounds. You may use, change and pass it on, provided the copyright notice travels with it. It comes with no warranty of any kind.").as_ref(),
        ));
        // The domain rather than the repository, and followed rather than
        // only shown. A released binary cannot be edited: if the repository
        // is ever renamed or moved, a link baked into it breaks for good,
        // where a domain we own can simply be pointed somewhere else. It is
        // also shorter to read from across a room and possible to type from
        // memory, which a full GitHub path is not.
        body.append(&about_link(
            tr!("Report issues or check for updates at").as_ref(),
            "https://tineplayer.app",
            "tineplayer.app",
        ));

        // The attribution without the numbers, which are worth stating exactly
        // and are stated below where they can be read off rather than picked
        // out of a sentence.
        body.append(&about_heading(&tr!("Built with")));
        body.append(&about_text(
            tr!(
                "GStreamer and GTK, both free software under the GNU Lesser General Public License."
            )
            .as_ref(),
        ));
        // Pointed at the copy in hand rather than at the one on the web. The
        // notices are compiled into the binary and sit one row below this, and
        // the machines this player is built for are televisions where opening
        // a browser is not something a D-pad does well.
        body.append(&about_text(
            tr!("Also the work of a good many people writing Rust libraries, all attributed under Third-Party Notices below.").as_ref(),
        ));

        // What a bug report needs, in one place and readable off the screen.
        //
        // The renderer earns its line here. GTK picks one for the machine, and
        // the same drawing can come out differently on two of them - a blend
        // node this application used to draw its backdrop with looked right on
        // Windows and was all but invisible on a Raspberry Pi, which is a
        // difference nobody can report without being told what to look at.
        body.append(&about_heading(&tr!("App Details")));
        // One label rather than a line each, so a single drag takes the lot.
        // Every paragraph on this page holds its own selection - GTK gives a
        // label one, and labels do not share - so five lines could be copied
        // only one at a time, which is the opposite of what somebody gathering
        // them for a bug report needs.
        //
        // GStreamer is asked for its numbers rather than its version string,
        // which begins with its own name and read as "GStreamer: GStreamer
        // 1.28.5".
        let (major, minor, micro, _) = gstreamer::version();
        body.append(&about_text(&format!(
            "TinePlayer: {}\nSystem: {} ({})\nGTK: {}.{}.{}\nGStreamer: {major}.{minor}.{micro}\nRenderer: {}",
            env!("CARGO_PKG_VERSION"),
            os_name(),
            std::env::consts::ARCH,
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version(),
            self.renderer_name(),
        )));

        // Where everything this copy keeps actually lives: the settings file,
        // the resume positions, the Jellyfin pairing, what the version check
        // remembers. It sat on the Clear Data row in Settings until now, which
        // named it after one of the six files in it - and put it behind a row
        // that goes insensitive when that one file is missing.
        //
        // Here instead, because this is the section about the installation
        // rather than about a setting, and because a bug report wants it for
        // the same reason it wants the version numbers above.
        //
        // Opened rather than printed. A path read off a television is a path
        // nobody is going to type, and the folder is the thing wanted anyway.
        if let Some(folder) = crate::config::positions_path().parent() {
            let folder = folder.to_path_buf();
            let link = about_link(
                &tr!("Settings and saved data are kept in"),
                &gtk::gio::File::for_path(&folder).uri(),
                &tr!("your user data folder"),
            );
            // Reported rather than swallowed: a link that does nothing looks
            // like a link that was pressed wrongly.
            link.connect_activate_link(move |_, _| {
                show_folder(&folder);
                glib::Propagation::Stop
            });
            body.append(&link);
        }
        body
    }

    /// Which of GTK's renderers is drawing this window.
    ///
    /// Read from the window rather than from `GSK_RENDERER`, which names only
    /// what was asked for: unset is the ordinary case, and a request GTK could
    /// not honour falls back to another without saying so.
    fn renderer_name(&self) -> String {
        self.window
            .renderer()
            .map(|renderer| renderer.type_().name().to_string())
            // Not translated: this sits beside the renderer's own
            // type name, which is a class name rather than words, and
            // the row exists to be read into a bug report.
            .unwrap_or_else(|| "not yet drawn".to_string())
    }

    /// The notices for everything TinePlayer is built from, in the
    /// application rather than only on a web page.
    ///
    /// Every package already carries THIRD-PARTY.md as a file, which is what
    /// the licenses actually ask for. This is about being able to read it: the
    /// machines this player is built for are televisions and HTPCs driven by a
    /// gamepad, where there may be no browser at all and opening one is not
    /// something a D-pad does well. The link on the About page stays for
    /// anyone who would rather read it on the web.
    ///
    /// Built into the binary rather than read from beside it, so it is there
    /// whichever way TinePlayer was installed, and cannot be separated from
    /// the thing it describes.
    pub(super) fn show_notices(self: &Rc<Self>) {
        let px = |base: f64| (base * self.scale.get()).round() as i32;
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(20.0))
            .margin_top(px(28.0))
            .margin_bottom(px(28.0))
            .margin_start(px(32.0))
            .margin_end(px(32.0))
            .build();
        page.append(&heading_label(&tr!("Third-Party Notices")));

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(10.0))
            .build();
        let mut blocks = notices_blocks(include_str!("../../THIRD-PARTY.md"));
        // The file's own title, which the dialog says above this already. Read
        // as a file it belongs there; read here it is the same three words
        // twice, an inch apart.
        if matches!(blocks.first(), Some(Notice::Heading(_))) {
            blocks.remove(0);
        }
        let last = blocks.len().saturating_sub(1);
        for (index, block) in blocks.into_iter().enumerate() {
            let widget = match block {
                Notice::Heading(text) => about_heading(&text),
                Notice::Text(text) => about_text(&text),
            };
            // The closing line is a remark about the list rather than part of
            // it, and sitting one row's gap under two hundred crates it read
            // as another entry. A heading would be too much for one sentence;
            // the space is enough to separate it.
            if index == last {
                widget.set_margin_top(px(24.0));
            }
            body.append(&widget);
        }

        // Two hundred crates will not fit on any screen, so the dialog keeps
        // to a share of the window and the list scrolls inside it.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&body)
            .build();
        scroller.set_focusable(false);
        let height = (self.window.height() as f64 * NOTICES_SHARE).round() as i32;
        scroller.set_max_content_height(height.max(px(320.0)));
        scroller.set_propagate_natural_height(true);
        // And a width, which the height alone does not give: the text wraps,
        // so its natural width is whatever the longest unwrapped line happens
        // to be, and left to that the dialog spans the window. A line of prose
        // is read at a comfortable length or not at all.
        scroller.set_propagate_natural_width(true);
        scroller.set_max_content_width(px(NOTICES_WIDTH));
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
                app.show_settings();
            });
        }

        // Over the settings rather than in place of them: the notices are
        // something looked up and dismissed, and the screen they were reached
        // from is still where the viewer was.
        //
        // Nothing to select, so up and down scroll instead - the arrangement
        // the About text uses beside it.
        self.set_nav(None, std::slice::from_ref(&close), &[]);
        *self.about_scroll.borrow_mut() = Some(scroller.vadjustment());
        *self.copy_root.borrow_mut() = Some(body.upcast());
        *self.screen.borrow_mut() = Screen::Notices;
        self.window.set_child(Some(&self.modal(&page)));
        close.grab_focus();
    }

    /// Copies whatever is selected on the screen being shown, and says
    /// whether there was anything. Each paragraph is its own label and holds
    /// its own selection, so the first one holding any is the one that was
    /// dragged across.
    ///
    /// Done by hand because GTK delivers Ctrl+C to whichever widget has
    /// focus, and selectable text here deliberately never takes focus: it
    /// would put a caret in the middle of a screen driven by arrow keys.
    pub(super) fn copy_selection(&self) -> bool {
        let root = self.copy_root.borrow().clone();
        let Some(root) = root else { return false };
        self.copy_from(&root)
    }

    fn copy_from(&self, widget: &gtk::Widget) -> bool {
        if let Some(label) = widget.downcast_ref::<gtk::Label>()
            && let Some((from, to)) = label.selection_bounds()
        {
            let selected: String = label
                .text()
                .chars()
                .skip(from as usize)
                .take((to - from) as usize)
                .collect();
            self.window.clipboard().set_text(&selected);
            return true;
        }
        let mut next = widget.first_child();
        while let Some(child) = next {
            if self.copy_from(&child) {
                return true;
            }
            next = child.next_sibling();
        }
        false
    }

    /// Moves the About page when there is nothing to select on it. Says
    /// whether it did, so ordinary navigation can carry on elsewhere.
    /// Whether what is on screen is a page of text with no rows to move
    /// through, so the arrows should scroll it instead.
    ///
    /// The About text no longer has a screen of its own - it is a block above
    /// the notices row in the settings pane - so this asks where the keyboard
    /// is as well as which screen it is. In the column of categories the
    /// arrows are moving between categories and must not scroll anything.
    fn reading_about(&self) -> bool {
        *self.screen.borrow() == Screen::Notices
            || (self.on_settings()
                && self.in_settings_pane.get()
                && self.settings_category.get() == Category::About)
    }

    pub(super) fn scroll_about(&self, delta: i32) -> bool {
        if !self.reading_about() {
            return false;
        }
        let Some(adjustment) = self.about_scroll.borrow().clone() else {
            return false;
        };
        // A third of a screenful a press: enough to make progress, little
        // enough to keep your place on the page.
        let step = adjustment.page_size() / 3.0;
        let moved = adjustment.value() + delta as f64 * step;
        adjustment.set_value(moved.clamp(adjustment.lower(), about_bottom(&adjustment)));
        true
    }

    /// The same for Home and End, on the pages with no rows to give them to.
    pub(super) fn scroll_about_edge(&self, end: bool) -> bool {
        if !self.reading_about() {
            return false;
        }
        let Some(adjustment) = self.about_scroll.borrow().clone() else {
            return false;
        };
        adjustment.set_value(if end {
            about_bottom(&adjustment)
        } else {
            adjustment.lower()
        });
        true
    }
}
