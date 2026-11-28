// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;

#[test]
fn each_state_offers_what_the_page_shows() {
    assert_eq!(next_step(State::Off), Some(Step::Start));
    assert_eq!(next_step(State::Failed), Some(Step::Start));
    assert_eq!(next_step(State::Running), Some(Step::OpenScreen));
    assert_eq!(next_step(State::StoppingStalled), Some(Step::ForceOff));
    assert_eq!(next_step(State::Stopping), None);
    assert_eq!(next_step(State::Starting), None);
    assert_eq!(next_step(State::Installing), None);
}

#[test]
fn a_machine_is_alive_from_starting_until_it_is_off() {
    for state in [State::Starting, State::Running, State::Stopping, State::StoppingStalled] {
        assert!(is_alive(state));
    }
    for state in [State::Off, State::Failed, State::Installing] {
        assert!(!is_alive(state));
    }
}

#[test]
fn force_off_is_for_coming_up_running_and_a_stalled_shutdown() {
    assert!(can_force_off(State::Starting) && can_force_off(State::Running));
    assert!(can_force_off(State::StoppingStalled));
    assert!(!can_force_off(State::Stopping) && !can_force_off(State::Off));
}

#[test]
fn the_screen_of_a_machine_the_app_runs_opens_while_it_is_up() {
    assert!(!can_open_screen(false, false, State::Running));
    assert!(can_open_screen(true, true, State::Running));
    assert!(!can_open_screen(true, true, State::Off));
    assert!(can_open_screen(true, false, State::Off));
}

#[test]
fn start_needs_a_startable_machine_nothing_in_the_way_and_the_agent() {
    assert!(can_start(true, false, true, State::Off));
    assert!(can_start(true, false, true, State::Failed));
    assert!(!can_start(false, false, true, State::Off));
    assert!(!can_start(true, true, true, State::Off));
    assert!(!can_start(true, false, false, State::Off));
    assert!(!can_start(true, false, true, State::Running));
}

#[test]
fn an_install_is_shown_while_it_runs_or_failed_and_once_when_done() {
    assert!(shows_install("running", true) && shows_install("failed", true));
    assert!(shows_install("done", false) && !shows_install("done", true));
    assert!(!shows_install("waiting", false));
}

#[test]
fn the_json_answers_use_the_shared_names() {
    assert_eq!(answer(r#"{"rule":"next_step","state":"stopping_stalled"}"#), r#"{"step":"force_off"}"#);
    assert_eq!(answer(r#"{"rule":"is_alive","state":"stopping"}"#), r#"{"yes":true}"#);
    assert_eq!(answer(r#"{"rule":"start_check_order"}"#), r#"{"order":["memory","disk","driver"]}"#);
    assert!(answer(r#"{"rule":"next_step","state":"stopped"}"#).contains("error"));
}
