//! Work on the card driven by whole messages through the core's session, so that the mappings the
//! core itself makes (its fence block, a first lock's move into guest pages) meet the checks a hand
//! built request might slip past. The card is the stand-in, and it runs nothing here: the stand-in
//! memory's host addresses are made up, so no batch may be let through to touch them.

#[path = "../../tests/common/mod.rs"]
mod common;

use crate::IntelBackend;
use crate::stage::Stage;
use crate::xe::abi::{self, Sync};
use crate::xe::batch;
use crate::xe::device::Xe;
use crate::xe::fake::{Call, FakeXe};
use common::*;
use guesthouse_backend::{Backend, GuestMemory};
use guesthouse_backend_standin::SparseMemory;
use guesthouse_core::{Config, MemoryLimits, Session};
use guesthouse_messages::wire::{set_u32, set_u64, u32_at, u64_at};
use guesthouse_messages::{Command, HEADER_SIZE};
use std::time::Duration;

const COMPUTE_NODE: u32 = 5;

/// A guest on the test card with everything running on the stand-in, and runs held back.
fn guest_with_work() -> (Guest, FakeXe) {
    let xe = FakeXe::default();
    let mut backend = IntelBackend::from_card(card()).unwrap();
    let open = || Ok(Box::new(xe.clone()) as Box<dyn Xe + Send>);
    backend.start_work_on_card((Stage::Everything, None), open, None);
    assert_eq!(backend.stage(), Stage::Everything);
    xe.hold_runs();
    let limits = MemoryLimits::new(backend.facts(), backend.video_memory_cap(), 16 << 30);
    let mut config = Config::new(limits);
    config.frame_wait = Duration::ZERO;
    let mut session = Session::new(backend, SparseMemory::new(WINDOW_END), config);
    let mut io_space = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut io_space, HEADER_SIZE, WINDOW);
    set_u64(&mut io_space, HEADER_SIZE + 8, 8 << 30);
    assert_eq!(send(&mut session, &io_space), SUCCESS.to_le_bytes());
    (session, xe)
}

fn binds(xe: &FakeXe) -> usize {
    xe.calls_named("bind").len()
}

#[test]
fn a_guests_first_steps_make_real_things_and_its_batch_is_sent_with_its_fence() {
    let (mut session, xe) = guest_with_work();
    let context = context(&mut session, COMPUTE_NODE, &context_data());
    assert_ne!(context, 0);
    let (queue, fence) = queue(&mut session, context);
    assert!(binds(&xe) >= 1, "the core's fence block is bound in the process's space");

    // Video memory, mapped where the guest's driver asked.
    let (made, closed) = (xe.calls_named("create_buffer").len(), xe.calls_named("close_buffer").len());
    let reply = allocate(&mut session, &render_target());
    assert_eq!(u32_at(&reply, 0), Ok(SUCCESS));
    let handle = u32_at(&reply, 0x10).unwrap();
    assert_eq!(xe.calls_named("create_buffer").len(), made + 1, "a buffer on the card");
    let mut map = message(Command::MapGpuAddresses, 0x50);
    set_u32(&mut map, 0x38, handle);
    let address = u64_at(&send(&mut session, &map), 0).unwrap();
    assert_ne!(address, 0);
    assert!(
        xe.calls_named("bind")
            .iter()
            .any(|call| matches!(call, Call::Bind { op, .. } if op.address == address))
    );

    // The CPU locks it, as Intel's drivers do with nearly everything: it moves into guest pages,
    // and the same address is bound again, to them.
    let before = binds(&xe);
    let mut lock = message(Command::LockForCpu, 0x20);
    set_u32(&mut lock, 0x1C, handle);
    let reply = send(&mut session, &lock);
    assert_eq!(u32_at(&reply, 0), Ok(SUCCESS));
    let guest_address = u64_at(&reply, 8).unwrap();
    let here: Vec<u32> = xe
        .calls_named("bind")
        .split_off(before)
        .iter()
        .filter_map(|call| match call {
            Call::Bind { op, .. } if op.address == address => Some(op.op),
            _ => None,
        })
        .collect();
    assert_eq!(
        here,
        [abi::BIND_UNMAP, abi::BIND_MAP_USERPTR],
        "unbound as video memory, bound as guest pages"
    );
    assert_eq!(xe.calls_named("close_buffer").len(), closed + 1, "and the buffer went");

    // A batch that ends, written by the guest's CPU, and submitted by its address.
    session.memory_mut().write_u32(guest_address, batch::BATCH_END).unwrap();
    let mut submit = submission(queue, 1, &compact_header());
    set_u64(&mut submit, 0x28, address);
    set_u32(&mut submit, 0x30, 4);
    let sent_before = xe.calls_named("exec").len();
    assert_eq!(send(&mut session, &submit), SUCCESS.to_le_bytes());
    let execs = xe.calls_named("exec");
    assert_eq!(execs.len(), sent_before + 1, "it was sent to the card");
    let Some(Call::Exec { batch: sent, syncs, .. }) = execs.last() else { panic!() };
    assert_eq!(*sent, address);
    assert!(
        syncs.iter().any(|sync| matches!(sync, Sync::UserFence { value: 1, .. })),
        "the card is asked to write the guest's fence: {syncs:?}"
    );
    assert_eq!(session.memory().read_u64(fence), Ok(0), "and the host has not written it");

    // Memory made and mapped after that work, and locked before any more, was written by nothing.
    // It moves with no read of the card's memory, which is slow enough to stop the desktop.
    session.memory_mut().write_u64(fence, 1).unwrap();
    let _ = session.poll(std::time::Instant::now());
    let fresh = u32_at(&allocate(&mut session, &render_target()), 0x10).unwrap();
    set_u32(&mut map, 0x38, fresh);
    assert_ne!(u64_at(&send(&mut session, &map), 0).unwrap(), 0);
    let read_before = xe.calls_named("map_buffer").len();
    set_u32(&mut lock, 0x1C, fresh);
    assert_eq!(u32_at(&send(&mut session, &lock), 0), Ok(SUCCESS));
    assert_eq!(xe.calls_named("map_buffer").len(), read_before, "the CPU never looked at the buffer");
}

#[test]
fn a_batch_that_does_not_end_fails_the_submission_and_nothing_is_sent() {
    let (mut session, xe) = guest_with_work();
    let context = context(&mut session, COMPUTE_NODE, &context_data());
    let (queue, _) = queue(&mut session, context);
    let sent_before = xe.calls_named("exec").len();
    // The helper's submission names commands at an address nothing is mapped at.
    let reply = send(&mut session, &submission(queue, 1, &compact_header()));
    assert_ne!(reply, SUCCESS.to_le_bytes(), "the guest hears its queue has stopped");
    assert_eq!(xe.calls_named("exec").len(), sent_before);
}
