//! Talking to a Jellyfin server: its artwork, its remote commands, and what we report back.

use super::*;

impl App {
    /// Puts artwork into the page that is already on screen.
    ///
    /// Only the two widgets it belongs in are touched, so focus, the row
    /// somebody is on, and anything they have open all stay exactly as they
    /// were. This is what a picture arriving three seconds after the page did
    /// should cost: a picture appearing, and nothing else moving.
    pub(super) fn show_late_art(self: &Rc<Self>) {
        if let Some(backdrop) = self.backdrop_widget.borrow().as_ref()
            && let Some(texture) = self.backdrop_art.borrow().clone()
        {
            backdrop.set_texture(Some(texture));
            fade_in(backdrop);
        }

        // The poster is a picture where the placeholder was, so the frame's
        // child is replaced rather than a texture set: with no artwork the
        // frame holds a mark rather than an empty picture.
        let (Some(frame), Some(texture)) = (
            self.poster_frame.borrow().clone(),
            self.poster_art.borrow().clone(),
        ) else {
            return;
        };
        while let Some(child) = frame.first_child() {
            frame.remove(&child);
        }
        // **The frame was sized before there was a picture to measure**, so a
        // wide one - an episode still - would still be cropped into a slot
        // shaped for a poster, which is why fixing this at build time alone
        // changed nothing: the artwork arrives on a thread, after the page.
        // Sized again now its shape is known, by the same rule.
        let scale = self.scale.get();
        let height = self.poster_height(scale);
        let width = height * 2.0 / 3.0;
        frame.set_size_request(
            width.round() as i32,
            poster_frame_height(Some(&texture), width, height).round() as i32,
        );
        let picture = crate::artwork::Artwork::poster();
        picture.set_texture(Some(texture));
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        fade_in(&picture);
        frame.append(&picture);
    }

    /// The same for the series' poster under the file details, which arrives by
    /// the same route and just as late.
    pub(super) fn show_late_series_art(self: &Rc<Self>) {
        let (Some(frame), Some(texture)) = (
            self.series_frame.borrow().clone(),
            self.series_art.borrow().clone(),
        ) else {
            return;
        };
        // Already hung, which is what a page rebuilt after the picture landed
        // looks like - every trip in and out of a chooser does that.
        if frame.first_child().is_some() {
            return;
        }
        // Sized again now the picture's shape is known. Until it arrived the
        // frame stood at two by three, and anything else would have been
        // cropped to fit it.
        let width = f64::from(frame.width_request().max(1));
        frame.set_size_request(
            width.round() as i32,
            series_frame_height(Some(&texture), width).round() as i32,
        );
        let picture = series_picture(texture);
        fade_in(&picture);
        frame.append(&picture);
    }

    /// Fills the page in from the library, for a video that came from one.
    ///
    /// Only the fields Jellyfin actually answered: an empty overview or a
    /// missing year leaves whatever the container had, on the grounds that
    /// something is better than nothing and the library is not always fuller
    /// than the file. The title is not among them - that already comes through
    /// `launcher_title` at the head of the same chain everything else uses.
    pub(super) fn overlay_jellyfin_details(self: &Rc<Self>) {
        let Some(item) = self.jellyfin_item.borrow().clone() else {
            return;
        };
        {
            let mut details = self.details.borrow_mut();
            if !item.plot.is_empty() {
                details.plot = item.plot.clone();
            }
            if item.year.is_some() {
                details.year = item.year;
            }
            if !item.certificate.is_empty() {
                details.certificate = item.certificate.clone();
            }
            if item.rating.is_some() {
                details.rating = item.rating;
            }
            if !item.genres.is_empty() {
                details.genres = item.genres.clone();
            }
            if item.episode.is_some() {
                details.episode = item.episode;
            }
            if !item.aired.is_empty() {
                details.aired = item.aired.clone();
            }
            // What a local episode reads out of `<showtitle>`, arriving from
            // the library instead. Both end up in the same field, so the page
            // draws one thing and does not care which source answered.
            if !item.series_name.is_empty() {
                details.series_title = item.series_name.clone();
            }
            if !item.season_name.is_empty() {
                details.season_name = item.season_name.clone();
            }
            // The stream measures itself, so a runtime is only worth taking
            // where the container could not say.
            if details.duration_s <= 0.0
                && let Some(runtime) = item.runtime_ns
            {
                details.duration_s = runtime as f64 / 1e9;
            }
        }
        self.load_jellyfin_art(&item);
    }

    /// Fetches the poster and backdrop, and redraws when they land.
    ///
    /// Separately from the details, and after the page is already up, because
    /// these are the slow part - a backdrop is a picture from across the
    /// house. The page is perfectly good without them until they arrive, which
    /// is the same bargain artwork beside a file already makes.
    fn load_jellyfin_art(self: &Rc<Self>, item: &crate::jellyfin::Item) {
        let Some(client) = self.jellyfin.borrow().clone() else {
            return;
        };
        if item.poster_tag.is_none()
            && item.backdrop_tag.is_none()
            && item.series_poster_tag.is_none()
            && item.season_id.is_empty()
        {
            return;
        }

        let id = item.id.clone();
        let poster_tag = item.poster_tag.clone();
        // The picture that says which programme - and which run of it - this
        // episode belongs to. A different thing from the episode's own Primary
        // image, which is a still from the episode.
        //
        // The season is asked for first and without a tag, because the episode
        // does not carry one: the season is an item of its own and only it
        // knows. Answering without a tag hands back whatever is current, which
        // is what a caller that never saw one wants, and saves fetching the
        // season item merely to learn a cache key. Not every season has a
        // picture - "Specials" often has none - so the series' is the fallback,
        // and that tag the episode does carry.
        let season_id = Some(item.season_id.clone()).filter(|id| !id.is_empty());
        let series = item
            .series_poster_tag
            .clone()
            .filter(|_| !item.series_id.is_empty())
            .map(|tag| (item.series_id.clone(), tag));
        let backdrop_tag = item.backdrop_tag.clone();
        // Not always the same item: an episode's backdrop belongs to its
        // series, and a tag is only good against the item it came from.
        let backdrop_id = item.backdrop_item.clone();
        // The film these belong to, so a viewer who casts one and immediately
        // casts another does not get the first one's backdrop.
        let generation = self.art_generation.get();

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // Asked for at the size they are drawn rather than whole: a
            // library backdrop can be several megabytes untouched.
            let poster = poster_tag
                .and_then(|tag| client.image(&id, "Primary", &tag, 600).ok())
                .map(crate::metadata::Art::Embedded);
            let backdrop = backdrop_tag
                .and_then(|tag| client.image(&backdrop_id, "Backdrop/0", &tag, 1920).ok())
                .map(crate::metadata::Art::Embedded);
            // Small on the page, so asked for small: this one is drawn under
            // the file details rather than in the poster slot.
            let series = season_id
                .and_then(|id| client.image(&id, "Primary", "", 300).ok())
                .or_else(|| {
                    series.and_then(|(id, tag)| client.image(&id, "Primary", &tag, 300).ok())
                })
                .map(crate::metadata::Art::Embedded);
            let _ = sender.send((poster, backdrop, series));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            let (poster, backdrop, series) = match receiver.try_recv() {
                Ok(art) => art,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            // Another video was opened while these were coming down.
            if app.art_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            {
                let mut details = app.details.borrow_mut();
                if poster.is_some() {
                    details.poster = poster;
                }
                if backdrop.is_some() {
                    details.backdrop = backdrop;
                }
                if series.is_some() {
                    details.series_poster = series;
                }
            }
            app.start_art_load();
            glib::ControlFlow::Break
        });
    }

    /// Reaches the paired server, if there is one, and stays reachable.
    ///
    /// Everything here is allowed to fail quietly. A server that is off, a
    /// network that is out, a pairing that was revoked - none of them are
    /// reasons for a video player to complain on startup, and all of them are
    /// answered the same way: no cast target until it comes back.
    pub(super) fn start_jellyfin(self: &Rc<Self>) {
        let Some(pairing) = crate::jellyfin::load() else {
            return;
        };
        // What the settings pane reads. Set here as well as when that pane is
        // built, so the rows are right the first time it is opened.
        *self.jellyfin_pairing.borrow_mut() = Some(pairing.clone());
        let Some(client) = crate::jellyfin::Client::new(&pairing) else {
            // Paired with a server but signed out of it, which is where a 401
            // leaves things. The settings screen offers a new code.
            return;
        };

        // Off the main thread: this talks to a server that may be asleep, and
        // the interface has a menu to draw.
        //
        // **Its answer is acted on, not merely printed.** This is the first
        // call made with a stored token, so it is the first thing to know that
        // a pairing has been revoked - and until 2026-08-15 it logged
        // "Jellyfin no longer accepts this connection" and carried on, leaving
        // the settings screen claiming to be connected to a server that had
        // deleted this device. Reported by Scott, who had done exactly that.
        let announcing = client.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(announcing.announce());
        });
        {
            let app = self.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                match receiver.try_recv() {
                    Ok(Ok(())) => {}
                    // The pairing is gone. Everything else about this server is
                    // now wrong, including the socket that is being opened
                    // below, which signing out puts down.
                    Ok(Err(crate::jellyfin::Error::Unauthorized)) => app.jellyfin_signed_out(),
                    // A server that is off or asleep, which is ordinary and
                    // not a reason to throw the pairing away.
                    Ok(Err(e)) => log::error!("Jellyfin would not take our capabilities: {e}"),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                }
                glib::ControlFlow::Break
            });
        }
        *self.jellyfin.borrow_mut() = Some(client);

        let app = self.clone();
        let session = crate::jellyfin::connect(&pairing, move |command| {
            app.handle_jellyfin(command);
        });
        *self.jellyfin_session.borrow_mut() = session;
    }

    /// What a phone asked for.
    ///
    /// Playstate commands are the ones TinePlayer already has actions for, so
    /// they go straight to the same places the remote and the media keys use -
    /// there is no second way to pause.
    fn handle_jellyfin(self: &Rc<Self>, command: crate::jellyfin::Command) {
        use crate::jellyfin::Command;
        match command {
            Command::Play {
                item_id,
                position_ns,
            } => self.play_jellyfin(&item_id, position_ns),
            Command::Pause => {
                if self.is_playing() {
                    self.toggle_pause();
                    self.wake_controls();
                }
            }
            Command::Unpause => {
                if self.playback.borrow().is_some() && !self.is_playing() {
                    self.toggle_pause();
                    self.wake_controls();
                }
            }
            Command::PlayPause => {
                if self.playback.borrow().is_some() {
                    self.toggle_pause();
                    self.wake_controls();
                }
            }
            Command::Stop => {
                if self.playback.borrow().is_some() {
                    self.go_back();
                }
            }
            Command::Seek(position_ns) => {
                if let Some(playback) = self.playback.borrow().as_ref() {
                    playback.aim_at(gstreamer::ClockTime::from_nseconds(position_ns));
                    playback.commit_seek();
                }
                self.publish_now_playing();
            }
            // Everything below drives the controls rather than the pipeline,
            // which is what keeps one answer to each of these questions: the
            // remote moves the same all-outputs level and picks from the same
            // lists the person in the room does, and the strip is woken so what
            // it did is visible rather than mysterious.
            Command::SetVolume(level) => {
                if let Some(controls) = self.controls.borrow().clone() {
                    controls.main_to(level);
                }
                self.wake_controls();
            }
            Command::Mute | Command::Unmute | Command::ToggleMute => {
                if let Some(controls) = self.controls.borrow().clone() {
                    match command {
                        Command::Mute => controls.set_hushed(true),
                        Command::Unmute => controls.set_hushed(false),
                        _ => controls.toggle_hush(),
                    }
                }
                self.wake_controls();
            }
            Command::SetAudioStream(index) => {
                if let Some(row) = self.library_audio_row(index) {
                    self.choose_audio(Role::Primary.key(), row);
                    self.wake_controls();
                }
            }
            Command::SetSubtitleStream(index) => {
                if let Some(row) = self.library_subtitle_row(index) {
                    self.choose_subtitle(row);
                    self.wake_controls();
                }
            }
            // The pairing was revoked while we held it. Everything about this
            // server is now wrong, so it is put down rather than retried.
            Command::SignedOut => self.jellyfin_signed_out(),
        }
    }

    /// What a controller should show: the main level, the blanket silence, and
    /// what the first output and the subtitles are playing, in Jellyfin's
    /// numbering.
    ///
    /// Worked out here rather than remembered as it changes, because every
    /// answer already lives somewhere - and a second copy kept in step by hand
    /// is how a remote comes to show something the player is not doing.
    fn reported_sound(&self) -> crate::jellyfin::Sound {
        use crate::subtitles::SubtitleChoice;
        let item = self.jellyfin_item.borrow();
        let streams = item.as_ref().map(|item| &item.streams);

        let audio = match (
            streams,
            self.playback
                .borrow()
                .as_ref()
                .and_then(|playback| playback.playing_on(Role::Primary.key())),
        ) {
            (Some(streams), Some(crate::pipeline::Playing::Track(position))) => {
                streams.audio_index(position)
            }
            // A separate audio file, which the library has no number for.
            _ => None,
        };

        let subtitle = match self.subtitle.borrow().as_ref() {
            // Off is an answer, and the one a controller most needs told: it is
            // what its selector falls back to showing when it is told nothing.
            None => Some(-1),
            // A file on the server, which already carries Jellyfin's own number.
            Some(SubtitleChoice::Library(index)) => Some(*index as i32),
            Some(SubtitleChoice::Embedded(position)) => streams
                .and_then(|streams| streams.subtitle_index(*position))
                .map(|index| index as i32),
            // A file on this machine, which a cast video does not have.
            Some(_) => None,
        };

        crate::jellyfin::Sound {
            level: self.config.borrow().main_volume(),
            muted: self.hushed.get(),
            audio,
            subtitle,
        }
    }

    /// Which row of the first output's soundtrack list one of Jellyfin's stream
    /// numbers is.
    ///
    /// That list is the film's own tracks in order and nothing else - the first
    /// output has no "None" row - so a position among the embedded tracks is
    /// the row. `None` for a stream that is external or is not audio, which is
    /// a remote asking for something this list cannot offer rather than an
    /// error worth reporting.
    fn library_audio_row(&self, index: u32) -> Option<usize> {
        let item = self.jellyfin_item.borrow();
        let position = item.as_ref()?.streams.audio_position(index)?;
        Some(position as usize)
    }

    /// The same for the subtitle chooser, whose first row is Off and whose rest
    /// follow `subtitle_options` in order.
    ///
    /// Matched against the options themselves rather than counted, because that
    /// list holds two kinds of thing at once: streams inside the container,
    /// which Jellyfin numbers among everything else, and files beside it on the
    /// server, which carry Jellyfin's own number already. Counting would put
    /// one kind out of step with the other.
    fn library_subtitle_row(&self, index: Option<u32>) -> Option<usize> {
        use crate::subtitles::Subtitle;
        // Off is a row like any other, and the one a remote can always reach.
        let Some(index) = index else { return Some(0) };
        let embedded = self
            .jellyfin_item
            .borrow()
            .as_ref()
            .and_then(|item| item.streams.subtitle_position(index));
        let options = self.subtitle_options.borrow();
        let at = options.iter().position(|option| match option {
            Subtitle::Library { index: at, .. } => *at == index,
            Subtitle::Embedded { index: at, .. } => Some(*at) == embedded,
            _ => false,
        })?;
        Some(at + 1)
    }

    fn is_playing(&self) -> bool {
        self.playback
            .borrow()
            .as_ref()
            .is_some_and(|playback| playback.is_playing())
    }

    /// Resolves what was cast and opens it.
    ///
    /// The command carries an item id and nothing else - no address and,
    /// usually, no position - so the item is asked about before anything can
    /// be played. That happens on a worker thread, because it is a request to
    /// a server that may be across a house.
    fn play_jellyfin(self: &Rc<Self>, item_id: &str, position_ns: Option<u64>) {
        let Some(client) = self.jellyfin.borrow().clone() else {
            return;
        };
        let id = item_id.to_string();

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(client.item(&id));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            match result {
                Ok(mut item) => {
                    // A controller saying "play from here" outranks the
                    // library's own idea of where this viewer stopped.
                    if let Some(position) = position_ns {
                        item.resume_ns = Some(position).filter(|position| *position > 0);
                    }
                    app.open_jellyfin(item);
                }
                Err(crate::jellyfin::Error::Unauthorized) => app.jellyfin_signed_out(),
                Err(e) => log::error!("Jellyfin would not describe that video: {e}"),
            }
            glib::ControlFlow::Break
        });
    }

    /// Takes a resolved item and plays it.
    fn open_jellyfin(self: &Rc<Self>, item: crate::jellyfin::Item) {
        let Some(client) = self.jellyfin.borrow().clone() else {
            return;
        };
        let source = Source::parse(&client.stream_url(&item));
        // Set before the source is opened, because everything that reads a
        // title or a resume position during opening looks here for it. Kodi's
        // is cleared for the same reason: two launchers claiming the same
        // video would be one of them wrong.
        *self.kodi_item.borrow_mut() = None;

        // What the tracks are, from the library rather than by reading the
        // file. The server has already analysed it, and asking again over HTTP
        // is redundant work that sometimes cannot finish: a QuickTime file
        // with its index at the end has to be read from the front to be
        // probed, which for a four-gigabyte film is minutes. Playback itself
        // is unaffected - it seeks straight to the index - so the probe was
        // the only thing that could not cope.
        //
        // Verified on 2026-08-14 that the library's stream order matches the
        // probe's exactly, which is what makes this safe: tracks are chosen by
        // position, and a different order would silently play the wrong one.
        let media = item.streams.as_media(item.runtime_ns.unwrap_or_default());
        *self.jellyfin_item.borrow_mut() = Some(item);

        // Straight in, with no spinner: there is nothing to wait for now that
        // the tracks are already known.
        match self.apply_media(&source, media) {
            Ok(()) => self.show_menu(),
            Err(e) => {
                log::error!("Couldn't open {}: {e}", source.uri());
                self.show_source_error(&source, &e, false);
            }
        }
    }

    /// Puts down a pairing the server no longer honours.
    ///
    /// The token goes and the device identity stays, so connecting again
    /// replaces the existing device rather than leaving a trail of them. Said
    /// out loud, because a cast target that has quietly stopped being one is
    /// the failure nobody can diagnose from the sofa.
    fn jellyfin_signed_out(self: &Rc<Self>) {
        // Both halves of the connection find this out for themselves - the
        // capabilities call by its 401, the socket by its 403 - and either
        // alone has to be enough, since a server may refuse one and not the
        // other. So the second one to arrive says nothing and writes nothing
        // rather than repeating the message and the file write.
        if self.jellyfin.borrow().is_none() && self.jellyfin_session.borrow().is_none() {
            return;
        }
        *self.jellyfin.borrow_mut() = None;
        *self.jellyfin_session.borrow_mut() = None;
        if let Some(mut pairing) = crate::jellyfin::load() {
            pairing.sign_out();
            if let Err(e) = crate::jellyfin::save(&pairing) {
                log::error!("Couldn't forget the Jellyfin token: {e}");
            }
            *self.jellyfin_pairing.borrow_mut() = Some(pairing);
        }
        log::error!("Jellyfin no longer accepts this connection. Connect to it again to cast.");
        // Redrawn only where it is being looked at. A pairing can be revoked
        // at any moment, and rebuilding a screen under somebody who is part
        // way through choosing a soundtrack would be a worse interruption than
        // the one being reported.
        if self.showing_jellyfin_pane() {
            self.show_settings();
        }
        // And the page shown when nothing is loaded, which offers a Connect
        // button only while there is nothing to disconnect from - so a token
        // revoked between that page being drawn and the server saying so left
        // it with no way to connect until something else redrew it. Safe to
        // rebuild only here: with no video there is nothing in hand to
        // interrupt, which is not true of the media page.
        if *self.screen.borrow() == Screen::Menu && self.file.borrow().is_none() {
            self.show_menu();
        }
    }

    /// Tells Jellyfin where playback has reached.
    ///
    /// Only for a video that came from there: a film opened from disk is
    /// nothing to do with the library, and reporting it would put a position
    /// against an item nobody watched.
    pub(super) fn report_to_jellyfin(&self, moment: JellyfinMoment) {
        let (Some(client), Some(id)) = (
            self.jellyfin.borrow().clone(),
            self.jellyfin_item
                .borrow()
                .as_ref()
                .map(|item| item.id.clone()),
        ) else {
            return;
        };
        let position = self
            .playback
            .borrow()
            .as_ref()
            .and_then(|playback| playback.position())
            .map(|position| position.nseconds())
            .unwrap_or(0);
        let paused = !self.is_playing();
        let sound = self.reported_sound();

        // A new name for the viewing when it starts, and the same one after.
        if moment == JellyfinMoment::Started {
            *self.jellyfin_play_session.borrow_mut() = crate::jellyfin::Client::new_play_session();
        }
        let play_session = self.jellyfin_play_session.borrow().clone();
        if play_session.is_empty() {
            // Nothing was ever started, so there is no viewing to report on.
            return;
        }

        // On a thread, because the server may be slow and this happens while a
        // film is playing. Nothing waits on the answer.
        std::thread::spawn(move || {
            let result = match moment {
                JellyfinMoment::Started => client.started(&id, &play_session, position, sound),
                JellyfinMoment::Progress => {
                    client.progress(&id, &play_session, position, paused, sound)
                }
                JellyfinMoment::Stopped => client.stopped(&id, &play_session, position),
            };
            if let Err(e) = result {
                log::error!("Jellyfin would not take the position: {e}");
            }
        });
    }
}
