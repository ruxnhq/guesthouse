//! How many updates and CPU waits may wait for their fences, and how hard they are looked at.

use super::fences::wait_message;
use super::mappings::{operation, update_with_flags};
use super::*;
use crate::pending::{LONG_WAIT, MOST_WAITING};

#[test]
fn address_updates_wait_only_up_to_a_limit() {
    let mut session = session();
    let (_, handle) = allocate(&mut session, private(0x2000));
    let (sync, _, _) = create_fence(&mut session, 0);
    let operations = [operation(0, 0x5000_0000, 0x2000, u64::from(handle), 0)];
    for _ in 0..MOST_WAITING {
        assert_eq!(status_of(&update_with_flags(&mut session, sync, 9, 0, &operations)), SUCCESS);
    }
    let refused = update_with_flags(&mut session, sync, 9, 0, &operations);
    assert_eq!(status_of(&refused), Status::NO_MEMORY.0);
    assert_eq!(session.pending.len(), MOST_WAITING);
}

#[test]
fn cpu_waits_wait_only_up_to_a_limit() {
    let mut session = session();
    let (sync, _, _) = create_fence(&mut session, 0);
    for event in 0..MOST_WAITING as u64 {
        assert_eq!(status_of(&send(&mut session, &wait_message(&[sync], &[9], false, event))), SUCCESS);
    }
    let refused = send(&mut session, &wait_message(&[sync], &[9], false, 0xE));
    assert_eq!(status_of(&refused), Status::NO_MEMORY.0);
    assert_eq!(session.pending.len(), MOST_WAITING);
}

#[test]
fn a_cpu_wait_that_goes_on_is_looked_at_with_the_background_pace() {
    let mut session = session();
    let (sync, _, _) = create_fence(&mut session, 0);
    let now = Instant::now();
    send(&mut session, &wait_message(&[sync], &[9], false, 0xE));
    assert_eq!(session.poll(now), Waiting::Urgent);
    assert_eq!(session.poll(now + LONG_WAIT * 2), Waiting::Background);
}
