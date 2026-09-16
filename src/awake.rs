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
    /// Whether we are currently holding. Kept apart from `cookie` because a
    /// session that refuses the inhibit hands back 0, and a zero cookie would
    /// otherwise read as "not holding" and strand the platform hold below.
    holding: Cell<bool>,
    /// What GTK gave us in return for the last inhibit, or 0 for "GTK is not
    /// holding one". GTK uses the same value to release it.
    cookie: Cell<u32>,
}

impl KeepAwake {
    pub fn new(app: &gtk::Application) -> Self {
        Self {
            app: app.clone(),
            holding: Cell::new(false),
            cookie: Cell::new(0),
        }
    }

    /// Holds the screen awake, or stops holding it.
    ///
    /// Safe to call with the state it is already in: asking twice for the same
    /// thing does nothing, which means callers can simply say what should be
    /// true rather than track what they last asked for.
    pub fn set(&self, awake: bool) {
        if awake == self.holding.get() {
            return;
        }
        self.holding.set(awake);

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
/// On Windows and macOS, GTK has no implementation of this, so the only thing
/// holding the display awake is the call below. Left in place alongside the
/// GTK one rather than instead of it: they are independent, and on a platform
/// where both work, releasing both is what matters.
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

/// macOS, where the hold is an IOKit power assertion.
///
/// `PreventUserIdleDisplaySleep` is the narrow one: the display stays on while
/// nobody touches anything, and the machine is still free to sleep for any
/// other reason. Same scope as the IDLE flag handed to GTK above.
///
/// The assertion id has to survive until the release, and the only caller is
/// `KeepAwake` on the GTK main thread, so it lives here rather than widening
/// the struct with a field that exists on one platform.
#[cfg(target_os = "macos")]
fn hold_platform(awake: bool) {
    use objc2_foundation::NSString;
    use std::cell::Cell;
    use std::ffi::c_void;

    const ASSERTION_LEVEL_ON: u32 = 255;
    const SUCCESS: i32 = 0;

    // SAFETY of the block: the two calls below are the documented C interface,
    // taking toll-free-bridged CFStringRefs and an out parameter we own.
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: *const c_void,
            level: u32,
            name: *const c_void,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    thread_local! {
        static ASSERTION: Cell<u32> = const { Cell::new(0) };
    }

    ASSERTION.with(|assertion| {
        if awake {
            // NSString is toll-free bridged to CFString, so these pointers are
            // the CFStringRefs IOKit is asking for. Both outlive the call.
            let kind = NSString::from_str("PreventUserIdleDisplaySleep");
            let name = NSString::from_str("Playing a video");
            let mut id = 0u32;
            let status = unsafe {
                IOPMAssertionCreateWithName(
                    (&*kind as *const NSString).cast(),
                    ASSERTION_LEVEL_ON,
                    (&*name as *const NSString).cast(),
                    &mut id,
                )
            };
            // Nothing to tell the viewer, who can do nothing about it; a zero
            // id simply means the release below has nothing to do.
            if status == SUCCESS {
                assertion.set(id);
            }
        } else {
            let id = assertion.replace(0);
            if id != 0 {
                unsafe { IOPMAssertionRelease(id) };
            }
        }
    });
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn hold_platform(_awake: bool) {}
