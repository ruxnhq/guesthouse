// SPDX-License-Identifier: GPL-2.0-or-later
//! The window's mouse and keyboard as QMP `input-send-event` events. The guest has a USB tablet
//! and a USB keyboard; this keeps track of what is held so it can all be let go at once, and
//! turns the window's points into the tablet's range.
//!
//! Nothing here touches a socket: each call returns the events for one command, or none, and
//! `link` sends them.

use crate::keys;
use serde_json::{Value, json};

/// The tablet's range on each axis, 0 to this (`INPUT_EVENT_ABS_MAX` in QEMU).
pub const ABS_MAX: f64 = 32767.0;
/// How many points of a trackpad's scroll make one wheel step. A guess, to tune by hand.
const POINTS_PER_WHEEL_STEP: f64 = 40.0;
/// A mouse wheel reports lines; one line is one step.
const LINES_PER_WHEEL_STEP: f64 = 1.0;

/// A mouse button, as the window numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    /// The window's button number (`NSEvent.buttonNumber`): 0 left, 1 right, 2 middle.
    pub fn from_number(number: u32) -> Option<Button> {
        match number {
            0 => Some(Button::Left),
            1 => Some(Button::Right),
            2 => Some(Button::Middle),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
        }
    }
}

/// The size of the view and of the picture in it. The picture is fitted inside the view
/// keeping its shape, with bars on two sides.
#[derive(Clone, Copy, Debug)]
pub struct Fit {
    pub view_width: f64,
    pub view_height: f64,
    pub picture_width: f64,
    pub picture_height: f64,
}

impl Fit {
    /// Where the point (`x`, `y`) of the view, from its top left, falls on the tablet. A point
    /// in the bars is taken to the picture's nearest edge. `None` while anything is empty.
    pub fn tablet(self, x: f64, y: f64) -> Option<(u32, u32)> {
        let sizes = [self.view_width, self.view_height, self.picture_width, self.picture_height];
        if sizes.iter().any(|size| !size.is_finite() || *size <= 0.0) {
            return None;
        }
        let scale = (self.view_width / self.picture_width).min(self.view_height / self.picture_height);
        let width = self.picture_width * scale;
        let height = self.picture_height * scale;
        let left = (self.view_width - width) / 2.0;
        let top = (self.view_height - height) / 2.0;
        let to_tablet = |fraction: f64| (fraction.clamp(0.0, 1.0) * ABS_MAX).round() as u32;
        Some((to_tablet((x - left) / width), to_tablet((y - top) / height)))
    }
}

/// What the guest has been told is held, and the scroll not yet sent.
#[derive(Default)]
pub struct Input {
    /// Keys down, by QEMU name, in the order they went down.
    keys: Vec<&'static str>,
    buttons: Vec<Button>,
    /// The tablet position last sent, so a move that lands on the same spot sends nothing.
    position: Option<(u32, u32)>,
    scroll_x: f64,
    scroll_y: f64,
    /// Whether the scroll gathered so far is in points (true) or lines.
    scroll_precise: bool,
}

impl Input {
    pub fn pointer(&mut self, fit: Fit, x: f64, y: f64) -> Vec<Value> {
        let Some(position) = fit.tablet(x, y) else {
            return Vec::new();
        };
        if self.position == Some(position) {
            return Vec::new();
        }
        self.position = Some(position);
        vec![abs_event("x", position.0), abs_event("y", position.1)]
    }

    pub fn button(&mut self, button: Button, down: bool) -> Vec<Value> {
        let held = self.buttons.contains(&button);
        if down == held {
            return Vec::new();
        }
        if down {
            self.buttons.push(button);
        } else {
            self.buttons.retain(|b| *b != button);
        }
        vec![button_event(button.name(), down)]
    }

    /// A scroll of (`dx`, `dy`), in points when `precise` (a trackpad) and in lines when not
    /// (a mouse wheel). Positive `dy` moves the page down to show what is above, which is the
    /// wheel turned up; positive `dx` is the wheel tilted left. Each whole step gathered is
    /// one press and release of a wheel button, as the tablet counts them.
    pub fn scroll(&mut self, dx: f64, dy: f64, precise: bool) -> Vec<Value> {
        if precise != self.scroll_precise {
            // Points and lines don't add up: start again in the new unit.
            self.scroll_precise = precise;
            self.scroll_x = 0.0;
            self.scroll_y = 0.0;
        }
        let step = if precise { POINTS_PER_WHEEL_STEP } else { LINES_PER_WHEEL_STEP };
        self.scroll_x += dx;
        self.scroll_y += dy;
        let mut events = Vec::new();
        for (gathered, positive, negative) in [
            (&mut self.scroll_y, "wheel-up", "wheel-down"),
            (&mut self.scroll_x, "wheel-left", "wheel-right"),
        ] {
            while gathered.abs() >= step {
                let name = if *gathered > 0.0 { positive } else { negative };
                *gathered -= step.copysign(*gathered);
                events.push(button_event(name, true));
                events.push(button_event(name, false));
            }
        }
        events
    }

    /// A key that is not a modifier went down or up. A repeat while held sends nothing more.
    pub fn key(&mut self, code: u16, down: bool) -> Vec<Value> {
        let Some(name) = keys::qcode(code) else {
            return Vec::new();
        };
        let held = self.keys.contains(&name);
        if down == held {
            return Vec::new();
        }
        if down {
            self.keys.push(name);
        } else {
            self.keys.retain(|k| *k != name);
        }
        vec![key_event(name, down)]
    }

    /// A modifier key changed (`flagsChanged`). macOS says only which key changed and whether
    /// its kind of modifier is now on; with both Shift keys down, letting one go leaves Shift
    /// on. So a held key goes up, a key not held goes down while its modifier is on, and a
    /// release of a key the guest never saw down is dropped.
    pub fn modifier(&mut self, code: u16, flag_on: bool) -> Vec<Value> {
        let Some(name) = keys::qcode(code) else {
            return Vec::new();
        };
        if code == keys::CAPS_LOCK {
            return vec![key_event(name, true), key_event(name, false)];
        }
        if self.keys.contains(&name) {
            self.key(code, false)
        } else if flag_on {
            self.key(code, true)
        } else {
            Vec::new()
        }
    }

    /// Lets go of every key and button still held, last pressed first.
    pub fn release_all(&mut self) -> Vec<Value> {
        let mut events: Vec<Value> = self.keys.drain(..).rev().map(|k| key_event(k, false)).collect();
        events.extend(self.buttons.drain(..).rev().map(|b| button_event(b.name(), false)));
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        events
    }
}

/// One QMP command line carrying `events`.
pub fn command(events: &[Value]) -> String {
    let command = json!({ "execute": "input-send-event", "arguments": { "events": events } });
    format!("{command}\r\n")
}

fn abs_event(axis: &str, value: u32) -> Value {
    json!({ "type": "abs", "data": { "axis": axis, "value": value } })
}

fn button_event(name: &str, down: bool) -> Value {
    json!({ "type": "btn", "data": { "down": down, "button": name } })
}

fn key_event(name: &str, down: bool) -> Value {
    json!({ "type": "key", "data": { "down": down, "key": { "type": "qcode", "data": name } } })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1080p picture in a square view: bars above and below.
    const SQUARE: Fit =
        Fit { view_width: 1000.0, view_height: 1000.0, picture_width: 1920.0, picture_height: 1080.0 };

    #[test]
    fn tablet_maps_the_picture_not_the_bars() {
        // The picture is 1000 x 562.5, from y 218.75 to 781.25.
        assert_eq!(SQUARE.tablet(500.0, 500.0), Some((16384, 16384)));
        assert_eq!(SQUARE.tablet(0.0, 218.75), Some((0, 0)));
        assert_eq!(SQUARE.tablet(1000.0, 781.25), Some((32767, 32767)));
        // In the bars: the nearest edge.
        assert_eq!(SQUARE.tablet(250.0, 10.0), Some((8192, 0)));
        assert_eq!(SQUARE.tablet(250.0, 990.0), Some((8192, 32767)));
    }

    #[test]
    fn tablet_with_bars_at_the_sides() {
        let wide =
            Fit { view_width: 2000.0, view_height: 500.0, picture_width: 800.0, picture_height: 500.0 };
        // The picture is 800 wide, from x 600 to 1400.
        assert_eq!(wide.tablet(600.0, 0.0), Some((0, 0)));
        assert_eq!(wide.tablet(1000.0, 250.0), Some((16384, 16384)));
        assert_eq!(wide.tablet(100.0, 250.0), Some((0, 16384)));
    }

    #[test]
    fn no_tablet_position_without_a_picture() {
        let empty = Fit { picture_width: 0.0, ..SQUARE };
        assert_eq!(empty.tablet(1.0, 1.0), None);
    }

    #[test]
    fn same_position_is_sent_once() {
        let mut input = Input::default();
        assert_eq!(input.pointer(SQUARE, 500.0, 500.0).len(), 2);
        assert!(input.pointer(SQUARE, 500.0, 500.0).is_empty());
    }

    #[test]
    fn repeats_and_unheld_releases_send_nothing() {
        let mut input = Input::default();
        assert_eq!(input.key(0x00, true), [key_event("a", true)]);
        assert!(input.key(0x00, true).is_empty());
        assert_eq!(input.key(0x00, false), [key_event("a", false)]);
        assert!(input.key(0x00, false).is_empty());
        assert!(input.button(Button::Left, false).is_empty());
    }

    #[test]
    fn both_shift_keys() {
        let mut input = Input::default();
        assert_eq!(input.modifier(0x38, true), [key_event("shift", true)]);
        assert_eq!(input.modifier(0x3C, true), [key_event("shift_r", true)]);
        // Left Shift up, but Shift stays on for the right one.
        assert_eq!(input.modifier(0x38, true), [key_event("shift", false)]);
        assert_eq!(input.modifier(0x3C, false), [key_event("shift_r", false)]);
        // A Shift that was down before the window had the keyboard.
        assert!(input.modifier(0x38, false).is_empty());
    }

    #[test]
    fn caps_lock_is_a_tap_each_time() {
        let mut input = Input::default();
        let tap = [key_event("caps_lock", true), key_event("caps_lock", false)];
        assert_eq!(input.modifier(keys::CAPS_LOCK, true), tap);
        assert_eq!(input.modifier(keys::CAPS_LOCK, false), tap);
        assert!(input.release_all().is_empty());
    }

    #[test]
    fn wheel_steps_gather() {
        let mut input = Input::default();
        assert_eq!(
            input.scroll(0.0, 1.0, false),
            [button_event("wheel-up", true), button_event("wheel-up", false)]
        );
        assert!(input.scroll(0.0, -30.0, true).is_empty());
        assert_eq!(
            input.scroll(0.0, -30.0, true),
            [button_event("wheel-down", true), button_event("wheel-down", false)]
        );
        assert_eq!(input.scroll(-2.0, 0.0, false).len(), 4);
    }

    #[test]
    fn release_all_lets_go_last_first() {
        let mut input = Input::default();
        input.modifier(0x3B, true);
        input.key(0x08, true);
        input.button(Button::Right, true);
        assert_eq!(
            input.release_all(),
            [key_event("c", false), key_event("ctrl", false), button_event("right", false)]
        );
        assert!(input.release_all().is_empty());
    }

    #[test]
    fn command_is_one_line() {
        let line = command(&[key_event("a", true)]);
        assert!(line.ends_with("\r\n"));
        assert_eq!(line.matches('\n').count(), 1);
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["execute"], "input-send-event");
        assert_eq!(value["arguments"]["events"][0]["data"]["key"]["data"], "a");
    }
}
