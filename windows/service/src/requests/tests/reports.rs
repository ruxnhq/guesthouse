//! A report asked for a machine that isn't there, or by a window that has gone.

use super::*;

#[test]
fn a_machine_that_isnt_there_is_refused_before_anything_is_sent() {
    let setup = setup("report-none", Pretend::default());
    let answers = ask(&setup.service, Request::ReportFiles { machine: Some("nope".into()) });
    assert_eq!(answers, [Answer::Refused { reason: "There is no machine called nope.".into() }]);
}

#[test]
fn a_report_stops_when_the_window_has_gone() {
    let setup = setup("report-gone", Pretend::default());
    let (answers, received) = channel();
    drop(received);
    let mut window = Window { answers, readable: true, gone: true };
    setup.service.answer(Request::ReportFiles { machine: None }, &mut window);
}
