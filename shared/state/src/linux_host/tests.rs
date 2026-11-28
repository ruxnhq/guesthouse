//! The Linux host's states as they were held in `app-core` before they moved here.

use super::*;

#[test]
fn each_state_and_its_word() {
    for (state, word) in [
        (State::Off, "Off"),
        (State::Starting, "Starting"),
        (State::Installing, "Installing"),
        (State::Running, "Running"),
        (State::Stopping, "Stopping"),
    ] {
        assert_eq!(state.name(), word);
    }
    assert_eq!(State::default(), State::Off);
}
