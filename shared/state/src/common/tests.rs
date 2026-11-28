//! Every state's word and its name on the wire, held as they are.

use super::*;

const ALL: [(State, &str, &str); 7] = [
    (State::Off, "off", "Off"),
    (State::Starting, "starting", "Starting"),
    (State::Installing, "installing", "Installing"),
    (State::Running, "running", "Running"),
    (State::Stopping, "stopping", "Stopping"),
    (State::StoppingStalled, "stopping_stalled", "Still stopping"),
    (State::Failed, "failed", "Failed"),
];

#[test]
fn each_state_its_wire_name_and_its_word() {
    for (state, wire, word) in ALL {
        let quoted = format!("\"{wire}\"");
        assert_eq!(serde_json::to_string(&state).unwrap(), quoted);
        assert_eq!(serde_json::from_str::<State>(&quoted).unwrap(), state);
        assert_eq!(state.name(), word);
    }
    assert_eq!(State::default(), State::Off);
}

#[test]
fn a_name_that_isnt_one_is_refused() {
    for name in ["Off", "stopped", "preparing", "shutting_down", "stoppingstalled", ""] {
        assert!(serde_json::from_str::<State>(&format!("\"{name}\"")).is_err(), "{name}");
    }
}
