//! How many xe exec queues and address spaces the guest may hold at once, and what each is counted
//! as against the guest's video memory cap. Each queue takes an id from a pool the whole card
//! shares and a context image pinned in video memory; each address space takes page tables. A
//! guest at work uses a few dozen of each, so the limits sit far above that.

use super::CardWork;
use super::queues::Queue;
use crate::xe::batch::{self, Ending};
use guesthouse_backend::{HostServices, SpaceId};
use std::io;

/// The most xe exec queues the guest holds at once.
pub const MOST_XE_QUEUES: usize = 1024;
/// The most xe address spaces the guest holds at once, a process's long-running one included.
pub const MOST_XE_VMS: usize = 512;
/// What a queue's context image and ring are counted as, against the cap.
pub const QUEUE_BYTES: u64 = 512 << 10;
/// What an address space's page tables and scratch page are counted as, against the cap.
pub const VM_BYTES: u64 = 1 << 20;

/// The most of a batch's end that is read again to find how it ends, where its last few
/// kilobytes don't say. The CPU reads the card's memory slowly.
pub(super) const MOST_RESCAN_BYTES: u64 = 32 * 1024;

impl Queue {
    /// How many xe queues it holds.
    pub fn xe_queues(&self) -> usize {
        usize::from(self.ordinary.is_some()) + usize::from(self.ring.is_some())
    }
}

impl CardWork {
    /// How the first `length` bytes at `address` end, from the last `most` of them at most, which
    /// are read through the CPU's view. `None` if they can't be read.
    pub(super) fn scan_end(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        address: u64,
        length: u64,
        most: u64,
    ) -> Option<Ending> {
        let scanned = length.min(most) & !3;
        let from = address + (length - scanned);
        let bytes = self.read_gpu(host, space, from, scanned).ok()?;
        Some(batch::ending_at(&bytes, from))
    }

    /// The xe queues the guest's queues hold now.
    pub(super) fn xe_queues(&self) -> usize {
        self.queues.values().map(|queue| queue.xe_queues()).sum()
    }

    /// The xe address spaces the guest's spaces hold now.
    pub(super) fn xe_vms(&self) -> usize {
        self.spaces.values().map(|space| space.xe_vms()).sum()
    }

    /// Video memory counted for queues and address spaces, beside the buffers held.
    pub(super) fn overhead(&self) -> u64 {
        (self.xe_queues() as u64) * QUEUE_BYTES + (self.xe_vms() as u64) * VM_BYTES
    }

    /// Whether one more xe queue may be made.
    pub(super) fn room_for_queue(&self) -> io::Result<()> {
        if self.xe_queues() >= MOST_XE_QUEUES {
            return Err(io::Error::other(format!("a guest holds at most {MOST_XE_QUEUES} xe queues")));
        }
        Ok(())
    }

    /// Whether one more xe address space may be made.
    pub(super) fn room_for_vm(&self) -> io::Result<()> {
        if self.xe_vms() >= MOST_XE_VMS {
            return Err(io::Error::other(format!("a guest holds at most {MOST_XE_VMS} xe address spaces")));
        }
        Ok(())
    }
}
