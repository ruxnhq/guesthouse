//! The Linux host's machine states, held as they are: the five of them, the word each is shown
//! with, and that a machine is off until something says otherwise.

#![cfg(target_os = "linux")]

use guesthouse_app_core::tasks::State;

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
