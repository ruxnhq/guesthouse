//! How much of a session's state the guest, and one process in it, may hold. Each limit is far past
//! what a guest uses, and what goes past it is refused like any other request the host can't meet.

use super::Session;
use crate::fences::{FENCE_AREA, FENCE_PAGE};
use guesthouse_backend::{Backend, GuestMemory, SpaceId};

/// Processes the guest has made and not ended.
pub(crate) const MOST_PROCESSES: usize = 4096;
/// Address spaces, one for each process number messages name.
pub(crate) const MOST_SPACES: usize = 4096;
/// Contexts, across all processes.
pub(crate) const MOST_CONTEXTS: usize = 16384;
/// Entries of the small tables keyed by numbers the guest picks: devices' present limits, budget
/// reservations, and the kinds of surface range a process described.
pub(crate) const MOST_KEYED: usize = 4096;
/// Ranges of GPU addresses reserved, across all processes.
pub(crate) const MOST_RESERVED_RANGES: usize = 16384;
/// Commands the host doesn't know that are each said once in the log.
pub(crate) const MOST_UNKNOWN_COMMANDS_SAID: usize = 64;
/// The fence pages one process may hold, for its monitored fences and queues together: half of
/// them, so others always have some.
pub(crate) const PROCESS_FENCE_SHARE: usize = (FENCE_AREA / FENCE_PAGE / 2) as usize;

/// The share of the guest's GPU addresses one process may reserve, in quarters: three, so a
/// driver that reserves a great range still gets it, and the other processes always have a quarter.
const PROCESS_ADDRESS_QUARTERS: u64 = 3;

impl<B: Backend, M: GuestMemory> Session<B, M> {
    /// Whether a process holds its share of fence pages already.
    pub(crate) fn fence_share_taken(&self, process: u32) -> bool {
        let space = self.spaces.get(&process).copied();
        let queues = self.queues.values().filter(|queue| Some(queue.space) == space).count();
        self.sync_objects.fences_held_by(process) + queues >= PROCESS_FENCE_SHARE
    }

    /// The GPU addresses one process may reserve, from what this backend gives the guest.
    pub(crate) fn process_address_share(&self) -> u64 {
        self.gpu_addresses.guest_reach() / 4 * PROCESS_ADDRESS_QUARTERS
    }

    /// Whether reserving `size` more bytes in a space would pass a process's share of addresses.
    pub(crate) fn address_share_passed(&self, space: SpaceId, size: u64) -> bool {
        let held = self.reserved_addresses.iter().filter(|range| range.space == space);
        let held = held.map(|range| range.size).fold(0, u64::saturating_add);
        held.saturating_add(size) > self.process_address_share()
    }
}
