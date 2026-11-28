//! A machine's state as it goes over the pipe: the states every host shares, by their names, held
//! as the JSON they are, both ways, and every other name refused. A name changes only with the
//! pipe's version (`protocol::PROTOCOL`), as an app of another version can't read it.

use guesthouse_control::{Event, State};
use serde_json::{Value, json};

const STATES: [(State, &str); 7] = [
    (State::Off, "off"),
    (State::Starting, "starting"),
    (State::Installing, "installing"),
    (State::Running, "running"),
    (State::Stopping, "stopping"),
    (State::StoppingStalled, "stopping_stalled"),
    (State::Failed, "failed"),
];

#[test]
fn every_state_is_its_wire_name() {
    for (state, name) in STATES {
        assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
        assert_eq!(serde_json::from_value::<State>(json!(name)).unwrap(), state);
    }
    assert_eq!(State::default(), State::Off);
}

#[test]
fn every_event_is_its_wire_line() {
    for (state, name) in STATES {
        let line = json!({"kind": "state", "text": name});
        assert_eq!(serde_json::to_value(Event::State(state)).unwrap(), line);
        assert_eq!(serde_json::from_value::<Event>(line).unwrap(), Event::State(state));
    }
    for (event, kind) in [
        (Event::Note("n".into()), "note"),
        (Event::Problem("p".into()), "problem"),
        (Event::Guest("g".into()), "guest"),
        (Event::Report("r".into()), "report"),
    ] {
        let text = match &event {
            Event::Note(text) | Event::Problem(text) | Event::Guest(text) | Event::Report(text) => {
                text.clone()
            }
            Event::State(_) => unreachable!(),
        };
        let line = json!({"kind": kind, "text": text});
        assert_eq!(serde_json::to_value(&event).unwrap(), line);
        assert_eq!(serde_json::from_value::<Event>(line).unwrap(), event);
    }
}

/// Each line byte for byte, as the service writes it.
#[test]
fn the_state_lines_byte_for_byte() {
    let lines: Vec<String> =
        STATES.iter().map(|(state, _)| serde_json::to_string(&Event::State(*state)).unwrap()).collect();
    assert_eq!(
        lines,
        [
            r#"{"kind":"state","text":"off"}"#,
            r#"{"kind":"state","text":"starting"}"#,
            r#"{"kind":"state","text":"installing"}"#,
            r#"{"kind":"state","text":"running"}"#,
            r#"{"kind":"state","text":"stopping"}"#,
            r#"{"kind":"state","text":"stopping_stalled"}"#,
            r#"{"kind":"state","text":"failed"}"#,
        ]
    );
}

#[test]
fn a_name_the_pipe_never_sends_is_refused() {
    // The Windows host's names before the states were shared, a capital, and others near them.
    for name in ["stopped", "preparing", "Off", "Running", "shutting_down", "stoppingstalled", ""] {
        assert!(serde_json::from_value::<State>(json!(name)).is_err(), "{name}");
        assert!(serde_json::from_value::<Event>(json!({"kind": "state", "text": name})).is_err(), "{name}");
    }
    assert!(serde_json::from_value::<Event>(json!({"kind": "other", "text": "x"})).is_err());
    assert!(serde_json::from_value::<State>(Value::Null).is_err());
}
