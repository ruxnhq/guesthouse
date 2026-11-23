//! Asking an Android guest to shut down. A press of its power button only turns the screen off, as on
//! a phone, and a long one opens the power menu. Our image sets a long press to power off without
//! asking (`power_button_long_press` 3), so the stop holds the key down long enough to be one.

use crate::monitor::Command as Qmp;
use serde_json::json;

/// Longer than Android's long-press time, which is half a second by default.
const HOLD_MS: u32 = 1500;

pub(super) fn long_power_press() -> Qmp {
    Qmp::new("send-key", json!({ "keys": [{ "type": "qcode", "data": "power" }], "hold-time": HOLD_MS }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_power_key_is_held_past_a_long_press() {
        let asked = long_power_press();
        assert_eq!(asked.name, "send-key");
        assert_eq!(asked.arguments["keys"][0]["data"], "power");
        assert!(asked.arguments["hold-time"].as_u64().unwrap() > 500);
    }
}
