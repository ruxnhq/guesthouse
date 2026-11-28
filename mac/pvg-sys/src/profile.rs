// SPDX-License-Identifier: GPL-2.0-or-later
//! What one macOS build's device looks like to a guest, as far as the guard has to read it.
//!
//! The device's registers and commands are not a published interface. The numbers here were seen
//! on that build, by playing the guest against the framework with a program of our own and reading
//! what the device did and what the framework logged. Two exceptions: three of the delete
//! operations were not seen to delete (see `delete_operations`), and the `most_*` limits are this
//! host's own choices. The guard rests on these numbers, so a build with no profile is refused: on
//! a build where a number has moved, the guard would check the wrong word.
//!
//! A profile names only what the guard reads or what this host passes on. Whatever else the device
//! has, a guest's write to it is not passed.

use crate::Error;

/// The device counts memory in pages of this size, whatever the host's or the guest's own.
pub const PAGE_SHIFT: u32 = 14;
pub const PAGE_BYTES: u64 = 1 << PAGE_SHIFT;

/// One build's numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub build: &'static str,
    /// The version of the device's commands a guest states, and the one all of this was seen at.
    /// A guest that states another is not served.
    pub version: u32,
    pub registers: Registers,
    pub ring: RingShape,
    pub child: ChildShape,
    pub commands: Commands,
    pub display: DisplayShape,
    pub early: EarlyShape,
    pub task: TaskShape,
    pub mapper: MapperShape,
}

/// The device's registers, 32 bits each, by offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registers {
    /// 1 turns the first ring on, at position 0. 0 turns it off.
    pub ring_on: u64,
    /// How long the ring's memory is.
    pub ring_bytes: u64,
    /// How far the guest has written, in bytes that count on past the ring's end. A write is the
    /// kick.
    pub written: u64,
    /// How far the device has read, counted the same way.
    pub read: u64,
    /// Where in the ring's memory commands start. Before that lie the cells for stamps.
    pub commands_at: u64,
    /// The page that holds the tables the device and the guest share.
    pub root_page: u64,
    /// A write has the ring go on after a refused command.
    pub resume: u64,
    /// Reads 1 while a refused command holds any ring.
    pub fault: u64,
    /// Where the first ring's refused command lies, or all ones while it has none.
    pub fault_at: u64,
    /// The first page of the ring's memory.
    pub ring_page: u64,
    pub version: u64,
    /// A write of a display's port has the device look at the cursor's place in the state page.
    pub cursor_look: u64,
    /// A write of a child ring's id tells the device of new commands in that ring.
    pub wake: u64,
    /// A write of a child ring's id has that ring go on after a refused command.
    pub resume_child: u64,
    pub early: EarlyRegisters,
}

/// A child ring: a ring of a fixed length whose pages a page of the guest's lists, and whose
/// positions lie in a descriptor in the device's root page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildShape {
    /// Ids run from 1 to this.
    pub most_id: u32,
    /// In the root page: the first ring's descriptor, and how long each is.
    pub descriptors_at: u64,
    pub descriptor_bytes: u64,
    /// A descriptor's words: how far the guest has written; how far the device has read; where a
    /// refused command lies, or all ones; the index of the ring's stamp cell with, above it, the
    /// bit the device sets while the ring is defined; and the page that lists the ring's pages.
    pub written_at: u64,
    pub read_at: u64,
    pub fault_at: u64,
    pub stamp_at: u64,
    pub list_page_at: u64,
    pub defined: u32,
    pub no_fault: u32,
    /// A child ring's length, which is this many pages.
    pub bytes: u32,
    pub pages: u32,
}

/// The early framebuffer's bank: a picture from one block of the guest's memory, with no ring and
/// no command, for a guest's firmware and for a driver that has nothing else yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlyRegisters {
    /// 1 turns the bank on. Until then the others keep nothing. 0 turns it off.
    pub on: u64,
    /// Chooses one of the display's modes, whose size is then read from the next register.
    pub mode_index: u64,
    /// Where the block starts, as a page's number.
    pub page: u64,
    /// The block's length in bytes, whole pages.
    pub bytes: u64,
    /// The bits of a pixel.
    pub depth: u64,
    /// The picture's height above its width, 16 bits each. The picture shows after this write.
    pub mode: u64,
}

/// What an early framebuffer may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlyShape {
    /// The one depth this host passes: four bytes a pixel.
    pub depth: u32,
    pub pixel_bytes: u32,
    /// The widest and highest picture, as for a surface.
    pub most_side: u32,
}

/// The first ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingShape {
    /// A command starts with its number, its length with this header, and a stamp.
    pub header_bytes: u32,
    /// The stamp cells before the commands: one for each ring the device can have.
    pub least_commands_at: u32,
    /// More than a guest's driver asks for by far. A ring is one stretch of guest memory.
    pub most_bytes: u32,
    /// How many cells for stamps lie at the start of the ring's memory, a word each. The device
    /// writes them, and nothing else there.
    pub stamp_cells: u32,
}

/// A command this host passes on: its number, and the least payload the device takes for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub number: u32,
    pub least_payload: u32,
}

/// The commands a guest's driver sends, for its display and for the programs that draw. Every
/// other number is turned into `refused`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commands {
    /// Does nothing, with any payload.
    pub nothing: Command,
    /// Does nothing seen, and wants one word.
    pub nothing_with_a_word: Command,
    /// Hands a display its state page: the port, and the page's number, or 0 to take it back.
    pub state_page: Command,
    /// Acknowledges a display's new mode list: the port, and the list's sequence number.
    pub acknowledge: Command,
    /// A picture for the cursor, in the kernel task's memory.
    pub cursor_glyph: Command,
    pub cursor_show: Command,
    /// Shows a mapped surface: the port, the task, the mapping's id, and six words for a table of
    /// colours that no guest of ours sends.
    pub swap: Command,
    /// Deletes a task: its id.
    pub delete_task: Command,
    /// Defines a task: its id shifted left by one, its length in 64 bits, its root page.
    pub define_task: Command,
    /// Defines a child ring, whose descriptor the guest has written: the ring's id. And lets
    /// one go.
    pub define_ring: Command,
    pub delete_ring: Command,
    /// Has the host let go of what it mapped of a stretch of a task's memory: the task, an
    /// address in it and a length, 64 bits each.
    pub unmap_memory: Command,
    /// Deletes a resource: the task and the object's id.
    pub delete_resource: Command,
    /// Deletes an object that is no resource: the task, and one of the serializer's delete
    /// operations, which is its number, its length and the object's id.
    pub delete_object: Command,
    /// Has the host write resources' content back to the guest's memory: the task, a count, and
    /// a resource's id for each.
    pub synchronize: Command,
    /// Hands over command buffers, in its older form: the task, a count of resources and one of
    /// buffers, 24 bytes for each resource, and for each buffer its address in the task and its
    /// length, 64 bits each.
    pub run_buffers_2: Command,
    /// Asks what a compute pipeline reports of itself: the task, the pipeline's id, a limit for
    /// the keys, how many pairs the answer may hold, and where in the task it goes, 64 bits.
    pub compute_info: Command,
    /// Hands over command buffers: as the older form, with a resource's id and its flags for
    /// each resource.
    pub run_buffers: Command,
    /// Says where a task's object list and its placement list are: the task, the two addresses
    /// in the task, 64 bits each, and the two lengths.
    pub set_lists: Command,
    /// A number in the device's group of invalid commands. It refuses it with its ordinary fault
    /// and reads on, which is what the guard makes of a command it won't pass.
    pub refused: u32,
}

/// A display and its state page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayShape {
    /// In the state page: how many modes the list has, 16 bits. The device writes it, and reads it
    /// back whenever it is about to write a list.
    pub mode_count_at: u64,
    /// More in that word ends the host's process.
    pub most_modes: u32,
    /// In the state page: bits the device sets for what has happened and the guest clears.
    pub events_at: u64,
    /// The guest's choice of the events it wants an interrupt for.
    pub event_mask_at: u64,
    /// Where the cursor is, and whether it shows: the guest writes them, and the device looks
    /// when told to.
    pub cursor_place_at: u64,
    pub cursor_shown_at: u64,
    /// In the device's root page: a word for each display, which holds the number of its state
    /// page while it has one. The device writes it.
    pub state_pages_at: u64,
}

/// A task, which is an address space the guest lays out for the device to read through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskShape {
    /// The only task a display's commands may name. The device keeps it in the host's process.
    pub kernel_task: u32,
    /// The last id a task may have. Every task but the kernel task is a user task, which the
    /// framework keeps in a helper process of its own.
    pub most_id: u32,
    /// A task of this many bytes or more is one the device refuses.
    pub too_many_bytes: u64,
    /// In the device's root page: a word for each task, by its id, which holds the number of the
    /// task's root page while the task is defined. The device writes it.
    pub roots_at: u64,
    /// An entry of an object list, which has one for each object's id.
    pub object_entry_bytes: u64,
    /// A resource in the older command that hands over buffers: its id, four bytes of which the
    /// second says the buffer writes it, and sixteen bytes that are zero in all that was seen.
    pub resource_2_words: u32,
    pub resource_2_written: u32,
    /// A resource's flags in the newer command: this bit says the buffers write it.
    pub resource_written: u32,
    /// A pair of the answer about a compute pipeline.
    pub info_pair_bytes: u64,
    /// The serializer's delete operations this host passes, by number: for a buffer, a texture,
    /// a depth and stencil state, a sampler, a function, and the two kinds of pipeline. Each is
    /// its number, this length, and the object's id. The numbers are what Apple's serializer
    /// writes for each, read from its output. 1001, 1003, 1005 and 1007 were seen to delete.
    /// 1002 (a depth and stencil state) and 1006 (a compute pipeline) are what the guest's driver
    /// sends and were not seen to delete; they pass, as refusing them would refuse every such
    /// object a driver lets go of. 1000 is not sent: command 37 deletes buffers.
    pub delete_operations: [u32; 7],
    pub delete_operation_bytes: u32,
    /// In a task's root page: the page of its table, and how many levels the table has.
    pub table_page_at: u64,
    pub depth_at: u64,
    /// In a task's root page: the task's length, 64 bits, which the device writes when it has
    /// taken the task.
    pub length_at: u64,
    /// In a table's entry, above the page number: the host may write the page.
    pub writable: u32,
    /// In a task's root page: where the object list is and how long, 12 bytes, and the same for
    /// the placement list. The length the device writes lies between them.
    pub objects_at: u64,
    pub placements_at: u64,
    pub list_words: u64,
    /// The most pages of a table's lowest level that this host copies for a user task. A page
    /// names 64 MiB of the task, so this bounds a user task at 4 GiB.
    pub most_user_leaves: u64,
    /// The largest picture a cursor command may name, which bounds what is read of the table.
    pub most_glyph_bytes: u64,
}

/// The surface mapper's device. Its registers are 64 bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapperShape {
    pub register_bytes: u64,
    /// Where the mapper's ring is, as an address.
    pub ring: u64,
    /// The ring's length in commands.
    pub slots: u64,
    /// Where the table of the pages of surface descriptions is.
    pub table: u64,
    /// How many commands the guest has written. A write is the kick.
    pub written: u64,
    /// How many the device has done.
    pub done: u64,
    pub slot_bytes: u64,
    /// The ring's length has to be a multiple of this many commands, which is one page of them.
    /// Another length ends the host's process.
    pub slots_unit: u64,
    pub most_slots: u64,
    /// A command in the mapper's ring: its number, a mapping's id, and a word for how it went.
    /// This one has the mapper make a host surface of the pages the id's description names.
    pub map: u32,
    /// This one has the mapper let the id's surface go. Map and this are the only commands the
    /// guard passes.
    pub unmap: u32,
    /// A number the mapper answers with "unsupported" and nothing else, which is what the guard
    /// makes of a map command it won't pass.
    pub unsupported: u32,
    pub most_id: u32,
    /// The table names a page of descriptions for every this many ids.
    pub ids_per_page: u32,
    pub surface: SurfaceShape,
}

/// A surface's description in the guest's memory, as far as a plain picture needs it. Its other
/// bytes are zero in every description this host passes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceShape {
    pub bytes: u64,
    /// The page that lists the surface's pages, named the mapper's way.
    pub page_list_at: usize,
    pub format_at: usize,
    /// The surface's length in bytes.
    pub length_at: usize,
    /// The size of one element in the lowest byte, and the size in pixels above it.
    pub width_at: usize,
    pub height_at: usize,
    pub row_bytes_at: usize,
    pub pixel_bytes_at: usize,
    /// Four bytes a pixel, blue first: the one format this host passes.
    pub format: u32,
    pub pixel_bytes: u32,
    /// The widest and highest picture the host's GPU takes.
    pub most_side: u32,
}

/// macOS 27.2.1.
const BUILD_26B131: Profile = Profile {
    build: "26B131",
    version: 252,
    registers: Registers {
        ring_on: 0x1000,
        ring_bytes: 0x1004,
        written: 0x1008,
        read: 0x100C,
        commands_at: 0x1010,
        root_page: 0x101C,
        resume: 0x1024,
        fault: 0x102C,
        fault_at: 0x103C,
        ring_page: 0x1030,
        version: 0x1034,
        cursor_look: 0x1220,
        wake: 0x1020,
        resume_child: 0x1028,
        early: EarlyRegisters {
            on: 0x1200,
            mode_index: 0x1208,
            page: 0x1210,
            bytes: 0x1214,
            depth: 0x1218,
            mode: 0x121C,
        },
    },
    ring: RingShape { header_bytes: 12, least_commands_at: 0x100, most_bytes: 0x10_0000, stamp_cells: 64 },
    child: ChildShape {
        most_id: 63,
        descriptors_at: 0x400,
        descriptor_bytes: 20,
        written_at: 0,
        read_at: 4,
        fault_at: 8,
        stamp_at: 12,
        list_page_at: 16,
        defined: 1 << 16,
        no_fault: u32::MAX,
        bytes: 0x1_0000,
        pages: 4,
    },
    commands: Commands {
        nothing: Command { number: 30, least_payload: 0 },
        nothing_with_a_word: Command { number: 0, least_payload: 4 },
        state_page: Command { number: 1, least_payload: 8 },
        acknowledge: Command { number: 2, least_payload: 8 },
        cursor_glyph: Command { number: 4, least_payload: 44 },
        cursor_show: Command { number: 5, least_payload: 8 },
        swap: Command { number: 8, least_payload: 40 },
        delete_task: Command { number: 32, least_payload: 4 },
        define_task: Command { number: 56, least_payload: 16 },
        define_ring: Command { number: 48, least_payload: 4 },
        delete_ring: Command { number: 49, least_payload: 4 },
        unmap_memory: Command { number: 34, least_payload: 20 },
        delete_resource: Command { number: 37, least_payload: 8 },
        delete_object: Command { number: 40, least_payload: 16 },
        synchronize: Command { number: 53, least_payload: 8 },
        run_buffers_2: Command { number: 55, least_payload: 12 },
        compute_info: Command { number: 59, least_payload: 24 },
        run_buffers: Command { number: 67, least_payload: 12 },
        set_lists: Command { number: 68, least_payload: 28 },
        refused: 13,
    },
    display: DisplayShape {
        mode_count_at: 0x208,
        most_modes: 128,
        events_at: 0x100,
        event_mask_at: 0x104,
        cursor_place_at: 0xE00,
        cursor_shown_at: 0xE04,
        state_pages_at: 0x900,
    },
    early: EarlyShape { depth: 32, pixel_bytes: 4, most_side: 16384 },
    task: TaskShape {
        kernel_task: 0,
        most_id: 255,
        too_many_bytes: 64 << 30,
        roots_at: 0x0,
        object_entry_bytes: 12,
        resource_2_words: 6,
        resource_2_written: 1 << 8,
        resource_written: 2,
        info_pair_bytes: 8,
        delete_operations: [1000, 1001, 1002, 1003, 1005, 1006, 1007],
        delete_operation_bytes: 12,
        table_page_at: 0x0,
        depth_at: 0x4,
        length_at: 0x14,
        writable: 1 << 31,
        objects_at: 0x08,
        placements_at: 0x1C,
        list_words: 3,
        most_user_leaves: 64,
        most_glyph_bytes: 256 * 256 * 4,
    },
    mapper: MapperShape {
        register_bytes: 0x1_0000,
        ring: 0x1000,
        slots: 0x1008,
        table: 0x1010,
        written: 0x1018,
        done: 0x1020,
        slot_bytes: 16,
        slots_unit: 1024,
        most_slots: 64 * 1024,
        map: 1,
        unmap: 2,
        unsupported: 0,
        most_id: 8191,
        ids_per_page: 32,
        surface: SurfaceShape {
            bytes: 0x200,
            page_list_at: 0x0,
            format_at: 0x4,
            length_at: 0x10,
            width_at: 0x14,
            height_at: 0x18,
            row_bytes_at: 0x1C,
            pixel_bytes_at: 0x20,
            format: u32::from_be_bytes(*b"BGRA"),
            pixel_bytes: 4,
            most_side: 16384,
        },
    },
};

const PROFILES: [Profile; 1] = [BUILD_26B131];

impl Profile {
    /// The profile for a build, if that build's device has been looked at.
    pub fn for_build(build: &str) -> Result<&'static Profile, Error> {
        PROFILES
            .iter()
            .find(|profile| profile.build == build)
            .ok_or_else(|| Error::UnknownBuild(build.to_owned()))
    }
}
