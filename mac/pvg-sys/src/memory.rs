// SPDX-License-Identifier: GPL-2.0-or-later
//! The guest's memory, as ranges of this process's own.
//!
//! The framework is handed these ranges once, when the device is made, and reads and writes them
//! from then on without asking. The guard reads the same memory first. A guest runs while both do,
//! so nothing here hands out a reference into the memory: every read is a copy out, and a second
//! read of the same place may say something else.

use crate::Error;
use crate::profile::PAGE_BYTES;
use std::any::Any;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const WORD_BYTES: u64 = 4;

/// One stretch of guest memory, and where it lies here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub guest_address: u64,
    pub length: u64,
    pub host_address: *mut u8,
}

struct Ranges {
    ranges: Vec<Range>,
    /// What keeps the ranges mapped. It goes when the last holder of this memory does.
    _owner: Box<dyn Any + Send + Sync>,
}

// SAFETY: the pointers are to memory that `_owner` keeps mapped and that any thread may touch, and
// this type only ever copies in and out of it.
unsafe impl Send for Ranges {}
// SAFETY: as above. Two threads writing the same place is no more than the guest does itself.
unsafe impl Sync for Ranges {}

/// All of a guest's memory that the device may be told of.
#[derive(Clone)]
pub struct GuestMemory {
    shared: Arc<Ranges>,
}

/// Memory of this process's own that stands for a guest's: for tests, and for a program that plays
/// the guest itself.
struct Owned {
    start: *mut u8,
    length: usize,
}

// SAFETY: the mapping is this value's alone, and is only unmapped by its drop.
unsafe impl Send for Owned {}
// SAFETY: as above; nothing reads or writes through `Owned` itself.
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: `start` came from `mmap` with this length, and is unmapped once, here.
        unsafe { libc::munmap(self.start.cast(), self.length) };
    }
}

fn on_a_page(value: u64) -> bool {
    value.is_multiple_of(PAGE_BYTES)
}

impl GuestMemory {
    /// Guest memory that is mapped in this process already.
    ///
    /// Each range has to start and end on one of the device's pages, in the guest and here, and no
    /// two may share a guest address.
    ///
    /// # Safety
    ///
    /// Every range must be mapped at `host_address`, readable and writable, for as long as `owner`
    /// lives, and nothing in this process may hold a reference into it.
    pub unsafe fn from_ranges(ranges: Vec<Range>, owner: Box<dyn Any + Send + Sync>) -> Result<Self, Error> {
        for (index, range) in ranges.iter().enumerate() {
            let Some(end) = range.guest_address.checked_add(range.length) else {
                return Err(Error::Memory(format!(
                    "the range at {:#x} runs past the end of all addresses",
                    range.guest_address
                )));
            };
            if range.length == 0
                || !on_a_page(range.guest_address)
                || !on_a_page(range.length)
                || !on_a_page(range.host_address as u64)
                || range.host_address.is_null()
                || (range.host_address as u64).checked_add(range.length).is_none()
            {
                return Err(Error::Memory(format!(
                    "the range at {:#x} doesn't lie on the device's pages",
                    range.guest_address
                )));
            }
            let overlaps = |other: &Range| {
                range.guest_address < other.guest_address + other.length && other.guest_address < end
            };
            if ranges[..index].iter().any(overlaps) {
                return Err(Error::Memory(format!("two ranges hold the address {:#x}", range.guest_address)));
            }
        }
        Ok(Self { shared: Arc::new(Ranges { ranges, _owner: owner }) })
    }

    /// Zeroed memory of this process's own, as one range at this guest address.
    pub fn anonymous(guest_address: u64, length: u64) -> Result<Self, Error> {
        let bytes = usize::try_from(length)
            .ok()
            .filter(|&bytes| bytes != 0)
            .ok_or_else(|| Error::Memory(format!("{length:#x} bytes are no size for a guest's memory")))?;
        // The framework shares each range with a helper process of its own, and for that the
        // range has to be a mapping of its own: memory from the allocator ends the process.
        //
        // A range starts on one of the device's pages here too, and the kernel only promises its
        // own page: 16 KiB on a Mac with Apple silicon, the device's size, but 4 KiB on most other
        // hosts, where the tests also run. So one device page more is mapped, and the range starts
        // at the first device page inside it.
        let mapped = bytes
            .checked_add(PAGE_BYTES as usize)
            .ok_or_else(|| Error::Memory(format!("{length:#x} bytes are no size for a guest's memory")))?;
        // SAFETY: an anonymous mapping at an address of the kernel's choosing touches nothing
        // that exists.
        let start = unsafe {
            libc::mmap(
                ptr::null_mut(),
                mapped,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            )
        };
        if start == libc::MAP_FAILED {
            return Err(Error::Memory(format!("no {length:#x} bytes to be had for a guest's memory")));
        }
        let start = start.cast::<u8>();
        let owned = Owned { start, length: mapped };
        let skip = (start as u64).next_multiple_of(PAGE_BYTES) - start as u64;
        // SAFETY: `skip` is less than one device page, and the mapping is one device page longer
        // than the range, so the range lies inside it.
        let first = unsafe { start.add(skip as usize) };
        let range = Range { guest_address, length, host_address: first };
        // SAFETY: the mapping is `length` bytes, lives as long as `owned`, and no reference to it
        // exists.
        unsafe { Self::from_ranges(vec![range], Box::new(owned)) }
    }

    /// This memory and another's as one, for a device that is to see both. Both stay mapped for
    /// as long as the joined memory lives.
    pub fn joined(&self, other: &Self) -> Result<Self, Error> {
        let ranges = self.ranges().iter().chain(other.ranges()).copied().collect();
        // SAFETY: every range is one that `self` or `other` was promised is mapped, and both are
        // kept in the owner.
        unsafe { Self::from_ranges(ranges, Box::new((self.clone(), other.clone()))) }
    }

    /// The first address above every range.
    pub fn end(&self) -> u64 {
        self.ranges().iter().map(|range| range.guest_address + range.length).max().unwrap_or(0)
    }

    pub fn ranges(&self) -> &[Range] {
        &self.shared.ranges
    }

    /// Where these bytes lie here, if one range holds them all.
    fn place(&self, address: u64, length: u64) -> Option<*mut u8> {
        let end = address.checked_add(length)?;
        let range = self
            .shared
            .ranges
            .iter()
            .find(|range| range.guest_address <= address && end <= range.guest_address + range.length)?;
        // SAFETY: the offset is inside the range, which `from_ranges` was promised is mapped.
        Some(unsafe { range.host_address.add((address - range.guest_address) as usize) })
    }

    /// Whether all of these bytes are the guest's, in one range.
    pub fn holds(&self, address: u64, length: u64) -> bool {
        self.place(address, length).is_some()
    }

    /// Copies guest memory out. `false`, with nothing copied, if not all of it is there.
    pub fn read(&self, address: u64, out: &mut [u8]) -> bool {
        let Some(from) = self.place(address, out.len() as u64) else {
            return false;
        };
        // SAFETY: `place` found all of `out.len()` bytes mapped, and `out` is not guest memory.
        unsafe { ptr::copy_nonoverlapping(from, out.as_mut_ptr(), out.len()) };
        true
    }

    /// Copies into guest memory. `false`, with nothing written, if not all of it is there.
    pub fn write(&self, address: u64, bytes: &[u8]) -> bool {
        let Some(to) = self.place(address, bytes.len() as u64) else {
            return false;
        };
        // SAFETY: `place` found all of `bytes.len()` bytes mapped, and `bytes` is not guest memory.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), to, bytes.len()) };
        true
    }

    /// A 32-bit word, as the device reads one.
    pub fn word(&self, address: u64) -> Option<u32> {
        let from = self.place(address, WORD_BYTES)?;
        // SAFETY: four bytes are mapped there. The guest may be writing them, hence volatile, and
        // bytes need no alignment.
        Some(u32::from_le_bytes(unsafe { ptr::read_volatile(from.cast::<[u8; 4]>()) }))
    }

    pub fn set_word(&self, address: u64, value: u32) -> bool {
        self.write(address, &value.to_le_bytes())
    }

    /// A word that another writer may change at the same moment, as one that can be swapped
    /// and added to in one step.
    fn shared_word(&self, address: u64) -> Option<&AtomicU32> {
        if !address.is_multiple_of(WORD_BYTES) {
            return None;
        }
        let place = self.place(address, WORD_BYTES)?;
        // SAFETY: four bytes are mapped there for as long as `self` lives, and the place is
        // aligned for a word: ranges start on a page here, and the address is a multiple of
        // four. Whoever else writes the word is the guest or the device, as for any atomic.
        Some(unsafe { AtomicU32::from_ptr(place.cast()) })
    }

    /// Puts a value in a word's place and gives back what was there, in one step.
    pub fn swap_word(&self, address: u64, value: u32) -> Option<u32> {
        Some(u32::from_le(self.shared_word(address)?.swap(value.to_le(), Ordering::SeqCst)))
    }

    /// Sets bits in a word, in one step, leaving the others as whoever else writes it has them.
    pub fn set_bits(&self, address: u64, bits: u32) -> bool {
        self.shared_word(address).map(|word| word.fetch_or(bits.to_le(), Ordering::SeqCst)).is_some()
    }

    /// Writes zeroes over a stretch. `false`, with nothing written, if not all of it is there.
    pub fn zero(&self, address: u64, length: u64) -> bool {
        let Some(to) = self.place(address, length) else {
            return false;
        };
        // SAFETY: `place` found all of `length` bytes mapped.
        unsafe { ptr::write_bytes(to, 0, length as usize) };
        true
    }

    /// Copies a stretch of this memory into another. `false`, with nothing written, if either
    /// stretch is not all there.
    pub fn copy_to(&self, address: u64, length: u64, other: &Self, other_address: u64) -> bool {
        let mut bytes = vec![0u8; length as usize];
        self.read(address, &mut bytes) && other.write(other_address, &bytes)
    }

    /// Two words as one number, the lower first.
    pub fn word64(&self, address: u64) -> Option<u64> {
        let low = self.word(address)?;
        let high = self.word(address.checked_add(WORD_BYTES)?)?;
        Some(u64::from(high) << 32 | u64::from(low))
    }

    /// Whether every byte of the stretch is zero, or `None` if not all of it is there.
    pub fn is_zero(&self, address: u64, length: u64) -> Option<bool> {
        let from = self.place(address, length)?;
        let mut all = 0u8;
        for index in 0..length as usize {
            // SAFETY: `index` is below the length `place` found mapped.
            all |= unsafe { ptr::read_volatile(from.add(index)) };
        }
        Some(all == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_of_this_process_starts_on_a_device_page_whatever_the_host_s_page() {
        // Odd sizes in between, so the kernel's next address isn't left on a device page by luck.
        let kept: Vec<GuestMemory> = (1..=32)
            .map(|pages| {
                let memory = GuestMemory::anonymous(0x4000_0000, pages * PAGE_BYTES).expect("memory");
                let _odd = GuestMemory::anonymous(0x8000_0000, PAGE_BYTES);
                memory
            })
            .collect();
        for memory in &kept {
            let range = memory.ranges()[0];
            assert!(on_a_page(range.host_address as u64), "{:p}", range.host_address);
        }
    }
}
