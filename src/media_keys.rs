//! The play, pause, stop and skip keys on a keyboard, a headset or a remote.
//!
//! Every platform delivers these differently, and only one of them delivers
//! them as keys at all:
//!
//! - **Linux** needs nothing here. X11 and Wayland report them as ordinary
//!   keysyms - `XF86AudioPlay` and its siblings - which `app.rs` matches like
//!   any other key. Verified 2026-08-09 on the Pi.
//! - **Windows** sends `WM_APPCOMMAND` to the focused window instead. The
//!   key events it also sends carry a keyval of `VoidSymbol`, with no identity
//!   to match on, because GDK's Windows backend has no mapping from the
//!   `VK_MEDIA_*` codes to those keysyms. Measured 2026-08-08. Hence the
//!   window subclass below, which is the documented way to receive them.
//! - **macOS** delivers them to whichever application the system considers to
//!   be playing, never to the focused window - which is why they drive Spotify
//!   in the background while TinePlayer is in front. No key handling can fix
//!   that; it needs `MPRemoteCommandCenter`, and is not done yet.

/// What a media key asked for, however the platform reported it.
///
/// `Play` and `Pause` are kept apart from `PlayPause` because Windows sends
/// all three, and a keyboard with separate play and pause keys means them
/// literally: play should not pause something already playing.
///
/// Only Windows ever reports those two, so nothing constructs them on the
/// other platforms - which is what the allowance below is for, rather than
/// dropping them and losing the distinction where it does exist.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub enum Command {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
}

#[cfg(target_os = "windows")]
mod platform {
    use super::Command;
    use std::cell::RefCell;
    use std::rc::Rc;

    use gtk::prelude::*;
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GetParent, IsWindowVisible,
    };

    const WM_APPCOMMAND: u32 = 0x0319;
    /// The top four bits of the command word say which device sent it, and
    /// are masked off before the command itself can be read.
    const FAPPCOMMAND_MASK: u16 = 0xF000;

    const APPCOMMAND_MEDIA_NEXTTRACK: u16 = 11;
    const APPCOMMAND_MEDIA_PREVIOUSTRACK: u16 = 12;
    const APPCOMMAND_MEDIA_STOP: u16 = 13;
    const APPCOMMAND_MEDIA_PLAY_PAUSE: u16 = 14;
    const APPCOMMAND_MEDIA_PLAY: u16 = 46;
    const APPCOMMAND_MEDIA_PAUSE: u16 = 47;

    /// Any value, so long as it is ours: Windows uses it to tell one
    /// subclass of the same window from another.
    const SUBCLASS_ID: usize = 1;

    /// What the window procedure calls when a media key arrives, and whether
    /// it used the key.
    type Handler = Rc<dyn Fn(Command) -> bool>;

    thread_local! {
        /// Held here rather than passed through `dwRefData`, which would mean
        /// handing a raw pointer to a closure across an FFI boundary and
        /// arranging to free it again. The window lives as long as the
        /// process, and this is only ever touched from the main thread.
        static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    }

    fn command_for(id: u16) -> Option<Command> {
        Some(match id {
            APPCOMMAND_MEDIA_PLAY_PAUSE => Command::PlayPause,
            APPCOMMAND_MEDIA_PLAY => Command::Play,
            APPCOMMAND_MEDIA_PAUSE => Command::Pause,
            APPCOMMAND_MEDIA_STOP => Command::Stop,
            APPCOMMAND_MEDIA_NEXTTRACK => Command::Next,
            APPCOMMAND_MEDIA_PREVIOUSTRACK => Command::Previous,
            _ => return None,
        })
    }

    /// Sits in front of GTK's own window procedure, takes the media keys, and
    /// passes everything else straight on.
    ///
    /// Returning 1 rather than falling through to `DefSubclassProc` is what
    /// stops Windows handing the key on to whatever else would have played:
    /// unhandled, it goes to the shell, which routes it to another player.
    ///
    /// Which is why the handler answers whether it used the key. With nothing
    /// playing there is nothing for it to mean here, and passing it on lets
    /// somebody sitting in the menus still pause their music.
    unsafe extern "system" fn subclass_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if message == WM_APPCOMMAND {
            let id = ((lparam as u32 >> 16) as u16) & !FAPPCOMMAND_MASK;
            if let Some(command) = command_for(id) {
                // Cloned out before calling, so the handler is free to do
                // anything at all without this borrow still being held.
                let handler = HANDLER.with(|handler| handler.borrow().clone());
                if let Some(handler) = handler
                    && handler(command)
                {
                    return 1;
                }
            }
        }
        unsafe { DefSubclassProc(window, message, wparam, lparam) }
    }

    /// Picks up the one visible top-level window this thread owns.
    ///
    /// `gdk4-win32` would hand the `HWND` over directly and is deliberately
    /// not used: it will not link against the GTK that GStreamer's Windows
    /// distribution supplies, which exports no `gdk_win32_screen_get_type`.
    /// Asking Windows costs one enumeration and no dependency at all.
    ///
    /// GTK also creates invisible utility windows on this thread, which is
    /// what the two tests are for. TinePlayer has exactly one window on
    /// screen, so there is nothing to disambiguate beyond that.
    unsafe extern "system" fn take_window(window: HWND, out: LPARAM) -> BOOL {
        // SAFETY: both take a window handle Windows has just given us.
        let top_level = unsafe { GetParent(window) }.is_null();
        let visible = unsafe { IsWindowVisible(window) } != 0;
        if top_level && visible {
            // SAFETY: `out` is the address of the caller's `HWND`, which
            // outlives this enumeration.
            unsafe { *(out as *mut HWND) = window };
            return 0; // Stop: one is all we want.
        }
        1
    }

    /// Puts the subclass on the application's window, once there is one.
    ///
    /// Waits for the window to be mapped rather than trusting `present` to
    /// have finished: presenting only asks for the window, and for a moment
    /// afterwards there is either no `HWND` at all or one Windows still calls
    /// invisible - which is exactly what `attach` refuses to match on.
    pub fn install(window: &gtk::ApplicationWindow, handler: impl Fn(Command) -> bool + 'static) {
        let handler: Handler = Rc::new(handler);
        HANDLER.with(|held| *held.borrow_mut() = Some(handler));

        if window.is_mapped() {
            attach();
        } else {
            window.connect_map(|_| attach());
        }
    }

    fn attach() {
        let mut found: HWND = std::ptr::null_mut();
        // SAFETY: a callback that matches the signature Windows expects, and
        // a pointer to a local that outlives the call.
        unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(take_window),
                &mut found as *mut HWND as LPARAM,
            );
        }
        if found.is_null() {
            return;
        }
        // SAFETY: a handle Windows just gave us, a function pointer that
        // outlives the process, and a call on the thread that owns the
        // window - which is what Windows requires.
        unsafe {
            SetWindowSubclass(found, Some(subclass_proc), SUBCLASS_ID, 0);
        }
    }
}

/// Linux reports these as keysyms and macOS does not report them at all, so
/// there is nothing to install on either.
#[cfg(not(target_os = "windows"))]
mod platform {
    use super::Command;

    pub fn install(_window: &gtk::ApplicationWindow, _handler: impl Fn(Command) -> bool + 'static) {
    }
}

pub use platform::install;
