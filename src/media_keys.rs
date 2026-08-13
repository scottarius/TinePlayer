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
//!   be playing, never to the focused window - which is why they used to drive
//!   Spotify in the background while TinePlayer was in front. No key handling
//!   can fix that, so this registers with `MPRemoteCommandCenter` instead and
//!   publishes what is playing to `MPNowPlayingInfoCenter`, which is how the
//!   system decides where to send them. That also puts the film in Control
//!   Center and on the lock screen.

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

/// What the operating system should say is playing, and route media keys by.
///
/// Only macOS has anywhere to put this today. It is not decoration there:
/// macOS chooses which application receives a media key by who has published
/// now-playing information, so a player that publishes none is never sent one.
///
/// Elapsed time and rate are published together and deliberately not on a
/// timer. The system extrapolates the position from the two, so it stays
/// correct between updates, and pushing it every tick would be both wasteful
/// and no more accurate.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct NowPlaying {
    pub title: String,
    pub duration_s: f64,
    pub elapsed_s: f64,
    pub playing: bool,
    /// The poster, as the page found it: a file beside the video, or cover
    /// art carried in the container. Read here rather than taken as pixels
    /// because AppKit will decode either form itself, and the menu may never
    /// have decoded it at all.
    pub artwork: Option<crate::metadata::Art>,
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{Command, NowPlaying};
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

    /// Nothing to tell. Windows routes a media key to the foreground window,
    /// so being in front is the whole qualification and there is no
    /// now-playing state to publish for it.
    pub fn set_now_playing(_state: Option<NowPlaying>) {}
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ptr::NonNull;
    use std::rc::Rc;

    use block2::RcBlock;
    // `alloc` lives on this trait rather than on the classes themselves.
    use objc2::AllocAnyThread;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_core_foundation::CGSize;
    use objc2_foundation::{NSData, NSDictionary, NSNumber, NSString};
    use objc2_media_player::{
        MPMediaItemArtwork, MPMediaItemPropertyArtwork, MPMediaItemPropertyPlaybackDuration,
        MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
        MPNowPlayingInfoPropertyElapsedPlaybackTime, MPNowPlayingInfoPropertyPlaybackRate,
        MPNowPlayingPlaybackState, MPRemoteCommandCenter, MPRemoteCommandEvent,
        MPRemoteCommandHandlerStatus,
    };

    use super::{Command, NowPlaying};

    /// Decodes the poster for the now-playing widget, or gives up quietly.
    ///
    /// AppKit reads whatever the file or the tag actually holds - JPEG, PNG,
    /// anything it knows - so nothing here has to care which. A poster that
    /// will not decode is simply absent: the widget is perfectly good without
    /// one, and refusing to publish the rest over it would be worse.
    fn decode(art: &crate::metadata::Art) -> Option<Retained<NSImage>> {
        // No unsafe on these: the bindings declare them safe, because loading
        // a picture cannot be made to misbehave by its arguments.
        match art {
            crate::metadata::Art::Path(path) => {
                let path = NSString::from_str(&path.to_string_lossy());
                NSImage::initWithContentsOfFile(NSImage::alloc(), &path)
            }
            crate::metadata::Art::Embedded(bytes) => {
                let data = NSData::with_bytes(bytes);
                NSImage::initWithData(NSImage::alloc(), &data)
            }
        }
    }

    /// TinePlayer's own icon, for a film that has no artwork of its own.
    ///
    /// Something has to be published. Left empty, the widget draws the
    /// application icon itself at a fraction of the space, which reads as a
    /// picture that failed to load rather than as a film without one. Handing
    /// the same icon over as artwork fills the slot properly.
    fn fallback_image() -> Option<Retained<NSImage>> {
        let mtm = MainThreadMarker::new()?;
        NSApplication::sharedApplication(mtm).applicationIconImage()
    }

    fn poster(art: Option<&crate::metadata::Art>) -> Option<Retained<MPMediaItemArtwork>> {
        let image = art.and_then(decode).or_else(fallback_image)?;

        // The handler is asked for a size and may be called more than once, at
        // more than one size. Handing back the one image each time is what
        // AppKit expects of a still picture: it scales, and a poster has no
        // better answer at one size than another.
        let size = image.size();
        let handler = RcBlock::new(move |_wanted: CGSize| NonNull::from(&*image));
        // SAFETY: bounds and a block of the declared shape.
        Some(unsafe {
            MPMediaItemArtwork::initWithBoundsSize_requestHandler(
                MPMediaItemArtwork::alloc(),
                size,
                &handler,
            )
        })
    }

    /// Registers for the six transport commands.
    ///
    /// The window is unused: these are addressed to the application rather
    /// than to a window, which is the whole difference from Windows.
    ///
    /// Handlers are called on the main thread, which is this one, so the
    /// non-`Send` handler shared between the six blocks is sound. `Rc` rather
    /// than a clone each because the closure has to own what it captures and
    /// there are six of them.
    pub fn install(_window: &gtk::ApplicationWindow, handler: impl Fn(Command) -> bool + 'static) {
        let handler = Rc::new(handler);
        // SAFETY: every call below is an Objective-C message to a shared
        // singleton, with arguments of the types its interface declares.
        unsafe {
            let center = MPRemoteCommandCenter::sharedCommandCenter();
            let commands = [
                (center.togglePlayPauseCommand(), Command::PlayPause),
                (center.playCommand(), Command::Play),
                (center.pauseCommand(), Command::Pause),
                (center.stopCommand(), Command::Stop),
                (center.nextTrackCommand(), Command::Next),
                (center.previousTrackCommand(), Command::Previous),
            ];
            for (command, which) in commands {
                let handler = handler.clone();
                let block = RcBlock::new(move |_event: NonNull<MPRemoteCommandEvent>| {
                    // Answering NoSuchContent rather than Success where the
                    // key went unused is what lets the system offer it to
                    // something else, which is the same courtesy the Windows
                    // backend does by falling through to DefSubclassProc.
                    match handler(which) {
                        true => MPRemoteCommandHandlerStatus::Success,
                        false => MPRemoteCommandHandlerStatus::NoSuchContent,
                    }
                });
                command.setEnabled(true);
                // The command center copies the block, so ours can go.
                command.addTargetWithHandler(&block);
            }
        }
    }

    /// Publishes what is playing, or that nothing is.
    ///
    /// Rate is the honest one rather than always 1.0: the system reads it to
    /// extrapolate the position, so a paused film reporting 1.0 would appear
    /// to keep advancing on the lock screen.
    pub fn set_now_playing(state: Option<NowPlaying>) {
        // SAFETY: as above - messages to a singleton with declared types.
        unsafe {
            let center = MPNowPlayingInfoCenter::defaultCenter();
            let Some(state) = state else {
                // Stopped before the dictionary goes, not after. Publishing a
                // state against an entry that has already been removed is what
                // left TinePlayer sitting in the widget with a dead play
                // button once playback had ended.
                center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
                center.setNowPlayingInfo(None);
                return;
            };

            let title = NSString::from_str(&state.title);
            let duration = NSNumber::new_f64(state.duration_s);
            let elapsed = NSNumber::new_f64(state.elapsed_s);
            let rate = NSNumber::new_f64(match state.playing {
                true => 1.0,
                false => 0.0,
            });

            // Built with the artwork or without it rather than inserted
            // afterwards, because the dictionary is immutable once made and
            // the poster is the one part that may not be there.
            let art = poster(state.artwork.as_ref());
            let info = match art.as_ref() {
                Some(art) => NSDictionary::<NSString, AnyObject>::from_slices(
                    &[
                        MPMediaItemPropertyTitle,
                        MPMediaItemPropertyPlaybackDuration,
                        MPNowPlayingInfoPropertyElapsedPlaybackTime,
                        MPNowPlayingInfoPropertyPlaybackRate,
                        MPMediaItemPropertyArtwork,
                    ],
                    &[&*title, &*duration, &*elapsed, &*rate, &**art],
                ),
                None => NSDictionary::<NSString, AnyObject>::from_slices(
                    &[
                        MPMediaItemPropertyTitle,
                        MPMediaItemPropertyPlaybackDuration,
                        MPNowPlayingInfoPropertyElapsedPlaybackTime,
                        MPNowPlayingInfoPropertyPlaybackRate,
                    ],
                    &[&*title, &*duration, &*elapsed, &*rate],
                ),
            };
            center.setNowPlayingInfo(Some(&info));
            center.setPlaybackState(match state.playing {
                true => MPNowPlayingPlaybackState::Playing,
                false => MPNowPlayingPlaybackState::Paused,
            });
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod platform {
    use super::{Command, NowPlaying};

    pub fn install(_window: &gtk::ApplicationWindow, _handler: impl Fn(Command) -> bool + 'static) {
    }

    /// Nothing to tell. Linux has no equivalent until there is an MPRIS
    /// service, and the keys already arrive there without one.
    pub fn set_now_playing(_state: Option<NowPlaying>) {}
}

pub use platform::{install, set_now_playing};
