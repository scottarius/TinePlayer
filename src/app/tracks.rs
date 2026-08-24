//! Choosing which audio track and which subtitle each output carries.

use super::*;

impl App {
    /// Turns subtitles on or off for the playback in progress, and brings the
    /// strip up so the change is visible: the letters dim or light, which is
    /// the only confirmation when the moment has no subtitle to draw anyway.
    pub(super) fn toggle_subtitles(&self) {
        let Some(playback) = self.playback.borrow().clone() else {
            return;
        };
        let showing = playback.toggle_subtitles();
        self.subtitles_hidden.set(!showing);
        self.push_subtitle_state();
        self.wake_controls();
    }

    /// Steps one output to the next audio track in the file, on `A` for the
    /// primary and `S` for the secondary.
    ///
    /// Ahead of the chooser rather than instead of it: switching live is
    /// proven, and this makes it reachable while the rest - a menu per output,
    /// and the branch regrouping that two outputs on one track needs - is
    /// built. The reason it says nothing on screen is that there is nowhere
    /// yet to say it; the chooser is where a track name belongs.
    pub(super) fn cycle_audio(&self, role: &str) {
        let Some(playback) = self.playback.borrow().clone() else {
            return;
        };
        if let Err(reason) = playback.cycle_audio(role) {
            log::error!("Cannot step the {role} audio: {reason}");
        }
        self.wake_controls();
    }

    /// The chooser's rows and which of them is in force: Off, then everything
    /// the video offers, in the order the media page lists them.
    ///
    /// Browsing for a file is deliberately not among them, though the media
    /// page's version of this list ends with it. That opens a screen of its
    /// own, and going looking on disk belongs to the page you choose from
    /// before pressing play rather than to a list laid over a running film.
    ///
    /// "Off" rather than the page's "None", because on the strip it is the
    /// same state the icon and the toggle already call off.
    fn subtitle_entries(&self) -> (Vec<String>, Option<usize>) {
        let mut entries = vec!["Off".to_string()];
        let chosen = self.subtitle.borrow().clone();
        // Off unless something matches, which is also the answer when a
        // remembered choice names a subtitle this video does not have.
        let mut current = Some(0);
        for (position, option) in self.subtitle_options.borrow().iter().enumerate() {
            if chosen.as_ref() == Some(&option.choice()) {
                current = Some(position + 1);
            }
            entries.push(crate::subtitles::row_native(option));
        }
        (entries, current)
    }

    /// The rows one output's soundtrack chooser offers while the film plays:
    /// what each says, and what picking it would play.
    ///
    /// The film's own tracks, with "None" first for the second output only:
    /// playing nothing on the second is a legitimate choice in a way it is not
    /// on the first, where it would mean a film with no sound at all.
    ///
    /// **Only what the pipeline was built with**, which is the film's tracks
    /// and the separate audio files the two outputs were given. Every file
    /// sitting beside the video is on the media page - see the track chooser in
    /// [`Self::chooser_entries`] - but one nobody chose before pressing play
    /// has no decoder in this pipeline, and none can be added to a running one:
    /// its chain would start at the beginning of the file rather than where the
    /// film is. Offering it would be a row that silently did nothing, since an
    /// output asked for something nothing is carrying simply keeps what it has.
    ///
    /// Both outputs' files, not just this one's. Two people watching with two
    /// separate soundtracks can each reach the other's, which costs nothing -
    /// both are already decoding - and is the same list on both, which is one
    /// less thing to explain.
    ///
    /// The rows stay after an output moves off a file, which is what makes them
    /// the way back: they say what this playback was given, not only what is
    /// being heard this second.
    ///
    /// Browsing for a *new* file is deliberately not here, though the media
    /// page's version of this list ends with it. That opens a screen of its
    /// own, and going looking on disk belongs to the page you choose from
    /// before pressing play - the same rule the subtitle chooser follows.
    ///
    /// The files go last for a second reason: Jellyfin's `SetAudioStreamIndex`
    /// arrives here as a row number counted through the embedded tracks alone,
    /// so a row added above them would shift every one of them.
    fn audio_rows(&self, role: Role) -> Vec<(String, Option<Playing>)> {
        let mut rows = Vec::new();
        if role == Role::Secondary {
            rows.push((trc!("audio track", "None").into_owned(), None));
        }
        for track in self.tracks.borrow().iter() {
            rows.push((
                describe_audio_track(track),
                Some(Playing::Track(track.index)),
            ));
        }
        // Named as they are on the media page's list, with the heading over
        // them saying once what each row used to begin by saying - see
        // `chooser_entries`.
        for file in self.attached_files() {
            rows.push((self.label_for_file(&file), Some(Playing::File(file.uri()))));
        }
        rows
    }

    /// The separate audio files this playback was built with: what the two
    /// outputs were given, deduplicated, which is exactly the set the pipeline
    /// attached a decoder for and so exactly the set an output can be moved to
    /// without stopping the film.
    fn attached_files(&self) -> Vec<Source> {
        let mut files: Vec<Source> = Vec::new();
        for role in [Role::Primary, Role::Secondary] {
            if let Some(file) = self.file_for(role).borrow().clone()
                && !files.contains(&file)
            {
                files.push(file);
            }
        }
        files
    }

    /// One output's soundtrack list as the strip wants it: the words, and
    /// which row it is playing.
    ///
    /// Takes the playback rather than reading it off `self`, because playback
    /// starting has not put it into its cell yet - the same reason
    /// [`Self::show_subtitle_state`] takes one. Reading `self` here marked the
    /// first row of both lists on every film, since with no playback to ask,
    /// nothing matched what was playing.
    fn audio_entries(
        &self,
        playback: &Playback,
        role: Role,
    ) -> (Vec<String>, Option<usize>, Option<usize>) {
        let playing = playback.playing_on(role.key());
        let rows = self.audio_rows(role);
        // Nothing is marked until a row matches, which on the first output is
        // the honest answer when it is playing nothing: it has no "None" row.
        let current = rows.iter().position(|(_, row)| *row == playing);
        // Where the separate files begin, for the heading over them - asked of
        // the rows rather than counted out again from the tracks and the
        // "None" row, which is the arithmetic `choose_audio` stopped doing for
        // the reason its own note gives.
        let files = rows
            .iter()
            .position(|(_, row)| matches!(row, Some(Playing::File(_))));
        (
            rows.into_iter().map(|(label, _)| label).collect(),
            current,
            files,
        )
    }

    /// Fills both outputs' menus with what this video offers.
    pub(super) fn push_audio_entries(&self, playback: &Playback, controls: &Rc<Controls>) {
        for (index, role) in [Role::Primary, Role::Secondary].into_iter().enumerate() {
            let (entries, current, files) = self.audio_entries(playback, role);
            controls.set_audio_entries(index, &entries, current, files);
        }
    }

    /// Puts one output onto the soundtrack at `at` in its own list, without
    /// stopping the film.
    ///
    /// Asks [`Self::audio_rows`] what that row stands for rather than counting
    /// the list out a second time. The two used to agree by arithmetic - an
    /// offset for the "None" row, and the file at one past the last track -
    /// which is two descriptions of one list, free to disagree the moment
    /// either grows a row.
    pub(super) fn choose_audio(self: &Rc<Self>, role: &str, at: usize) {
        let Some(playback) = self.playback.borrow().clone() else {
            return;
        };
        let which = if role == Role::Secondary.key() {
            Role::Secondary
        } else {
            Role::Primary
        };
        // A row from a list since redrawn stands for nothing, and is not a
        // reason to change what somebody is listening to.
        let Some((_, wanted)) = self.audio_rows(which).into_iter().nth(at) else {
            return;
        };
        if let Err(reason) = playback.set_audio(role, wanted) {
            log::error!("Cannot change the {role} soundtrack: {reason}");
        }
        // After the switch, not before: what the sink should be held back by
        // depends on what it is now playing, and only the routing knows that.
        self.push_offset(&playback, role);
        // Same reason, and the subtitle preference is written in terms of what
        // the outputs are playing - so it is asked again now that has moved.
        self.follow_audio_with_subtitle(which);
        if let Some(controls) = self.controls.borrow().clone() {
            self.push_audio_entries(&playback, &controls);
        }
    }

    /// Tells the strip what subtitles are doing and what there is to choose
    /// from, which is one answer in three places: whether the icon can be
    /// worked at all, whether it is lit, and what the chooser lists.
    ///
    /// Takes both rather than reading them off `self`, because playback
    /// starting has not put either into its cell yet.
    pub(super) fn show_subtitle_state(&self, playback: &Playback, controls: &Rc<Controls>) {
        // What the video offers, not what is attached. The icon opens a list
        // that includes turning subtitles on, so a film started with them off
        // has to be able to reach it - which asking whether anything is
        // attached would refuse.
        let offers = !self.subtitle_options.borrow().is_empty() || playback.has_subtitles();
        // What has been chosen, not what the pipeline has got to yet.
        //
        // A switch takes a moment to arrive, and the overlay is deliberately
        // blank until it does - see `Playback::set_subtitle`. An icon that
        // read the pipeline therefore dimmed at the start of every switch and
        // stayed dim, because nothing comes back to ask again once the
        // subtitle lands. The choice is the honest answer to what the icon is
        // saying: subtitles are on, and one is on its way.
        let showing = self.subtitle.borrow().is_some() && !self.subtitles_hidden.get();
        controls.set_subtitles(offers, showing);
        let (entries, current) = self.subtitle_entries();
        controls.set_subtitle_entries(&entries, current);
    }

    /// The same, for everywhere that can simply ask what is playing.
    fn push_subtitle_state(&self) {
        let playback = self.playback.borrow().clone();
        let controls = self.controls.borrow().clone();
        if let (Some(playback), Some(controls)) = (playback, controls) {
            self.show_subtitle_state(&playback, &controls);
        }
    }

    /// Takes a row from the chooser and puts it into the film already running.
    ///
    /// Row zero is Off; the rest follow `subtitle_options` in order, which is
    /// the order [`Self::subtitle_entries`] built them in. The choice is
    /// remembered as well as applied, the same as choosing one from the media
    /// page: it is the same decision, made later.
    pub(super) fn choose_subtitle(self: &Rc<Self>, entry: usize) {
        self.apply_subtitle(entry, Chose::ByHand);
    }

    /// The same, plus who asked for it.
    ///
    /// A person choosing a subtitle has settled the question: nothing should
    /// overrule it afterwards. The preference following a soundtrack change
    /// has not, and must not overrule a person - so the two go through one
    /// path and differ only in what they leave behind. See
    /// [`Self::follow_audio_with_subtitle`].
    fn apply_subtitle(self: &Rc<Self>, entry: usize, how: Chose) {
        let playback = self.playback.borrow().clone();
        let file = self.file.borrow().clone();
        let (Some(playback), Some(file)) = (playback, file) else {
            return;
        };
        let picked = match entry.checked_sub(1) {
            None => None,
            Some(index) => match self.subtitle_options.borrow().get(index) {
                Some(option) => Some(option.choice()),
                // A list that changed under the press. Nothing to apply, and
                // the mark stays where it was.
                None => return,
            },
        };

        // Already what is playing, and already showing it. Nothing to do, and
        // doing it anyway would rebuild the subtitle chain to arrive back
        // where it started - a blank second in the middle of a film for no
        // reason. This is also what makes pressing straight through the
        // chooser a way of closing it, since it opens on this very row.
        //
        // The second half is not redundant: picking the subtitle that is
        // already chosen but switched off is how it is asked for again.
        //
        // Asked of what has been chosen rather than of the pipeline, for the
        // reason `show_subtitle_state` gives - mid-switch the pipeline is
        // deliberately showing nothing, and taking that at face value would
        // make every second press of the same row a needless switch.
        if picked == *self.subtitle.borrow() && self.subtitles_hidden.get() == (entry == 0) {
            return;
        }

        // Located here for the reason it is at the start of playback: finding
        // one can need the server address and access token, which are ours to
        // know and the pipeline's to be kept out of.
        let located = match self.locate_subtitle(&file, picked.as_ref()) {
            Ok(located) => located,
            // The same answer playback gives when a subtitle cannot be found
            // as a film opens: it gives up the subtitle and not the film.
            // Nothing is recorded either, so the mark stays on whatever is
            // still playing - which is what says the choice did not take.
            Err(e) => {
                log::error!("{e}");
                return;
            }
        };
        if let Err(e) = playback.set_subtitle(located.as_ref()) {
            log::error!("{e}");
            return;
        }

        *self.subtitle.borrow_mut() = picked;
        // Choosing one is asking to see it, whatever the toggle was doing for
        // the last. Off is the exception, being the toggle said deliberately.
        self.subtitles_hidden.set(entry == 0);
        // Only a person settles the question. Left false by the automatic
        // follow, so a later soundtrack change is still free to move it.
        if how == Chose::ByHand {
            self.subtitle_by_hand.set(true);
        }
        self.remember_tracks();
        self.push_subtitle_state();
        self.wake_controls();
    }

    /// Re-runs the subtitle preference after the soundtrack on `changed` moved,
    /// where that preference depends on what `changed` is now playing.
    ///
    /// The preference is written in terms of the outputs - "forced subtitles
    /// matching the first output's language" - so the answer it gives is only
    /// as current as the soundtracks it was asked about. Change what an output
    /// is playing and the answer can be stale without anything having gone
    /// wrong, which is what this fixes.
    ///
    /// Three things hold it back, and each is a case that would otherwise be a
    /// bug rather than a feature:
    ///
    /// - **A subtitle somebody chose is left alone.** Overruling a deliberate
    ///   choice because the soundtrack moved is worse than being out of date.
    /// - **A fixed-language preference is not about the outputs at all**, so
    ///   nothing an output does can change its answer.
    /// - **The full modes name one output and never fall back**, so only that
    ///   output matters. The forced modes prefer one output but will take the
    ///   other, so for those either output can change the answer - which is
    ///   why the test is on the mode rather than on the role alone.
    fn follow_audio_with_subtitle(self: &Rc<Self>, changed: Role) {
        if self.subtitle_by_hand.get() {
            return;
        }
        let (kind, place) = {
            let config = self.config.borrow();
            (
                config
                    .subtitle_kind
                    .clone()
                    .unwrap_or_else(|| crate::subtitles::DEFAULT_KIND.to_string()),
                config
                    .subtitle_language
                    .clone()
                    .unwrap_or_else(|| crate::subtitles::DEFAULT_PLACE.to_string()),
            )
        };
        let prefer = crate::subtitles::Wanted::parse(&kind);
        if prefer == crate::subtitles::Wanted::None
            || !crate::subtitles::follows_output(&place, matches!(changed, Role::Secondary))
        {
            return;
        }

        let language_of = |index: Option<u32>| {
            index.and_then(|index| {
                self.tracks
                    .borrow()
                    .iter()
                    .find(|track| track.index == index)
                    .map(|track| track.language.clone())
            })
        };
        let primary = language_of(*self.primary_track.borrow());
        let secondary = language_of(*self.secondary_track.borrow());

        let wanted = crate::subtitles::automatic(
            &prefer,
            &place,
            &self.subtitle_options.borrow(),
            primary.as_deref(),
            secondary.as_deref(),
        );
        if wanted == *self.subtitle.borrow() {
            return;
        }

        // Back to a row number, because that is what applying one takes: row
        // zero is Off, and the rest follow `subtitle_options` in order.
        let entry = match &wanted {
            None => 0,
            Some(choice) => {
                let found = self
                    .subtitle_options
                    .borrow()
                    .iter()
                    .position(|option| option.choice() == *choice);
                match found {
                    Some(index) => index + 1,
                    // The preference named something no longer on offer, which
                    // is not a reason to take away what is playing.
                    None => return,
                }
            }
        };
        self.apply_subtitle(entry, Chose::Automatically);
    }

    /// Starts a hold on the left face button. Nothing happens yet: what the
    /// press meant is only known when it is let go, or when it has been down
    /// long enough to have meant the other thing.
    pub(super) fn press_subtitles(self: &Rc<Self>) {
        if self.subtitles_holding.replace(true) {
            return;
        }
        self.subtitles_held.set(false);
        let mark = self.subtitles_hold.get() + 1;
        self.subtitles_hold.set(mark);
        let app = self.clone();
        glib::timeout_add_local_once(crate::controls::HOLD, move || {
            if app.subtitles_hold.get() != mark {
                return;
            }
            app.subtitles_held.set(true);
            app.toggle_mute();
        });
    }

    /// Changes the subtitles, unless the hold already silenced everything.
    pub(super) fn release_subtitles(self: &Rc<Self>) {
        self.subtitles_holding.set(false);
        self.subtitles_hold.set(self.subtitles_hold.get() + 1);
        if !self.subtitles_held.replace(false) {
            self.toggle_subtitles();
        }
    }
}
