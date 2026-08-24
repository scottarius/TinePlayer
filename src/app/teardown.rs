//! Leaving a video: stopping it, remembering where it got to, and naming it.

use super::*;

impl App {
    pub(super) fn stop_playback(self: &Rc<Self>) {
        self.finish_playback(false);
    }

    /// Leaves playback for the menu, remembering where it had reached.
    ///
    /// What Escape, the stop button and the settings button all do, so that
    /// stepping out to change something and coming back is one motion however
    /// it was asked for.
    pub(super) fn leave_playback(self: &Rc<Self>) {
        let position = self
            .playback
            .borrow()
            .as_ref()
            .and_then(|playback| playback.position())
            .map(|position| position.nseconds())
            .filter(|position| *position > 0);
        if let Some((key, position)) = self.storage_key().zip(position) {
            *self.session_resume.borrow_mut() = Some((key, position));
        }
        self.stop_playback();
        self.show_menu();
    }

    /// Tears playback down, saving or clearing the resume position as it goes.
    ///
    /// `wait_for_kodi` holds on until the last progress report has actually
    /// reached Kodi. That only matters when the process is about to end, since
    /// the report goes out on a detached thread and exiting would take it
    /// along; everywhere else it would be a stall for nothing.
    pub(super) fn finish_playback(self: &Rc<Self>, wait_for_kodi: bool) {
        // Whatever else happens below, stop holding the display awake: this
        // is reached from the window closing as well as from playback ending.
        self.awake.set(false);
        if let Some(tick) = self.tick.borrow_mut().take() {
            tick.remove();
        }
        if let Some(controls) = self.controls.borrow_mut().take() {
            controls.cancel();
            // Playback ending with the pointer hidden would leave the menus
            // behind it without one.
            controls.reveal_pointer();
        }
        // Before the playback is taken, because the position is read from
        // it and a stopped report with nowhere to read from would file zero.
        self.report_to_jellyfin(JellyfinMoment::Stopped);
        if let Some(playback) = self.playback.borrow_mut().take() {
            playback.stop();
            if wait_for_kodi {
                playback.finish_reporting();
            }
        }
        self.window.set_title(Some("TinePlayer"));
        // After the playback is dropped above, so this reads "nothing".
        self.publish_now_playing();
    }

    /// Works out where a chosen subtitle actually comes from.
    ///
    /// The three kinds resolve against three different things - the video's
    /// own folder, the path as given, and the paired server - which is why
    /// this is here rather than in the pipeline: only the application knows
    /// all three. A library's subtitle resolves to a URL carrying the access
    /// token, and that URL is built here, used, and never stored.
    pub(super) fn locate_subtitle(
        &self,
        source: &Source,
        choice: Option<&crate::subtitles::SubtitleChoice>,
    ) -> Result<Option<crate::subtitles::SubtitleSource>, String> {
        use crate::subtitles::{SubtitleChoice, SubtitleSource};

        let uri_for = |path: std::path::PathBuf| {
            glib::filename_to_uri(&path, None)
                .map(|uri| SubtitleSource::Uri(uri.to_string()))
                .map_err(|e| format!("Can't open {}: {e}", path.display()))
        };

        match choice {
            None => Ok(None),
            Some(SubtitleChoice::Embedded(index)) => Ok(Some(SubtitleSource::Embedded(*index))),
            // A name, which means the folder the video is in. A source with no
            // folder - anything opened by URL - has no subtitle files beside
            // it to have chosen in the first place.
            Some(SubtitleChoice::External(name)) => source
                .local()
                .and_then(|video| video.parent())
                .map(|folder| folder.join(name))
                .ok_or_else(|| format!("Can't find {name}: it sits beside a local video"))
                .and_then(uri_for)
                .map(Some),
            // A path, which means itself. Chosen by hand from somewhere else
            // on disk, or named on the command line, and so not tied to where
            // the video happens to live.
            Some(SubtitleChoice::File(path)) => uri_for(path.clone()).map(Some),
            // Only a video the library is playing has these, and both halves
            // are needed: the client holds the address and token, the item
            // holds which media source the index counts against.
            Some(SubtitleChoice::Library(index)) => {
                let client = self.jellyfin.borrow().clone();
                let item = self.jellyfin_item.borrow().clone();
                match (client, item) {
                    (Some(client), Some(item)) => Ok(Some(SubtitleSource::Uri(
                        client.subtitle_url(&item, *index),
                    ))),
                    _ => Err("Can't fetch that subtitle: it belongs to a library this video did not come from".to_string()),
                }
            }
        }
    }

    /// Where playback should pick up.
    ///
    /// Under Kodi its library is the authority, so playback starts from the
    /// position Kodi's own interface was just showing and the two never
    /// visibly disagree. Its answer stands even when it holds no resume point:
    /// a film Kodi considers unwatched starts at the beginning rather than
    /// wherever our own file happens to remember. Only a Kodi that does not
    /// answer at all falls back to `positions.json`.
    pub(super) fn resume_position(&self) -> Option<u64> {
        let key = self.storage_key()?;
        // Ahead of everything, including Kodi's library: this is where the
        // viewer actually was, seconds ago, and no stored answer is better
        // informed than that.
        if let Some((remembered, position)) = self.session_resume.borrow().as_ref()
            && *remembered == key
        {
            return Some(*position);
        }
        if let Some(item) = self.kodi_item.borrow().as_ref() {
            return item.resume_ns;
        }
        if let Some(item) = self.jellyfin_item.borrow().as_ref() {
            return item.resume_ns;
        }
        crate::config::load_resume(&key)
            .and_then(|resume| resume.resume_position(self.config.borrow().resume_min_percent()))
    }

    /// How this video's position and track choices are filed.
    ///
    /// Kodi's own id when it launched us, which survives an add-on stream URL
    /// changing and is the same whichever form of the path is in play.
    /// Otherwise the source names itself.
    pub(super) fn storage_key(&self) -> Option<String> {
        self.key_for(None)
    }

    /// The same key for a video that is not the current one yet.
    ///
    /// `apply_media` needs this: it reads what was remembered about the file it
    /// is loading, and `self.file` does not become that file until the end of
    /// it. Asking `storage_key` there returns the *previous* video's key - or
    /// none at all on the first file of a session, which is why remembered
    /// choices were quietly ignored at startup.
    pub(super) fn storage_key_for(&self, source: &Source) -> String {
        // Always an answer, because naming the video removes the only reason
        // `storage_key` can fail.
        self.key_for(Some(source)).unwrap_or_else(|| source.key())
    }

    /// The one place that decides, for both of the callers above.
    ///
    /// **They used to decide separately, and disagreed.** `storage_key`
    /// checked Kodi, then Jellyfin, then the source; `storage_key_for` checked
    /// Kodi and fell straight through to the source, with no Jellyfin branch
    /// at all. So a cast video was *written* under `jellyfin:<id>` by every
    /// saver and *read back* under its stream URL by `apply_media`, and the
    /// audio and subtitle choices remembered for it were never found again.
    /// Nothing reported it: the language preferences answered instead, which
    /// looks exactly like a video being opened for the first time.
    ///
    /// `source` is what separates the two callers, and it is not decoration.
    /// `self.file` is not the video being loaded until the end of
    /// `apply_media`, so a caller part-way through has to name the video it
    /// means - which is the whole reason there were two functions to disagree.
    fn key_for(&self, source: Option<&Source>) -> Option<String> {
        if let Some(item) = self.kodi_item.borrow().as_ref() {
            return Some(item.key());
        }
        // The item id, never the stream address: that carries an access token
        // which changes when it is regenerated, and every position filed
        // against the old one would be orphaned.
        if let Some(item) = self.jellyfin_item.borrow().as_ref() {
            return Some(format!("jellyfin:{}", item.id));
        }
        match source {
            Some(source) => Some(source.key()),
            None => self.file.borrow().as_ref().map(Source::key),
        }
    }

    /// The title whatever launched us gave for this video, or empty.
    ///
    /// Handed to `metadata::resolve`, which puts it at the head of the same
    /// chain everything else uses. Kept as one accessor so no caller has to
    /// know that Kodi is currently the only thing that supplies one.
    pub(super) fn launcher_title(&self) -> String {
        if let Some(title) = self
            .kodi_item
            .borrow()
            .as_ref()
            .map(|item| item.title.clone())
            .filter(|title| !title.is_empty())
        {
            return title;
        }
        self.jellyfin_item
            .borrow()
            .as_ref()
            .map(|item| item.title.clone())
            .unwrap_or_default()
    }

    /// What to call the current video on screen.
    ///
    /// Read from `details` rather than worked out a second time, so the
    /// titlebar cannot disagree with the media page about what is playing.
    /// The order behind it lives in `metadata::resolve`: the launcher's title,
    /// then a sidecar's, then the container's own tag, then the file name with
    /// its extension and any trailing year taken off.
    pub(super) fn file_label(&self) -> Option<String> {
        if self.file.borrow().is_none() {
            return None;
        }
        let title = self.details.borrow().title.clone();
        if !title.is_empty() {
            return Some(title);
        }
        // Only reachable if resolve found nothing at all to call it, which its
        // own file-name fallback makes unlikely.
        self.file.borrow().as_ref().map(Source::label)
    }
}
