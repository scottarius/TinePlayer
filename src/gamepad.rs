//! Gamepad navigation.
//!
//! Polled from the GTK main loop rather than run on its own thread, so the
//! handler runs where every other input handler does and needs no
//! synchronisation with the widgets it drives.
//!
//! Nothing here refers to a particular controller. Devices are discovered by
//! gilrs and iterated as a set, so one connected halfway through a session
//! works immediately and one unplugged mid-session simply stops contributing.

use std::time::{Duration, Instant};

use gilrs::{Axis, Button, EventType, Gilrs};
use gtk::glib;

/// What a control does, rather than which control it was. Keyboard and
/// gamepad both reduce to these so neither owns the behaviour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Activate,
    Back,
    PlayPause,
    Fullscreen,
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
            eprintln!("Gamepad support unavailable: {e}");
            return;
        }
    };

    // The direction currently held, and when it should next fire.
    let mut held: Option<(Action, Instant)> = None;

    glib::timeout_add_local(POLL, move || {
        // Draining the queue is also what keeps the button and axis state
        // below current, so it happens whether or not anything is mapped.
        while let Some(event) = gilrs.next_event() {
            if let EventType::ButtonPressed(button, _) = event.event
                && let Some(action) = button_action(button)
            {
                handler(action);
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
            (None, _) => held = None,
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
        Button::Start => Some(Action::PlayPause),
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
