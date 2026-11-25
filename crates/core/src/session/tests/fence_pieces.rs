//! The fence area reached in pieces, for a backend that asks for them: which pieces the GPU gets,
//! when, and in which address spaces.

use super::*;
use crate::FENCE_PAGE;

const PIECE: u64 = 1 << 20;
const FENCES_IN_A_PIECE: u64 = PIECE / FENCE_PAGE;
const FIRST_PROCESS: u32 = 100;
const OTHER_PROCESS: u32 = 200;
const BLOCK: u64 = crate::GPU_RANGES[0].start;

fn session_in_pieces() -> TestSession {
    let mut session = session();
    session.backend_mut().fence_piece = Some(PIECE);
    session
}

/// Asks for a monitored fence, and returns the reply's status and the fence's GPU address.
fn ask_for_fence(session: &mut TestSession) -> (u32, u64) {
    let mut request = message(Command::CreateSyncObject, 0x30);
    set_u32(&mut request, 0x20, 5);
    let reply = send(session, &request);
    (status_of(&reply), u64_at(&reply, 8).unwrap_or(0))
}

/// Hands out every fence of the first piece.
fn fill_the_first_piece(session: &mut TestSession) {
    for _ in 0..FENCES_IN_A_PIECE {
        create_fence(session, 0);
    }
}

fn piece_mapped(session: &TestSession, space: SpaceId, piece: u64) -> bool {
    session.backend().mapped_length(space, BLOCK + piece * PIECE) == Some(PIECE)
}

#[test]
fn a_backend_that_asks_for_nothing_gets_the_whole_area_at_once() {
    let mut session = session();
    let space = session.space_for(FIRST_PROCESS).unwrap();
    assert_eq!(session.backend().adopted.len(), 1);
    assert_eq!(session.backend().adopted[0].1, FENCE_AREA);
    assert_eq!(session.backend().mapped_length(space, BLOCK), Some(FENCE_AREA));
    fill_the_first_piece(&mut session);
    create_fence(&mut session, 0);
    assert_eq!(session.backend().adopted.len(), 1);
    assert_eq!(session.backend().mapping_count(), 1);
}

#[test]
fn only_the_first_piece_is_there_until_a_fence_lies_past_it() {
    let mut session = session_in_pieces();
    let space = session.space_for(FIRST_PROCESS).unwrap();
    fill_the_first_piece(&mut session);
    assert_eq!(session.backend().adopted.len(), 1);
    assert_eq!(session.backend().adopted[0].1, PIECE);
    assert!(piece_mapped(&session, space, 0));
    assert!(!piece_mapped(&session, space, 1));
    // The addresses after the area are still the next ones handed out, as with the area whole.
    assert_eq!(session.take_gpu_addresses(FENCE_PAGE, crate::ALLOCATION_ALIGN), Ok(BLOCK + FENCE_AREA));
}

#[test]
fn a_fence_past_the_pieces_there_brings_its_piece_to_every_space() {
    let mut session = session_in_pieces();
    let first = session.space_for(FIRST_PROCESS).unwrap();
    let other = session.space_for(OTHER_PROCESS).unwrap();
    fill_the_first_piece(&mut session);

    let (_, gpu_address, fence) = create_fence(&mut session, 0);
    assert_eq!(fence, IO_SPACE + PIECE);
    assert_eq!(gpu_address, BLOCK + PIECE);
    assert!(piece_mapped(&session, first, 1) && piece_mapped(&session, other, 1));
    let adopted = &session.backend().adopted;
    assert_eq!(adopted.len(), 2);
    assert_eq!(adopted[1], (adopted[0].0 + PIECE, PIECE));

    // The rest of that piece's fences bring nothing more.
    create_fence(&mut session, 0);
    assert_eq!(session.backend().adopted.len(), 2);
    assert_eq!(session.backend().mapping_count(), 4);
}

#[test]
fn a_new_space_gets_every_piece_there() {
    let mut session = session_in_pieces();
    fill_the_first_piece(&mut session);
    create_fence(&mut session, 0);
    assert_eq!(session.backend().mapping_count(), 0);
    let space = session.space_for(FIRST_PROCESS).unwrap();
    assert!(piece_mapped(&session, space, 0) && piece_mapped(&session, space, 1));
    assert_eq!(session.backend().mapping_count(), 2);
}

#[test]
fn a_closing_space_gives_up_its_pieces_and_gets_no_more() {
    let mut session = session_in_pieces();
    let closing = session.space_for(FIRST_PROCESS).unwrap();
    let staying = session.space_for(OTHER_PROCESS).unwrap();
    fill_the_first_piece(&mut session);
    create_fence(&mut session, 0);
    assert_eq!(session.backend().mapping_count(), 4);

    session.close_space(closing);
    assert_eq!(session.backend().mapping_count(), 2);
    assert_eq!(session.backend().live_address_spaces(), 1);
    assert_eq!(session.fences.block.as_ref().unwrap().spaces, [staying]);
}

#[test]
fn a_reused_fence_brings_no_piece() {
    let mut session = session_in_pieces();
    session.space_for(FIRST_PROCESS).unwrap();
    let (handle, gpu_address, _) = create_fence(&mut session, 0);
    destroy_sync(&mut session, handle);
    assert_eq!(create_fence(&mut session, 0).1, gpu_address);
    assert_eq!(session.backend().adopted.len(), 1);
}

#[test]
fn a_paging_fence_brings_no_piece() {
    let mut session = session_in_pieces();
    session.space_for(FIRST_PROCESS).unwrap();
    fill_the_first_piece(&mut session);
    send(&mut session, &message(Command::CreatePagingQueue, 0x20));
    assert_eq!(session.backend().adopted.len(), 1);
}

#[test]
fn a_piece_that_cannot_go_everywhere_goes_nowhere() {
    let mut session = session_in_pieces();
    let first = session.space_for(FIRST_PROCESS).unwrap();
    let other = session.space_for(OTHER_PROCESS).unwrap();
    fill_the_first_piece(&mut session);
    let memory_before = session.backend().live_memory_blocks();

    // Spaces get a piece oldest first, so the first has it by the time the other refuses.
    session.backend_mut().fail_maps_in = Some(other);
    let (status, _) = ask_for_fence(&mut session);
    assert_ne!(status, SUCCESS);
    assert!(!piece_mapped(&session, first, 1) && !piece_mapped(&session, other, 1));
    assert_eq!(session.backend().mapping_count(), 2);
    assert_eq!(session.backend().live_memory_blocks(), memory_before);

    // The page it would have had is the next one given, once the piece can be mapped.
    session.backend_mut().fail_maps_in = None;
    let (_, gpu_address, fence) = create_fence(&mut session, 0);
    assert_eq!((fence, gpu_address), (IO_SPACE + PIECE, BLOCK + PIECE));
    assert!(piece_mapped(&session, first, 1) && piece_mapped(&session, other, 1));
}

#[test]
fn a_queue_past_the_first_piece_gets_its_fence_mapped_too() {
    let mut session = session_in_pieces();
    fill_the_first_piece(&mut session);
    let space = session.space_for(PROCESS).unwrap();
    let context = create_context(&mut session, 0);
    assert!(!piece_mapped(&session, space, 1));
    create_queue(&mut session, context);
    assert!(piece_mapped(&session, space, 1));
}
