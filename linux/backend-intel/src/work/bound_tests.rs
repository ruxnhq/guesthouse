//! The limits on what one guest holds and does at once: queues, address spaces, what a mapping
//! that xe won't take back leaves, and how much of a batch is read again.

use super::limits::{MOST_XE_QUEUES, MOST_XE_VMS, VM_BYTES};
use super::testing::{self, Rig, map_request};
use crate::stage::Stage;
use crate::xe::batch::{self, Ending};
use crate::xe::plan::BufferUse;
use guesthouse_backend::{CommandBuffer, GPU_RANGES, UnmapRequest, WorkStart};

#[test]
fn a_guest_holds_a_bounded_number_of_xe_address_spaces() {
    let (mut work, xe) = testing::work(Stage::Compute);
    let spaces: Vec<_> = (0..MOST_XE_VMS).map(|_| work.create_space(&GPU_RANGES).unwrap()).collect();
    let refused = work.create_space(&GPU_RANGES).unwrap_err();
    assert!(refused.0.contains("at most"), "{refused}");
    assert_eq!(xe.calls_named("create_vm").len(), MOST_XE_VMS, "xe is not asked for one more");
    work.destroy_space(spaces[0]);
    work.create_space(&GPU_RANGES).expect("room again once one is gone");
}

#[test]
fn a_long_running_address_space_counts_against_the_same_limit() {
    let (mut work, _xe) = testing::work(Stage::Compute);
    let first = work.create_space(&GPU_RANGES).unwrap();
    for _ in 1..MOST_XE_VMS - 1 {
        work.create_space(&GPU_RANGES).unwrap();
    }
    work.ring_vm(first).expect("the last one");
    assert!(work.create_space(&GPU_RANGES).is_err());
}

#[test]
fn a_guest_holds_a_bounded_number_of_xe_queues_and_the_rest_of_its_work_goes_on() {
    let mut rig = Rig::new(Stage::Everything);
    rig.write_batch(&[batch::NOOP, batch::BATCH_END]);
    for queue in 1..=MOST_XE_QUEUES as u32 {
        let work = rig.submission(queue, 1);
        assert!(matches!(rig.start(&work), WorkStart::Started { .. }), "queue {queue}");
    }
    let work = rig.submission(MOST_XE_QUEUES as u32 + 1, 1);
    assert_eq!(rig.start(&work), WorkStart::QueueFailed);
    assert_eq!(rig.xe.calls_named("create_queue").len(), MOST_XE_QUEUES);
    rig.work.close_queue(1);
    let work = rig.submission(MOST_XE_QUEUES as u32 + 2, 1);
    assert!(matches!(rig.start(&work), WorkStart::Started { .. }), "room again once a queue is closed");
}

#[test]
fn queues_and_address_spaces_are_counted_against_the_video_memory_cap() {
    let room = 0x10_0000;
    let (mut work, _xe) = testing::work_with_cap(Stage::Compute, VM_BYTES + room);
    work.create_space(&GPU_RANGES).unwrap();
    assert!(work.allocate(room + 0x1000, BufferUse::Card).is_err(), "the space's tables take their share");
    work.allocate(room, BufferUse::Card).unwrap();
}

#[test]
fn a_mapping_stays_in_the_books_when_the_ring_cannot_take_it_and_the_first_binding_is_undone() {
    let (mut work, xe) = testing::work(Stage::Compute);
    let space = work.create_space(&GPU_RANGES).unwrap();
    let (memory, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
    let ring = work.ring_vm(space).unwrap();
    let vm = work.spaces[&space].vm;
    xe.fail_call(xe.calls().len() + 1, libc::ENOMEM);
    assert!(work.map(&map_request(space, memory, 0x400_0000, 0, 0x1000)).is_err());
    assert!(!xe.is_bound(vm, 0x400_0000), "the first binding is taken back");
    assert!(!xe.is_bound(ring, 0x400_0000));
    assert!(work.spaces[&space].mappings.is_empty());
    work.map(&map_request(space, memory, 0x400_0000, 0, 0x1000)).expect("the addresses are free");
}

#[test]
fn an_unbind_xe_refuses_keeps_the_mapping_until_the_space_is_taken_down() {
    let (mut work, xe) = testing::work(Stage::Compute);
    let space = work.create_space(&GPU_RANGES).unwrap();
    let (memory, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
    work.map(&map_request(space, memory, 0x400_0000, 0, 0x1000)).unwrap();
    work.ring_vm(space).unwrap();
    let live = xe.live();
    xe.fail("bind", libc::EINVAL);
    let unmap = UnmapRequest { space, memory, address: 0x400_0000, length: 0x1000 };
    assert!(work.unmap(&unmap).is_err());
    xe.heal();
    assert!(work.space_failed(space), "nothing more of it runs");
    assert_eq!(xe.live(), live - 2, "both of its address spaces are destroyed");
    assert!(work.spaces[&space].mappings.is_empty());
    assert!(work.map(&map_request(space, memory, 0x500_0000, 0, 0x1000)).is_err());
    work.destroy_space(space);
    assert_eq!(xe.calls_named("destroy_vm").len(), 2, "each destroyed once");
}

#[test]
fn freed_memory_whose_unbind_xe_refuses_takes_its_address_space_down() {
    let (mut work, xe) = testing::work(Stage::Compute);
    let (first, second) = (work.create_space(&GPU_RANGES).unwrap(), work.create_space(&GPU_RANGES).unwrap());
    let (memory, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
    work.map(&map_request(first, memory, 0x400_0000, 0, 0x1000)).unwrap();
    work.map(&map_request(second, memory, 0x400_0000, 0, 0x1000)).unwrap();
    xe.fail_call(xe.calls().len(), libc::EBUSY);
    work.release(memory);
    assert_eq!([work.space_failed(first), work.space_failed(second)].iter().filter(|&&lost| lost).count(), 1);
    assert_eq!(work.spaces.values().map(|space| space.mappings.len()).sum::<usize>(), 0);
}

/// The ring OpenCL's driver submitted, as in the encoder's own tests: a wait, a switch and a chain
/// to its own offset 0x58.
fn ring_dwords() -> Vec<u32> {
    let mut dwords = vec![
        0x6109_0001,
        0x6cf1_0000,
        0x0000_0217,
        0x7a00_0004,
        0x0014_0480,
        0,
        0,
        0,
        0,
        0x1880_0101,
        0x044c_0000,
        0x0000_007b,
        0x0280_0101,
        0x0e00_9003,
        0x0000_0002,
        0x0457_0000,
        0x0000_007b,
        0,
        0x0480_0001,
        0x1880_0101,
        0x044d_0058,
        0x0000_007b,
        0x0280_0100,
    ];
    dwords.resize(63, 0);
    dwords
}

#[test]
fn a_batch_is_looked_at_again_only_by_its_last_few_kilobytes() {
    const RING_AT: u64 = 0x7b_044d_0000;
    let mut rig = Rig::new(Stage::Everything);
    let space = rig.space;
    let (memory, _) = rig.work.allocate(0x40_0000, BufferUse::Card).unwrap();
    rig.work.map(&map_request(space, memory, RING_AT, 0, 0x40_0000)).unwrap();
    let ring = batch::bytes(&ring_dwords());
    let view = rig.work.cpu_view(memory).unwrap();
    assert!(view.write(0, &ring));
    let short = CommandBuffer { address: RING_AT, length: ring.len() as u32 };
    let found = rig.work.ending(&mut rig.host, space, short).unwrap();
    assert_eq!(found, Ending::Parks, "a ring with a little padding is one");
    let long = CommandBuffer { address: RING_AT, length: 0x10_0000 };
    let found = rig.work.ending(&mut rig.host, space, long).unwrap();
    assert_eq!(found, Ending::RunsOn, "a megabyte is not read to find the chain");
}

#[test]
fn a_space_whose_work_xe_ended_is_handed_to_the_core_once() {
    let mut rig = Rig::new(Stage::Everything);
    let mut dwords = batch::store_qword(0x9_0000_0000, 1).to_vec();
    dwords.push(batch::BATCH_END);
    rig.write_batch(&dwords);
    let work = rig.submission(1, 5);
    let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
    rig.work.ring_doorbell(doorbell).unwrap();
    assert_eq!(rig.work.take_fault(), None);
    rig.poll();
    assert_eq!(rig.work.take_fault(), Some(rig.space));
    assert_eq!(rig.work.take_fault(), None);
}
