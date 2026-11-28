// SPDX-License-Identifier: GPL-2.0-or-later
//! The surface mapper's registers and commands, read before the mapper reads them.
//!
//! The mapper is not shown the guest's ring, table, descriptions or page lists. It is told of
//! pages of the host's own in their place (see [`super::Shadow`]): a kick copies each new
//! command there, and for a map command the description and the page list it names, and the copy
//! is what is judged. A guest's second processor can change its own pages while this runs, but not
//! what the mapper reads.

use super::shadow::mapper_entry;
use super::{Guard, Refusal, What, Why};
use crate::memory::GuestMemory;
use crate::profile::{PAGE_BYTES, PAGE_SHIFT, SurfaceShape};

const WORD_BYTES: u64 = 4;
/// In a command: its number, and the mapping's id.
const NUMBER: u64 = 0;
const ID: u64 = 4;
/// How the mapper names a page: the lowest bit says there is one, and the number is two bits up.
const THERE: u32 = 1;
const PAGE_NUMBER_SHIFT: u32 = 2;
/// A surface's pages are listed in one page.
const MOST_SURFACE_PAGES: u64 = PAGE_BYTES / WORD_BYTES;
/// A driver may round a row's bytes up to this, as our Linux guest driver does for widths whose
/// rows a GPU would not take as they are.
const ROW_ALIGN: u32 = 64;

/// The address of the page a word names the mapper's way, if it names one of the guest's.
fn named_page(memory: &GuestMemory, word: u32) -> Option<u64> {
    let address = u64::from(word >> PAGE_NUMBER_SHIFT) << PAGE_SHIFT;
    (word & THERE != 0 && memory.holds(address, PAGE_BYTES)).then_some(address)
}

fn word_of(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

/// A copy of a surface's description and of the page list it names, read once each.
struct Surface {
    description: Vec<u8>,
    /// The list's entries, as many as the description's size needs.
    pages: Vec<u32>,
}

/// Reads a description and its page list out of the guest's memory, if they are a plain
/// picture: one plane of the one format, as wide and high as it says, its rows the pixels'
/// bytes or those rounded up to [`ROW_ALIGN`], in whole pages of the guest's that its list
/// names, and nothing else set. What is judged is the copy.
fn plain_picture(memory: &GuestMemory, shape: &SurfaceShape, at: u64) -> Option<Surface> {
    let mut description = vec![0u8; shape.bytes as usize];
    if !memory.read(at, &mut description) {
        return None;
    }
    let word = |at| word_of(&description, at);
    let side = |at| {
        (word(at) & 0xFF == 1).then_some(word(at) >> 8).filter(|side| (1..=shape.most_side).contains(side))
    };
    let (width, height) = (side(shape.width_at)?, side(shape.height_at)?);
    let tight = width * shape.pixel_bytes;
    let row_bytes = word(shape.row_bytes_at);
    if row_bytes != tight && row_bytes != tight.next_multiple_of(ROW_ALIGN) {
        return None;
    }
    let length = (u64::from(row_bytes) * u64::from(height)).next_multiple_of(PAGE_BYTES);
    let count = length / PAGE_BYTES;
    let list = named_page(memory, word(shape.page_list_at))?;
    let named = [
        (shape.page_list_at, word(shape.page_list_at)),
        (shape.format_at, shape.format),
        (shape.length_at, length as u32),
        (shape.width_at, word(shape.width_at)),
        (shape.height_at, word(shape.height_at)),
        (shape.row_bytes_at, row_bytes),
        (shape.pixel_bytes_at, shape.pixel_bytes),
    ];
    // Every word is either one of those, with the value it must have, or zero.
    let words_fit = (0..description.len()).step_by(WORD_BYTES as usize).all(|at| {
        let wanted = named.iter().find(|(place, _)| *place == at).map_or(0, |(_, value)| *value);
        word(at) == wanted
    });
    if !words_fit || count > MOST_SURFACE_PAGES {
        return None;
    }
    let mut bytes = vec![0u8; (count * WORD_BYTES) as usize];
    if !memory.read(list, &mut bytes) {
        return None;
    }
    let pages: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|entry| u32::from_le_bytes(*entry)).collect();
    pages.iter().all(|&entry| named_page(memory, entry).is_some()).then_some(Surface { description, pages })
}

impl Guard {
    /// What one of the mapper's registers reads as that the mapper was told another value for
    /// than the guest wrote: the guest finds its own addresses there.
    pub fn mapper_reads_as(&self, offset: u64) -> Option<u64> {
        let shape = &self.profile.mapper;
        if offset == shape.ring {
            Some(self.mapper.ring)
        } else if offset == shape.table {
            Some(self.mapper.table)
        } else {
            None
        }
    }

    /// Judges a write to one of the mapper's registers, and gives the value to write in its
    /// place: for the ring and the table the host's pages, otherwise the guest's value.
    /// `written_now` is what the mapper's own count of written commands reads: it outlives a
    /// guest's driver, so it is asked, not kept.
    ///
    /// A write of the count is the kick. The commands it tells of are copied to the host's ring
    /// and judged there. One that would have the mapper make a surface the guard doesn't know to
    /// be good becomes a command the mapper answers with "unsupported", in the host's ring, and is
    /// noted in `refused`.
    pub fn mapper_write(
        &mut self,
        memory: &GuestMemory,
        offset: u64,
        value: u64,
        written_now: u64,
        refused: &mut Vec<Refusal>,
    ) -> Result<u64, Refusal> {
        let shape = &self.profile.mapper;
        let no = |why| Err(Refusal { what: What::MapperWrite { offset, value }, why });
        let ring_bytes = |slots: u64| slots.max(shape.slots_unit) * shape.slot_bytes;
        let on_a_page = |address: u64| address != 0 && address.is_multiple_of(PAGE_BYTES);

        if offset == shape.ring {
            if !on_a_page(value) || !memory.holds(value, ring_bytes(self.mapper.slots)) {
                return no(Why::MapperLayout);
            }
            self.mapper.ring = value;
            Ok(self.shadow.mapper_ring())
        } else if offset == shape.slots {
            if value == 0 || !value.is_multiple_of(shape.slots_unit) || value > shape.most_slots {
                return no(Why::MapperRingLength);
            }
            if self.mapper.ring != 0 && !memory.holds(self.mapper.ring, ring_bytes(value)) {
                return no(Why::MapperLayout);
            }
            self.mapper.slots = value;
            Ok(value)
        } else if offset == shape.table {
            if !on_a_page(value) || !memory.holds(value, PAGE_BYTES) {
                return no(Why::MapperLayout);
            }
            self.mapper.table = value;
            Ok(self.shadow.mapper_table())
        } else if offset == shape.written {
            if self.mapper.ring == 0 || self.mapper.slots == 0 || self.mapper.table == 0 {
                return no(Why::MapperLayout);
            }
            if value < written_now {
                return no(Why::PositionWentBack);
            }
            if value - written_now > self.mapper.slots {
                return no(Why::PositionTooFar);
            }
            for count in written_now..value {
                self.copy_mapper_command(memory, count, refused);
            }
            Ok(value)
        } else {
            no(Why::UnknownRegister)
        }
    }

    /// Copies one command to the host's ring, and judges the copy.
    fn copy_mapper_command(&self, memory: &GuestMemory, count: u64, refused: &mut Vec<Refusal>) {
        let shape = &self.profile.mapper;
        let host = self.shadow.mapper_memory();
        let at = count % self.mapper.slots * shape.slot_bytes;
        let (guest_slot, host_slot) = (self.mapper.ring + at, self.shadow.mapper_ring() + at);
        let mut slot = vec![0u8; shape.slot_bytes as usize];
        // The ring was judged to lie in the guest's memory, so the read finds it.
        memory.read(guest_slot, &mut slot);
        host.write(host_slot, &slot);
        self.mirror_now().mapper_sent(count, self.mapper.slots, guest_slot, host_slot);
        // Only making a surface takes a description, and letting a mapping go names nothing but
        // its id. Any other number is made into the one the mapper answers "unsupported" to.
        let (number, id) = (word_of(&slot, NUMBER as usize), word_of(&slot, ID as usize));
        if number == shape.unmap {
            return;
        }
        if number != shape.map {
            host.set_word(host_slot + NUMBER, shape.unsupported);
            refused.push(Refusal { what: What::MapperCommand { count, id }, why: Why::UnknownCommand });
            return;
        }
        if !self.copy_surface(memory, id) {
            host.set_word(host_slot + NUMBER, shape.unsupported);
            refused.push(Refusal { what: What::MapperCommand { count, id }, why: Why::SurfaceDescription });
        }
    }

    /// Copies a mapping's description and page list to the host's pages, if they describe a
    /// plain picture. The host's description names the host's page list.
    fn copy_surface(&self, memory: &GuestMemory, id: u32) -> bool {
        let shape = &self.profile.mapper;
        if id == 0 || id > shape.most_id {
            return false;
        }
        // The table has a word for each page of descriptions, and a page has room for a few.
        let index = u64::from(id / shape.ids_per_page);
        let entry = memory.word(self.mapper.table + index * WORD_BYTES);
        let Some(page) = entry.and_then(|entry| named_page(memory, entry)) else {
            return false;
        };
        let place = u64::from(id % shape.ids_per_page) * shape.surface.bytes;
        let Some(mut surface) = plain_picture(memory, &shape.surface, page + place) else {
            return false;
        };
        let host = self.shadow.mapper_memory();
        let list = self.shadow.page_list(id);
        let mut bytes = vec![0u8; PAGE_BYTES as usize];
        for (index, entry) in surface.pages.iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&entry.to_le_bytes());
        }
        let at = shape.surface.page_list_at;
        surface.description[at..at + 4].copy_from_slice(&mapper_entry(list).to_le_bytes());
        host.write(list, &bytes)
            && host.write(self.shadow.mapper_descriptions(index) + place, &surface.description)
    }
}
