//! Which file or track feeds each output, and its level, mute and delay.

use super::*;

impl App {
    /// What an output is playing, for its row on the menu: the name of a
    /// separate audio file when one is chosen, and otherwise the track.
    pub(super) fn describe_audio(&self, role: Role) -> String {
        // The same words the list under this row shows against the same file,
        // which is what makes the row and the list one thing.
        if let Some(file) = self.file_for(role).borrow().as_ref() {
            return self.label_for_file(file);
        }
        let chosen = *self.track_for(role).borrow();
        let tracks = self.tracks.borrow();
        match chosen {
            Some(index) => tracks
                .iter()
                .find(|track| track.index == index)
                .map(describe_audio_track)
                .unwrap_or_else(|| trc!("audio track", "None").into_owned()),
            None => trc!("subtitle track", "None").into_owned(),
        }
    }

    /// The alignment row for one output, when there is anything to align.
    ///
    /// Only offered against a separate audio file: a track inside the video
    /// shares the video's timeline and cannot be out of step with it. The rest
    /// are the things measuring needs and cannot do without - a track inside
    /// the video to line the file up against, a running time to place the
    /// three windows across, and a path on disk to file the answer under.
    pub(super) fn alignment_row(&self, role: Role) -> Option<(String, String, bool, MenuAction)> {
        let file = self.file_for(role).borrow();
        let path = file.as_ref()?.local()?;
        if self.tracks.borrow().is_empty() || self.duration_s.get() <= 0.0 {
            return None;
        }
        let stored = self
            .storage_key()
            .and_then(|key| crate::config::load_alignment(&key, path));
        Some((
            // One name whether or not there is a stored answer. It used to say
            // "Auto-align" or "Re-align" to name what pressing it would do,
            // which the value beside it now says better: "Unsynced" against a
            // measured offset is the same distinction, in the column that
            // exists to carry state.
            "Sync".to_string(),
            match stored {
                Some(millis) => describe_lateness(millis),
                None => tr!("Unsynced").into_owned(),
            },
            true,
            MenuAction::Align(role),
        ))
    }

    /// Reads back what alignment worked out for whatever each output is
    /// playing, so the baseline is in force before the pipeline is built.
    ///
    /// Zero for a track inside the video: alignment is about a pairing of two
    /// files and there is nothing to pair a track with.
    pub(super) fn load_baselines(&self) {
        let key = self.storage_key();
        for role in [Role::Primary, Role::Secondary] {
            let stored = key.as_deref().and_then(|key| {
                let file = self.file_for(role).borrow();
                let path = file.as_ref()?.local()?;
                crate::config::load_alignment(key, path)
            });
            // Negated on the way in: alignment says how late the audio runs,
            // and a sink is held back by a negative offset.
            let cell = match role {
                Role::Primary => &self.primary_baseline,
                Role::Secondary => &self.secondary_baseline,
            };
            cell.set(-stored.unwrap_or(0.0));
        }
    }

    /// The alignment baseline for one output.
    pub(super) fn baseline_ms(&self, role: &str) -> f64 {
        match role {
            "primary" => self.primary_baseline.get(),
            _ => self.secondary_baseline.get(),
        }
    }

    /// What the sink should actually be held back by: what the viewer asked
    /// for, plus what alignment worked out. The two are separate quantities -
    /// one describes the headphones, the other describes the pair of files -
    /// and only the first is ever shown on the slider.
    /// The baseline counts only while the output is playing the file it was
    /// measured for. Alignment describes a *pairing* of two files, so it means
    /// nothing to a track inside the video - and carrying it across a switch
    /// left that track running seconds out of step with the picture, which
    /// reads as the film being broken rather than as a setting being wrong.
    fn offset_for(&self, playback: &Playback, role: &str) -> f64 {
        let paired = matches!(playback.playing_on(role), Some(Playing::File(_)));
        let baseline = if paired { self.baseline_ms(role) } else { 0.0 };
        self.config.borrow().applied_offset_ms(role) + baseline
    }

    /// Sends an output's whole delay to the pipeline: what the viewer asked
    /// for, plus what alignment worked out for the file being played.
    ///
    /// The one road to a sink, deliberately. The sum used to be rebuilt by
    /// hand at each of the four places that change either half, and the one
    /// behind the sync control during playback rebuilt it wrong - it sent the
    /// slider's own value, so touching sync threw the alignment away and left
    /// the audio seconds out. A half-applied offset is worse than none, and
    /// the way to stop that recurring is to leave nowhere else to apply one.
    pub(super) fn push_offset(&self, playback: &Playback, role: &str) {
        playback.set_offset_ms(role, self.offset_for(playback, role));
    }

    /// Sends an output's level to the pipeline: what that output is set to,
    /// times the main level over both of them.
    ///
    /// The one road to a sink's level, for the reason `push_offset` is the one
    /// road to its delay. Two outputs and a main level mean two numbers behind
    /// every level, and every place that rebuilt the sum by hand would be a
    /// place free to leave the main level out - which sounds exactly like a
    /// level that ignores the control somebody just moved.
    pub(super) fn push_volume(&self, role: &str) {
        let level = self.config.borrow().volume(role);
        self.push_volume_at(role, level);
    }

    /// The same, for a level that is not in the configuration - which is any
    /// level that is not being kept, such as everything silenced for a knock at
    /// the door.
    pub(super) fn push_volume_at(&self, role: &str, level: f64) {
        let level = self.effective(level);
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.set_volume(role, level);
        }
    }

    /// What a level actually plays at once the main level over both outputs is
    /// taken into account. The only place the two are multiplied together.
    pub(super) fn effective(&self, level: f64) -> f64 {
        level * self.config.borrow().main_volume()
    }

    /// Sends whether an output is actually silent: whether it is muted in its
    /// own right, or everything is.
    ///
    /// The two are kept apart all the way down to here, which is what lets the
    /// menu go on showing each output's own state while everything is quiet.
    /// `muted` is passed in rather than read back, because a silence nobody is
    /// keeping never reaches the configuration to be read from.
    pub(super) fn push_mute(&self, role: &str, muted: bool) {
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.set_muted(role, muted || self.hushed.get());
        }
    }

    /// The same, for whatever is playing now, if anything is. Cloned out of
    /// the cell rather than borrowed across the call, since what it reaches
    /// takes the same borrows.
    pub(super) fn push_offset_live(&self, role: &str) {
        if let Some(playback) = self.playback.borrow().clone() {
            self.push_offset(&playback, role);
        }
    }

    /// The track chosen for one output, and the file chosen for it, where the
    /// two outputs are otherwise handled by the same code.
    pub(super) fn track_for(&self, role: Role) -> &RefCell<Option<u32>> {
        match role {
            Role::Primary => &self.primary_track,
            Role::Secondary => &self.secondary_track,
        }
    }

    pub(super) fn file_for(&self, role: Role) -> &RefCell<Option<Source>> {
        match role {
            Role::Primary => &self.primary_file,
            Role::Secondary => &self.secondary_file,
        }
    }

    /// Puts a chosen audio file on the output the browser was opened for.
    /// Opens the browser to find a subtitle file, starting where the video is.
    pub(super) fn browse_for_subtitle(self: &Rc<Self>) {
        self.errand.set(Errand::Subtitle);
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

    /// Takes a subtitle file chosen by hand.
    ///
    /// Added to the options as well as chosen, so the menu can show it and the
    /// chooser can show it selected. Everything else in that list was found by
    /// looking beside the video, and this one never would be.
    pub(super) fn set_subtitle_file(self: &Rc<Self>, path: &std::path::Path) {
        let option = crate::subtitles::chosen_file(path);
        let choice = option.choice();
        {
            let mut options = self.subtitle_options.borrow_mut();
            if !options.iter().any(|other| other.choice() == choice) {
                options.push(option);
            }
        }
        *self.subtitle.borrow_mut() = Some(choice);
        self.subtitle_by_hand.set(true);
        // Choosing a subtitle is asking to see it, whatever the toggle was
        // doing for the last one.
        self.subtitles_hidden.set(false);
        self.errand.set(Errand::Video);
        self.remember_tracks();
    }

    pub(super) fn set_audio_file(self: &Rc<Self>, path: &std::path::Path) {
        let Errand::Audio(role) = self.errand.get() else {
            return;
        };
        self.errand.set(Errand::Video);
        self.use_audio_file(role, path);
    }

    /// Puts a separate audio file on one output, whether it was picked out of
    /// the list of files beside the video or found by hand from somewhere
    /// else. The two are the same choice once the file is known.
    pub(super) fn use_audio_file(self: &Rc<Self>, role: Role, path: &std::path::Path) {
        *self.file_for(role).borrow_mut() = Some(Source::File(path.to_path_buf()));
        // Written down here, not left to playback to save: choosing a
        // soundtrack and then quitting without pressing play is choosing it,
        // and every other chooser on this screen remembers itself the same way.
        self.remember_tracks();
        // A pairing measured before comes back already lined up.
        self.load_baselines();
    }

    /// What a separate audio file is called in a list.
    ///
    /// What the convention put in its name, where it is one of the files
    /// sitting beside the video, and the file's own name where it came from
    /// anywhere else - which is all there is to say about a file nothing
    /// named to a convention.
    pub(super) fn label_for_file(&self, file: &Source) -> String {
        file.local()
            .and_then(|path| {
                self.audio_files
                    .borrow()
                    .iter()
                    .find(|found| found.path == path)
                    .map(crate::beside::AudioFile::label)
            })
            .unwrap_or_else(|| {
                // Named to no convention, so its own name stands - through the
                // same formatter every other row goes through, which reads it
                // for what it says and leaves it alone where it says nothing.
                let name = file.label();
                crate::label::named_after_nothing(&name, crate::label::kind_of_audio_tag)
            })
    }
}
