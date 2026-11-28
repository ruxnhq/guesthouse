//! Who speaks for the screen: the process whose ring was shown first, until it ends.

use super::indirect::{NAME, OTHER_PROCESS, STATUS_REFUSED, escape_from, registered, ring, say};
use super::*;
use guesthouse_messages::indirect::as_the_driver_sends as driver;
use guesthouse_messages::indirect::{CursorMove, Look};

const MOVE: CursorMove = CursorMove { monitor: 0, x: 10, y: 20, visible: true, sdr_white_nits: 80 };
const STATUS_NOT_WANTED: u32 = 3;

/// Sends another process's REGISTER, and says whether it was refused at once.
fn refused_at_once(session: &mut TestSession) -> bool {
    let message = escape_from(OTHER_PROCESS, &driver::register(&ring(Look::SDR)));
    let mut out = Vec::new();
    let answer = session.handle_message(&message, Some(ReplyTicket::default()), &mut out);
    answer == Answer::Now && driver::reply(&out).unwrap().0 == STATUS_REFUSED
}

/// The display driver's ring, ended by the driver as a mode change ends it.
fn between_rings() -> TestSession {
    let (mut session, _) = registered();
    session.space_for(OTHER_PROCESS).unwrap();
    say(&mut session, PROCESS, &driver::close(NAME));
    assert!(session.frames.indirect.is_none());
    session
}

#[test]
fn between_its_rings_no_other_process_takes_the_screen() {
    let mut session = between_rings();
    assert!(refused_at_once(&mut session));
}

#[test]
fn between_its_rings_no_other_process_shapes_the_cursor() {
    let mut session = between_rings();
    session.frames.viewer_wants_cursor = true;
    assert_eq!(say(&mut session, OTHER_PROCESS, &driver::cursor_move(&MOVE)).0, STATUS_NOT_WANTED);
    assert_eq!(say(&mut session, PROCESS, &driver::cursor_move(&MOVE)).0, 0, "the driver's own still goes");
}

#[test]
fn once_the_display_drivers_process_ends_another_may_show_a_ring() {
    let mut session = between_rings();
    let mut end = message(Command::DestroyProcess, HEADER_SIZE);
    set_u32(&mut end, 8, PROCESS);
    send(&mut session, &end);
    assert!(!refused_at_once(&mut session));
}

#[test]
fn another_processs_refused_ring_leaves_the_screens_mode() {
    let mut session = between_rings();
    let mode = session.frames.display_mode;
    assert!(mode.is_some(), "the mode is remembered between rings");
    assert!(refused_at_once(&mut session));
    assert_eq!(session.frames.display_mode, mode);
}
