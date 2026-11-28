//! The lab's card only: the one copy it does for a batch it ends unrun.
//!
//! The display driver stamps each slot of a new frame ring with UpdateSubresource, which Intel's
//! driver runs as a copy on the 3D engine from memory the guest's CPU wrote. The lab's card runs
//! nothing, so the slots stay zero and the core never finds the ring. Here, when a batch ends in a
//! VM that has just been given large buffers, the VM's memory is searched for the stamps the guest
//! wrote, and once a generation has as many stamps as the VM has blank buffers of one size made
//! last, they are copied to the start of those buffers in the order they were made, as the driver makes its slots. Which
//! buffer gets which slot is the lab's guess, as the batch itself is never read. Nothing of this is
//! what a card does: it lets the code after the ring is found meet a real guest.

use super::{Bytes, Vm};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// How a stamp starts: the display driver's `GH_STAMP_MAGIC`.
const MAGIC: &[u8; 8] = b"GHOUSEST";
const STAMP_BYTES: usize = 16;
const GENERATION_AT: usize = 8;
const SLOT_AT: usize = 12;
/// Smaller buffers are no slot: a slot holds a whole picture.
const SMALLEST_SLOT: usize = 1 << 20;
/// Bindings larger than this aren't searched, to keep a batch's end quick.
const LARGEST_SEARCHED: u64 = 64 << 20;
/// How many batch ends after a large buffer is bound the VM is searched at.
const SEARCHES: u32 = 16;

/// What the lab has done for the stamps, per VM.
#[derive(Debug, Default)]
pub(super) struct Stamps {
    /// Searches left, by VM.
    searches: HashMap<u32, u32>,
    /// By VM, the stamps that were in its memory when its last ring's were copied: what is still
    /// there unchanged is old. A driver that starts over numbers its rings from one again, and its
    /// upload memory can put the new stamps where the old ones were, so neither a generation nor a
    /// place alone can say a stamp is new.
    copied: HashMap<u32, HashSet<(u64, [u8; STAMP_BYTES])>>,
    /// Buffers stamped here, by their serial. Their stamps are copies, never the guest's own.
    stamped: HashSet<u64>,
}

/// Whether a buffer is big enough to be a slot.
pub(super) fn may_be_a_slot(bytes: &Bytes) -> bool {
    bytes.length >= SMALLEST_SLOT
}

/// A stamp found in a VM's memory: its generation and slot, its bytes, and where it was.
#[derive(Debug, Clone, Copy)]
struct Found {
    generation: u32,
    slot: u32,
    bytes: [u8; STAMP_BYTES],
    at: u64,
}

impl Stamps {
    /// A large buffer was bound in a VM: its next batches may carry stamps.
    pub(super) fn watch(&mut self, vm: u32) {
        self.searches.insert(vm, SEARCHES);
    }

    /// At the end of a batch in a VM, copies a generation's stamps if they are all there.
    pub(super) fn copy_any(&mut self, vm_id: u32, vm: &Vm) {
        let Some(left) = self.searches.get_mut(&vm_id).filter(|left| **left > 0) else { return };
        *left -= 1;
        let last = *left == 0;
        let all = stamps_in(vm, &self.stamped);
        let old = self.copied.get(&vm_id);
        let found: Vec<Found> = all
            .iter()
            .filter(|stamp| !old.is_some_and(|old| old.contains(&(stamp.at, stamp.bytes))))
            .copied()
            .collect();
        let Some(generation) = found.iter().map(|stamp| stamp.generation).max() else {
            if last {
                log::warn!(
                    "intel: lab card: no new slot stamps in VM {vm_id}'s memory in the {SEARCHES} batches \
                     after its last large buffer"
                );
            }
            return;
        };
        let mut slots: BTreeMap<u32, Found> = BTreeMap::new();
        for stamp in found.into_iter().filter(|stamp| stamp.generation == generation) {
            slots.entry(stamp.slot).or_insert(stamp);
        }
        let count = slots.len();
        if slots.keys().copied().ne(0..count as u32) {
            log::warn!(
                "intel: lab card: VM {vm_id} holds stamps of generation {generation} for slots {:?}, \
                 not 0 to n; nothing copied yet",
                slots.keys().collect::<Vec<_>>()
            );
            return;
        }
        let Some(targets) = newest_blank_buffers(vm, count) else {
            log::warn!(
                "intel: lab card: VM {vm_id} holds {count} stamps of generation {generation}, but not \
                 {count} blank buffers of one size made last; nothing copied"
            );
            return;
        };
        for (stamp, target) in slots.values().zip(&targets) {
            // SAFETY: the target is a whole buffer of at least SMALLEST_SLOT bytes, alive while the
            // VM holds it.
            unsafe { std::ptr::copy_nonoverlapping(stamp.bytes.as_ptr(), target.start, STAMP_BYTES) };
        }
        self.stamped.extend(targets.iter().map(|target| target.serial));
        self.copied.insert(vm_id, all.iter().map(|stamp| (stamp.at, stamp.bytes)).collect());
        // Until the VM is given large buffers again, nothing more is looked for.
        self.searches.remove(&vm_id);
        let first = slots.values().next().map_or(0, |stamp| stamp.at);
        log::warn!(
            "intel: lab card: copied the display driver's {count} slot stamps of generation {generation} \
             (the first at {first:#x} in VM {vm_id}) to the start of its newest {count} buffers of \
             {:#x} bytes, in the order they were made. A stand-in for the copy the 3D engine would \
             have run; which buffer has which slot is the lab's guess",
            targets[0].length
        );
    }
}

/// Every stamp in the VM's memory, at any 4-byte boundary, but for the buffers stamped here.
fn stamps_in(vm: &Vm, stamped: &HashSet<u64>) -> Vec<Found> {
    let mut found = Vec::new();
    for (&start, bound) in &vm.bound {
        let copy = bound.buffer.as_ref().is_some_and(|bytes| stamped.contains(&bytes.serial));
        if copy || bound.length > LARGEST_SEARCHED || (bound.length as usize) < STAMP_BYTES {
            continue;
        }
        // SAFETY: a binding's host bytes stay mapped while the VM holds it.
        let bytes = unsafe { std::slice::from_raw_parts(bound.host, bound.length as usize) };
        for at in (0..=bytes.len() - STAMP_BYTES).step_by(4) {
            if bytes[at..at + MAGIC.len()] != *MAGIC {
                continue;
            }
            let stamp: [u8; STAMP_BYTES] = bytes[at..at + STAMP_BYTES].try_into().expect("16 bytes");
            let word = |from: usize| u32::from_le_bytes(stamp[from..from + 4].try_into().expect("4 bytes"));
            found.push(Found {
                generation: word(GENERATION_AT),
                slot: word(SLOT_AT),
                bytes: stamp,
                at: start + at as u64,
            });
        }
    }
    found
}

/// The large buffers made last in the VM, as long as they are of one size and blank at the start,
/// oldest first, if there are `count` of them: a driver makes its slots one after another.
fn newest_blank_buffers(vm: &Vm, count: usize) -> Option<Vec<Arc<Bytes>>> {
    let mut buffers: Vec<Arc<Bytes>> = Vec::new();
    for bound in vm.bound.values() {
        let Some(bytes) = &bound.buffer else { continue };
        if may_be_a_slot(bytes) && !buffers.iter().any(|seen| Arc::ptr_eq(seen, bytes)) {
            buffers.push(Arc::clone(bytes));
        }
    }
    buffers.sort_unstable_by_key(|bytes| std::cmp::Reverse(bytes.serial));
    let length = buffers.first()?.length;
    // SAFETY: each buffer is at least SMALLEST_SLOT bytes and mapped while held.
    let blank = |bytes: &Arc<Bytes>| {
        unsafe { std::slice::from_raw_parts(bytes.start, STAMP_BYTES) }.iter().all(|b| *b == 0)
    };
    let mut newest: Vec<Arc<Bytes>> =
        buffers.into_iter().take_while(|bytes| bytes.length == length && blank(bytes)).collect();
    if newest.len() != count {
        return None;
    }
    newest.reverse();
    Some(newest)
}

#[cfg(test)]
mod tests {
    use super::super::{FakeXe, abi};
    use crate::xe::abi::{BindOp, NewBuffer};
    use crate::xe::device::Xe;
    use guesthouse_messages::indirect::{STAMP_SIZE, Stamp, as_the_driver_sends};

    const SLOT: u64 = 0x1f_0000;

    fn buffer(xe: &mut FakeXe, vm: u32, size: u64, address: u64) -> u32 {
        let new = NewBuffer { size, placement: 1, flags: 0, cpu_caching: abi::CPU_CACHING_WRITE_BACK };
        let handle = xe.create_buffer(&new).unwrap();
        let op = BindOp {
            op: abi::BIND_MAP,
            buffer: handle,
            offset_or_host_address: 0,
            length: size,
            address,
            pat_index: crate::xe::plan::pat::SHARED_WITH_CPU,
        };
        xe.bind(vm, &op, None).unwrap();
        handle
    }

    fn start_of(xe: &mut FakeXe, handle: u32, size: u64) -> Option<Stamp> {
        let map = xe.map_buffer(handle, size).unwrap();
        let mut bytes = [0; STAMP_SIZE];
        assert!(map.read(0, &mut bytes));
        Stamp::parse(&bytes)
    }

    #[test]
    fn the_magic_is_the_drivers() {
        let bytes = as_the_driver_sends::stamp(Stamp { generation: 3, slot: 2 });
        assert_eq!(&bytes[..8], super::MAGIC);
        assert_eq!(bytes.len(), super::STAMP_BYTES);
    }

    #[test]
    fn a_generations_stamps_go_to_the_newest_buffers_of_one_size_in_order() {
        let mut xe = FakeXe::lab();
        xe.stop_running();
        let vm = xe.create_vm(0).unwrap();
        let decoys: Vec<u32> =
            (0..3).map(|i| buffer(&mut xe, vm, 0x23_0000, 0x1000_0000 + i * 0x100_0000)).collect();
        let slots: Vec<u32> =
            (0..2).map(|i| buffer(&mut xe, vm, SLOT, 0x2000_0000 + i * 0x100_0000)).collect();
        let upload = buffer(&mut xe, vm, 0x1000, 0x3000_0000);
        let batch = buffer(&mut xe, vm, 0x1000, 0x3001_0000);
        let queue = xe
            .create_queue(vm, abi::EngineInstance { class: 0, instance: 0, gt: 0 }, abi::Keying::BindingIndex)
            .unwrap();

        let mut map = xe.map_buffer(upload, 0x1000).unwrap();
        map.write(0x44, &as_the_driver_sends::stamp(Stamp { generation: 1, slot: 0 }));
        xe.exec(queue, 0x3001_0000, &[]).unwrap();
        for &handle in &slots {
            assert_eq!(start_of(&mut xe, handle, SLOT), None, "one stamp for two slots: wait");
        }

        map.write(0x80, &as_the_driver_sends::stamp(Stamp { generation: 1, slot: 1 }));
        xe.exec(queue, 0x3001_0000, &[]).unwrap();
        for (slot, &handle) in slots.iter().enumerate() {
            assert_eq!(start_of(&mut xe, handle, SLOT), Some(Stamp { generation: 1, slot: slot as u32 }));
        }
        for handle in decoys {
            assert_eq!(start_of(&mut xe, handle, 0x23_0000), None);
        }
        let _ = batch;
    }

    #[test]
    fn a_driver_that_starts_over_at_generation_one_in_the_same_place_gets_its_new_slots_stamped() {
        const BIGGER: u64 = 0x7f_0000;
        let mut xe = FakeXe::lab();
        xe.stop_running();
        let vm = xe.create_vm(0).unwrap();
        let upload = buffer(&mut xe, vm, 0x1000, 0x3000_0000);
        let queue = xe
            .create_queue(vm, abi::EngineInstance { class: 0, instance: 0, gt: 0 }, abi::Keying::BindingIndex)
            .unwrap();
        let mut map = xe.map_buffer(upload, 0x1000).unwrap();
        // As the guest did: 800x600 as generations 1 and 2, then 1920x1080 as generation 1 again,
        // each ring's stamps where the last ones were.
        for (ring, (generation, size)) in [(1, SLOT), (2, SLOT), (1, BIGGER)].into_iter().enumerate() {
            let at = 0x4000_0000 + 0x1000_0000 * ring as u64;
            let slots: Vec<u32> = (0..2).map(|i| buffer(&mut xe, vm, size, at + i * 0x100_0000)).collect();
            xe.exec(queue, 0x3000_0000, &[]).unwrap();
            assert_eq!(start_of(&mut xe, slots[0], size), None, "ring {ring}: the old stamps are old");
            for slot in 0..2 {
                map.write(
                    0x40 + 0x10 * slot,
                    &as_the_driver_sends::stamp(Stamp { generation, slot: slot as u32 }),
                );
            }
            xe.exec(queue, 0x3000_0000, &[]).unwrap();
            for (slot, &handle) in slots.iter().enumerate() {
                assert_eq!(
                    start_of(&mut xe, handle, size),
                    Some(Stamp { generation, slot: slot as u32 }),
                    "ring {ring}"
                );
            }
        }
    }
}
