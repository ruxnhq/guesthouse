//! What preparing or running a machine has to say, as it happens: the service sends these to every
//! app window watching that machine, and `guesthouse-hcs-boot` prints them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "lowercase")]
pub enum Event {
    /// What the work is doing.
    Note(String),
    /// Something that went wrong, which the work may get past.
    Problem(String),
    /// A line the guest wrote to its serial port, made printable.
    Guest(String),
    /// A line of the report a Windows guest left on its disk.
    Report(String),
    /// Where the machine stands now.
    State(State),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Preparing,
    Starting,
    Running,
    Stopping,
    #[default]
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_tagged_by_kind_as_the_pipe_sends_them() {
        assert_eq!(
            serde_json::to_value(Event::Guest("hi".into())).unwrap(),
            json!({"kind": "guest", "text": "hi"})
        );
        assert_eq!(
            serde_json::to_value(Event::State(State::Running)).unwrap(),
            json!({"kind": "state", "text": "running"})
        );
    }
}
