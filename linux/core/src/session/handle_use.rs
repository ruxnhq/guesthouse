//! Which handles are still in use, so the counter never hands one out twice.

use super::Session;
use crate::handles::HANDLE_COUNT;
use crate::pending::{GateStep, Pending, QueueAction};
use guesthouse_backend::{Backend, BackendError, GuestMemory};

impl<B: Backend, M: GuestMemory> Session<B, M> {
    /// A handle for a new object. Until the counter has come to the end of the numbers each one is
    /// new; after that, a number something still has is passed over.
    pub(crate) fn take_handle(&mut self) -> Result<u32, BackendError> {
        for _ in 0..HANDLE_COUNT {
            let (handle, handed_out_before) = self.handles.take();
            if !handed_out_before || !self.handle_in_use(handle) {
                return Ok(handle);
            }
        }
        Err(BackendError::new("every handle is in use"))
    }

    /// Whether anything here still has this handle: a table, a wait, a copy or the frames.
    pub(crate) fn handle_in_use(&self, handle: u32) -> bool {
        let keyed = self.processes_32_bit.contains(&handle)
            || self.guest_pids.contains_key(&handle)
            || self.programs.contains_key(&handle)
            || self.spaces.contains_key(&handle)
            || self.devices.contains(&handle)
            || self.display_driver == Some(handle)
            || self.allocations.contains_key(&handle)
            || self.resources.contains_key(&handle)
            || self.resource_users.contains_key(&handle)
            || self.shared_objects.contains_key(&handle)
            || self.paging_queues.contains_key(&handle)
            || self.queue_progress.contains_key(&handle)
            || self.queues.contains_key(&handle)
            || self.context_queues.contains_key(&handle)
            || self.context_nodes.contains_key(&handle)
            || self.context_hints.contains_key(&handle)
            || self.context_processes.contains_key(&handle)
            || self.present_limits.contains_key(&handle)
            || self.gates.contains_key(&handle)
            || self.rings_not_before.contains_key(&handle)
            || self.first_locks_waited.contains(&handle)
            || self.described_surfaces.contains_key(&handle)
            || self.sync_objects.mentions(handle);
        keyed || self.handle_held_inside(handle)
    }

    /// Whether a handle is held inside another object, where no table is keyed by it.
    fn handle_held_inside(&self, handle: u32) -> bool {
        let process_keyed = self.reservations.keys().any(|&(process, _)| process == handle)
            || self.surface_range_sizes.keys().any(|&(process, _)| process == handle);
        let in_tables = self.shared_objects.values().any(|&object| object == handle)
            || self.resources.values().flatten().any(|&member| member == handle)
            || self.context_queues.values().flatten().any(|&queue| queue == handle)
            || self.context_processes.values().any(|&process| process == handle)
            || self.described_surfaces.values().any(|&(allocation, _)| allocation == handle)
            || self
                .newest_description
                .is_some_and(|(process, allocation, _)| process == handle || allocation == handle)
            || self
                .allocations
                .values()
                .any(|allocation| allocation.owner == handle || allocation.resource == handle)
            || self.queues.values().any(|queue| queue.context == handle);
        let waiting = self.deferred.iter().flat_map(|item| &item.waits).any(|&(queue, _)| queue == handle)
            || self.gates.values().flatten().any(|step| match step {
                GateStep::Wait { signals, .. } => signals.iter().any(|&(sync, _)| sync == handle),
                _ => false,
            })
            || self.pending.iter().any(|item| pending_names(item, handle))
            || self.copy_uses(handle);
        let frames = self.frames.screen.is_some_and(|screen| screen.staging == handle)
            || self.frames.indirect.as_ref().is_some_and(|ring| ring.owner == handle || ring.holds(handle))
            || self.frames.registering.as_ref().is_some_and(|registering| registering.process == handle);
        process_keyed || in_tables || waiting || frames
    }
}

/// Whether something waiting names this handle. A message held to be handled again isn't read, as
/// it is handled again soon and checks its handles then.
fn pending_names(item: &Pending, handle: u32) -> bool {
    match item {
        Pending::AfterQueues { action: QueueAction::WriteFences(signals), .. }
        | Pending::CpuWait { signals, .. } => signals.iter().any(|&(sync, _)| sync == handle),
        Pending::Gate { queue } | Pending::RingRoom { queue, .. } | Pending::Watch { queue, .. } => {
            *queue == handle
        }
        Pending::Update { fence, .. } => *fence == handle,
        _ => false,
    }
}
