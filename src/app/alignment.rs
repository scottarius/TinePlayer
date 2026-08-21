//! Measuring a separate audio file against the video's own sound, and applying what it finds.

use super::*;

impl App {
    /// The frame the three alignment steps share.
    ///
    /// One panel carrying all three in turn, rather than three screens: it is
    /// one errand, and the film it belongs to should stay visible behind it
    /// throughout. An overlay rather than a real modal window, for the reason
    /// the browser is one - a `transient_for` window takes the pointer but not
    /// the keyboard or the gamepad, both of which are driven from the main
    /// window and would carry on working the menu hidden behind it.
    fn align_page(&self, hint: &str) -> gtk::Box {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(20)
            // Centered and no taller than its contents, so the panel is the
            // size of the question rather than the size of the window.
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(32)
            .margin_bottom(32)
            .margin_start(44)
            .margin_end(44)
            .build();
        // The floor. Without it the panel shrinks around whatever the shortest
        // step has on it, and the three read as three differently sized
        // windows rather than one panel changing what it says.
        page.set_size_request((ALIGN_PANEL_MIN * self.scale.get()).round() as i32, -1);

        let heading = heading_label(&tr!("Sync Audio"));
        heading.set_halign(gtk::Align::Center);
        page.append(&heading);

        let hint = gtk::Label::builder()
            .label(hint)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            // The ceiling, and with the two set alike the floor as well. A
            // GtkBox has no maximum width, so the cap has to sit on the text
            // that would otherwise push it wide - and asking for the same
            // measure as a minimum is what makes all three steps come out the
            // same width instead of each shrinking to fit its own sentence.
            // In characters rather than pixels because that is what wraps, and
            // it holds at every interface scale without being multiplied.
            .width_chars(ALIGN_PANEL_CHARS)
            .max_width_chars(ALIGN_PANEL_CHARS)
            .css_classes(["tp-hint"])
            .build();
        page.append(&hint);
        page
    }

    /// Step one: which track inside the video to measure the audio file
    /// against.
    ///
    /// Asked rather than inferred, so the viewer can point it at the original
    /// soundtrack when the automatic pick would have taken a dub. It arrives
    /// with a sensible one already selected, so the common answer is a single
    /// press of Next.
    pub(super) fn show_align(self: &Rc<Self>, role: Role) {
        // Nothing to align without both halves of the pairing.
        let tracks = self.tracks.borrow().clone();
        if self.file_for(role).borrow().is_none() || tracks.is_empty() {
            return;
        }

        let page = self.align_page(
            "Choose a reference audio track to align the external audio file with. \
             Usually the original language, or a language that matches the audio \
             description.",
        );

        let (scroller, list) = scrolling_list();
        name_it(&list, &tr!("Reference track"));
        // Only as tall as the tracks need, up to a few rows. A list left to
        // expand makes the panel the height of the window whether it holds one
        // track or twelve, which is the opposite of what a short question wants.
        scroller.set_vexpand(false);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height((240.0 * self.scale.get()).round() as i32);
        page.append(&scroller);
        for track in &tracks {
            let text = describe_audio_track(track);
            let row = chooser_row(&text);
            row.set_xalign(0.5);
            // Held to the same measure as the body text. A track carrying a
            // long title would otherwise widen the whole panel, and it already
            // ellipsizes rather than wrapping.
            row.set_max_width_chars(ALIGN_PANEL_CHARS);
            append_named(&list, &row, &text);
        }

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        let next = gtk::Button::with_label(&tr!("Next"));
        next.add_css_class("tp-button");
        next.add_css_class("tp-action");
        buttons.append(&cancel);
        buttons.append(&next);
        page.append(&buttons);

        // What the list is pointing at when Next is pressed, and what
        // activating a row means, are the same thing: the row is the choice.
        let start = {
            let app = self.clone();
            let list = list.clone();
            let tracks = tracks.clone();
            move || {
                let index = list.selected_row().map(|row| row.index()).unwrap_or(0);
                let Some(track) = tracks.get(index.max(0) as usize) else {
                    return;
                };
                app.sounds.borrow().click();
                app.show_align_progress(role, track.index);
            }
        };
        {
            let start = start.clone();
            list.connect_row_activated(move |_, _| start());
        }
        next.connect_clicked(move |_| start());
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.wire_navigation(&list, &[], &[cancel.clone(), next.clone()]);
        self.remember_origin();
        *self.screen.borrow_mut() = Screen::AlignChoose;
        self.window.set_child(Some(&self.modal(&page)));

        // The first track that is not a description, because a description is
        // the thing being lined up rather than the thing to line it up with -
        // it correlates against itself perfectly and says nothing. Falls back
        // to the first track when description is all the file has.
        let opening = tracks
            .iter()
            .position(|track| !track.is_described())
            .unwrap_or(0);
        if let Some(row) = list.row_at_index(opening as i32) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Step two: the measuring, which happens on a thread.
    ///
    /// Three sixty-second windows out of each of two files is around twelve
    /// seconds on a desktop and several times that on a Pi, so it cannot run
    /// on the main loop: the window would stop redrawing and the interface
    /// would read as having crashed. The thread reports through a channel this
    /// polls, which is how the rest of the application already waits on work -
    /// everything the answer touches is `Rc` and has to be applied here.
    fn show_align_progress(self: &Rc<Self>, role: Role, reference: u32) {
        let (video, audio) = {
            let file = self.file.borrow().clone();
            let audio = self.file_for(role).borrow().clone();
            match (file, audio) {
                (Some(video), Some(audio)) => (video, audio),
                _ => return,
            }
        };

        let page = self.align_page(&tr!(
            "Analyzing audio to align the tracks. This may take a few moments."
        ));

        let bar = gtk::ProgressBar::new();
        bar.add_css_class("tp-align-bar");
        page.append(&bar);

        let status = gtk::Label::builder()
            .label("0%")
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&status);

        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        cancel.add_css_class("tp-button");
        cancel.set_halign(gtk::Align::Center);
        page.append(&cancel);
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.remember_origin();
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&cancel);
        *self.screen.borrow_mut() = Screen::AlignProgress;
        self.window.set_child(Some(&self.modal(&page)));
        cancel.grab_focus();

        let (sender, receiver) = std::sync::mpsc::channel();
        let duration = self.duration_s.get();
        let (video_uri, audio_uri) = (video.uri(), audio.uri());
        std::thread::spawn(move || {
            let progress = sender.clone();
            let verdict = crate::align::align(
                &video_uri,
                &audio_uri,
                duration,
                reference,
                // A failed send means nobody is listening any more, which is
                // what cancelling looks like from here. There is no way to
                // stop a decode part-way, so the thread runs to the end and
                // its answer is dropped.
                move |done| {
                    let _ = progress.send(Step::Window(done));
                },
            );
            let _ = sender.send(Step::Done(verdict));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            // Cancelled, or moved on some other way, while it was working.
            if *app.screen.borrow() != Screen::AlignProgress {
                return glib::ControlFlow::Break;
            }
            loop {
                match receiver.try_recv() {
                    Ok(Step::Window(done)) => {
                        // Three steps rather than a smooth climb: a window is
                        // one decode and cannot report its own progress, so
                        // anything finer would be invented.
                        let fraction = done as f64 / crate::align::WINDOWS as f64;
                        bar.set_fraction(fraction);
                        status.set_label(&format!("{:.0}%", fraction * 100.0));
                    }
                    Ok(Step::Done(verdict)) => {
                        app.show_align_result(role, verdict);
                        return glib::ControlFlow::Break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    // The thread is gone without an answer, which leaves
                    // nothing to report and no reason to keep looking.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        return glib::ControlFlow::Break;
                    }
                }
            }
        });
    }

    /// Step three: what it found, and applying it when there is anything to
    /// apply.
    ///
    /// A hidden baseline must never hide a wrong answer, so every outcome is
    /// said out loud. The two that change nothing say so plainly and point at
    /// the sync slider, which is what someone is left with when measuring
    /// cannot help.
    fn show_align_result(self: &Rc<Self>, role: Role, verdict: crate::align::Verdict) {
        use crate::align::Verdict;

        // Never named by output, because the answer is not one: it belongs to
        // this video and this audio file, and applies wherever that file is
        // played.
        let (hint, retry) = match verdict {
            Verdict::Offset { millis, .. } => {
                self.apply_alignment(role, millis);
                let rounded = millis.round();
                // The number is rounded before it is handed over rather than
                // inside the placeholder: `fill` substitutes by name and has
                // no format specifiers, so `{ms:.0}` would reach the screen
                // written out.
                let shift = if rounded > 0.0 {
                    tr!(
                        "The audio file runs {ms}ms late, and has been adjusted to sync with the video.",
                        ms = format!("{rounded:.0}")
                    )
                    .into_owned()
                } else if rounded < 0.0 {
                    tr!(
                        "The audio file runs {ms}ms early, and has been adjusted to sync with the video.",
                        ms = format!("{:.0}", -rounded)
                    )
                    .into_owned()
                } else {
                    tr!("The audio file is already in sync with the video, no adjustment needed.")
                        .into_owned()
                };
                (shift, false)
            }
            // A rate difference is a slope rather than a shift, so no single
            // offset fixes it and averaging one would be a guess that drifts.
            Verdict::RateMismatch { .. } => (
                "The audio file runs at a different speed than the video and cannot be \
                 automatically adjusted."
                    .to_string(),
                true,
            ),
            Verdict::Unsure => (
                tr!("The audio file could not be matched with the video.").into_owned(),
                true,
            ),
        };

        let page = self.align_page(&hint);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        // Offered only where it could help. Trying another reference track is
        // the answer when the one measured against was a dub and the separate
        // recording was made from the original.
        let again = gtk::Button::with_label(&tr!("Try another reference track"));
        again.add_css_class("tp-button");
        again.add_css_class("tp-action");

        // What the second button means depends on what happened. Where the
        // measurement worked there is nothing to do but accept it; where it
        // did not, the useful thing is to measure again against a different
        // track, and this button becomes the way out beside it.
        let done = gtk::Button::with_label(&match retry {
            true => tr!("Cancel"),
            false => tr!("Finish"),
        });
        done.add_css_class("tp-button");
        if !retry {
            done.add_css_class("tp-action");
        }
        // Cancel first, then the action, which is the order every other pair
        // in the application sits in.
        buttons.append(&done);
        if retry {
            buttons.append(&again);
        }
        page.append(&buttons);

        {
            let app = self.clone();
            again.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_align(role);
            });
        }
        {
            let app = self.clone();
            done.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.go_back();
            });
        }

        self.remember_origin();
        // In the order they now sit, so Tab walks the row left to right.
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&done);
        if retry {
            self.add_nav_stop(&again);
        }
        *self.screen.borrow_mut() = Screen::AlignResult;
        self.window.set_child(Some(&self.modal(&page)));
        // Whichever button is the action here: measuring again where that is
        // still worth doing, and accepting the answer where it is not.
        match retry {
            true => again.grab_focus(),
            false => done.grab_focus(),
        };
    }

    /// Writes an alignment down and puts it into force.
    ///
    /// Stored against the two paths together, so the same pairing never pays
    /// for the measuring twice, and read straight back rather than set here -
    /// `load_baselines` owns the sign convention, and two places deciding it
    /// would eventually disagree.
    fn apply_alignment(&self, role: Role, millis: f64) {
        let stored = {
            let file = self.file_for(role).borrow();
            file.as_ref()
                .and_then(Source::local)
                .map(|path| path.to_path_buf())
        };
        if let Some((key, path)) = self.storage_key().zip(stored) {
            crate::config::save_alignment(&key, &path, Some(millis));
        }
        self.load_baselines();
    }
}
