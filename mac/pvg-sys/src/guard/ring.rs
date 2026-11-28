// SPDX-License-Identifier: GPL-2.0-or-later
//! The first ring's commands, read before the device reads them.

use super::{
    Child, Effect, Glyph, Guard, KernelTask, PORTS, Refusal, Step, UserTask, What, Why, page_address,
    page_number,
};
use crate::memory::GuestMemory;
use crate::profile::{Command, PAGE_BYTES};

const WORD_BYTES: u32 = 4;
/// Where a command's number, its length and its payload lie, in words from its start.
const NUMBER: u32 = 0;
const LENGTH: u32 = 1;
const PAYLOAD: u32 = 3;
/// A table of one level names this many pages, and the guard takes no deeper table.
const MOST_TASK_PAGES: u64 = 4096;
const GLYPH_PIXEL_BYTES: u64 = 4;
const ONE_LEVEL: u32 = 1;
const TWO_LEVELS: u32 = 2;
/// A page of a table holds this many entries, so a page of its lowest level names this much.
const TABLE_ENTRIES: u64 = PAGE_BYTES / WORD_BYTES as u64;
const LEAF_BYTES: u64 = TABLE_ENTRIES * PAGE_BYTES;
/// In a command that hands over buffers: the task and the two counts, a resource in the newer
/// form, which is its id and its flags, and a buffer, which is its address and its length.
const COUNTS_WORDS: u32 = 3;
const RESOURCE_WORDS: u32 = 2;
const BUFFER_WORDS: u32 = 4;
/// The six words of a swap behind the mapping's id.
const COLOUR_TABLE_WORDS: std::ops::Range<u32> = 4..10;

/// What the guard makes of one command.
enum Judged {
    /// Pass it with its neighbours.
    Plain,
    /// Pass it alone, and ask the device how it went.
    Alone(Effect),
    No(Why),
}

/// One command in the host's copy of the ring, for reading its words.
struct Words<'a> {
    /// The host's pages, where the copy is.
    ring: &'a GuestMemory,
    /// The guest's memory, for what a command names there.
    guest: &'a GuestMemory,
    /// Where commands start in the copy, and how many bytes they wrap in.
    base: u64,
    room: u32,
    position: u32,
}

impl Words<'_> {
    /// A command may lie across the ring's end, so each word wraps by itself. Positions are
    /// counters that wrap at 2^32, and the place is the counter modulo the room.
    fn address(&self, word: u32) -> u64 {
        let place = self.position.wrapping_add(word * WORD_BYTES) % self.room;
        self.base + u64::from(place)
    }

    fn word(&self, word: u32) -> Option<u32> {
        self.ring.word(self.address(word))
    }

    /// Puts another word into the copy, for the device to read in place of the guest's.
    fn set_payload(&self, word: u32, value: u32) {
        self.ring.set_word(self.address(PAYLOAD + word), value);
    }

    fn payload(&self, word: u32) -> Option<u32> {
        self.word(PAYLOAD + word)
    }

    fn payload64(&self, word: u32) -> Option<u64> {
        Some(u64::from(self.payload(word + 1)?) << 32 | u64::from(self.payload(word)?))
    }
}

impl Guard {
    /// Reads on from where the device was last told the guest had written, towards `target`, the
    /// position the guest wrote. Returns the next stretch to tell the device of. The caller writes
    /// `upto` to the device, settles the step's effect if it has one, and asks again until `upto`
    /// is `target` or the step says it stopped.
    ///
    /// The new commands are first copied to the host's own pages, where the device reads them,
    /// and it is the copy that is judged. `faulted` is whether a refused command holds the first
    /// ring now.
    /// Commands the guard won't pass are made into refused ones in the copy and noted in
    /// `refused`. The guest's ring is not written.
    pub fn next_step(
        &mut self,
        memory: &GuestMemory,
        target: u32,
        faulted: bool,
        refused: &mut Vec<Refusal>,
    ) -> Step {
        let header = self.profile.ring.header_bytes;
        let start = self.ring.passed;
        let ours = self.shadow.memory();
        let mut words = Words {
            ring: ours,
            guest: memory,
            base: self.shadow.ring() + u64::from(self.ring.commands_at),
            room: self.ring.room(),
            position: start,
        };
        // The copy, of all that is new, in the one or two pieces the ring's end makes of it.
        // What is judged below is what the device will read, whatever the guest writes next.
        let theirs = page_address(self.ring.page) + u64::from(self.ring.commands_at);
        let room = u64::from(words.room);
        let (from, bytes) = (u64::from(start) % room, u64::from(target.wrapping_sub(start)).min(room));
        let first = bytes.min(room - from);
        let copied = memory.copy_to(theirs + from, first, ours, words.base + from)
            && memory.copy_to(theirs, bytes - first, ours, words.base);
        if !copied {
            let what = What::Command { position: start, number: 0 };
            return Step { upto: start, effect: None, stopped: Some(Refusal { what, why: Why::RingLayout }) };
        }
        let step = loop {
            let position = words.position;
            if position == target {
                break Step { upto: target, effect: None, stopped: None };
            }
            let left = target.wrapping_sub(position);
            let read = (words.word(NUMBER), words.word(LENGTH));
            let (Some(number), Some(length)) = read else {
                let what = What::Command { position, number: 0 };
                break Step {
                    upto: position,
                    effect: None,
                    stopped: Some(Refusal { what, why: Why::RingLayout }),
                };
            };
            let what = What::Command { position, number };
            if left < header || length < header || !length.is_multiple_of(WORD_BYTES) || length > left {
                let stopped = Some(Refusal { what, why: Why::BrokenCommand });
                break Step { upto: position, effect: None, stopped };
            }

            let mut refuse = |why| {
                ours.set_word(words.address(NUMBER), self.profile.commands.refused);
                refused.push(Refusal { what, why });
            };
            match self.judge(&words, number, length - header) {
                Judged::Plain => {}
                Judged::No(why) => refuse(why),
                // What came before goes to the device first, so that this one is judged, and then
                // settled, against what the device has become.
                Judged::Alone(_) if position != start => {
                    break Step { upto: position, effect: None, stopped: None };
                }
                Judged::Alone(effect) if faulted && effect.told_by_the_fault() => {
                    refuse(Why::BehindARefusedCommand);
                }
                Judged::Alone(effect) => {
                    break Step { upto: position.wrapping_add(length), effect: Some(effect), stopped: None };
                }
            }
            words.position = position.wrapping_add(length);
        };
        self.ring.passed = step.upto;
        step
    }

    /// The numbers of the first ring's commands from `position` on, up to `until` and at most
    /// `most` of them, from the host's copy that the device reads: for a line when the device has
    /// not read the ring in time.
    pub fn first_ring_numbers(&self, position: u32, until: u32, most: usize) -> Vec<u32> {
        let room = self.ring.room();
        if room == 0 {
            return Vec::new();
        }
        let ours = self.shadow.memory();
        let mut words = Words {
            ring: ours,
            guest: ours,
            base: self.shadow.ring() + u64::from(self.ring.commands_at),
            room,
            position,
        };
        let mut numbers = Vec::new();
        while words.position != until && numbers.len() < most {
            let (Some(number), Some(length)) = (words.word(NUMBER), words.word(LENGTH)) else {
                break;
            };
            numbers.push(number);
            if length == 0 || !length.is_multiple_of(WORD_BYTES) {
                break;
            }
            words.position = words.position.wrapping_add(length);
        }
        numbers
    }

    /// Whether a command that was passed alone failed, once the device has read it, or `None`
    /// while the device has not said yet. `faulted_before` and `faulted` are whether a refused
    /// command held the first ring before the command was passed, and holds it now.
    ///
    /// The caller asks again for a while, and takes no answer in that time for a failure.
    pub fn failed(&self, effect: &Effect, faulted_before: bool, faulted: bool) -> Option<bool> {
        // The device's own word for a task in its root page: the task's root page while the task
        // is defined, and 0 otherwise. The root page the device has is the host's.
        let root_word = |id: u32| {
            let roots = self.shadow.root() + self.profile.task.roots_at;
            self.shadow.memory().word(roots + u64::from(id) * u64::from(WORD_BYTES))
        };
        // And for a child ring, in its descriptor there: a bit that is set while the ring is
        // defined.
        let ring_is_defined = |id: u32| {
            let word = self.shadow.memory().word(self.child_descriptor(id) + self.profile.child.stamp_at);
            word.map(|word| word & self.profile.child.defined != 0)
        };
        let done = match *effect {
            // The device was given a root page of the host's for the task.
            Effect::DefineUserTask { id, .. } => {
                root_word(id) == Some(page_number(self.shadow.task_root(id)))
            }
            Effect::DeleteUserTask { id } => root_word(id) == Some(0),
            Effect::DefineRing { id, .. } => ring_is_defined(id) == Some(true),
            Effect::DeleteRing { id } => ring_is_defined(id) == Some(false),
            _ => return Some(faulted),
        };
        if done {
            Some(false)
        } else if faulted && !faulted_before {
            Some(true)
        } else {
            None
        }
    }

    /// Takes in how a command that was passed alone went: `failed` is whether the device never
    /// came to it, or refused it.
    ///
    /// The device notes a task and a display's page in its root page, which is the host's. What
    /// a guest would find in its own root page is written there from here, with the guest's own
    /// page numbers.
    pub fn settle(&mut self, memory: &GuestMemory, effect: Effect, failed: bool) {
        let shape = &self.profile.task;
        let root = page_address(self.ring.root_page);
        let note_task = |id: u32, root_page: u32| {
            memory.set_word(root + shape.roots_at + u64::from(id) * u64::from(WORD_BYTES), root_page);
        };
        let note_state_page = |port: u32, page: u32| {
            let at = self.profile.display.state_pages_at + u64::from(port) * u64::from(WORD_BYTES);
            memory.set_word(root + at, page);
        };
        match effect {
            Effect::DefineRing { .. } | Effect::DeleteRing { .. } if failed => {}
            Effect::DefineRing { id, pages, .. } => {
                let theirs = root + self.child_descriptor(id) - self.shadow.root();
                self.mirror_now().set_child(id, Some(theirs));
                self.children.insert(id, Child { pages, passed: 0 });
            }
            Effect::DeleteRing { id } => {
                self.mirror_now().set_child(id, None);
                self.children.remove(&id);
            }
            Effect::DefineUserTask { .. } | Effect::DeleteUserTask { .. } if failed => {}
            Effect::DefineUserTask { id, length, root_page, table, depth } => {
                // The device wrote the task's length into the root page it was given.
                let length_at = page_address(root_page) + shape.length_at;
                memory.set_word(length_at, length as u32);
                memory.set_word(length_at + u64::from(WORD_BYTES), (length >> 32) as u32);
                note_task(id, root_page);
                self.user_tasks.insert(id, UserTask { length, table, depth });
            }
            Effect::DeleteUserTask { id } => {
                note_task(id, 0);
                self.user_tasks.remove(&id);
            }
            // The guard passes no hand-over it knows a reason against. One that fails all the same
            // may have failed half done, and what state the display is in then is not known.
            Effect::HandOver { .. } | Effect::TakeBack { .. } if failed => self.spoiled = true,
            Effect::HandOver { port, page } => {
                note_state_page(port, page);
                self.mirror_now().set_state_page(port, Some(page_address(page)));
                self.state_pages[port as usize] = page;
            }
            Effect::TakeBack { port } => {
                note_state_page(port, 0);
                self.mirror_now().set_state_page(port, None);
                self.state_pages[port as usize] = 0;
            }
            Effect::DefineTask(_) | Effect::DeleteTask if failed => {}
            Effect::DefineTask(task) => {
                // The device wrote the task's length into the root page it was given.
                let length_at = page_address(task.root_page) + shape.length_at;
                memory.set_word(length_at, task.length as u32);
                memory.set_word(length_at + u64::from(WORD_BYTES), (task.length >> 32) as u32);
                note_task(shape.kernel_task, task.root_page);
                self.kernel_task = Some(task);
            }
            Effect::DeleteTask => {
                note_task(shape.kernel_task, 0);
                self.kernel_task = None;
            }
        }
    }

    fn judge(&self, words: &Words<'_>, number: u32, payload_bytes: u32) -> Judged {
        let commands = &self.profile.commands;
        let is = |command: Command| command.number == number;
        let Some(command) = [
            commands.nothing,
            commands.nothing_with_a_word,
            commands.state_page,
            commands.acknowledge,
            commands.cursor_glyph,
            commands.cursor_show,
            commands.swap,
            commands.delete_task,
            commands.define_task,
            commands.define_ring,
            commands.delete_ring,
            commands.unmap_memory,
            commands.delete_resource,
            commands.delete_object,
            commands.synchronize,
            commands.run_buffers_2,
            commands.compute_info,
            commands.run_buffers,
            commands.set_lists,
        ]
        .into_iter()
        .find(|&command| is(command)) else {
            return Judged::No(Why::UnknownCommand);
        };
        if payload_bytes < command.least_payload {
            return Judged::No(Why::ShortCommand);
        }

        let judged = if is(commands.state_page) {
            self.judge_state_page(words)
        } else if is(commands.acknowledge) {
            self.judge_acknowledge(words)
        } else if is(commands.cursor_glyph) {
            self.judge_cursor_glyph(words)
        } else if is(commands.cursor_show) {
            self.port(words).map(|_| Judged::Plain)
        } else if is(commands.swap) {
            self.judge_swap(words)
        } else if is(commands.delete_task) {
            self.judge_delete_task(words)
        } else if is(commands.define_task) {
            self.judge_define_task(words)
        } else if is(commands.define_ring) {
            self.judge_define_ring(words)
        } else if is(commands.delete_ring) {
            let id = words.payload(0).ok_or(Why::RingLayout);
            id.and_then(|id| match self.children.contains_key(&id) {
                true => Ok(Judged::Alone(Effect::DeleteRing { id })),
                false => Err(Why::NoSuchRing),
            })
        } else if is(commands.unmap_memory) {
            self.judge_unmap_memory(words, payload_bytes)
        } else if is(commands.delete_resource) || is(commands.delete_object) {
            self.judge_delete(words, payload_bytes, command)
        } else if is(commands.synchronize) {
            self.judge_synchronize(words, payload_bytes)
        } else if is(commands.run_buffers_2) {
            self.judge_run_buffers(words, payload_bytes, self.profile.task.resource_2_words)
        } else if is(commands.run_buffers) {
            self.judge_run_buffers(words, payload_bytes, RESOURCE_WORDS)
        } else if is(commands.compute_info) {
            self.judge_compute_info(words, payload_bytes)
        } else if is(commands.set_lists) {
            self.judge_set_lists(words, payload_bytes)
        } else {
            Ok(Judged::Plain)
        };
        judged.unwrap_or_else(Judged::No)
    }

    /// The display a command names in its first word, if this host has it.
    fn port(&self, words: &Words<'_>) -> Result<u32, Why> {
        words.payload(0).filter(|&port| port < PORTS).ok_or(Why::NoSuchDisplay)
    }

    fn kernel_task_named(&self, id: Option<u32>) -> Result<(), Why> {
        if id == Some(self.profile.task.kernel_task) { Ok(()) } else { Err(Why::NotTheKernelTask) }
    }

    fn judge_state_page(&self, words: &Words<'_>) -> Result<Judged, Why> {
        if self.spoiled {
            return Err(Why::Spoiled);
        }
        let port = self.port(words)?;
        let page = words.payload(1).ok_or(Why::RingLayout)?;
        let held = self.state_pages[port as usize];
        if page == 0 {
            return if held == 0 {
                Err(Why::DisplayNotInUse)
            } else {
                Ok(Judged::Alone(Effect::TakeBack { port }))
            };
        }
        if held != 0 {
            return Err(Why::DisplayInUse);
        }
        // The device takes what a page holds for the display's state to go on from, and a page of
        // anything else can fail the hand-over half way. Empty is what a driver hands over.
        match words.guest.is_zero(page_address(page), PAGE_BYTES) {
            None => Err(Why::NotGuestMemory),
            Some(false) => Err(Why::StatePageNotEmpty),
            Some(true) => {
                // The device gets a page of the host's, empty, and the guest's is where its
                // words are copied to.
                let ours = self.shadow.state_page(port);
                self.shadow.memory().zero(ours, PAGE_BYTES);
                words.set_payload(1, page_number(ours));
                Ok(Judged::Alone(Effect::HandOver { port, page }))
            }
        }
    }

    fn judge_acknowledge(&self, words: &Words<'_>) -> Result<Judged, Why> {
        let port = self.port(words)?;
        // With no page the device refuses this by itself.
        if self.state_pages[port as usize] != 0 && !self.mode_count_fits(port) {
            return Err(Why::TooManyModes);
        }
        Ok(Judged::Plain)
    }

    fn judge_cursor_glyph(&self, words: &Words<'_>) -> Result<Judged, Why> {
        self.port(words)?;
        self.kernel_task_named(words.payload(1))?;
        let task = self.kernel_task.as_ref().ok_or(Why::NoTask)?;
        let address = words.payload64(2).ok_or(Why::RingLayout)?;
        let mapped_length = words.payload64(4).ok_or(Why::RingLayout)?;
        if !task.maps(address, mapped_length) {
            return Err(Why::PictureNotMapped);
        }
        *self.glyph.borrow_mut() = self.read_glyph(words, address, mapped_length);
        Ok(Judged::Plain)
    }

    /// The picture a cursor command names, read through the host's copy of the kernel task's
    /// table: a row's length in bytes, then the width and the height, and the place that points.
    /// `None` for a picture of no size, of more than the largest, or that its memory doesn't hold.
    fn read_glyph(&self, words: &Words<'_>, address: u64, mapped_length: u64) -> Option<Glyph> {
        let shape = &self.profile.task;
        let row_bytes = words.payload64(6)?;
        let (size, hot) = (words.payload(8)?, words.payload(9)?);
        let (width, height) = (size & 0xFFFF, size >> 16);
        let row = u64::from(width) * GLYPH_PIXEL_BYTES;
        let fits = width != 0
            && height != 0
            && row * u64::from(height) <= shape.most_glyph_bytes
            && row_bytes >= row
            && row_bytes.checked_mul(u64::from(height) - 1)?.checked_add(row)? <= mapped_length;
        if !fits {
            return None;
        }
        let mut pixels = vec![0u8; (row * u64::from(height)) as usize];
        for (line, out) in pixels.chunks_exact_mut(row as usize).enumerate() {
            // A row may lie across two of the task's pages.
            let mut at = address + line as u64 * row_bytes;
            let mut done = 0;
            while done < out.len() {
                let within = at % PAGE_BYTES;
                let piece = ((PAGE_BYTES - within) as usize).min(out.len() - done);
                let entry_at = self.shadow.kernel_table() + at / PAGE_BYTES * u64::from(WORD_BYTES);
                let page = self.shadow.memory().word(entry_at)? & !shape.writable;
                if !words.guest.read(page_address(page) + within, &mut out[done..done + piece]) {
                    return None;
                }
                done += piece;
                at += piece as u64;
            }
        }
        Some(Glyph { width, height, hot_x: hot & 0xFFFF, hot_y: hot >> 16, pixels })
    }

    fn judge_swap(&self, words: &Words<'_>) -> Result<Judged, Why> {
        self.port(words)?;
        self.kernel_task_named(words.payload(1))?;
        if COLOUR_TABLE_WORDS.into_iter().all(|word| words.payload(word) == Some(0)) {
            Ok(Judged::Plain)
        } else {
            Err(Why::ColourTable)
        }
    }

    fn judge_delete_task(&self, words: &Words<'_>) -> Result<Judged, Why> {
        let id = words.payload(0).ok_or(Why::RingLayout)?;
        if id != self.profile.task.kernel_task {
            self.user_task(words.guest, Some(id))?;
            return Ok(Judged::Alone(Effect::DeleteUserTask { id }));
        }
        if self.kernel_task.is_none() {
            return Err(Why::NoTask);
        }
        Ok(Judged::Alone(Effect::DeleteTask))
    }

    fn judge_define_task(&self, words: &Words<'_>) -> Result<Judged, Why> {
        let shape = &self.profile.task;
        let memory = words.guest;
        // The id is shifted left by one. What the bit below it does is not known, so it stays 0.
        let id = words.payload(0).filter(|id| id & 1 == 0).map(|id| id >> 1).ok_or(Why::TaskId)?;
        if id != shape.kernel_task {
            return self.judge_define_user_task(words, id);
        }
        if self.kernel_task.is_some() {
            return Err(Why::TaskExists);
        }
        let length = words.payload64(1).ok_or(Why::RingLayout)?;
        let pages = length.div_ceil(PAGE_BYTES);
        if length == 0 || pages > MOST_TASK_PAGES {
            return Err(Why::TaskLayout);
        }
        let root_page = words.payload(3).ok_or(Why::RingLayout)?;
        let root = page_address(root_page);
        let (Some(table_page), Some(depth)) =
            (memory.word(root + shape.table_page_at), memory.word(root + shape.depth_at))
        else {
            return Err(Why::NotGuestMemory);
        };
        if depth != ONE_LEVEL {
            return Err(Why::TaskLayout);
        }
        // An entry is a page's number, with one bit above for whether the host may write the page.
        // A table outside the guest's memory maps nothing, and neither does such an entry. The
        // device gets a root page and a table of the host's: each entry read once, and copied if
        // it names a page of the guest's.
        let table = page_address(table_page);
        let ours = self.shadow.memory();
        ours.zero(self.shadow.kernel_root(), PAGE_BYTES);
        ours.zero(self.shadow.kernel_table(), PAGE_BYTES);
        let mapped = (0..pages)
            .map(|index| {
                let at = index * u64::from(WORD_BYTES);
                let entry = memory.word(table + at).filter(|entry| {
                    let page = entry & !shape.writable;
                    page != 0 && memory.holds(page_address(page), PAGE_BYTES)
                });
                if let Some(entry) = entry {
                    ours.set_word(self.shadow.kernel_table() + at, entry);
                }
                entry.is_some()
            })
            .collect();
        ours.set_word(
            self.shadow.kernel_root() + shape.table_page_at,
            page_number(self.shadow.kernel_table()),
        );
        ours.set_word(self.shadow.kernel_root() + shape.depth_at, ONE_LEVEL);
        words.set_payload(3, page_number(self.shadow.kernel_root()));
        Ok(Judged::Alone(Effect::DefineTask(KernelTask { length, root_page, mapped })))
    }

    /// A task for a program's work. The helper process reads through the task's table whenever
    /// it needs the task's memory, and it is shown the host's own pages beside the guest's. So
    /// the device gets a root page and a table of the host's for a user task too, in which every
    /// entry names a page of the guest's or nothing. What the task's memory holds, its object
    /// list and its command buffers, is the helper's to judge.
    fn judge_define_user_task(&self, words: &Words<'_>, id: u32) -> Result<Judged, Why> {
        let shape = &self.profile.task;
        let memory = words.guest;
        if id > shape.most_id {
            return Err(Why::TaskId);
        }
        if self.user_tasks.contains_key(&id) {
            return Err(Why::TaskExists);
        }
        let length = words.payload64(1).ok_or(Why::RingLayout)?;
        let root_page = words.payload(3).ok_or(Why::RingLayout)?;
        let root = page_address(root_page);
        let (Some(table_page), Some(depth)) =
            (memory.word(root + shape.table_page_at), memory.word(root + shape.depth_at))
        else {
            return Err(Why::NotGuestMemory);
        };
        // One level names 64 MiB, and of a second level this host copies a few pages.
        let most = match depth {
            ONE_LEVEL => LEAF_BYTES,
            TWO_LEVELS => shape.most_user_leaves * LEAF_BYTES,
            _ => return Err(Why::TaskLayout),
        };
        if length == 0 || length > most || length >= shape.too_many_bytes {
            return Err(Why::TaskLayout);
        }

        let ours = self.shadow.memory();
        let our_root = self.shadow.task_root(id);
        self.shadow.clear_task(id);
        ours.set_word(our_root + shape.table_page_at, page_number(self.shadow.task_top(id)));
        ours.set_word(our_root + shape.depth_at, depth);
        // Where the lists are is said in addresses of the task, which the table judges.
        for at in [shape.objects_at, shape.placements_at] {
            for word in 0..shape.list_words {
                let at = at + word * u64::from(WORD_BYTES);
                ours.set_word(our_root + at, memory.word(root + at).unwrap_or(0));
            }
        }
        let task = UserTask { length, table: page_address(table_page), depth };
        self.copy_task_table(memory, id, &task);
        words.set_payload(3, page_number(our_root));
        Ok(Judged::Alone(Effect::DefineUserTask { id, length, root_page, table: task.table, depth }))
    }

    /// Copies a user task's table to the host's pages for it, as the guest has it now. An entry
    /// is copied if it names a page of the guest's, and is nothing otherwise.
    fn copy_task_table(&self, memory: &GuestMemory, id: u32, task: &UserTask) {
        let shape = &self.profile.task;
        let ours = self.shadow.memory();
        let pages = task.length.div_ceil(PAGE_BYTES);
        if task.depth == ONE_LEVEL {
            self.copy_table_page(memory, task.table, self.shadow.task_top(id), pages);
            return;
        }
        for leaf in 0..task.length.div_ceil(LEAF_BYTES) {
            let at = leaf * u64::from(WORD_BYTES);
            let entry = memory.word(task.table + at).unwrap_or(0);
            let theirs = page_address(entry & !shape.writable);
            let our_leaf = self.shadow.task_leaf(id, leaf);
            let named = entry & !shape.writable != 0 && memory.holds(theirs, PAGE_BYTES);
            let top = if named {
                self.copy_table_page(
                    memory,
                    theirs,
                    our_leaf,
                    (pages - leaf * TABLE_ENTRIES).min(TABLE_ENTRIES),
                );
                page_number(our_leaf) | entry & shape.writable
            } else {
                0
            };
            ours.set_word(self.shadow.task_top(id) + at, top);
        }
    }

    /// One page of a table's lowest level, or the first entries of it. Most entries are as they
    /// were at the last command, so only what differs is judged and written.
    fn copy_table_page(&self, memory: &GuestMemory, theirs: u64, ours: u64, entries: u64) {
        let shape = &self.profile.task;
        let bytes = (entries * u64::from(WORD_BYTES)) as usize;
        let (mut wanted, mut held) = (vec![0u8; bytes], vec![0u8; bytes]);
        if !memory.read(theirs, &mut wanted) {
            wanted.fill(0);
        }
        self.shadow.memory().read(ours, &mut held);
        for (index, (wanted, held)) in
            wanted.as_chunks::<4>().0.iter().zip(held.as_chunks::<4>().0).enumerate()
        {
            if wanted == held {
                continue;
            }
            let entry = u32::from_le_bytes(*wanted);
            let page = entry & !shape.writable;
            let entry = if page != 0 && memory.holds(page_address(page), PAGE_BYTES) { entry } else { 0 };
            self.shadow.memory().set_word(ours + index as u64 * u64::from(WORD_BYTES), entry);
        }
    }

    /// The length of the user task a command names. The task's table is copied anew first: the
    /// command has the helper process read through it.
    fn user_task(&self, memory: &GuestMemory, id: Option<u32>) -> Result<u64, Why> {
        let id = id.ok_or(Why::RingLayout)?;
        if id == self.profile.task.kernel_task || id > self.profile.task.most_id {
            return Err(Why::TaskId);
        }
        let task = self.user_tasks.get(&id).ok_or(Why::NoSuchTask)?;
        self.copy_task_table(memory, id, task);
        Ok(task.length)
    }

    /// An object's id has to be one whose entry of the object list can lie in the task.
    fn object_fits(&self, id: u32, task_bytes: u64) -> Result<(), Why> {
        let entry = self.profile.task.object_entry_bytes;
        if (u64::from(id) + 1) * entry <= task_bytes { Ok(()) } else { Err(Why::OutsideTheTask) }
    }

    fn stretch_fits(address: u64, length: u64, task_bytes: u64) -> Result<(), Why> {
        match address.checked_add(length) {
            Some(end) if end <= task_bytes => Ok(()),
            _ => Err(Why::OutsideTheTask),
        }
    }

    /// Command buffers handed over, in either form: the task, how many resources and how many
    /// buffers, the resources, the buffers.
    fn judge_run_buffers(
        &self,
        words: &Words<'_>,
        payload_bytes: u32,
        resource_words: u32,
    ) -> Result<Judged, Why> {
        let shape = &self.profile.task;
        let task_bytes = self.user_task(words.guest, words.payload(0))?;
        let (resources, buffers) = match (words.payload(1), words.payload(2)) {
            (Some(resources), Some(buffers)) => (resources, buffers),
            _ => return Err(Why::RingLayout),
        };
        let needed = u64::from(COUNTS_WORDS)
            + u64::from(resources) * u64::from(resource_words)
            + u64::from(buffers) * u64::from(BUFFER_WORDS);
        if needed * u64::from(WORD_BYTES) != u64::from(payload_bytes) {
            return Err(Why::CommandLayout);
        }
        // The counts fit the command, so they are small, and so is every index below.
        for resource in 0..resources {
            let at = COUNTS_WORDS + resource * resource_words;
            let id = words.payload(at).ok_or(Why::RingLayout)?;
            self.object_fits(id, task_bytes)?;
            let flags = words.payload(at + 1).ok_or(Why::RingLayout)?;
            let written = if resource_words == RESOURCE_WORDS {
                shape.resource_written
            } else {
                shape.resource_2_written
            };
            let rest_is_zero = (2..resource_words).all(|word| words.payload(at + word) == Some(0));
            if flags & !written != 0 || !rest_is_zero {
                return Err(Why::ResourceFlags);
            }
        }
        let buffers_at = COUNTS_WORDS + resources * resource_words;
        for buffer in 0..buffers {
            let at = buffers_at + buffer * BUFFER_WORDS;
            let address = words.payload64(at).ok_or(Why::RingLayout)?;
            let length = words.payload64(at + 2).ok_or(Why::RingLayout)?;
            if length == 0 {
                return Err(Why::OutsideTheTask);
            }
            Self::stretch_fits(address, length, task_bytes)?;
        }
        Ok(Judged::Plain)
    }

    fn judge_unmap_memory(&self, words: &Words<'_>, payload_bytes: u32) -> Result<Judged, Why> {
        let task_bytes = self.user_task(words.guest, words.payload(0))?;
        if payload_bytes != self.profile.commands.unmap_memory.least_payload {
            return Err(Why::CommandLayout);
        }
        let address = words.payload64(1).ok_or(Why::RingLayout)?;
        let length = words.payload64(3).ok_or(Why::RingLayout)?;
        Self::stretch_fits(address, length, task_bytes)?;
        Ok(Judged::Plain)
    }

    /// An object deleted, in either form: a resource by its id, and any other object by the
    /// serializer's operation for deleting one of its sort.
    fn judge_delete(&self, words: &Words<'_>, payload_bytes: u32, command: Command) -> Result<Judged, Why> {
        let shape = &self.profile.task;
        let task_bytes = self.user_task(words.guest, words.payload(0))?;
        if payload_bytes != command.least_payload {
            return Err(Why::CommandLayout);
        }
        let id_at = if command == self.profile.commands.delete_object {
            let operation = words.payload(1).ok_or(Why::RingLayout)?;
            if !shape.delete_operations.contains(&operation)
                || words.payload(2) != Some(shape.delete_operation_bytes)
            {
                return Err(Why::CommandLayout);
            }
            3
        } else {
            1
        };
        self.object_fits(words.payload(id_at).ok_or(Why::RingLayout)?, task_bytes)?;
        Ok(Judged::Plain)
    }

    fn judge_synchronize(&self, words: &Words<'_>, payload_bytes: u32) -> Result<Judged, Why> {
        let task_bytes = self.user_task(words.guest, words.payload(0))?;
        let count = words.payload(1).ok_or(Why::RingLayout)?;
        if (u64::from(count) + 2) * u64::from(WORD_BYTES) != u64::from(payload_bytes) {
            return Err(Why::CommandLayout);
        }
        for resource in 0..count {
            self.object_fits(words.payload(2 + resource).ok_or(Why::RingLayout)?, task_bytes)?;
        }
        Ok(Judged::Plain)
    }

    /// The helper writes the answer into the task's memory, as pairs of a key and a value.
    fn judge_compute_info(&self, words: &Words<'_>, payload_bytes: u32) -> Result<Judged, Why> {
        let task_bytes = self.user_task(words.guest, words.payload(0))?;
        if payload_bytes != self.profile.commands.compute_info.least_payload {
            return Err(Why::CommandLayout);
        }
        self.object_fits(words.payload(1).ok_or(Why::RingLayout)?, task_bytes)?;
        // The third word is a limit for the keys, and any number is one.
        let pairs = words.payload(3).ok_or(Why::RingLayout)?;
        let answer = words.payload64(4).ok_or(Why::RingLayout)?;
        Self::stretch_fits(answer, u64::from(pairs) * self.profile.task.info_pair_bytes, task_bytes)?;
        Ok(Judged::Plain)
    }

    fn judge_set_lists(&self, words: &Words<'_>, payload_bytes: u32) -> Result<Judged, Why> {
        let task_bytes = self.user_task(words.guest, words.payload(0))?;
        if payload_bytes != self.profile.commands.set_lists.least_payload {
            return Err(Why::CommandLayout);
        }
        let objects = words.payload64(1).ok_or(Why::RingLayout)?;
        let placements = words.payload64(3).ok_or(Why::RingLayout)?;
        let objects_bytes = words.payload(5).ok_or(Why::RingLayout)?;
        let placements_bytes = words.payload(6).ok_or(Why::RingLayout)?;
        // The device takes an object list only where a page starts.
        if !objects.is_multiple_of(PAGE_BYTES) {
            return Err(Why::TaskLayout);
        }
        Self::stretch_fits(objects, u64::from(objects_bytes), task_bytes)?;
        Self::stretch_fits(placements, u64::from(placements_bytes), task_bytes)?;
        Ok(Judged::Plain)
    }

    /// Where a child ring's descriptor is in the root page the device has, which is the host's.
    /// Forgets what the device forgets when the guest turns the first ring off, as seen on QEMU's
    /// device: every task, the kernel task too, every child ring and the display's page. The
    /// device then clears the display's word and each child ring's defined bit in its root page,
    /// and leaves the tasks' words as they were. The guest's root page is given the same. In the
    /// host's the tasks' words are cleared besides, so that a task defined again is not taken
    /// for defined by its old word before the device has written the new one.
    pub(super) fn forget_with_the_ring(&mut self, memory: &GuestMemory) {
        let child = &self.profile.child;
        let ours = self.shadow.memory();
        let roots = self.shadow.root() + self.profile.task.roots_at;
        let theirs = page_address(self.ring.root_page);
        let kernel_task = self.kernel_task.take().map(|_| self.profile.task.kernel_task);
        for id in self.user_tasks.keys().copied().chain(kernel_task) {
            ours.set_word(roots + u64::from(id) * u64::from(WORD_BYTES), 0);
        }
        self.user_tasks.clear();
        for &id in self.children.keys() {
            let at = self.child_descriptor(id) + child.stamp_at;
            for (memory, at) in [(ours, at), (memory, theirs + (at - self.shadow.root()))] {
                if let Some(word) = memory.word(at) {
                    memory.set_word(at, word & !child.defined);
                }
            }
            self.mirror_now().set_child(id, None);
        }
        self.children.clear();
        for port in 0..PORTS {
            if self.state_pages[port as usize] == 0 {
                continue;
            }
            let at = self.profile.display.state_pages_at + u64::from(port) * u64::from(WORD_BYTES);
            ours.set_word(self.shadow.root() + at, 0);
            memory.set_word(theirs + at, 0);
            self.mirror_now().set_state_page(port, None);
            self.state_pages[port as usize] = 0;
        }
    }

    fn child_descriptor(&self, id: u32) -> u64 {
        let child = &self.profile.child;
        self.shadow.root() + child.descriptors_at + u64::from(id.max(1) - 1) * child.descriptor_bytes
    }

    /// A child ring. The guest has written its descriptor into its own root page: that is read
    /// here, once, and the device gets a descriptor in the host's root page that names pages of
    /// the host's for the ring. The ring has to start empty: the device runs at once whatever
    /// lies before the written position.
    fn judge_define_ring(&self, words: &Words<'_>) -> Result<Judged, Why> {
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
