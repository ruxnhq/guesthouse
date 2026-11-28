//! What a free waits for: the runs the backend sent, whatever the guest's fences say.

use super::*;

/// A session whose work runs, with a queue, an allocation, and one run on the card that the backend
/// says hasn't ended. Returns the session, the queue, its fence, and the allocation's resource and
/// handle.
fn one_run_on_the_card() -> (TestSession, u32, u64, (u32, u32)) {
    let mut session = session();
    session.backend_mut().run_work = true;
    let context = create_context(&mut session, 0);
    let (queue, _, fence) = create_queue(&mut session, context);
    let allocation = allocate(&mut session, private(0x2000));
    submit(&mut session, queue, 1);
    assert_eq!(session.backend().runs_sent, 1);
    session.backend_mut().runs_ended = Some(0);
    (session, queue, fence, allocation)
}

#[test]
fn a_free_waits_for_a_run_whose_fence_the_guest_wrote() {
    let (mut session, _, fence, (resource, allocation)) = one_run_on_the_card();
    // The fence block is guest memory, and its value isn't what decides.
    session.memory_mut().write_u64(fence, 1).unwrap();
    let blocks = session.backend().live_memory_blocks();
    destroy(&mut session, resource, &[allocation]);
    assert_eq!(session.backend().live_memory_blocks(), blocks, "freed while its run is on the card");
    session.backend_mut().runs_ended = None;
    session.run_deferred();
    assert_eq!(session.backend().live_memory_blocks(), blocks - 1);
}

#[test]
fn a_free_waits_for_the_runs_of_a_queue_that_failed() {
    let (mut session, queue, _, (resource, allocation)) = one_run_on_the_card();
    session.backend_mut().fail_rings = true;
    submit(&mut session, queue, 2);
    assert!(session.queues[&queue].failed);
    let blocks = session.backend().live_memory_blocks();
    destroy(&mut session, resource, &[allocation]);
    assert_eq!(session.backend().live_memory_blocks(), blocks, "freed while its run is on the card");
    session.backend_mut().runs_ended = None;
    session.run_deferred();
    assert_eq!(session.backend().live_memory_blocks(), blocks - 1);
}

#[test]
fn a_closed_queues_fence_waits_for_its_runs() {
    let (mut session, queue, fence, _) = one_run_on_the_card();
    session.memory_mut().write_u64(fence, 1).unwrap();
    let mut request = message(Command::DestroyHardwareQueue, 0x20);
    set_u32(&mut request, 0x18, queue);
    send(&mut session, &request);
    let context = create_context(&mut session, 0);
    let (_, _, next) = create_queue(&mut session, context);
    assert_ne!(next, fence, "the fence goes to another queue once the runs that write it have ended");
    session.backend_mut().runs_ended = None;
    session.run_deferred();
    assert!(session.deferred.is_empty());
}

#[test]
fn a_free_goes_at_once_when_every_run_has_ended() {
    let (mut session, _, fence, (resource, allocation)) = one_run_on_the_card();
    session.memory_mut().write_u64(fence, 1).unwrap();
    session.backend_mut().runs_ended = Some(1);
    let blocks = session.backend().live_memory_blocks();
    destroy(&mut session, resource, &[allocation]);
    assert_eq!(session.backend().live_memory_blocks(), blocks - 1);
    assert!(session.deferred.is_empty());
}

#[test]
fn a_free_waits_for_runs_sent_while_the_memory_was_still_mapped() {
    let (mut session, queue, fence, (resource, allocation)) = one_run_on_the_card();
    map(&mut session, allocation, 0x5000_0000, 0, 2);
    let blocks = session.backend().live_memory_blocks();
    destroy(&mut session, resource, &[allocation]);
    // Sent after the destroy, while the free still waits and the memory is mapped.
    submit(&mut session, queue, 2);
    session.memory_mut().write_u64(fence, 2).unwrap();
    session.backend_mut().runs_ended = Some(1);
    session.run_deferred();
    assert!(session.allocations.is_empty());
    assert_eq!(session.backend().live_memory_blocks(), blocks, "released while run 2 may use it");
    session.backend_mut().runs_ended = None;
    session.run_deferred();
    assert_eq!(session.backend().live_memory_blocks(), blocks - 1);
}
