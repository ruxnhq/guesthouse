//! How the service says a machine's state in a refusal: the common word, in lower case.

use super::words;
use guesthouse_control::State;

#[test]
fn each_state_in_the_services_words() {
    for (state, said) in [
        (State::Off, "off"),
        (State::Starting, "starting"),
        (State::Installing, "installing"),
        (State::Running, "running"),
        (State::Stopping, "stopping"),
        (State::StoppingStalled, "still stopping"),
        (State::Failed, "failed"),
    ] {
        assert_eq!(words(state), said);
    }
}
