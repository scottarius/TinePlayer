//! The page a chosen video gets: its poster, its facts, and the rows that start it.

use super::*;

impl App {
    /// How tall the page is, or is about to be.
    ///
    /// Before the window is mapped it has no size, but it does already know
    /// the size it is going to open at - and that is not simply the interface
    /// scale times a constant, because the opening size is capped to the
    /// monitor. Guessing it as `700 * scale` put a 1050px poster in a 1325px
    /// window at 3x, which pushed the rows and the whole footer off the bottom
    /// of the screen.
    fn page_height(&self, scale: f64) -> f64 {
        match (self.window.height(), self.window.default_height()) {
            (0, 0) => 700.0 * scale,
            (0, planned) => planned as f64,
            (height, _) => height as f64,
        }
    }

    /// How tall the poster should be for the window as it stands: a share of
    /// the page, within hard bounds at both ends.
    ///
    /// The ceiling matters for more than composition. This is a size
    /// *request*, which is a minimum its window must honor, so a poster sized
    /// from the window's own height is a loop: the taller the window, the more
    /// height its contents insist on. Capping it breaks that - past this size
    /// the poster stops following the window, and the window stays free to be
    /// made smaller again.
    ///
    /// The floor is absolute rather than scaled for the opposite reason:
    /// scaled, it grows with the interface exactly when there is least room
    /// for it.
    pub(super) fn poster_height(&self, scale: f64) -> f64 {
        (self.page_height(scale) * POSTER_SHARE).clamp(120.0, 620.0 * scale)
    }

    /// Remembers the window's size while it is an ordinary window.
    ///
    /// Neither maximized nor fullscreen: both report the screen's dimensions,
    /// and a size taken then is not a size the window can be restored to.
    pub(super) fn note_windowed_size(&self) {
        if self.window.is_maximized() || self.window.is_fullscreen() {
            return;
        }
        let (width, height) = (self.window.width(), self.window.height());
        if width > 0 && height > 0 {
            self.windowed_size.set((width, height));
        }
    }

    /// Writes down where the window was left, on the way out.
    ///
    /// Every way of leaving goes through `window.close()`, which is what makes
    /// this one handler enough - the close button, Ctrl+Q, the confirmation,
    /// and a fatal error all end here.
    pub(super) fn remember_window_size(&self) {
        let (width, height) = self.windowed_size.get();
        if width <= 0 || height <= 0 {
            return;
        }
        let mut config = self.config.borrow_mut();
        if config.window_width == Some(width) && config.window_height == Some(height) {
            return;
        }
        config.window_width = Some(width);
        config.window_height = Some(height);
        if let Err(e) = config.save() {
            eprintln!("Could not save the window size: {e}");
        }
    }

    /// Rebuilds the media page once a drag-resize has stopped moving.
    ///
    /// GTK has no "the resize finished" signal - `layout` arrives on every
    /// frame of a drag, and rebuilding the page on each one would be both slow
    /// and unpleasant to watch, the poster jumping under the pointer. So the
    /// rebuild is put on a short timer that each new size cancels and restarts,
    /// and only the last one in a drag survives to fire.
    ///
    /// Without this the poster only resized on maximize and restore, which
    /// have their own handler and change the height in one step. Dragging a
    /// window smaller left the page built for the size it used to be, which is
    /// the sort of thing that looks like a bug rather than a decision.
    ///
    /// The guard is the poster's own height rather than the window's: past the
    /// ceiling in [`App::poster_height`] the window can grow as much as it
    /// likes without the page looking any different, and rebuilding then would
    /// throw away the viewer's place in the list for nothing.
    pub(super) fn rebuild_when_resize_ends(self: &Rc<Self>) {
        /// Long enough to sit out a drag, short enough that letting go and
        /// seeing the page settle reads as one action rather than two.
        const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

        if let Some(pending) = self.resize_settle.borrow_mut().take() {
            pending.remove();
        }
        let app = self.clone();
        let source = glib::timeout_add_local_once(SETTLE, move || {
            *app.resize_settle.borrow_mut() = None;
            // The automatic size follows the window now, not only the screen,
            // so a drag that changes its height changes the size everything is
            // drawn at. It waits out the drag with the rebuild below rather
            // than answering every layout event: restyling reloads the sheet
            // and rebuilds the page, and doing that on each frame of a drag
            // takes the page out from under the pointer holding the edge.
            //
            // Armed on every screen, unlike the rebuild, because the size is
            // the whole interface and settings can be resized too.
            //
            // **Except while a film is playing.** The control strip takes its
            // scale once, when `Controls::new` builds it, and holds it in a
            // plain field: the panel widths, the list heights and every icon
            // are sized in Rust at that moment, and nothing rebuilds them.
            // Restyling underneath moves the CSS around those fixed sizes, and
            // the strip stops agreeing with itself about where each button is.
            // Nothing should be resizing the interface out from under a film
            // anyway.
            //
            // This is the narrow half of the fix. Toggling fullscreen during
            // playback restyles by another route and has the same problem,
            // which is a matter for `Controls` rather than for here.
            if app.playback.borrow().is_some() {
                return;
            }
            let before = app.scale.get();
            app.follow_automatic_scale(&app.window.clone());
            // `restyle` rebuilds the menu itself when the size moved, so
            // going on would only build the same page a second time.
            if app.scale.get() != before {
                return;
            }
            if *app.screen.borrow() != Screen::Menu {
                return;
            }
            if app.poster_height(app.scale.get()) == app.built_poster.get() {
                return;
            }
            app.show_menu();
        });
        *self.resize_settle.borrow_mut() = Some(source);
    }

    /// The poster, and the facts about the file under it.
    ///
    /// The two belong together and to nothing else on the page: one is what
    /// the film looks like and the other is what this copy of it is, and
    /// neither is a choice anybody makes. Keeping them in their own column
    /// leaves the whole of the space beside it for the choices.
    pub(super) fn poster_column(self: &Rc<Self>, scale: f64) -> gtk::Box {
        let px = |base: f64| (base * scale).round() as i32;

        // Half the page's height, which is the proportion the comps are drawn
        // to - 550px of 1080 - and the reason this is not simply a size in
        // interface units. On a maximized ultrawide the page is held to a
        // 16:9 column far taller than the default window, and a poster fixed
        // in scaled pixels sits in the corner of it looking like a thumbnail
        // of itself. Bounded at both ends so a very short window still gets
        // something poster-shaped and a very tall one does not get a
        // billboard.
        //
        // Read when the page is built rather than tracked, so a window
        // resized while the menu is up keeps the size it was built at until
        // something rebuilds the page - which every trip into a chooser does.
        // The alternative is another custom widget, and this is a proportion
        // rather than a constraint: being a little out until the next rebuild
        // costs nothing that anyone can see.
        let height = self.poster_height(scale);
        // Two by three, which every poster in every library is drawn to.
        let width = height * 2.0 / 3.0;

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(12.0))
            .valign(gtk::Align::Start)
            .build();
        // Exactly as wide as the poster and no wider. Without this the column
        // is as wide as its widest *fact*, so a long codec name pushed the
        // whole page to the right and left a gap beside the poster that
        // belonged to nothing.
        column.set_size_request(width.round() as i32, -1);
        // Explicitly not expanding, and this is load-bearing. GTK propagates
        // `hexpand` up from children, so the poster picture asking to fill its
        // own frame quietly made this whole column an expanding one - and a
        // box then splits the spare width between it and the page beside it.
        // Measured: a column asking for 291px was being handed 567, which is
        // the gap that appeared to sit between the poster and the rows.
        column.set_hexpand(false);

        // How tall the frame actually is, which is the poster's height for a
        // poster and less for anything wider.
        //
        // **An episode is not a poster.** Its Primary image is a 16:9 still
        // from the episode, and cropping that into a two-by-three slot scales
        // it to fill the height and throws away the sides - which is most of
        // the picture. Reported on 2026-08-16. So a picture wider than the
        // slot keeps the slot's *width* and takes only the height it needs,
        // sitting shorter in the column. Anything at or narrower than two by
        // three is a real poster and is unaffected.
        //
        // The width never changes, whatever the shape: it is what the column
        // is sized to, and letting it vary would move the page beside it every
        // time a different kind of thing was loaded.
        let frame_height = poster_frame_height(self.poster_art.borrow().as_ref(), width, height);

        let frame = gtk::Box::builder()
            .css_classes(["tp-poster"])
            .halign(gtk::Align::Start)
            // Clipped, so a poster a few pixels out of square is cropped by
            // the frame rather than allowed to reshape it. A picture of a
            // quite different shape is given a frame of its own shape above,
            // so there is nothing left for this to cut off.
            .overflow(gtk::Overflow::Hidden)
            .build();
        frame.set_size_request(width.round() as i32, frame_height.round() as i32);

        match self.poster_art.borrow().clone() {
            Some(texture) => {
                // Fills the frame and keeps its shape, which is the same rule
                // the backdrop follows and the reason both are cropped rather
                // than letterboxed: a poster with bars down its sides reads
                // as a mistake. Real posters are two by three and are not
                // cropped at all; what this rescues is an episode thumbnail
                // or a scan that is a few pixels out.
                // Expanding is how it fills the frame: the widget draws a
                // texture and measures as nothing, so without this the frame
                // allocates it no width at all and the poster disappears.
                // The request stops at the column, which sets its own
                // `hexpand` explicitly - see there.
                let picture = crate::artwork::Artwork::poster();
                picture.set_texture(Some(texture));
                picture.set_hexpand(true);
                picture.set_vexpand(true);
                if self.fade_art.get() {
                    fade_in(&picture);
                }
                frame.append(&picture);
            }
            // Nothing found, which is the common case: of the 123 film folders
            // in the library this was written against, 28 carry artwork. The
            // mark is sized from the frame rather than from the interface, so
            // it keeps its place inside it at every window size.
            None => frame.append(&video_file_image(width * 0.42)),
        }
        *self.poster_frame.borrow_mut() = Some(frame.clone());
        column.append(&frame);

        let facts = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(1.0))
            // A few pixels in from the poster's edges, and the same on both:
            // the readings are ranged right, so without this they sat hard
            // against the frame above on one side while the names were inset
            // on the other.
            .margin_start(px(4.0))
            .margin_end(px(4.0))
            .build();
        // Two columns: what it is on the left, what it says on the right,
        // ranged against the poster's own right edge. As one run of text the
        // readings started at a different place on every line and there was
        // nothing to read down; against an edge they line up as a table, which
        // is what a column of measurements wants to be.
        // How wide the names are allowed to ask to be, measured from the
        // names themselves rather than fixed at what English happens to need.
        //
        // It was 12, chosen against "Resolution:" - which is right in English
        // and wrong the moment anything is translated: German's Framerate is
        // "Bildwiederholrate", eighteen characters, and a fixed cap cut it to
        // "Bildwiederho..." - a label that has stopped labelling anything.
        // The floor keeps the column steady when every name is short, and the
        // ceiling keeps one very long name from pushing the page right, which
        // is the fault the cap exists to prevent.
        let readings = self.file_facts();
        let name_chars = readings
            .iter()
            .map(|(name, _)| name.chars().count() as i32 + 1)
            .max()
            .unwrap_or(12)
            .clamp(12, 20);

        for (name, value) in readings {
            let line = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(px(8.0))
                .build();

            // Ellipsizing decides how a label *shrinks*; it does nothing to
            // what one asks for in the first place, which stays the full width
            // of its text. So a long reading here would widen the column past
            // the poster and push the whole page right. Both halves are capped
            // in what they may ask for; the width they actually get comes from
            // the poster above, and anything longer is cut with an ellipsis.
            let key = gtk::Label::new(Some(&format!("{name}:")));
            key.add_css_class("tp-fact");
            key.add_css_class("tp-fact-name");
            key.set_xalign(appearance::text_start());
            key.set_justify(appearance::text_justify());
            key.set_ellipsize(gtk::pango::EllipsizeMode::End);
            key.set_max_width_chars(name_chars);
            line.append(&key);

            let reading = gtk::Label::new(Some(&value));
            reading.add_css_class("tp-fact");
            reading.set_xalign(appearance::text_end());
            reading.set_justify(appearance::text_justify());
            reading.set_ellipsize(gtk::pango::EllipsizeMode::End);
            reading.set_max_width_chars(12);
            // Pushes itself to the far edge. Safe only because the column
            // sets `hexpand` false outright - otherwise this request would
            // travel up and widen the whole left column, which is the fault
            // the poster picture caused before it.
            reading.set_hexpand(true);
            line.append(&reading);

            facts.append(&line);
        }
        column.append(&facts);

        // What show this is, under the details of the file it lives in.
        //
        // Only for an episode, and only under everything else: the page names
        // the episode, because that is what is being watched, and the series
        // is the answer to "which programme is that" rather than a heading for
        // it. A film has neither and this is simply absent.
        //
        // The picture is the *series'* poster, which is a different thing from
        // the poster slot above: for an episode that slot holds a still from
        // the episode itself.
        let series_title = self.details.borrow().series_title.clone();
        let series_art = self.series_art.borrow().clone();
        // Whether a picture is on the way, which is not the same as having
        // one: this page is built long before either source answers.
        //
        // The library is asked directly rather than through `Details`, and it
        // has to be. A local episode's series poster is a path found while the
        // page is being resolved, so it is in hand by now; a cast one is an
        // HTTP request still in flight, so `series_poster` is empty at this
        // moment and stays empty until after the page exists. Consulting only
        // that field built no frame for a cast episode, and the picture then
        // had nowhere to go when it arrived - which is exactly what it looked
        // like: the title without it.
        let expecting_series_art =
            series_art.is_some()
                || self.details.borrow().series_poster.is_some()
                || self.jellyfin_item.borrow().as_ref().is_some_and(|item| {
                    item.series_poster_tag.is_some() || !item.season_id.is_empty()
                });
        if !series_title.is_empty() || expecting_series_art {
            let series = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(px(8.0))
                .margin_top(px(10.0))
                .margin_start(px(4.0))
                .margin_end(px(4.0))
                .build();

            // Built whenever a picture is *expected* rather than only when
            // one is already decoded: a library's is a file read on a thread
            // and a cast one is an HTTP request, so at the moment this page is
            // built there is usually nothing to draw yet. The frame is kept and
            // `show_late_series_art` fills it - which is why the title alone
            // appeared at first.
            if expecting_series_art {
                // Three eighths of the poster's width: a reference beside the
                // details rather than a second poster competing with the one
                // above it, and settled by looking at it rather than by
                // reasoning about it.
                let small = width * 3.0 / 8.0;
                let frame = gtk::Box::builder()
                    .css_classes(["tp-poster"])
                    .halign(gtk::Align::Start)
                    .valign(gtk::Align::Start)
                    .overflow(gtk::Overflow::Hidden)
                    .build();
                // **Explicitly not expanding, and this is the whole bug it
                // fixes.** The picture inside asks to expand so that it fills
                // this frame, and GTK propagates that upwards - so in a
                // horizontal row the frame grew to take half the width, while
                // its height stayed at what was asked for. The picture then
                // covered a box far wider than it was tall and was cropped top
                // and bottom. A size request is a minimum; this is what makes
                // it the size. The column above sets the same two for the same
                // reason.
                frame.set_hexpand(false);
                frame.set_vexpand(false);
                frame.set_size_request(
                    small.round() as i32,
                    series_frame_height(series_art.as_ref(), small).round() as i32,
                );
                if let Some(texture) = series_art {
                    frame.append(&series_picture(texture));
                }
                *self.series_frame.borrow_mut() = Some(frame.clone());
                series.append(&frame);
            }

            // The show, and which run of it, stacked beside the picture.
            let words = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Start)
                .hexpand(true)
                .build();
            for (text, dim) in [(series_title.clone(), false), (self.season_label(), true)] {
                if text.is_empty() {
                    continue;
                }
                let line = gtk::Label::new(Some(&text));
                line.add_css_class("tp-fact");
                if dim {
                    // Subordinate to the name above it: which season is a
                    // qualifier, not a second thing of equal weight.
                    line.add_css_class("tp-fact-name");
                }
                line.set_xalign(appearance::text_start());
                line.set_justify(appearance::text_justify());
                line.set_yalign(0.0);
                line.set_wrap(true);
                line.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                // Capped like the facts above it, and for the same reason: a
                // long show name would otherwise ask for the width of its own
                // text and push the whole page to the right.
                line.set_max_width_chars(16);
                words.append(&line);
            }
            if words.first_child().is_some() {
                series.append(&words);
            }

            column.append(&series);
        }

        column
    }

    /// What this copy of the film is, as opposed to what the film is.
    ///
    /// Only what is actually known: a remote source can be measured for none
    /// of it, and a line reading "Unknown" is worse than no line, so anything
    /// unanswered is simply absent. The order runs from what a viewer checks
    /// first to what they check last.
    fn file_facts(&self) -> Vec<(String, String)> {
        let details = self.details.borrow();
        [
            // Two lines rather than "1080p (H.264)". Together they are the
            // longest reading in the column, and the column is only as wide
            // as the poster - so as one line they were the thing that decided
            // how much room the picture got.
            (tr!("Resolution"), details.resolution()),
            (tr!("Codec"), details.codec()),
            (tr!("Framerate"), details.framerate()),
            (tr!("Bitrate"), details.bitrate()),
            (
                tr!("Container"),
                Some(details.container.clone()).filter(|c| !c.is_empty()),
            ),
            // Last, under the readings that describe the picture. It is the
            // one line here that says nothing about how the film will look.
            (tr!("File size"), details.filesize()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name.into_owned(), value)))
        .collect()
    }

    /// The title, the facts line, the summary, and what languages the file
    /// holds - everything above the choices.
    ///
    /// Everything except the summary keeps its natural height; the summary is
    /// held to three lines whether it has them or not, which is what stops the
    /// rows underneath moving between one film and the next.
    pub(super) fn heading_block(self: &Rc<Self>, scale: f64) -> Vec<gtk::Widget> {
        let px = |base: f64| (base * scale).round() as i32;
        let details = self.details.borrow();
        let mut block: Vec<gtk::Widget> = Vec::new();

        let title = gtk::Label::new(Some(&details.title));
        title.add_css_class("tp-film-title");
        title.set_xalign(appearance::text_start());
        title.set_justify(appearance::text_justify());
        // One line, cut with an ellipsis. A filename with a release tag on it
        // is long and would happily take two - but the rows below sit at a
        // fixed distance from the top, and a title that is sometimes one line
        // and sometimes two is exactly the thing that moves them.
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        block.push(title.upcast());

        // Year, running time, certificate, score, genres - whichever of them
        // anything answered. Spaced rather than punctuated between, which is
        // what the comps do and what keeps a line of three facts from reading
        // as a sentence.
        let mut facts: Vec<String> = Vec::new();
        // An episode says when it went out, in place of the year a film shows:
        // a date is what anybody would recognise an episode by, where a year
        // barely distinguishes it from the twenty others made alongside it.
        // Only where the sidecar gave one - an episode without a date falls
        // back to the year like anything else.
        match (&details.aired, details.year) {
            (aired, _) if !aired.is_empty() => facts.push(aired.clone()),
            (_, Some(year)) => facts.push(year.to_string()),
            _ => {}
        }
        // Beside the date rather than near the title: which episode this is
        // belongs with the facts about it, and the title is the episode's own
        // name. Two digits each, which is how everything else writes it and
        // what makes a column of them line up.
        facts.extend(
            details
                .episode
                .map(|(season, episode)| format!("S{season:02}E{episode:02}")),
        );
        facts.extend(details.runtime());
        if !details.certificate.is_empty() {
            facts.push(details.certificate.clone());
        }
        // A star, so a bare number is not left to be guessed at. Out of ten is
        // what every writer of this format stores and what the star implies,
        // and the sidecar is the only place it comes from - nothing is ever
        // fetched to produce it.
        //
        // The star is in a font TinePlayer ships, which the other marks in the
        // interface are not: see `INTERFACE_SYMBOLS` in
        // packaging/fonts/build-fonts.py before using any new symbol here.
        //
        // One decimal: the scrapers store three, and "8.235" is a precision
        // nobody asked for about an opinion.
        facts.extend(details.rating.map(|score| format!("★ {score:.1}")));
        if !details.genres.is_empty() {
            // Three at most. A scraper will happily list six, and the line has
            // the width of one line.
            facts.push(
                details
                    .genres
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !facts.is_empty() {
            let line = gtk::Label::new(Some(&facts.join("     ")));
            line.add_css_class("tp-film-facts");
            line.set_xalign(appearance::text_start());
            line.set_justify(appearance::text_justify());
            line.set_ellipsize(gtk::pango::EllipsizeMode::End);
            line.set_margin_top(px(4.0));
            block.push(line.upcast());
        }

        // The summary, in a space of its own that is the same height whether
        // there is one or not. This is the only thing on the page held to a
        // fixed height, and it is the only one that needs to be: a plot runs
        // from nothing to a paragraph, and everything else here is one line or
        // absent. Reserving three lines for it is what keeps the rows below
        // from walking up and down the page as you step through a folder.
        let plot = gtk::Label::new(Some(&details.plot));
        plot.add_css_class("tp-film-plot");
        plot.set_xalign(appearance::text_start());
        plot.set_justify(appearance::text_justify());
        plot.set_yalign(0.0);
        plot.set_wrap(true);
        plot.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        plot.set_lines(3);
        plot.set_ellipsize(gtk::pango::EllipsizeMode::End);
        plot.set_margin_top(px(12.0));
        // Filling the width it is given rather than a fraction of it. A
        // wrapping label asks for its whole text on one line, so it used to be
        // capped at twenty characters to stop it stretching the page - which
        // capped where it *wrapped* too, and left it running down the middle
        // of the column at about half width. Nothing needs to cap it now that
        // the poster column no longer expands and `Column` decides the page's
        // width outright.
        // -1, which is the value that means "no cap". Zero is a cap of zero
        // characters, and left it wrapping down the middle of the column.
        plot.set_max_width_chars(-1);
        plot.set_size_request(-1, px(PLOT_UNITS));
        block.push(plot.upcast());
        drop(details);

        // What is in the file, in languages rather than in track numbers.
        // The rows below say which track is going where; this says what there
        // was to choose from, which is the question someone asks before they
        // start opening choosers.
        //
        // Both lines are always drawn, even when there is nothing to put on
        // them. They are the two facts this application exists to act on, and
        // a line that comes and goes with the file moves everything under it.
        let spoken = (self.audio_languages(), self.subtitle_languages());
        let summary = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(1.0))
            .margin_top(px(14.0))
            .build();
        for (name, languages) in [
            (tr!("Audio"), spoken.0),
            (trc!("media page heading", "Subtitles"), spoken.1),
        ] {
            let line = gtk::Label::new(None);
            line.add_css_class("tp-fact");
            line.set_xalign(appearance::text_start());
            line.set_justify(appearance::text_justify());
            // Cut rather than wrapped: a second line here would push the rows
            // down on exactly the files that carry the most languages.
            line.set_ellipsize(gtk::pango::EllipsizeMode::End);

            line.set_markup(&summary_markup(&name, &languages));
            summary.append(&line);
        }
        block.push(summary.upcast());
        block
    }

    /// Every language the file offers sound in, in the order the tracks are
    /// listed, with description called out.
    ///
    /// Deduplicated, because a file with four English tracks is offering one
    /// language four ways and a line reading "English, English, English,
    /// English" says less than one reading "English". A described track is a
    /// separate entry rather than a duplicate: it is a genuinely different
    /// thing to listen to, and for this application the most important entry
    /// on the line.
    fn audio_languages(&self) -> Vec<String> {
        let mut named: Vec<String> = Vec::new();
        for track in self.tracks.borrow().iter() {
            // A track that never said what it is still counts. Plenty of files
            // tag nothing at all - an AVI usually does not - and a line that
            // quietly left those out would claim a file had no soundtrack.
            // The language's own name for itself - see native_of_tag.
            let name = crate::languages::native_of_tag(&track.language)
                .map(Cow::Borrowed)
                .unwrap_or_else(unknown_language);
            let entry = match track.is_described() {
                true => tr!("{language} (Described)", language = name).into_owned(),
                false => name.into_owned(),
            };
            if !named.contains(&entry) {
                named.push(entry);
            }
        }
        named
    }

    /// The same for subtitles, over everything on offer - streams inside the
    /// file and files sitting beside it alike.
    ///
    /// Both are things the viewer can pick, so a line that counted only the
    /// embedded ones would understate a folder full of `.srt` files, which is
    /// exactly the shape most of this library is in.
    fn subtitle_languages(&self) -> Vec<String> {
        let mut named: Vec<String> = Vec::new();
        for option in self.subtitle_options.borrow().iter() {
            // Labels arrive as a tag and possibly a title after it - "eng",
            // "eng - Forced", "en.hi" - and the language is the first word of
            // whichever shape it is.
            let tag = option
                .label()
                .split(" - ")
                .next()
                .unwrap_or_default()
                .split('.')
                .next()
                .unwrap_or_default();
            let name = crate::languages::native_of_tag(tag)
                .map(Cow::Borrowed)
                .unwrap_or_else(unknown_language);
            if !named.iter().any(|held| *held == name) {
                named.push(name.into_owned());
            }
        }
        named
    }

    /// The panel that offers the two ways to choose a video: the prompt, and
    /// a button for each.
    ///
    /// Shared by the screen shown when nothing is loaded and by the panel the
    /// browse button opens over a film, because they say the same thing and
    /// should not drift apart. `cancel` adds a third button and is what tells
    /// them apart: the empty screen has nowhere to go back to, while the panel
    /// is floating over a film that is still loaded.
    ///
    /// Returns the panel and its buttons, since what each one does depends on
    /// which screen asked for it. The Jellyfin button is absent when there is
    /// already a pairing, and the Cancel button when `cancel` is false.
    pub(super) fn choose_source_panel(
        self: &Rc<Self>,
        scale: f64,
        cancel: bool,
    ) -> (
        gtk::Box,
        gtk::Button,
        gtk::Button,
        Option<gtk::Button>,
        Option<gtk::Button>,
    ) {
        let px = |base: f64| (base * scale).round() as i32;

        let middle = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(24.0))
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .vexpand(true)
            .build();
        // The mark only where the screen is otherwise empty. Over a film it
        // would be the application introducing itself in the middle of being
        // used.
        //
        // The mark alone rather than the lockup with the name beneath it: the
        // window is titled TinePlayer and the settings header carries the
        // full logo, so spelling the name out again here bought nothing and
        // took the room the prompt below it wants.
        if !cancel {
            middle.append(&marked_image(APP_MARK, EMPTY_MARK * scale));
        }

        let prompt = gtk::Label::new(Some(
            tr!("Drop a video file here, browse for a local file, or enter a URL").as_ref(),
        ));
        prompt.add_css_class("tp-empty-prompt");
        prompt.set_wrap(true);
        prompt.set_justify(gtk::Justification::Center);
        middle.append(&prompt);

        const BROWSE_ICON: &[u8] = include_bytes!("../../data/ui/browse.png");
        const LINK_ICON: &[u8] = include_bytes!("../../data/ui/link.png");
        const CONNECT_ICON: &[u8] = include_bytes!("../../data/ui/connect.png");
        // Green in the file rather than tinted here, because a GTK image
        // cannot be recoloured - the same reason the muted soundtrack mark
        // fades with opacity instead of changing colour.
        const CONNECTED_ICON: &[u8] = include_bytes!("../../data/ui/connected.png");

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::Center)
            .build();
        // Straight to the thing itself rather than to a menu row that opens
        // it: with one file to choose and two ways to choose it, a step in
        // between is a step for nothing.
        //
        // Each carries the mark of what it opens, and Browse carries the same
        // one the media page's button does - so the button on the page and the
        // button in the panel it opens are visibly the same errand.
        let browse = gtk::Button::new();
        browse.set_child(Some(&marked_face(
            marked_image(BROWSE_ICON, PLAY_MARK_PX * scale),
            &format!("  {}", tr!("Browse...")),
        )));
        browse.add_css_class("tp-button");
        browse.add_css_class("tp-action");
        name_it(&browse, &tr!("Browse..."));

        let address = gtk::Button::new();
        address.set_child(Some(&marked_face(
            marked_image(LINK_ICON, PLAY_MARK_PX * scale),
            &format!("  {}", tr!("Enter URL")),
        )));
        address.add_css_class("tp-button");
        address.add_css_class("tp-action");
        name_it(&address, &tr!("Enter URL"));

        buttons.append(&browse);
        buttons.append(&address);
        middle.append(&buttons);

        // Beneath the pair rather than beside them, for the reason the Cancel
        // button below is: those two choose a video and this does not. It
        // makes the television reachable from a phone, and the video is chosen
        // there afterwards.
        //
        // A button only while there is something to do. Once TinePlayer is
        // paired it is already a cast target whenever it is running, so the
        // button would offer to do something that is done - but the space says
        // so rather than going quiet, because "is this television reachable
        // from my phone?" is exactly the question this screen is looked at to
        // answer, and an absence is not an answer.
        let connect = match self.jellyfin_connected() {
            false => {
                let connect = gtk::Button::new();
                connect.set_child(Some(&marked_face(
                    marked_image(CONNECT_ICON, PLAY_MARK_PX * scale),
                    &format!("  {}", tr!("Connect to Jellyfin")),
                )));
                connect.add_css_class("tp-button");
                connect.add_css_class("tp-jellyfin");
                connect.set_halign(gtk::Align::Center);
                name_it(&connect, &tr!("Connect to Jellyfin"));
                middle.append(&connect);
                Some(connect)
            }
            // Stated, not offered. It is not focusable and takes no part in
            // the navigation: there is nothing to press, and a stop that does
            // nothing is worse than no stop at all.
            true => {
                let words = match self.jellyfin_server_label() {
                    Some(server) => format!(
                        "  {}",
                        tr!("Connected to Jellyfin ({server})", server = server)
                    ),
                    None => format!("  {}", tr!("Connected to Jellyfin")),
                };
                // The same mark-and-words shape the buttons above use, so the
                // line reads as belonging with them rather than as a caption
                // that wandered in - but as a plain box, since there is
                // nothing to press.
                let connected =
                    marked_face(marked_image(CONNECTED_ICON, PLAY_MARK_PX * scale), &words);
                connected.add_css_class("tp-connected");
                connected.set_halign(gtk::Align::Center);
                connected.set_can_focus(false);
                name_it(&connected, &words);
                middle.append(&connected);
                None
            }
        };

        // On a row of its own beneath them rather than beside them: it is not
        // a third way to choose a video, and standing in line with two that
        // are made it look like one.
        let back = cancel.then(|| {
            let back = gtk::Button::with_label(&tr!("Cancel"));
            back.add_css_class("tp-button");
            back.set_halign(gtk::Align::Center);
            middle.append(&back);
            back
        });
        (middle, browse, address, connect, back)
    }

    /// The screen with no video on it: an invitation, and the two ways to
    /// accept it.
    ///
    /// Deliberately not the menu with everything greyed out. There is nothing
    /// to choose until there is a film to choose it for, and a page of dashes
    /// asks to be read before it can be dismissed. The gear stays, because
    /// this is where somebody who has just installed the application arrives
    /// and every setting they might need is behind it.
    pub(super) fn build_empty_page(self: &Rc<Self>) -> gtk::Overlay {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(px(30.0))
            .margin_bottom(px(26.0))
            .margin_start(px(34.0))
            .margin_end(px(34.0))
            // Filled for the reason the media page is: `Column` does the
            // centering, and a box that centers itself as well collapses to
            // its contents and takes the footer's corner with it.
            .build();

        let (middle, browse, address, connect, _) = self.choose_source_panel(scale, false);
        content.append(&middle);

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

        // The same pair as the media page carries, in the same corner, so
        // they do not appear to move when a film is chosen.
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::End)
            .build();
        let (fullscreen, gear) = self.corner_buttons();
        footer.append(&gear);
        if let Some(fullscreen) = fullscreen.as_ref() {
            footer.append(fullscreen);
        }
        content.append(&footer);

        // Two rows rather than one run of four: the pair that chooses a
        // video, and the pair in the corner. Up and down move between the
        // rows, which is what they look like they should do - as one list they
        // fell through to GTK's own directional search, and it will not find a
        // button in the bottom corner from one in the middle of the page.
        let header = vec![browse.clone(), address];
        // Its own row, because it is drawn on its own line: down from the pair
        // reaches it, and down again reaches the corner. As a third entry in
        // the header it was reachable only sideways, which is not what a
        // button under two others looks like it should need.
        let connect_row: Vec<gtk::Button> = connect.into_iter().collect();
        let mut footer = vec![gear];
        footer.extend(fullscreen);
        self.set_nav(None, &header, &footer);
        self.set_nav_middle(&connect_row);
        // And the arrows have to be sent somewhere. `wire_navigation` does
        // this for every screen built around a list, and this one is not - so
        // without it the keys reached a focused button, which does nothing
        // with them, and stopped there.
        for button in header.iter().chain(connect_row.iter()).chain(footer.iter()) {
            self.wire_arrows(button.upcast_ref());
        }
        // Deferred until the page is actually in the window. This is built
        // before `show_menu` installs it, and focus cannot be taken by a
        // widget that is not on screen yet - the same reason `settle_on`
        // waits for the map on the first screen of a session.
        match browse.is_mapped() {
            true => browse.grab_focus(),
            false => {
                browse.connect_map(|browse| {
                    browse.grab_focus();
                });
                true
            }
        };

        self.behind_artwork(&content)
    }

    /// The mark that closes the player, at the far end of the row.
    ///
    /// Where a window's own close button would be, and worth having because
    /// on a television there is no window: TinePlayer opens fullscreen with no
    /// titlebar, and quitting otherwise means knowing that Escape asks. It
    /// asks the same question Escape does rather than quitting outright.
    pub(super) fn close_button(self: &Rc<Self>) -> gtk::Button {
        const ICON: &[u8] = include_bytes!("../../data/ui/close.png");

        let close = gtk::Button::new();
        close.set_child(Some(&marked_image(ICON, CORNER_MARK_PX * self.scale.get())));
        close.add_css_class("tp-gear");
        close.set_focus_on_click(false);
        close.set_tooltip_text(Some(tr!("Close the player").as_ref()));
        name_it(&close, &tr!("Close the player"));
        {
            let app = self.clone();
            close.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_confirm_quit();
            });
        }
        close
    }

    /// The mark that opens the panel for choosing a different video.
    ///
    /// Drawn and placed like the settings and fullscreen marks rather than
    /// like the play button, because it is the same kind of thing: something
    /// the page can do, rather than the thing the page is for.
    pub(super) fn browse_button(self: &Rc<Self>) -> gtk::Button {
        const ICON: &[u8] = include_bytes!("../../data/ui/browse.png");

        let open = gtk::Button::new();
        open.set_child(Some(&marked_image(ICON, CORNER_MARK_PX * self.scale.get())));
        open.add_css_class("tp-gear");
        open.set_focus_on_click(false);
        open.set_tooltip_text(Some(tr!("Choose a video").as_ref()));
        name_it(&open, &tr!("Choose a video"));
        {
            let app = self.clone();
            open.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.choose_video();
            });
        }
        open
    }

    /// The fullscreen mark and the gear, which sit together at the end of
    /// every footer on these two screens.
    ///
    /// Built here rather than twice, because the pair has three details worth
    /// not getting differently right in two places: the mark follows the
    /// window's own state, the gear carries the update badge, and neither
    /// takes focus from a click.
    pub(super) fn corner_buttons(self: &Rc<Self>) -> (Option<gtk::Button>, gtk::Button) {
        // Maximize and restore rather than the usual fullscreen pair, which
        // is absent from the icon theme on both platforms and would draw the
        // missing-image glyph.
        let fullscreen = gtk::Button::new();
        fullscreen.set_child(Some(&fullscreen_image(
            self.window.is_fullscreen(),
            self.scale.get(),
        )));
        fullscreen.add_css_class("tp-gear");
        fullscreen.set_focus_on_click(false);
        fullscreen.set_tooltip_text(Some(tr!("Toggle fullscreen").as_ref()));
        name_it(&fullscreen, &tr!("Toggle fullscreen"));
        {
            let app = self.clone();
            fullscreen.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.toggle_fullscreen();
            });
        }
        {
            let weak = fullscreen.downgrade();
            let scale = self.scale.get();
            self.window.connect_fullscreened_notify(move |window| {
                if let Some(button) = weak.upgrade() {
                    button.set_child(Some(&fullscreen_image(window.is_fullscreen(), scale)));
                }
            });
        }

        let gear = gtk::Button::new();
        gear.set_child(Some(&settings_image(CORNER_MARK_PX * self.scale.get())));
        gear.add_css_class("tp-gear");
        gear.set_focus_on_click(false);
        gear.set_tooltip_text(Some(tr!("Settings").as_ref()));
        name_it(&gear, &tr!("Settings"));
        {
            let app = self.clone();
            gear.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.enter_settings();
            });
        }
        *self.update_badges.borrow_mut() = vec![gear.clone()];
        self.draw_update_badge();

        // Left out entirely when fullscreen is not this viewer's to change: a
        // button that declines to do the one thing it offers is worse than no
        // button.
        match self.locked_fullscreen {
            true => (None, gear),
            false => (Some(fullscreen), gear),
        }
    }

    /// Reads the artwork for the file just loaded, and redraws the page when
    /// it arrives.
    ///
    /// On a thread, because this is the part with a megabyte in it. A backdrop
    /// over a network share is long enough to be felt, and the page has to be
    /// on screen before it - a film's details held back until its wallpaper
    /// loads is the wrong thing to wait for.
    pub(super) fn start_art_load(self: &Rc<Self>) {
        let (poster, backdrop, series) = {
            let details = self.details.borrow();
            (
                details.poster.clone(),
                details.backdrop.clone(),
                details.series_poster.clone(),
            )
        };
        if poster.is_none() && backdrop.is_none() && series.is_none() {
            return;
        }

        // What the artwork being read belongs to. A viewer who opens one film
        // and immediately opens another gets the second one's backdrop, not
        // whichever thread happened to finish last.
        let generation = self.art_generation.get();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let read = |art: Option<crate::metadata::Art>| {
                art.as_ref().and_then(crate::metadata::load_image)
            };
            let _ = sender.send((read(poster), read(backdrop), read(series)));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
            let (poster, backdrop, series) = match receiver.try_recv() {
                Ok(art) => art,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            if app.art_generation.get() != generation {
                return glib::ControlFlow::Break;
            }

            // Decoding happens here rather than on the thread: a GdkTexture
            // belongs to the main thread, and this is the only place that can
            // make one.
            let decode = |bytes: Option<Vec<u8>>| {
                let bytes = bytes?;
                match gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
                    Ok(texture) => Some(texture),
                    // Said out loud, because a poster that silently fails to
                    // appear looks like one that was never found - and the two
                    // want completely different things done about them.
                    Err(e) => {
                        eprintln!("Couldn't decode artwork: {e}");
                        None
                    }
                }
            };
            *app.poster_art.borrow_mut() = decode(poster);
            *app.backdrop_art.borrow_mut() = decode(backdrop);
            *app.series_art.borrow_mut() = decode(series);

            // Put into the page rather than rebuilding it, so that somebody
            // already choosing their tracks is left where they were.
            if *app.screen.borrow() == Screen::Menu {
                app.show_late_art();
                app.show_late_series_art();
            }
            glib::ControlFlow::Break
        });
    }
}
