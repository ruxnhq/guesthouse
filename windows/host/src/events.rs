//! What preparing or running a machine has to say, as it happens. The work hands each event to a
//! sink, so the same work can print to a terminal for `guesthouse-hcs-boot` or be sent to every app
//! window watching that machine for the service.

pub use guesthouse_control::{Event, State};
use std::sync::Mutex;

/// Where events go. It's called from the thread doing the work and from the thread reading the
/// guest's serial port, so it must take events from either.
pub trait Sink: Send + Sync {
    fn event(&self, event: Event);

    /// The pipe this boot's console is served on, once the machine runs: its name is new each boot,
    /// so whoever opens the console learns it here. A sink that offers no console lets it pass.
    fn console(&self, _pipe: &str) {}
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

    #[test]
    fn a_kept_sink_keeps_events_in_order() {
        let kept = Kept::default();
        let sink: &dyn Sink = &kept;
        sink.note("one");
        sink.problem("two");
        assert_eq!(*kept.0.lock().unwrap(), [Event::Note("one".into()), Event::Problem("two".into())]);
    }
}
