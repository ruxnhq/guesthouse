//! What a guest that restarts leaves behind of its last start: nothing that waits.

use super::fences::wait_message;
use super::*;

#[test]
fn a_restart_drops_the_last_starts_waits_and_sync_objects() {
    let mut session = session();
    let (sync, _, fence) = create_fence(&mut session, 0);
    send(&mut session, &wait_message(&[sync], &[9], false, 0xE));
    assert!(!session.pending.is_empty());
    session.guest_reset();
    assert!(session.pending.is_empty(), "no wake-up of the last start reaches the next");
    assert_eq!(session.sync_objects.handles(), 0);
    session.run_deferred();
    let (_, _, next) = create_fence(&mut session, 0);
    assert_eq!(next, fence, "its fence page is given out again");
    assert_eq!(session.take_notices().count(), 0);
}
