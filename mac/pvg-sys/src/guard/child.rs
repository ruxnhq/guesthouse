// SPDX-License-Identifier: GPL-2.0-or-later
//! Child rings: their definition and their commands, read before the device reads them.

use super::ring::{Judged, LENGTH, NUMBER, WORD_BYTES, Words};
use super::{Child, Effect, Guard, Refusal, What, Why, page_address, page_number};
use crate::memory::GuestMemory;
use crate::profile::PAGE_BYTES;

impl Guard {
    /// Where a child ring's descriptor is in the root page the device has, which is the host's.
    pub(super) fn child_descriptor(&self, id: u32) -> u64 {
        let child = &self.profile.child;
        self.shadow.root() + child.descriptors_at + u64::from(id.max(1) - 1) * child.descriptor_bytes
    }

    /// A child ring. The guest has written its descriptor into its own root page: that is read
    /// here, once, and the device gets a descriptor in the host's root page that names pages of
    /// the host's for the ring. The ring has to start empty: the device runs at once whatever
    /// lies before the written position.
    pub(super) fn judge_define_ring(&self, words: &Words<'_>) -> Result<Judged, Why> {
        let child = &self.profile.child;
        let memory = words.guest;
        let id = words.payload(0).ok_or(Why::RingLayout)?;
        if id == 0 || id > child.most_id {
            return Err(Why::RingId);
        }
        if self.children.contains_key(&id) {
            return Err(Why::RingExists);
        }
        let ours = self.child_descriptor(id);
        let theirs = page_address(self.ring.root_page) + (ours - self.shadow.root());
        let word = |at: u64| memory.word(theirs + at).ok_or(Why::NotGuestMemory);
        let (written, stamp_cell, list_page) =
            (word(child.written_at)?, word(child.stamp_at)?, word(child.list_page_at)?);
        // Cell 0 is the first ring's own, and what lies above the index is the device's to write.
        if written != 0 || stamp_cell == 0 || stamp_cell >= self.profile.ring.stamp_cells {
            return Err(Why::ChildRingLayout);
        }
        let list = page_address(list_page);
        let pages: Vec<u32> = (0..u64::from(child.pages))
            .map(|index| memory.word(list + index * u64::from(WORD_BYTES)).ok_or(Why::NotGuestMemory))
            .collect::<Result<_, _>>()?;
        if !pages.iter().all(|&page| memory.holds(page_address(page), PAGE_BYTES)) {
            return Err(Why::NotGuestMemory);
        }

        let shadow = self.shadow.memory();
        shadow.zero(self.shadow.child_list(id), PAGE_BYTES);
        for index in 0..u64::from(child.pages) {
            let page = page_number(self.shadow.child_ring(id) + index * PAGE_BYTES);
            shadow.set_word(self.shadow.child_list(id) + index * u64::from(WORD_BYTES), page);
        }
        shadow.set_word(ours + child.written_at, 0);
        shadow.set_word(ours + child.read_at, 0);
        shadow.set_word(ours + child.fault_at, child.no_fault);
        shadow.set_word(ours + child.stamp_at, stamp_cell);
        shadow.set_word(ours + child.list_page_at, page_number(self.shadow.child_list(id)));
        // The cell may hold the last stamp of whoever had it before.
        self.mirror_now().empty_stamp_cell(stamp_cell);
        Ok(Judged::Alone(Effect::DefineRing { id, pages, stamp_cell }))
    }

    /// Judges the guest's wish to tell the device of new commands in a child ring. The written
    /// position is read from the guest's descriptor, once; the new commands are copied to the
    /// host's pages for the ring and judged there, as the first ring's are; and the device's
    /// descriptor gets the position. The caller then writes the ring's id to the device.
    ///
    /// A command that would change what the guard keeps is not passed in a child ring.
    pub fn wake(&mut self, memory: &GuestMemory, id: u32, refused: &mut Vec<Refusal>) -> Result<(), Refusal> {
        let child = &self.profile.child;
        let registers = &self.profile.registers;
        let header = self.profile.ring.header_bytes;
        let no = |why| Err(Refusal { what: What::DeviceWrite { offset: registers.wake, value: id }, why });
        let Some(Child { pages, passed }) = self.children.get(&id).cloned() else {
            return no(Why::NoSuchRing);
        };
        let ours = self.child_descriptor(id);
        let theirs = page_address(self.ring.root_page) + (ours - self.shadow.root());
        let Some(target) = memory.word(theirs + child.written_at) else {
            return no(Why::NotGuestMemory);
        };
        let ahead = target.wrapping_sub(passed);
        if ahead > child.bytes {
            return no(if ahead > u32::MAX / 2 { Why::PositionWentBack } else { Why::PositionTooFar });
        }
        // As in the first ring, the copy only goes into the room after the device's read
        // position, which the device keeps in its descriptor in the host's root page.
        let read = self.shadow.memory().word(ours + child.read_at);
        if !read.is_some_and(|read| Self::fits_after_read(read, passed, target, child.bytes)) {
            return no(Why::PositionTooFar);
        }

        // The copy, a page's piece at a time: the guest's pages lie anywhere, the host's in a row.
        let shadow = self.shadow.memory();
        let base = self.shadow.child_ring(id);
        let mut position = passed;
        while position != target {
            let within = u64::from(position % child.bytes);
            let piece = (PAGE_BYTES - within % PAGE_BYTES).min(u64::from(target.wrapping_sub(position)));
            let from = page_address(pages[(within / PAGE_BYTES) as usize]) + within % PAGE_BYTES;
            if !memory.copy_to(from, piece, shadow, base + within) {
                return no(Why::NotGuestMemory);
            }
            position = position.wrapping_add(piece as u32);
        }

        let mut words = Words { ring: shadow, guest: memory, base, room: child.bytes, position: passed };
        let reached = loop {
            let position = words.position;
            if position == target {
                break target;
            }
            let left = target.wrapping_sub(position);
            let (number, length) = (words.word(NUMBER).unwrap_or(0), words.word(LENGTH).unwrap_or(0));
            let what = What::ChildCommand { ring: id, position, number };
            if left < header || length < header || !length.is_multiple_of(WORD_BYTES) || length > left {
                // The device is told of what lies before, and of no more until the guest
                // writes a position that ends a command.
                refused.push(Refusal { what, why: Why::BrokenCommand });
                break position;
            }
            let why = match self.judge(&words, number, length - header) {
                Judged::Plain => None,
                Judged::No(why) => Some(why),
                Judged::Alone(_) => Some(Why::NotInAChildRing),
            };
            if let Some(why) = why {
                shadow.set_word(words.address(NUMBER), self.profile.commands.refused);
                refused.push(Refusal { what, why });
            }
            words.position = position.wrapping_add(length);
        };
        shadow.set_word(ours + child.written_at, reached);
        if let Some(child) = self.children.get_mut(&id) {
            child.passed = reached;
        }
        Ok(())
    }
}
