//! Taking on a chosen video: reading it, remembering it, and reporting one that will not open.

use super::*;

impl App {
    pub(super) fn open_file_chooser(self: &Rc<Self>, start: &std::path::Path) {
        // FileChooserNative rather than FileDialog: the latter needs GTK
        // 4.10, above this project's 4.6 baseline. It also gives the real
        // system file dialog on each platform.
        // Which errand this is on, decided the same way the built-in browser
        // decides it, so the two always agree about what is being chosen.
        let errand = self.errand.get();
        let chooser = gtk::FileChooserNative::new(
            Some(
                match errand {
                    Errand::Audio(_) => tr!("Choose an audio file"),
                    Errand::Subtitle => tr!("Choose a subtitle file"),
                    _ => tr!("Choose a video"),
                }
                .as_ref(),
            ),
            Some(&self.window),
            gtk::FileChooserAction::Open,
            Some(tr!("Open").as_ref()),
            Some(tr!("Cancel").as_ref()),
        );

        // The pipeline typefinds rather than assuming a container, so this
        // list is about not cluttering the dialog with non-video files, not
        // about what will actually play. Anything GStreamer can demux works,
        // which is why "All files" stays available below.
        let filter = gtk::FileFilter::new();
        let (name, extensions) = if errand == Errand::Subtitle {
            (tr!("Subtitle files"), &crate::subtitles::EXTENSIONS[..])
        } else if matches!(errand, Errand::Audio(_)) {
            (tr!("Audio files"), crate::browser::AUDIO_EXTENSIONS)
        } else {
            (tr!("Video files"), &crate::browser::VIDEO_EXTENSIONS[..])
        };
        filter.set_name(Some(name.as_ref()));
        for extension in extensions {
            // Case-insensitive by hand: GTK's pattern matching is not, and
            // ".MKV" off a camera or an old disc is common enough to matter.
            filter.add_pattern(&format!("*.{extension}"));
            filter.add_pattern(&format!("*.{}", extension.to_uppercase()));
        }
        chooser.add_filter(&filter);
        open_at(&chooser, start);

        let all = gtk::FileFilter::new();
        all.set_name(Some(tr!("All files").as_ref()));
        all.add_pattern("*");
        chooser.add_filter(&all);

        let app = self.clone();
        // Where this was opened from, so canceling returns there rather than
        // dropping to the menu. Reached from the browser, canceling should
        // leave you in the folder you were looking at.
        let from_browser = *self.screen.borrow() == Screen::Browser;
        let folder = self.config.borrow().last_folder.clone();

        // Held by the closure so the dialog outlives this function; a
        // dropped FileChooserNative closes before the user can answer.
        let held = RefCell::new(Some(chooser.clone()));
        chooser.connect_response(move |chooser, response| {
            let chosen = (response == gtk::ResponseType::Accept)
                .then(|| chooser.file().and_then(|f| f.path()))
                .flatten();
            held.borrow_mut().take();

            match chosen {
                // A subtitle or a soundtrack for the video already loaded,
                // rather than a video to load.
                Some(path) if errand == Errand::Subtitle => {
                    app.set_subtitle_file(&path);
                    app.show_menu();
                }
                Some(path) if matches!(errand, Errand::Audio(_)) => {
                    app.set_audio_file(&path);
                    app.show_menu();
                }
                // A file was picked, so the menu is where to go next either
                // way.
                Some(path) => {
                    let source = Source::File(path);
                    match app.set_file(&source) {
                        Ok(()) => app.show_menu(),
                        Err(e) => app.show_source_error(&source, &e, false),
                    }
                }
                None => match folder.as_deref().filter(|_| from_browser) {
                    Some(folder) => app.show_browser(folder, None),
                    None => app.show_menu(),
                },
            }
        });
        chooser.show();
    }

    /// Probes the file and chooses tracks for it.
    ///
    /// A file played before comes back with the tracks it was played with;
    /// otherwise the first track goes to the primary output and a different
    /// one to the secondary, which is the whole point of the application.
    pub(super) fn set_file(self: &Rc<Self>, source: &Source) -> Result<(), String> {
        match crate::probe::probe_media(source) {
            Ok(media) => self.apply_media(source, media),
            Err(e) => {
                eprintln!("Couldn't read {}: {e}", source.uri());
                self.forget_file();
                Err(e)
            }
        }
    }

    /// Drops everything that described the file that was loaded.
    pub(super) fn forget_file(&self) {
        *self.details.borrow_mut() = Default::default();
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        *self.series_art.borrow_mut() = None;
        // Anything still being read for the file being forgotten is now for
        // the wrong one, and this is what tells it so.
        self.art_generation.set(self.art_generation.get() + 1);
        *self.tracks.borrow_mut() = Vec::new();
        *self.subtitle_options.borrow_mut() = Vec::new();
        *self.audio_files.borrow_mut() = Vec::new();
        *self.primary_track.borrow_mut() = None;
        *self.secondary_track.borrow_mut() = None;
        *self.subtitle.borrow_mut() = None;
        self.subtitle_by_hand.set(false);
        *self.file.borrow_mut() = None;
        self.duration_s.set(0.0);
    }

    /// Takes up a probed source: which tracks to start on, which subtitle,
    /// and what to show in the menu.
    ///
    /// Separate from the probing so that a caller which probed on a thread,
    /// rather than making the interface wait for it, has somewhere to hand
    /// the result back on the main thread.
    pub(super) fn apply_media(
        self: &Rc<Self>,
        source: &Source,
        media: crate::probe::Media,
    ) -> Result<(), String> {
        // A different video starts with its subtitles showing, whatever the
        // last one was left doing.
        self.subtitles_hidden.set(false);
        // Kodi's one video player slot is necessarily this playback while it
        // waits for us, but a session started by hand with --kodi could attach
        // to a *different* external player's item. Lengths agreeing is a cheap
        // guard against that, and against writing progress onto the wrong film.
        if let Some(runtime) = self
            .kodi_item
            .borrow()
            .as_ref()
            .map(|item| item.runtime_s)
            .filter(|runtime| *runtime > 0)
            && media.duration_ns > 0
        {
            let ours = media.duration_ns / 1_000_000_000;
            if ours.abs_diff(runtime) > 5 {
                eprintln!(
                    "Kodi reports a {runtime}s item but this source is {ours}s;                      ignoring what it said and keeping local positions."
                );
                *self.kodi_item.borrow_mut() = None;
            }
        }

        // A video that did not come from Jellyfin must not wear the details of
        // one that did. Cleared here rather than when playback ends, because
        // `begin_playback` stops the previous playback on its way in - which
        // wiped the item a moment before anything could be reported about it.
        let cast = self
            .jellyfin_item
            .borrow()
            .as_ref()
            .is_some_and(|item| source.uri().contains(&item.id));
        if !cast {
            *self.jellyfin_item.borrow_mut() = None;
        }

        // What the page shows about the file, from the sidecar beside it and
        // the container's own tags. Cheap - a small file and a few `is_file`
        // calls - and the artwork behind whatever it found is read separately,
        // on a thread, because that is the part with a megabyte in it.
        //
        // Taken here rather than further down because the lists below are
        // moved out of `media`, and this reads the whole of it.
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        *self.series_art.borrow_mut() = None;
        self.art_generation.set(self.art_generation.get() + 1);
        let beside = {
            let config = self.config.borrow();
            crate::metadata::Beside {
                metadata: config.read_metadata,
                backdrop: config.show_backdrop,
            }
        };
        *self.details.borrow_mut() =
            crate::metadata::resolve(source, &media, beside, &self.launcher_title());

        let duration_ns = media.duration_ns;
        let tracks = media.audio;
        // What the library holds beside the video, which only a cast video has.
        // These are files on the server rather than streams in the container,
        // so they are offered alongside the embedded ones rather than counted
        // among them.
        let library = self
            .jellyfin_item
            .borrow()
            .as_ref()
            .map(|item| item.streams.subtitle_options())
            .unwrap_or_default();
        let mut options = crate::subtitles::options(source.local(), &media.subtitles, &library);

        let (primary_language, secondary_language, subtitle_language, described) = {
            let config = self.config.borrow();
            (
                config.primary_language.clone(),
                config.secondary_language.clone(),
                config.subtitle_language.clone(),
                (
                    config.primary_audio_description,
                    config.secondary_audio_description,
                ),
            )
        };
        let describes = |track: &crate::probe::AudioTrack| track.is_described();

        // What ordinary selection is allowed to pick from: everything except
        // the described tracks, which are only ever chosen by asking for them.
        // Without this, a file whose first English track happens to be the
        // described one would hand narration to someone who never wanted it.
        //
        // Unless description is all there is. A file with nothing else would
        // otherwise start silent, which reads as the player being broken
        // rather than as a preference being honored.
        let pool: Vec<&crate::probe::AudioTrack> = {
            let plain: Vec<_> = tracks.iter().filter(|track| !describes(track)).collect();
            if plain.is_empty() {
                tracks.iter().collect()
            } else {
                plain
            }
        };

        // First track in the preferred language, if one was named.
        let by_language = |preferred: &Option<String>| -> Option<u32> {
            let code = preferred.as_deref()?;
            pool.iter()
                .find(|track| crate::languages::matches(&track.language, code))
                .map(|track| track.index)
        };
        // A described track for an output that asked for one. Not finding one
        // is not a failure - most files have none - so it falls back to the
        // ordinary choice rather than leaving the output silent.
        //
        // A named language is a hard requirement, not a preference to relax:
        // description narrated in a language you do not speak is worse than no
        // description at all, so the fallback is the right language undescribed
        // rather than the wrong language described.
        let described_track = |want: bool, preferred: &Option<String>| -> Option<u32> {
            if !want {
                return None;
            }
            let Some(code) = preferred.as_deref() else {
                return tracks
                    .iter()
                    .find(|track| describes(track))
                    .map(|track| track.index);
            };
            tracks
                .iter()
                .find(|track| describes(track) && crate::languages::matches(&track.language, code))
                // Then one whose language is not stated. Unknown is not the
                // same as wrong: a track tagged for another language is
                // rejected, but plenty of description carries no tag at all -
                // the tool most people use to add one sets a title and no
                // language - and refusing those would mean finding nothing in
                // the commonest case of all.
                .or_else(|| {
                    tracks
                        .iter()
                        .find(|track| describes(track) && !crate::languages::known(&track.language))
                })
                .map(|track| track.index)
        };

        // Keyed on the video being loaded rather than the one still current,
        // which is not this one until the end of this function.
        let saved = crate::config::load_resume(&self.storage_key_for(source))
            .and_then(|resume| resume.tracks);
        let (primary, secondary) = match saved.clone() {
            // A saved None is a real choice ("no audio on that output"), so a
            // saved pair is taken as it stands rather than filled in.
            Some(choice) => (choice.primary, choice.secondary),
            // Otherwise the preferred languages decide, falling back to the
            // old behavior of the first track and a different one.
            None => (
                described_track(described.0, &primary_language)
                    .or_else(|| by_language(&primary_language))
                    .or_else(|| pool.first().map(|t| t.index)),
                described_track(described.1, &secondary_language)
                    .or_else(|| by_language(&secondary_language))
                    .or_else(|| pool.get(1).map(|t| t.index)),
            ),
        };
        // The file may have been re-encoded since it was last played.
        let known = |choice: Option<u32>| choice.filter(|i| tracks.iter().any(|t| t.index == *i));

        *self.primary_track.borrow_mut() = known(primary);
        *self.secondary_track.borrow_mut() = if self.config.borrow().secondary_sink.is_some() {
            known(secondary)
        } else {
            // Without a device to play it on, holding a secondary track only
            // produces a pipeline that fails to build.
            None
        };
        // A separate audio file, kept only if it is still where it was. One
        // that has been deleted, renamed, or is on a drive not mounted today
        // falls back to the track underneath it rather than failing when play
        // is pressed - the same rule the subtitle below follows.
        let still_there = |path: Option<&std::path::PathBuf>| {
            path.filter(|path| path.exists())
                .map(|path| Source::File(path.clone()))
        };
        *self.primary_file.borrow_mut() = still_there(
            saved
                .as_ref()
                .and_then(|choice| choice.primary_file.as_ref()),
        );
        *self.secondary_file.borrow_mut() = if self.config.borrow().secondary_sink.is_some() {
            still_there(
                saved
                    .as_ref()
                    .and_then(|choice| choice.secondary_file.as_ref()),
            )
        } else {
            None
        };

        // Only kept if it still resolves: an embedded stream the file no
        // longer has, or a subtitle file since deleted, quietly reverts to
        // none rather than failing when play is pressed.
        let subtitle = match saved {
            Some(choice) => choice.subtitle,
            // Follows whichever audio is actually going to each output, not
            // the language preference: the preference may have found nothing,
            // and what is being heard is what subtitles have to match.
            None => {
                let language_of = |index: Option<u32>| {
                    index.and_then(|index| {
                        tracks
                            .iter()
                            .find(|track| track.index == index)
                            .map(|track| track.language.as_str())
                    })
                };
                crate::subtitles::automatic(
                    &crate::subtitles::Auto::parse(
                        subtitle_language
                            .as_deref()
                            .unwrap_or(crate::subtitles::DEFAULT_MODE),
                    ),
                    &options,
                    language_of(known(primary)),
                    language_of(known(secondary)),
                )
            }
        };
        // A file chosen by hand is not beside the video, so nothing above
        // found it. Put it back before the check below, or the choice would be
        // dropped as unrecognised every time the file was loaded again - and
        // only if it is still on disk, since a remembered path can outlive the
        // file it names.
        if let Some(crate::subtitles::SubtitleChoice::File(path)) = subtitle.as_ref()
            && path.is_file()
        {
            options.push(crate::subtitles::chosen_file(path));
        }
        *self.subtitle.borrow_mut() =
            subtitle.filter(|choice| options.iter().any(|option| option.choice() == *choice));
        *self.subtitle_options.borrow_mut() = options;
        *self.tracks.borrow_mut() = tracks;
        // Separate soundtracks beside the video, found by the same convention
        // and the same code as the subtitle files above. Only for a local
        // file, for the reason subtitles are: there is no folder to look in
        // otherwise, and a media server hands over what it holds in the stream.
        *self.audio_files.borrow_mut() =
            source.local().map(crate::beside::audio).unwrap_or_default();
        *self.file.borrow_mut() = Some(source.clone());
        self.duration_s.set(duration_ns as f64 / 1e9);
        // Now that the video and its audio files are both settled, whatever
        // was measured about that pairing applies again.
        self.load_baselines();
        // What the library says, over what the stream could be asked. A cast
        // video has no sidecar beside it and its container tags are thin, so
        // without this it arrives with a title and an empty page.
        self.overlay_jellyfin_details();
        // The page can be drawn without artwork and filled in when it lands,
        // so this is started rather than waited for.
        self.start_art_load();

        // Only a local file is worth reopening: a remote URL can carry an
        // access token that expires, and whatever launched us will hand it over
        // again anyway.
        if let Some(path) = source.local() {
            let mut config = self.config.borrow_mut();
            config.last_video = Some(path.to_path_buf());
            let _ = config.save();
        }

        // The video is loaded and its page is about to be shown, so the system
        // is told what it is now rather than at the first play. Otherwise the
        // panel in the task bar sits there enabled with no title, and Windows
        // fills the gap with the application's own identifier - which is how
        // "Scottarius.TinePlayer" ended up where a film's name belongs, until
        // something had been played once.
        self.publish_now_playing();
        Ok(())
    }

    /// Says, on screen, why a video could not be opened.
    ///
    /// Worth a screen rather than a line on stderr: when something else
    /// launched the player there is no terminal to read, and the window
    /// closing again immediately is all anyone sees. That is exactly the case
    /// most likely to fail, because a media center can hand over a path or a
    /// URL that means nothing on this machine.
    ///
    /// The message GStreamer gave is shown as it stands. It is more specific
    /// than anything that could be inferred from the kind of source - an
    /// unmounted share, a refused connection and a missing file all arrive
    /// here, and guessing between them would sometimes be wrong.
    pub(super) fn show_source_error(self: &Rc<Self>, source: &Source, error: &str, fatal: bool) {
        // Percent escaping is how a URI carries a space; it is not how anyone
        // wants to read a path. Decoded for display only - what gets opened is
        // still the escaped form. Anything that is not valid escaping is left
        // alone rather than mangled.
        let readable = |text: &str| {
            glib::Uri::unescape_string(text, None)
                .map(|decoded| decoded.to_string())
                .unwrap_or_else(|| text.to_string())
        };

        let mut message = tr!(
            "Couldn't open:\n{source}\n\n{reason}",
            source = readable(&source.uri()),
            reason = readable(error)
        )
        .into_owned();
        // Whatever launched us handed over a path or URL this machine could
        // not open, so what helps is knowing which paths and URLs work, rather
        // than anything about the launcher itself.
        if self.external {
            message.push_str(&tr!(
                "\n\nSee tineplayer.app/docs/ for the paths and URLs that can be played."
            ));
        }
        self.show_error(&message, fatal);
    }
}
