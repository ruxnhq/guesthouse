// SPDX-License-Identifier: GPL-2.0-or-later
//! Pages of the host's own that the device reads in place of the guest's.
//!
//! A guest has more than one processor. While the guard reads a word in the guest's memory, and
//! afterwards, another processor can write it, and a device that reads the same memory reads
//! what was never judged. So the device is not shown the guest's ring, its root page, a
//! display's state page or the kernel task's table. It is shown pages here, which lie in a range
//! of their own that the device takes for guest memory and that the guest has no way to write:
//!
//! - The ring. A kick copies the new commands here, and the guard judges the copy.
//! - The root page. The device keeps its tables there.
//! - A display's state page. The device reads its own words back from it.
//! - The kernel task's root page and its table, copied when the task is defined.
//! - A user task's root page and its table, copied again before each command that names the
//!   task. The framework hands a task's work to a helper process, which is shown the same ranges
//!   as the device. A table of the guest's own could name the pages here, and a command buffer
//!   would then have the GPU write them.
//!
//! The guest goes on reading its own pages, so what the device writes is copied there, word by
//! word as it changes: stamps, a display's state. [`Mirror`] does that, and takes the few words a
//! guest writes for the device to read the other way.
//!
//! The surface mapper has a range of its own, above the device's, which the device is not shown
//! and the mapper is shown beside the guest's memory: its ring, its table, a page of descriptions
//! for each 32 mapping ids, and a page list for each id. A kick copies each new command here, and
//! a map command's description and page list with it, and the guard judges the copies. The
//! mapper's outcome words go back to the guest's ring.

use crate::Error;
use crate::memory::GuestMemory;
use crate::profile::{PAGE_BYTES, PAGE_SHIFT, Profile};

use super::PORTS;
use std::collections::{BTreeMap, VecDeque};

const WORD_BYTES: u64 = 4;
/// Where the range lies above the guest's memory: at the next multiple of this.
const PLACE_UNIT: u64 = 1 << 30;
/// A page's number has to leave room for the bit that a task's table sets beside it.
const MOST_PAGES: u64 = 1 << 31;
const PAGE_WORDS: u64 = PAGE_BYTES / WORD_BYTES;
/// The mapper's word for a page has 30 bits for its number.
const MOST_MAPPER_PAGES: u64 = 1 << 30;
/// Where in a mapper's command the mapper writes how it went.
const MAPPER_OUTCOME: u64 = 8;

/// The range, and which of its pages is what.
#[derive(Clone)]
pub struct Shadow {
    memory: GuestMemory,
    base: u64,
    ring_pages: u64,
    /// A child ring's pages here: one that lists the others, and the ring's own.
    child_pages: u64,
    /// The pages before the user tasks', which are the ones a reset zeroes.
    fixed_pages: u64,
    /// A user task's pages here: its root page, the top of its table, and the table's lowest
    /// level.
    task_pages: u64,
    /// The mapper's range, and how many pages its ring has.
    mapper: GuestMemory,
    mapper_ring_pages: u64,
    mapper_description_pages: u64,
}

impl Shadow {
    /// A range above a guest's memory, zeroed, with room for a ring of the profile's largest
    /// size.
    pub fn above(guest: &GuestMemory, profile: &Profile) -> Result<Self, Error> {
        let ring_pages = u64::from(profile.ring.most_bytes).div_ceil(PAGE_BYTES);
        let child_pages = 1 + u64::from(profile.child.pages);
        let fixed_pages = Self::RING + ring_pages + u64::from(profile.child.most_id) * child_pages;
        let task_pages = 2 + profile.task.most_user_leaves;
        let pages = fixed_pages + u64::from(profile.task.most_id) * task_pages;
        let base = guest.end().next_multiple_of(PLACE_UNIT);
        if (base >> PAGE_SHIFT) + pages > MOST_PAGES {
            return Err(Error::Memory(
                "the guest's memory leaves no page numbers for the host's own pages".to_owned(),
            ));
        }
        let memory = GuestMemory::anonymous(base, pages * PAGE_BYTES)?;

        let shape = &profile.mapper;
        let mapper_ring_pages = (shape.most_slots * shape.slot_bytes).div_ceil(PAGE_BYTES);
        let mapper_description_pages = u64::from(shape.most_id / shape.ids_per_page) + 1;
        let mapper_pages = mapper_ring_pages + 1 + mapper_description_pages + u64::from(shape.most_id) + 1;
        let mapper_base = memory.end().next_multiple_of(PLACE_UNIT);
        // The mapper names a page by a word with its number two bits up.
        if (mapper_base >> PAGE_SHIFT) + mapper_pages > MOST_MAPPER_PAGES {
            return Err(Error::Memory(
                "the guest's memory leaves no page numbers for the mapper's own pages".to_owned(),
            ));
        }
        let mapper = GuestMemory::anonymous(mapper_base, mapper_pages * PAGE_BYTES)?;
        let shadow = Self {
            memory,
            base,
            ring_pages,
            child_pages,
            fixed_pages,
            task_pages,
            mapper,
            mapper_ring_pages,
            mapper_description_pages,
        };
        // The mapper's table names the host's pages of descriptions, always the same ones.
        for index in 0..mapper_description_pages {
            let entry = mapper_entry(shadow.mapper_descriptions(index));
            shadow.mapper.set_word(shadow.mapper_table() + index * WORD_BYTES, entry);
        }
        Ok(shadow)
    }

    const ROOT: u64 = 0;
    const KERNEL_ROOT: u64 = 1;
    const KERNEL_TABLE: u64 = 2;
    const STATE: u64 = 3;
    const RING: u64 = Self::STATE + PORTS as u64;

    fn page(&self, index: u64) -> u64 {
        self.base + index * PAGE_BYTES
    }

    /// The range as memory, to read and write it, and to hand to the device beside the guest's.
    pub fn memory(&self) -> &GuestMemory {
        &self.memory
    }

    pub fn root(&self) -> u64 {
        self.page(Self::ROOT)
    }

    pub fn kernel_root(&self) -> u64 {
        self.page(Self::KERNEL_ROOT)
    }

    pub fn kernel_table(&self) -> u64 {
        self.page(Self::KERNEL_TABLE)
    }

    pub fn state_page(&self, port: u32) -> u64 {
        self.page(Self::STATE + u64::from(port))
    }

    pub fn ring(&self) -> u64 {
        self.page(Self::RING)
    }

    /// The page that lists a child ring's pages, and the first of those pages, which lie in a
    /// row. Ids start at 1.
    pub fn child_list(&self, id: u32) -> u64 {
        self.page(Self::RING + self.ring_pages + u64::from(id - 1) * self.child_pages)
    }

    pub fn child_ring(&self, id: u32) -> u64 {
        self.child_list(id) + PAGE_BYTES
    }

    /// A user task's root page, the top page of its table, and the pages of the table's lowest
    /// level. Ids start at 1. They are zeroed when a task is defined, and not touched before:
    /// most are never used.
    pub fn task_root(&self, id: u32) -> u64 {
        self.page(self.fixed_pages + u64::from(id - 1) * self.task_pages)
    }

    pub fn task_top(&self, id: u32) -> u64 {
        self.task_root(id) + PAGE_BYTES
    }

    pub fn task_leaf(&self, id: u32, leaf: u64) -> u64 {
        self.task_top(id) + (1 + leaf) * PAGE_BYTES
    }

    pub fn clear_task(&self, id: u32) {
        self.memory.zero(self.task_root(id), self.task_pages * PAGE_BYTES);
    }

    /// Zeroes all but the user tasks' pages, as a reset leaves the device knowing nothing.
    pub fn clear(&self) {
        self.memory.zero(self.base, self.fixed_pages * PAGE_BYTES);
    }

    /// The mapper's range, to hand to the mapper beside the guest's memory. The device is not
    /// shown it.
    pub fn mapper_memory(&self) -> &GuestMemory {
        &self.mapper
    }

    fn mapper_page(&self, index: u64) -> u64 {
        self.mapper.ranges()[0].guest_address + index * PAGE_BYTES
    }

    /// The mapper's ring, room for the longest the profile allows.
    pub fn mapper_ring(&self) -> u64 {
        self.mapper_page(0)
    }

    pub fn mapper_table(&self) -> u64 {
        self.mapper_page(self.mapper_ring_pages)
    }

    /// The page of descriptions that the table's entry `index` names.
    pub fn mapper_descriptions(&self, index: u64) -> u64 {
        self.mapper_page(self.mapper_ring_pages + 1 + index)
    }

    /// The page list of a mapping's surface.
    pub fn page_list(&self, id: u32) -> u64 {
        self.mapper_page(self.mapper_ring_pages + 1 + self.mapper_description_pages + u64::from(id))
    }
}

/// How the mapper names a page: its number two bits up, and the lowest bit for "there is one".
pub fn mapper_entry(address: u64) -> u32 {
    ((address >> PAGE_SHIFT) as u32) << 2 | 1
}

/// A command the mapper was passed whose outcome word the guest has not had yet.
struct MapperCommand {
    count: u64,
    guest_slot: u64,
    host_slot: u64,
}

/// A display's state page: where the guest has it, and each word as it was last copied there.
struct StateMirror {
    guest: u64,
    last: Vec<u32>,
}

/// Copies what the device wrote into the host's pages to where the guest reads it, and the words
/// a guest writes for the device the other way. Shared between the thread that serves the
/// guest's register accesses and the framework's threads, which raise interrupts: what the device
/// wrote has to be in the guest's memory before the guest hears of it.
pub struct Mirror {
    profile: &'static Profile,
    guest: GuestMemory,
    shadow: Shadow,
    /// Where the guest's ring is, while it is on.
    ring: Option<u64>,
    stamps: Vec<u32>,
    states: [Option<StateMirror>; PORTS as usize],
    /// For each child ring that is defined, where its descriptor is in the guest's root page.
    children: BTreeMap<u32, u64>,
    /// The mapper's commands that are passed and whose outcome may not be in the guest's ring yet,
    /// oldest first.
    mapper: VecDeque<MapperCommand>,
}

impl Mirror {
    pub fn new(profile: &'static Profile, guest: GuestMemory, shadow: Shadow) -> Self {
        let stamps = vec![0; profile.ring.stamp_cells as usize];
        Self {
            profile,
            guest,
            shadow,
            ring: None,
            stamps,
            states: [const { None }; PORTS as usize],
            children: BTreeMap::new(),
            mapper: VecDeque::new(),
        }
    }

    /// The ring is on, at this place in the guest's memory, or off. The stamp cells start empty.
    pub fn set_ring(&mut self, guest_ring: Option<u64>) {
        self.ring = guest_ring;
        self.stamps.fill(0);
        self.shadow.memory.zero(self.shadow.ring(), self.stamps.len() as u64 * WORD_BYTES);
    }

    /// A display has this page of the guest's for its state, or none. The device has the host's
    /// page for it, which starts empty.
    pub fn set_state_page(&mut self, port: u32, guest_page: Option<u64>) {
        self.states[port as usize] =
            guest_page.map(|guest| StateMirror { guest, last: vec![0; PAGE_WORDS as usize] });
        if guest_page.is_some() {
            // The device raises an interrupt for an event the guest's mask has. The guest's mask
            // is in the guest's page, where the device doesn't look, so the device is told to
            // raise one for every event, and the guest finds in its own page which it was.
            let mask = self.shadow.state_page(port) + self.profile.display.event_mask_at;
            self.shadow.memory.set_word(mask, u32::MAX);
        }
    }

    /// A child ring is defined, with its descriptor at this place in the guest's root page, or
    /// gone.
    pub fn set_child(&mut self, id: u32, guest_descriptor: Option<u64>) {
        match guest_descriptor {
            Some(descriptor) => self.children.insert(id, descriptor),
            None => self.children.remove(&id),
        };
    }

    /// A stamp cell gets a new user: whatever stamp it held is nobody's.
    pub fn empty_stamp_cell(&mut self, cell: u32) {
        if let Some(last) = self.stamps.get_mut(cell as usize) {
            *last = 0;
            self.shadow.memory.set_word(self.shadow.ring() + u64::from(cell) * WORD_BYTES, 0);
        }
    }

    pub fn clear(&mut self) {
        self.ring = None;
        self.stamps.fill(0);
        self.states = [const { None }; PORTS as usize];
        self.children.clear();
        self.mapper.clear();
    }

    /// The mapper was passed command `count`, which lies at these places in the guest's ring and
    /// the host's. Its outcome goes to the guest's when the mapper has written it.
    pub fn mapper_sent(&mut self, count: u64, slots: u64, guest_slot: u64, host_slot: u64) {
        // A guest that never asks how far the mapper is has its slots used again, a ring's length
        // of commands on: what lies in one is the newest command's.
        // Counts only go up, unless the mapper was reset.
        if self.mapper.back().is_some_and(|newest| newest.count >= count) {
            self.mapper.clear();
        }
        while self.mapper.front().is_some_and(|oldest| oldest.count + slots <= count) {
            self.mapper.pop_front();
        }
        self.mapper.push_back(MapperCommand { count, guest_slot, host_slot });
    }

    /// Copies the mapper's outcome words to the guest's ring. With `done`, what the mapper's count
    /// of commands done reads, the commands below it are done with and forgotten. A guest learns
    /// that a command is done from that count, so its outcome is there before the guest reads it.
    pub fn mapper_to_guest(&mut self, done: Option<u64>) {
        let host = &self.shadow.mapper;
        for command in &self.mapper {
            if let Some(outcome) = host.word(command.host_slot + MAPPER_OUTCOME)
                && self.guest.word(command.guest_slot + MAPPER_OUTCOME) != Some(outcome)
            {
                self.guest.set_word(command.guest_slot + MAPPER_OUTCOME, outcome);
            }
        }
        if let Some(done) = done {
            self.mapper.retain(|command| command.count >= done);
        }
    }

    /// Copies to the guest's pages what the device has written since the last call.
    pub fn to_guest(&mut self) {
        let display = &self.profile.display;
        let shadow = &self.shadow.memory;
        if let Some(ring) = self.ring {
            for (cell, last) in self.stamps.iter_mut().enumerate() {
                let at = cell as u64 * WORD_BYTES;
                if let Some(stamp) = shadow.word(self.shadow.ring() + at).filter(|stamp| stamp != last) {
                    self.guest.set_word(ring + at, stamp);
                    *last = stamp;
                }
            }
        }
        // Of a child ring's descriptor the device writes how far it has read, where a refused
        // command lies, and the bit that says the ring is defined.
        let child = &self.profile.child;
        for (&id, &theirs) in &self.children {
            let ours = self.shadow.root() + child.descriptors_at + u64::from(id - 1) * child.descriptor_bytes;
            for at in [child.read_at, child.fault_at, child.stamp_at] {
                if let Some(word) =
                    shadow.word(ours + at).filter(|&word| self.guest.word(theirs + at) != Some(word))
                {
                    self.guest.set_word(theirs + at, word);
                }
            }
        }
        for (port, state) in self.states.iter_mut().enumerate() {
            let Some(state) = state else {
                continue;
            };
            let page = self.shadow.state_page(port as u32);
            // The guest's own words stay the guest's.
            let guests = [display.event_mask_at, display.cursor_place_at, display.cursor_shown_at];
            for (index, last) in state.last.iter_mut().enumerate() {
                let at = index as u64 * WORD_BYTES;
                if guests.contains(&at) {
                    continue;
                }
                if at == display.events_at {
                    // The device sets bits here and the guest clears them, each in its own page:
                    // what the device has set is taken from the one and added to the other.
                    match shadow.swap_word(page + at, 0) {
                        Some(0) | None => {}
                        Some(bits) => {
                            self.guest.set_bits(state.guest + at, bits);
                        }
                    }
                    continue;
                }
                if let Some(word) = shadow.word(page + at).filter(|word| word != last) {
                    self.guest.set_word(state.guest + at, word);
                    *last = word;
                }
            }
        }
    }

    /// The words at these places of a display's state page, in the host's page the device writes
    /// and in the guest's, for a line when something stands still; `None` while the display has
    /// no page.
    pub fn state_words(&self, port: u32, places: &[u64]) -> Option<(Vec<u32>, Vec<u32>)> {
        let state = self.states.get(port as usize)?.as_ref()?;
        let page = self.shadow.state_page(port);
        let host = places.iter().map(|&at| self.shadow.memory.word(page + at).unwrap_or(0)).collect();
        let guest = places.iter().map(|&at| self.guest.word(state.guest + at).unwrap_or(0)).collect();
        Some((host, guest))
    }

    /// Takes over the words a guest writes into a state page for the device to read: where the
    /// cursor is, and whether it shows.
    pub fn from_guest(&mut self) {
        let display = &self.profile.display;
        for (port, state) in self.states.iter().enumerate() {
            let Some(state) = state else {
                continue;
            };
            let page = self.shadow.state_page(port as u32);
            if let Some(place) = self.guest.word(state.guest + display.cursor_place_at) {
                self.shadow.memory.set_word(page + display.cursor_place_at, place);
            }
            if let Some(shown) = self.guest.word(state.guest + display.cursor_shown_at) {
                self.shadow.memory.set_word(page + display.cursor_shown_at, u32::from(shown != 0));
            }
        }
    }
}
