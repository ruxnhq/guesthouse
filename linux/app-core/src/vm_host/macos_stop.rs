//! Asking a macOS guest to shut down. It has no agent, and its power button only puts a question on
//! its screen, which nobody may be looking at. Control-Option-Command-Power on its keyboard quits
//! every app and shuts down, asking only about documents that aren't saved.

use crate::monitor::Command as Qmp;
use serde_json::json;

const KEYS: [&str; 4] = ["ctrl", "alt", "meta_l", "power"];
/// Long enough for macOS to see the keys as held together.
const HOLD_MS: u32 = 200;
/// The part of a stop's time the keys get before the power button is pressed as well, for a guest
/// they didn't reach.
pub(super) const KEYS_SHARE: u64 = 3;

pub(super) fn shutdown_keys() -> Qmp {
    let keys: Vec<_> = KEYS.iter().map(|key| json!({ "type": "qcode", "data": key })).collect();
    Qmp::new("send-key", json!({ "keys": keys, "hold-time": HOLD_MS }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_keys_go_down_together() {
        let asked = shutdown_keys();
        assert_eq!(asked.name, "send-key");
        let held: Vec<&str> = asked.arguments["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key["data"].as_str().unwrap())
            .collect();
        assert_eq!(held, KEYS);
        assert_eq!(asked.arguments["hold-time"], 200);
    }
}
