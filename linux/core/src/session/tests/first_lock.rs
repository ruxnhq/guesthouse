//! What a first lock waits for before it moves an allocation: only work an engine was handed after
//! the allocation was mapped in its space.

use super::*;

/// A running session with one queue, and an allocation of a page mapped in the guest's space.
fn with_queue() -> (TestSession, u32, u64) {
    let mut session = session();
    session.backend_mut().run_work = true;
    let context = create_context(&mut session, 0);
    let (queue, _, fence) = create_queue(&mut session, context);
    (session, queue, fence)
}

fn mapped_allocation(session: &mut TestSession) -> u32 {
    let (_, allocation) = allocate(session, private(0x1000));
    map(session, allocation, 0, 0, 0);
    allocation
}

fn lock_answer(session: &mut TestSession, allocation: u32) -> Answer {
    let mut request = message(Command::LockForCpu, 0x20);
    set_u32(&mut request, 0x1C, allocation);
    session.handle_message(&request, Some(ReplyTicket::default()), &mut Vec::new())
}

#[test]
fn work_handed_to_the_engine_after_the_mapping_holds_the_lock_until_it_ends() {
    let (mut session, queue, fence) = with_queue();
    let allocation = mapped_allocation(&mut session);
    submit(&mut session, queue, 1);
    assert_eq!(lock_answer(&mut session, allocation), Answer::Held);
    session.memory_mut().write_u64(fence, 1).unwrap();
    let _ = session.poll(Instant::now());
    let _ = session.poll(Instant::now());
    assert_eq!(session.take_replies().count(), 1);
    assert_eq!(session.backend().preserved.len(), 1);
}

#[test]
fn work_held_back_behind_a_wait_does_not_hold_the_lock() {
    let (mut session, queue, _) = with_queue();
    let (sync, _, _) = create_fence(&mut session, 0);
    let allocation = mapped_allocation(&mut session);
    gpu_wait(&mut session, queue, sync, 1);
    submit(&mut session, queue, 1);
    assert!(session.gates.contains_key(&queue), "the work waits here, not on the engine");
    assert_eq!(lock_answer(&mut session, allocation), Answer::Now);
    assert_eq!(session.backend().preserved.len(), 1, "started, so it is copied all the same");
}

#[test]
fn work_held_back_and_let_go_after_the_mapping_holds_the_lock() {
    let (mut session, queue, _) = with_queue();
    let (sync, _, sync_fence) = create_fence(&mut session, 0);
    gpu_wait(&mut session, queue, sync, 1);
    submit(&mut session, queue, 1);
    let allocation = mapped_allocation(&mut session);
    session.memory_mut().write_u64(sync_fence, 1).unwrap();
    let _ = session.poll(Instant::now());
    assert!(!session.gates.contains_key(&queue), "rung once the wait was over");
    assert_eq!(lock_answer(&mut session, allocation), Answer::Held, "it may name what was mapped first");
}

#[test]
fn work_handed_over_before_the_mapping_does_not_hold_the_lock() {
    let (mut session, queue, _) = with_queue();
    submit(&mut session, queue, 1);
    let allocation = mapped_allocation(&mut session);
    assert_eq!(lock_answer(&mut session, allocation), Answer::Now);
}

#[test]
fn work_in_a_space_the_allocation_has_left_does_not_hold_the_lock() {
    let (mut session, queue, _) = with_queue();
    let allocation = mapped_allocation(&mut session);
    submit(&mut session, queue, 1);
    let mut other = message(Command::CreateContext, 0x34);
    set_u32(&mut other, 8, PROCESS + 0x1000);
    send(&mut session, &other);
    let mapping = session.allocations[&allocation].mappings[0];
    let other_space = session.space_for(PROCESS + 0x1000).unwrap();
    let moved = crate::tables::Mapping { space: other_space, ..mapping };
    session.allocations.get_mut(&allocation).unwrap().mappings = vec![moved];
    assert_eq!(lock_answer(&mut session, allocation), Answer::Now);
}
