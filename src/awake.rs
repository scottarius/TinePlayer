//! Keeping the screen on while a film is playing.
//!
//! A video player is the one kind of application where the user is watching
//! intently and touching nothing, which is exactly what every screensaver and
//! display-sleep timer reads as "gone away". Without this, a long quiet scene
//! can end with the screen blanking on the viewer.
//!
//! Only while actually playing. Paused counts as away: a film paused an hour
//! ago should not be holding the display awake, and a viewer who paused to
//! leave the room would rather the screen slept.

use gtk::prelude::*;
use std::cell::Cell;

/// Holds the display awake, and lets go when asked or when dropped.
pub struct KeepAwake {
    app: gtk::Application,
    /// What GTK gave us in return for the last inhibit, or 0 for "not
    /// currently holding". GTK uses the same value to release it.
    cookie: Cell<u32>,
}

impl KeepAwake {
    pub fn new(app: &gtk::Application) -> Self {
        Self {
            app: app.clone(),
            cookie: Cell::new(0),
        }
    }

    /// Holds the screen awake, or stops holding it.
    ///
    /// Safe to call with the state it is already in: asking twice for the same
    /// thing does nothing, which means callers can simply say what should be
    /// true rather than track what they last asked for.
    pub fn set(&self, awake: bool) {
        if awake == (self.cookie.get() != 0) {
            return;
        }

        if awake {
            // IDLE only. Inhibiting suspend as well would stop the machine
            // sleeping, which is more than a player has any business doing:
            // the point is that the picture stays visible, not that the
            // computer never rests.
            //
            // The reason is what a desktop shows when it lists what is holding
            // the session awake. No window is passed with it, so a desktop
            // that would otherwise name the application may name the bare
            // process instead.
            let cookie = self.app.inhibit(
                None::<&gtk::Window>,
                gtk::ApplicationInhibitFlags::IDLE,
                Some("Playing a video"),
            );
            self.cookie.set(cookie);
            // A zero cookie means the session refused or has no way to do it.
            // Not worth reporting to the viewer, who can do nothing about it,
            // but the platform hold below is what covers the common case of
            // GTK having no implementation at all.
            hold_platform(true);
        } else {
            let cookie = self.cookie.replace(0);
            if cookie != 0 {
                self.app.uninhibit(cookie);
            }
            hold_platform(false);
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// What GTK's own inhibit does not reach.
///
/// On Windows, GTK has no implementation of this, so the only thing holding
/// the display awake is the call below. Left in place alongside the GTK one
/// rather than instead of it: they are independent, and on a platform where
/// both work, releasing both is what matters.
#[cfg(target_os = "windows")]
fn hold_platform(awake: bool) {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, SetThreadExecutionState,
    };
    // ES_CONTINUOUS makes the request stick until it is changed, rather than
    // counting as a single "something happened just now". Releasing means
    // setting it back to ES_CONTINUOUS alone.
    //
    // SAFETY: a plain system call taking a flags word, with no pointers and
    // no memory to get wrong. It reports the previous state, which we do not
    // need, and 0 for failure, which we cannot do anything about.
    unsafe {
        if awake {
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED);
        } else {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn hold_platform(_awake: bool) {}
