//! Gamepad navigation.
//!
//! Polled from the GTK main loop rather than run on its own thread, so the
//! handler runs where every other input handler does and needs no
//! synchronization with the widgets it drives.
//!
//! Nothing here refers to a particular controller. Devices are discovered by
//! gilrs and iterated as a set, so one connected halfway through a session
//! works immediately and one unplugged mid-session simply stops contributing.

use std::time::{Duration, Instant};

use gilrs::{Axis, Button, EventType, Gilrs};
use gtk::glib;

/// What a control does, rather than which control it was. Keyboard and
/// gamepad both reduce to these so neither owns the behavior.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Activate,
    Back,
    /// The held direction was let go. Playback uses it to end a scrub; the
    /// menus ignore it.
    DirectionReleased,
    /// The lower face button was let go. Only the controls listen for it,
    /// where holding a button means something other than pressing it.
    ActivateReleased,
    /// The same for the left face button, which is held to silence
    /// everything and tapped to change the subtitles.
    SubtitlesReleased,
    PageUp,
    PageDown,
    /// The next or previous thing worth stopping on, which is what Tab does
    /// on a keyboard: out of a list and onto the buttons, rather than another
    /// step within it.
    FocusNext,
    FocusPrevious,
    PlayPause,
    Fullscreen,
    Subtitles,
    /// Swap the right-hand readout between the running time and what is left
    /// of it.
    TimeReadout,
    /// Show the list of keys and buttons, or put it away.
    Shortcuts,
}

/// 60Hz. Polling has to be fast enough that a press feels immediate, and this
/// costs nothing measurable: with no controller attached it is an empty queue
/// check.
const POLL: Duration = Duration::from_millis(16);
/// Held-direction repeat, matching the feel of keyboard auto-repeat: a pause
/// before the first repeat so a single nudge moves one row, then steady
/// movement for scrolling a long list of devices.
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_INTERVAL: Duration = Duration::from_millis(120);
/// Well past the resting noise of a worn stick, and far enough that a
/// diagonal push does not register as both axes at once.
const DEADZONE: f32 = 0.6;

/// Starts polling. Does nothing but warn if gamepad support is unavailable,
/// since the application is perfectly usable without it.
pub fn install<F: Fn(Action) + 'static>(handler: F) {
    let mut gilrs = match Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(e) => {
            crate::log!("Gamepad support unavailable: {e}");
            return;
        }
    };

    // The direction currently held, and when it should next fire.
    let mut held: Option<(Action, Instant)> = None;

    glib::timeout_add_local(POLL, move || {
        // Draining the queue is also what keeps the button and axis state
        // below current, so it happens whether or not anything is mapped.
        while let Some(event) = gilrs.next_event() {
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(action) = button_action(button) {
                        handler(action);
                    }
                }
                // Only the two that are held for meaning: a release for every
                // button would be noise to filter.
                EventType::ButtonReleased(Button::South, _) => {
                    handler(Action::ActivateReleased);
                }
                EventType::ButtonReleased(Button::West, _) => {
                    handler(Action::SubtitlesReleased);
                }
                _ => {}
            }
        }

        // Directions are read from current state rather than from press
        // events, because holding one has to keep repeating, and because the
        // stick produces a stream of axis events rather than a press.
        let now = Instant::now();
        match (direction(&gilrs), held) {
            (Some(action), Some((previous, next))) if previous == action => {
                if now >= next {
                    handler(action);
                    held = Some((action, now + REPEAT_INTERVAL));
                }
            }
            (Some(action), _) => {
                handler(action);
                held = Some((action, now + REPEAT_DELAY));
            }
            (None, Some(_)) => {
                held = None;
                handler(Action::DirectionReleased);
            }
            (None, None) => {}
        }

        glib::ControlFlow::Continue
    });
}

/// Face buttons, in the layout gilrs normalises to: South is the lower face
/// button (A on an Xbox pad, Cross on a PlayStation one), East the right.
fn button_action(button: Button) -> Option<Action> {
    match button {
        Button::South => Some(Action::Activate),
        Button::East => Some(Action::Back),
        Button::North => Some(Action::Fullscreen),
        Button::West => Some(Action::Subtitles),
        // The bumpers move between elements and the triggers move by the page,
        // which puts the coarser jump on the harder pull - and is what makes a
        // folder of a hundred films navigable. gilrs names the bumpers
        // LeftTrigger and the triggers LeftTrigger2, which reads backwards but
        // is what the crate calls them.
        Button::LeftTrigger => Some(Action::FocusPrevious),
        Button::RightTrigger => Some(Action::FocusNext),
        Button::LeftTrigger2 => Some(Action::PageUp),
        Button::RightTrigger2 => Some(Action::PageDown),
        Button::Start => Some(Action::PlayPause),
        // Select, which on a pad is where "what do the buttons do" has lived
        // since long before this application. gilrs also reports it as `Mode`
        // on some layouts, so both are taken.
        Button::Select | Button::Mode => Some(Action::Shortcuts),
        // Clicking the right stick: a deliberate press that nothing else
        // wants, for a readout somebody either cares about or never touches.
        Button::RightThumb => Some(Action::TimeReadout),
        _ => None,
    }
}

/// The direction any connected controller is currently indicating.
///
/// Both the D-pad and the left stick, because which one a person reaches for
/// is not predictable, and some controllers report their D-pad as an axis
/// anyway.
fn direction(gilrs: &Gilrs) -> Option<Action> {
    for (_id, gamepad) in gilrs.gamepads() {
        let from_dpad = if gamepad.is_pressed(Button::DPadUp) {
            Some(Action::Up)
        } else if gamepad.is_pressed(Button::DPadDown) {
            Some(Action::Down)
        } else if gamepad.is_pressed(Button::DPadLeft) {
            Some(Action::Left)
        } else if gamepad.is_pressed(Button::DPadRight) {
            Some(Action::Right)
        } else {
            None
        };
        if from_dpad.is_some() {
            return from_dpad;
        }

        // Vertical first: in a list of rows, up and down are what matter, and
        // testing them first stops a slightly-off vertical push from reading
        // as horizontal.
        let vertical = gamepad.value(Axis::LeftStickY);
        if vertical > DEADZONE {
            return Some(Action::Up);
        }
        if vertical < -DEADZONE {
            return Some(Action::Down);
        }
        let horizontal = gamepad.value(Axis::LeftStickX);
        if horizontal < -DEADZONE {
            return Some(Action::Left);
        }
        if horizontal > DEADZONE {
            return Some(Action::Right);
        }
    }
    None
}
