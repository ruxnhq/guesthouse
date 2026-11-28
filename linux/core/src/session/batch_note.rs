//! With verbose logging, a line for each hardware queue submission's batch: its address and length,
//! the allocation it lies in and what backs that, and its dwords where the host can read them: all of
//! a small batch, the first and last few of a big one. It is for bringing up a vendor's real submission path, and says nothing to the guest.

use super::Session;
use crate::tables::Allocation;
use guesthouse_backend::{Backend, GuestMemory};
use std::fmt::Write;
use std::sync::atomic::{AtomicU32, Ordering};

/// Dwords shown from each end of a batch.
const DWORDS_SHOWN: u64 = 8;
/// A batch this long or shorter is shown whole: how it ends may be in the middle of it.
const WHOLE_BATCH: u64 = 1024;
const DWORD: u64 = 4;
/// Lines at most for one daemon, so a long verbose run can't fill a disk with them.
const MOST_LINES: u32 = 4096;
static LINES: AtomicU32 = AtomicU32::new(0);

/// What is behind an allocation, in the line's words.
fn backing(allocation: &Allocation) -> &'static str {
    if allocation.view.is_some() {
        "guest-pages"
    } else if allocation.guest_address.is_some() {
        "shared-window"
    } else {
        "card-memory"
    }
}

fn hex_dwords(bytes: &[u8]) -> String {
    let mut text = String::new();
    for dword in bytes.as_chunks::<4>().0 {
        let value = u32::from_le_bytes(*dword);
        let _ = write!(text, "{}{value:08x}", if text.is_empty() { "" } else { " " });
    }
    text
}

impl<B: Backend, M: GuestMemory> Session<B, M> {
    /// Logs `length` bytes of commands at `address`, in the space of `queue`.
    pub(super) fn note_batch(&self, queue: u32, address: u64, length: u32) {
        if !log::log_enabled!(log::Level::Debug) || LINES.fetch_add(1, Ordering::Relaxed) >= MOST_LINES {
            return;
        }
        let Some(space) = self.queues.get(&queue).map(|queue| queue.space) else { return };
        let found = self.allocations.iter().find_map(|(handle, allocation)| {
            let mapping = allocation
                .mappings
                .iter()
                .find(|mapping| mapping.space == space && mapping.contains(address))?;
            Some((*handle, allocation, *mapping))
        });
        let Some((handle, allocation, mapping)) = found else {
            log::debug!(
                "batch queue={queue:#x} address={address:#x} length={length} in=no-mapped-allocation"
            );
            return;
        };
        let offset = mapping.first + (address - mapping.address);
        let length = u64::from(length);
        if length <= WHOLE_BATCH {
            let whole = self.read_allocation(allocation, offset, length);
            log::debug!(
                "batch queue={queue:#x} address={address:#x} length={length} allocation={handle:#x} offset={offset:#x} \
                 allocation_size={:#x} in={} whole=[{}]",
                allocation.size,
                backing(allocation),
                whole.as_deref().map_or("unreadable".into(), hex_dwords),
            );
            return;
        }
        let shown = DWORDS_SHOWN * DWORD;
        let first = self.read_allocation(allocation, offset, length.min(shown));
        let tail_start = offset + length.saturating_sub(shown);
        let last = self.read_allocation(allocation, tail_start, (offset + length).saturating_sub(tail_start));
        log::debug!(
            "batch queue={queue:#x} address={address:#x} length={length} allocation={handle:#x} offset={offset:#x} \
             allocation_size={:#x} in={} first=[{}] last=[{}]",
            allocation.size,
            backing(allocation),
            first.as_deref().map_or("unreadable".into(), hex_dwords),
            last.as_deref().map_or("unreadable".into(), hex_dwords),
        );
    }

    /// `length` bytes at `offset` in an allocation in system memory, or nothing for one on the card.
    fn read_allocation(&self, allocation: &Allocation, offset: u64, length: u64) -> Option<Vec<u8>> {
        let mut bytes = vec![0; usize::try_from(length).ok()?];
        let read = match (allocation.view, allocation.guest_address) {
            (Some(view), _) => self.memory.read_view(view, offset, &mut bytes),
            (None, Some(guest_address)) => {
                self.memory.read_bytes(guest_address.checked_add(offset)?, &mut bytes)
            }
            (None, None) => return None,
        };
        (read == bytes.len()).then_some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwords_are_shown_as_the_gpu_reads_them() {
        let bytes = [0x00, 0x00, 0x00, 0x05, 0xef, 0xbe, 0xad, 0xde, 0xff];
        assert_eq!(hex_dwords(&bytes), "05000000 deadbeef", "a part dword is left out");
        assert_eq!(hex_dwords(&[]), "");
    }

    #[test]
    fn the_backing_is_named_by_what_the_allocation_has() {
        assert_eq!(backing(&Allocation::default()), "card-memory");
        assert_eq!(
            backing(&Allocation { guest_address: Some(0x1000), ..Allocation::default() }),
            "shared-window"
        );
    }
}
