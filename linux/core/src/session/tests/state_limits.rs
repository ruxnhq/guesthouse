//! How much of the session's state a guest, and one process in it, may hold.

use super::*;
use crate::session::limits::{MOST_CONTEXTS, MOST_PROCESSES, MOST_SPACES, PROCESS_FENCE_SHARE};

fn from_process(mut request: Vec<u8>, process: u32) -> Vec<u8> {
    set_u32(&mut request, 8, process);
    request
}

#[test]
fn address_spaces_stop_at_a_limit() {
    let mut session = session();
    for process in 1..=MOST_SPACES as u32 + 8 {
        let free = from_process(message(Command::FreeGpuAddresses, 0x30), process);
        let mut out = Vec::new();
        let _ = session.handle_message(&free, Some(ReplyTicket::default()), &mut out);
    }
    assert_eq!(session.spaces.len(), MOST_SPACES);
}

#[test]
fn processes_stop_at_a_limit_and_an_end_makes_room() {
    let mut session = session();
    let mut last = 0;
    for _ in 0..MOST_PROCESSES + 8 {
        let mut out = Vec::new();
        let request = message(Command::CreateProcess, 0x233);
        let _ = session.handle_message(&request, Some(ReplyTicket::default()), &mut out);
        if session.guest_pids.len() < MOST_PROCESSES {
            last = u32_at(&out, 0).unwrap();
        }
    }
    assert_eq!(session.guest_pids.len(), MOST_PROCESSES);
    send(&mut session, &from_process(message(Command::DestroyProcess, HEADER_SIZE), last));
    send(&mut session, &message(Command::CreateProcess, 0x233));
    assert_eq!(session.guest_pids.len(), MOST_PROCESSES);
}

#[test]
fn contexts_stop_at_a_limit() {
    let mut session = session();
    for _ in 0..MOST_CONTEXTS {
        assert_ne!(create_context(&mut session, 0), 0);
    }
    assert_eq!(create_context(&mut session, 0), 0);
}

#[test]
fn one_process_holds_at_most_its_share_of_fence_pages() {
    const GREEDY: u32 = 0x40;
    let mut session = session();
    let mut fence = message(Command::CreateSyncObject, 0x30);
    set_u32(&mut fence, 0x20, 5);
    let fence = from_process(fence, GREEDY);
    for _ in 0..PROCESS_FENCE_SHARE {
        send(&mut session, &fence);
    }
    assert_eq!(session.sync_objects.handles(), PROCESS_FENCE_SHARE);
    let mut out = Vec::new();
    let _ = session.handle_message(&fence, Some(ReplyTicket::default()), &mut out);
    assert_eq!(session.sync_objects.handles(), PROCESS_FENCE_SHARE, "no more for that process");
    create_fence(&mut session, 0);
    assert_eq!(session.sync_objects.handles(), PROCESS_FENCE_SHARE + 1, "another process still gets one");
}

#[test]
fn one_process_reserves_at_most_its_share_of_addresses() {
    let mut session = session();
    let reserve = |session: &mut TestSession, process: u32, size: u64| {
        let mut request = from_process(message(Command::ReserveGpuAddresses, 0x40), process);
        set_u64(&mut request, 0x38, size);
        u64_at(&send(session, &request), 0).unwrap()
    };
    let share = session.process_address_share();
    assert_eq!(share, session.gpu_addresses.guest_reach() / 4 * 3, "from what the backend gives the guest");
    assert!(share > 512 << 30, "a driver's great range still fits");
    assert_eq!(reserve(&mut session, 1, share + (64 << 10)), 0);
    assert_ne!(reserve(&mut session, 1, 512 << 30), 0);
    assert_eq!(reserve(&mut session, 1, share - (512 << 30) + (64 << 10)), 0, "past its share");
    assert_ne!(reserve(&mut session, 2, 1 << 30), 0, "another process still gets some");
}
