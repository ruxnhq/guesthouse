//! Handles once the counter has come to the end of the numbers.

use super::*;
use crate::handles::HandleCounter;

#[test]
fn a_handle_still_in_use_is_not_handed_out_again() {
    let mut session = session();
    let (resource, allocation) = allocate(&mut session, private(0x10000));
    let (sync, ..) = create_fence(&mut session, 0);
    let device = u32_at(&send(&mut session, &message(Command::CreateDevice, HEADER_SIZE)), 0).unwrap();
    let size = session.allocations[&allocation].size;
    let live = [resource, allocation, sync, device];

    // The last number, then the counter starts again from the first.
    session.handles = HandleCounter::near_the_end(1);
    let (last, wrapped) = allocate(&mut session, private(0x20000));
    let (again, ..) = create_fence(&mut session, 0);
    let made = [last, wrapped, again];
    assert!(made.iter().all(|handle| !live.contains(handle)), "{made:x?} against {live:x?}");
    assert_eq!(session.allocations[&allocation].size, size);
    assert_eq!(session.allocations[&allocation].resource, resource);
    assert_eq!(session.allocations.len(), 2);
    assert_eq!(session.resources[&resource], [allocation]);
}

#[test]
fn a_handle_given_up_is_handed_out_again() {
    let mut session = session();
    let (first, ..) = create_fence(&mut session, 0);
    destroy_sync(&mut session, first);
    session.handles = HandleCounter::near_the_end(1);
    create_fence(&mut session, 0);
    let (reused, ..) = create_fence(&mut session, 0);
    assert_eq!(reused, first);
}

#[test]
fn a_destroyed_queue_takes_its_progress_handle_with_it() {
    let mut session = session();
    let context = create_context(&mut session, 0);
    let (queue, progress, _) = create_queue(&mut session, context);
    let mut request = message(Command::DestroyHardwareQueue, 0x20);
    set_u32(&mut request, 0x18, queue);
    send(&mut session, &request);
    assert!(!session.handle_in_use(queue));
    assert!(!session.handle_in_use(progress));
}
