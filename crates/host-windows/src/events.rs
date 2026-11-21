//! What preparing or running a machine has to say, as it happens. The work hands each event to a
//! sink, so the same work can print to a terminal for `guesthouse-hcs-boot` or be sent to every app
//! window watching that machine for the service.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Preparing,
    Starting,
    Running,
    Stopping,
    Stopped,
}

/// Where events go. It's called from the thread doing the work and from the thread reading the
/// guest's serial port, so it must take events from either.
pub trait Sink: Send + Sync {
    fn event(&self, event: Event);
}

impl dyn Sink + '_ {
    pub fn note(&self, text: impl Into<String>) {
        self.event(Event::Note(text.into()));
    }

    pub fn problem(&self, text: impl Into<String>) {
        self.event(Event::Problem(text.into()));
    }

    pub fn state(&self, state: State) {
        self.event(Event::State(state));
    }
}

/// A sink that keeps every event, for tests and for a caller that wants them all at the end.
#[derive(Debug, Default)]
pub struct Kept(pub Mutex<Vec<Event>>);

impl Sink for Kept {
    fn event(&self, event: Event) {
        self.0.lock().expect("a sink's lock is never poisoned").push(event);
    }
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

    #[test]
    fn a_kept_sink_keeps_events_in_order() {
        let kept = Kept::default();
        let sink: &dyn Sink = &kept;
        sink.note("one");
        sink.problem("two");
        assert_eq!(*kept.0.lock().unwrap(), [Event::Note("one".into()), Event::Problem("two".into())]);
    }
}
