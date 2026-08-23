//! Volume, mute and audio delay as the settings screen and the controls drive them.

use super::*;

impl App {
    /// Moves the level on the settings row that is selected, and says whether
    /// there was one. Left and right do nothing else on this screen, so they
    /// are free to mean this where a slider is sitting.
    pub(super) fn settings_slider(self: &Rc<Self>, direction: isize) -> bool {
        // On that screen and no other. The sliders are held on the application
        // rather than on the page they belong to, and they outlive it: leaving
        // settings does not empty the list, so this went on matching by row
        // number against whatever screen came next. Backing out to the media
        // page and pressing Left moved the interface size, because the row
        // selected there had the same number as the row the size sits on.
        if *self.screen.borrow() != Screen::Settings || !self.in_settings_pane.get() {
            return false;
        }
        let Some(index) = self
            .nav_list
            .borrow()
            .as_ref()
            .and_then(|list| list.selected_row())
            .map(|row| row.index())
        else {
            return false;
        };
        let Some(item) = self.item_at(index) else {
            return false;
        };
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((kind, scale, value)) = found else {
            return false;
        };
        // A bar that is switched off is not a bar to move. It is drawn greyed
        // and the pointer cannot reach it, and the keyboard reaching it anyway
        // is how one press turned automatic scaling off: the interface size
        // row keeps its bar while automatic owns the size, so Left wrote a
        // fixed size, and writing a size is what turning automatic off means.
        // The whole of it was silent - the row said "Automatic" until the next
        // time the page was built.
        //
        // Swallowed rather than declined, because the row still owns these
        // keys. Handing them back would send them to whatever GTK found to the
        // side, which is the wandering focus that Enter and Escape exist to
        // replace.
        if !scale.is_sensitive() {
            return true;
        }
        // Snapped to the step rather than added to: a value set finely with a
        // pointer, or from the panel during playback, otherwise carries its
        // odd remainder through every press that follows.
        let step = kind.step();
        let now = scale.value();
        // Nudged by a step from where it is, snapped onto the step grid. The
        // nudge is what the epsilon protects: a value already sitting exactly
        // on a step would otherwise floor to itself and go nowhere, which is
        // what stopped the interface size after one press - its steps are a
        // tenth, and rounding to a whole number made every press compute the
        // same target.
        let ratio = now / step;
        let moved = if direction > 0 {
            ((ratio + 1e-6).floor() + 1.0) * step
        } else {
            ((ratio - 1e-6).ceil() - 1.0) * step
        };
        let range = kind.range();
        let moved = moved.clamp(*range.start(), *range.end());
        scale.set_value(moved);
        self.set_slider(kind, moved, &value);
        // Safe here: nothing is holding the bar, so redrawing cannot be read
        // as another movement.
        if kind == Slider::Scale {
            self.apply_scale(moved);
        }
        true
    }

    /// Silences the output the selected row belongs to, or lets it go. What
    /// activating a level row does, since there is nothing to open.
    pub(super) fn toggle_settings_mute(self: &Rc<Self>, item: Item) {
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((Slider::Volume(role), scale, value)) = found else {
            return;
        };
        let muted = !self.config.borrow().muted(role);
        {
            let mut config = self.config.borrow_mut();
            config.set_volume(role, scale.value() / 100.0);
            config.set_muted(role, muted);
        }
        value.set_text(&volume_label(scale.value() / 100.0, muted));
        // On is unmuted, so the switch reads as the output being heard rather
        // than as the mute being applied. A silenced output's bar is dimmed
        // with it: the level it will come back to is worth still showing, and
        // moving it while nothing can be heard is not.
        scale.set_sensitive(!muted);
        value.set_sensitive(!muted);
        self.set_settings_switch(item, !muted);
        self.save_volume_soon();
    }

    /// Turns an output's delay on or off, keeping whatever it is set to.
    ///
    /// Off is how somebody checks whether a delay is helping: winding it to
    /// zero would answer the same question and lose the value they spent time
    /// finding.
    pub(super) fn toggle_settings_offset(self: &Rc<Self>, item: Item) {
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((Slider::Offset(role), scale, value)) = found else {
            return;
        };
        let on = !self.config.borrow().offset_on(role);
        {
            let mut config = self.config.borrow_mut();
            config.set_offset_on(role, on);
            let _ = config.save();
        }
        // Heard straight away, like the delay itself: the point of the switch
        // is comparing with and without while something is playing.
        self.push_offset_live(role);
        scale.set_sensitive(on);
        value.set_text(&offset_label(self.config.borrow().applied_offset_ms(role)));
        value.set_sensitive(on);
        self.set_settings_switch(item, on);
    }

    /// Where a slider stands now, and how that reads beside it.
    pub(super) fn slider_state(&self, kind: Slider) -> (f64, String) {
        let config = self.config.borrow();
        match kind {
            Slider::Volume(role) => {
                let level = config.volume(role);
                (level * 100.0, volume_label(level, config.muted(role)))
            }
            Slider::Offset(role) => {
                // The bar keeps the stored delay, so turning it back on shows
                // what it will be; the reading says what is actually being
                // applied, which while it is off is nothing.
                (
                    config.offset_ms(role),
                    offset_label(config.applied_offset_ms(role)),
                )
            }
            Slider::ResumeThreshold => {
                let percent = config.resume_min_percent().round();
                (percent, format!("{percent}%"))
            }
            Slider::WatchedThreshold => {
                let percent = config.watched_percent().round();
                (percent, format!("{percent}%"))
            }
            Slider::Scale => {
                // The bar sits at whatever size is in force either way, so
                // turning the switch off starts from what is on screen. The
                // reading says Auto rather than the number, since while the
                // switch is on that number is a consequence and not a
                // setting.
                let chosen = chosen_scale(&config);
                let scale = chosen.unwrap_or_else(|| self.scale.get());
                let reading = match chosen {
                    Some(scale) => scale_label(scale),
                    None => tr!("Auto").into_owned(),
                };
                (steps_from_scale(scale), reading)
            }
            Slider::SubtitleSize => {
                let size = config
                    .subtitle_size
                    .unwrap_or(crate::pipeline::DEFAULT_SUBTITLE_SIZE);
                (size as f64, size.to_string())
            }
        }
    }

    /// Writes a slider through to the configuration and puts the reading
    /// beside it in step. Turning an output up unmutes it, as the panel
    /// during playback does.
    pub(super) fn set_slider(self: &Rc<Self>, kind: Slider, moved: f64, value: &gtk::Label) {
        let range = kind.range();
        let moved = moved.clamp(*range.start(), *range.end());
        {
            let mut config = self.config.borrow_mut();
            match kind {
                Slider::Volume(role) => {
                    config.set_volume(role, moved / 100.0);
                    config.set_muted(role, false);
                }
                Slider::Offset(role) => config.set_offset_ms(role, moved),
                Slider::ResumeThreshold => config.resume_min_percent = Some(moved),
                Slider::WatchedThreshold => config.watched_percent = Some(moved),
                Slider::Scale => config.ui_scale = Some(scale_from_steps(moved)),
                Slider::SubtitleSize => config.subtitle_size = Some(moved.round() as u32),
            }
        }
        // Nothing redrawn here. Restyling moves the bar under whatever is
        // moving it, which GTK reads as another movement, which restyles
        // again - a loop that ran the size to its limit as soon as it was
        // dragged. Who calls this decides when it is safe: a key press
        // applies at once, a drag waits to be let go.
        // Heard straight away when a film is playing, so a delay can be placed
        // against the picture rather than guessed at and checked later.
        // The configuration above already holds `moved`, so this reads the
        // same number back rather than adding the baseline to it by hand.
        if let Slider::Offset(role) = kind {
            self.push_offset_live(role);
        }
        value.set_text(&match kind {
            Slider::Volume(_) => volume_label(moved / 100.0, false),
            Slider::Offset(_) => offset_label(moved),
            Slider::Scale => scale_label(scale_from_steps(moved)),
            Slider::SubtitleSize => format!("{}", moved.round()),
            _ => format!("{}%", moved.round()),
        });
        self.save_volume_soon();
    }

    /// Swaps the right-hand readout between the length and what is left.
    pub(super) fn toggle_time_readout(&self) {
        let controls = self.controls.borrow().clone();
        if let Some(controls) = controls {
            controls.toggle_remaining();
        }
    }

    /// Silences every output at once, or puts back what each was doing. The
    /// same thing holding the volume button does, reached directly.
    pub(super) fn toggle_mute(&self) {
        let controls = self.controls.borrow().clone();
        if let Some(controls) = controls {
            controls.toggle_hush();
        }
    }
}
