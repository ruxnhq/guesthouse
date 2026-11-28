//! How the app says a machine's state: the common word, and in its log a sentence with the word in
//! lower case.

use super::Line;
use guesthouse_control::{Event, State};

#[test]
fn each_state_in_the_apps_words() {
    for (state, said) in [
        (State::Off, "It's off."),
        (State::Starting, "It's starting."),
        (State::Installing, "It's installing."),
        (State::Running, "It's running."),
        (State::Stopping, "It's stopping."),
        (State::StoppingStalled, "It's still stopping."),
        (State::Failed, "It failed."),
    ] {
        assert_eq!(Line::of(Event::State(state)).text, said);
    }
}
