//! How far a mapping may reach: into its allocation's memory, inside the GPU ranges, under the cap.

use super::mappings::{operation, update_with_flags};
use super::*;

const CHOSEN: u64 = 0x5000_0000;

#[test]
fn a_map_at_a_chosen_address_reaches_only_into_its_memory() {
    let mut session = session();
    let (_, handle) = allocate(&mut session, private(0x2000));
    assert_eq!(map(&mut session, handle, CHOSEN, 0, 2), (CHOSEN, SUCCESS));
    let maps = session.backend().mapped_protections.len();
    let (_, status) = map(&mut session, handle, CHOSEN + 0x10_0000, 0, 16);
    assert_eq!(status, Status::INVALID_PARAMETER.0);
    assert_eq!(session.backend().mapped_protections.len(), maps, "nothing reached the backend");
}

#[test]
fn growing_an_allocation_on_its_first_map_is_held_to_the_video_cap() {
    let mut session = session_with(Some(0x3000));
    let (_, handle) = allocate(&mut session, private(0x2000));
    let (_, status) = map(&mut session, handle, CHOSEN, 0, 4);
    assert_eq!(status, Status::NO_VIDEO_MEMORY.0);
    assert_eq!(session.video_use().on_card, 0x2000);
    assert_eq!(
        map(&mut session, handle, CHOSEN, 0, 3),
        (CHOSEN, SUCCESS),
        "growing under the cap still works"
    );
    assert_eq!(session.video_use().on_card, 0x3000);
}

#[test]
fn an_update_maps_only_inside_its_memory_and_the_ranges() {
    let mut session = session();
    let (_, handle) = allocate(&mut session, private(0x2000));
    let (sync, _, _) = create_fence(&mut session, 0);
    let past_the_memory = [operation(0, CHOSEN, 0x10_000, u64::from(handle), 0)];
    update_with_flags(&mut session, sync, 1, 1, &past_the_memory);
    assert!(session.allocations[&handle].mappings.is_empty());
    let over_a_range_end = [operation(0, 0xFFFF_F000, 0x2000, u64::from(handle), 0)];
    update_with_flags(&mut session, sync, 1, 1, &over_a_range_end);
    assert!(session.allocations[&handle].mappings.is_empty());
    let fits = [operation(0, CHOSEN, 0x2000, u64::from(handle), 0)];
    update_with_flags(&mut session, sync, 1, 1, &fits);
    assert_eq!(session.allocations[&handle].mappings.len(), 1);
}

#[test]
fn an_update_copies_mappings_only_to_inside_the_ranges() {
    let mut session = session();
    let (_, handle) = allocate(&mut session, private(0x2000));
    let (sync, _, _) = create_fence(&mut session, 0);
    update_with_flags(&mut session, sync, 1, 1, &[operation(0, CHOSEN, 0x2000, u64::from(handle), 0)]);
    let over_a_range_end = [operation(2, CHOSEN, 0x2000, 0xFFFF_F000, 0)];
    update_with_flags(&mut session, sync, 1, 1, &over_a_range_end);
    assert_eq!(session.allocations[&handle].mappings.len(), 1, "nothing was copied there");
}
