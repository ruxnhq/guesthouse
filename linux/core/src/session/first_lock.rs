//! What a first lock waits for. The first CPU lock moves an allocation out of video memory into
//! pages shared with the guest, at the same GPU addresses, and an engine still using it would lose
//! what it writes, which shows up as damage here and there in encoded video.
//!
//! Only work an engine was handed after the allocation was mapped in its space can be using it.
//! Work still held back here, behind a wait on the GPU, reaches the engine after the move and finds
//! the new pages. Work handed over before the mapping can't name it. So a lock waits for neither,
//! which matters most for the work held back: it may wait on something the locking thread does
//! next.

use super::Session;
use crate::pending::{QueueAction, Target};
use guesthouse_backend::{Backend, GuestMemory, SpaceId};
use std::time::Instant;

impl<B: Backend, M: GuestMemory> Session<B, M> {
    /// Holds a first lock until work that may be using the allocation has ended, if any hasn't.
    pub(super) fn wait_for_first_lock(&mut self, handle: u32, message: &[u8]) -> bool {
        let targets = self.first_lock_targets(handle);
        if self.targets_reached(&targets) {
            return false;
        }
        self.first_locks_waited.insert(handle);
        let action =
            QueueAction::Retry { message: message.to_vec(), ticket: self.ticket, started: Instant::now() };
        self.after_targets(targets, action);
        self.hold_reply();
        true
    }

    /// Where the fence of each queue that may be using the allocation is once that work ends: a
    /// queue in a space it is mapped in, whose engine was last handed work after it was mapped there.
    fn first_lock_targets(&self, handle: u32) -> Vec<Target> {
        let allocation = &self.allocations[&handle];
        let mapped_since = |space: SpaceId| {
            let mapped = allocation.mappings.iter().any(|mapping| mapping.space == space);
            let first = allocation.mapped_at.iter().find(|&&(at, _)| at == space);
            first.filter(|_| mapped).map(|&(_, writes)| writes)
        };
        let live = self.queues.values().filter(|queue| !queue.failed && !queue.stays);
        live.filter_map(|queue| {
            let rung = queue.rung?;
            let mapped = mapped_since(queue.space)?;
            (rung.space_writes > mapped).then_some((queue.fence, rung.value))
        })
        .collect()
    }

    /// Whether nothing can have written an allocation's video memory: no work was started in any
    /// address space since it was first mapped there, none of them runs work unseen, and it was
    /// never mapped in the daemon's own space, where the daemon's copies write.
    pub(super) fn never_written(&self, handle: u32) -> bool {
        self.allocations[&handle].mapped_at.iter().all(|(space, writes)| {
            Some(*space) != self.own_space
                && !self.ring_spaces.contains(space)
                && self.space_writes.get(space).copied().unwrap_or(0) == *writes
        })
    }
}
