// SPDX-License-Identifier: GPL-2.0-or-later
//! The guard against a guest's memory, with no device: what it passes, what it refuses, and what it
//! makes of a refused command in the guest's ring.

use super::{Effect, Guard, Pass, Refusal, Shadow, Step, What, Why};
use crate::memory::GuestMemory;
use crate::profile::{PAGE_BYTES, PAGE_SHIFT, Profile};

const MEMORY_AT: u64 = 0x4000_0000;
const MEMORY_BYTES: u64 = 16 << 20;
const ROOT_PAGE: u64 = MEMORY_AT + 0x10_0000;
const RING: u64 = MEMORY_AT + 0x20_0000;
const TASK_ROOT: u64 = MEMORY_AT + 0x30_0000;
const TASK_TABLE: u64 = MEMORY_AT + 0x30_4000;
const STATE_PAGE: u64 = MEMORY_AT + 0x40_0000;
const GLYPH: u64 = MEMORY_AT + 0x50_0000;
const MAPPER_RING: u64 = MEMORY_AT + 0x60_0000;
const MAPPER_TABLE: u64 = MEMORY_AT + 0x70_0000;
const DESCRIPTIONS: u64 = MEMORY_AT + 0x74_0000;
/// Mapping 1's description: the second in its page.
const DESCRIPTION: u64 = DESCRIPTIONS + 0x200;
const SURFACE_LIST: u64 = MEMORY_AT + 0x78_0000;
const SURFACE: u64 = MEMORY_AT + 0x80_0000;
const USER_ROOT: u64 = MEMORY_AT + 0x34_0000;
const USER_TABLE: u64 = MEMORY_AT + 0x34_4000;
const USER_LEAF: u64 = MEMORY_AT + 0x34_8000;
const EARLY_BLOCK: u64 = MEMORY_AT + 0xA0_0000;
const CHILD_LIST: u64 = MEMORY_AT + 0xC0_0000;
const CHILD_RING: u64 = MEMORY_AT + 0xC4_0000;
const OUTSIDE: u64 = MEMORY_AT + MEMORY_BYTES;

const RING_BYTES: u32 = 0x1_0000;
const COMMANDS_AT: u32 = 0x1000;
const ROOM: u32 = RING_BYTES - COMMANDS_AT;
const HEADER: u32 = 12;
const TASK_BYTES: u32 = 0x10_0000;

fn page(address: u64) -> u32 {
    (address >> PAGE_SHIFT) as u32
}

/// How the mapper names a page.
fn mapper_page(address: u64) -> u32 {
    page(address) << 2 | 1
}

/// A guest's memory and its ring, the guard, and a device that does as it is told: what was passed
/// to it is kept, and each command passed alone fails or not as the test says.
struct Play {
    profile: &'static Profile,
    memory: GuestMemory,
    /// The host's own pages, which are what the device reads.
    shadow: Shadow,
    guard: Guard,
    written: u32,
    /// Every position the device was told of.
    passed: Vec<u32>,
    /// Every register the device was told another value for than the guest wrote, with it.
    told: Vec<(u64, u32)>,
    refused: Vec<Refusal>,
    /// Whether the device's fault register reads set.
    faulted: bool,
    /// Whether the device refuses the next command it is passed alone.
    fails: bool,
    /// Where the device has stopped reading the first ring, or `None` while it reads all it is
    /// told, as it does unless a refused command holds it.
    read: Option<u32>,
    /// The same for child rings, by id.
    child_read: Vec<(u32, u32)>,
}

impl Play {
    fn new() -> Self {
        let profile = Profile::for_build("26B131").expect("the profile");
        let memory = GuestMemory::anonymous(MEMORY_AT, MEMORY_BYTES).expect("memory");
        let shadow = Shadow::above(&memory, profile).expect("the host's pages");
        Self {
            profile,
            guard: Guard::new(profile, &memory, shadow.clone()),
            memory,
            shadow,
            written: 0,
            passed: Vec::new(),
            told: Vec::new(),
            refused: Vec::new(),
            faulted: false,
            fails: false,
            read: None,
            child_read: Vec::new(),
        }
    }

    /// A guest with its ring on, as a driver sets it up.
    fn with_ring() -> Self {
        let mut play = Self::new();
        play.start_ring();
        play
    }

    fn start_ring(&mut self) {
        let registers = &self.profile.registers;
        let writes = [
            (registers.version, self.profile.version),
            (registers.root_page, page(ROOT_PAGE)),
            (registers.ring_page, page(RING)),
            (registers.ring_bytes, RING_BYTES),
            (registers.commands_at, COMMANDS_AT),
            (registers.ring_on, 1),
        ];
        for (offset, value) in writes {
            assert_eq!(self.write(offset, value), Ok(Pass::AsItIs), "register {offset:#x}");
        }
        self.written = 0;
    }

    /// A register write as the guard judges it. Where the device is told another value, that
    /// value is kept in `told`, and the write counts as passed.
    fn write(&mut self, offset: u64, value: u32) -> Result<Pass, Refusal> {
        match self.guard.device_write(&self.memory, offset, value)? {
            Pass::Other(ours) => {
                self.told.push((offset, ours));
                Ok(Pass::AsItIs)
            }
            pass => Ok(pass),
        }
    }

    fn put(&self, address: u64, words: &[u32]) {
        for (index, &word) in words.iter().enumerate() {
            assert!(self.memory.set_word(address + index as u64 * 4, word));
        }
    }

    /// A word of the ring as the device reads it, which is the host's copy.
    fn ring_word(&self, position: u32) -> u32 {
        let at = self.shadow.ring() + u64::from(COMMANDS_AT + position % ROOM);
        self.shadow.memory().word(at).expect("the copy of the ring")
    }

    /// The same word in the guest's own ring.
    fn guest_ring_word(&self, position: u32) -> u32 {
        self.memory.word(RING + u64::from(COMMANDS_AT + position % ROOM)).expect("the ring is the guest's")
    }

    /// Writes a command behind the last and returns where it starts.
    fn queue(&mut self, number: u32, payload: &[u32]) -> u32 {
        let start = self.written;
        let length = HEADER + payload.len() as u32 * 4;
        for (index, word) in [number, length, 0].into_iter().chain(payload.iter().copied()).enumerate() {
            self.put(RING + u64::from(COMMANDS_AT + (start + index as u32 * 4) % ROOM), &[word]);
        }
        self.written += length;
        start
    }

    /// The guest's kick, passed on in the guard's steps as [`crate::Device`] passes it.
    fn kick(&mut self) -> Result<Vec<Step>, Refusal> {
        let target = self.written;
        let mut steps = Vec::new();
        match self.write(self.profile.registers.written, target)? {
            Pass::Kick => {}
            other => panic!("a kick was judged {other:?}"),
        }
        loop {
            let read = self.read.unwrap_or(self.guard.ring.passed);
            let step = self.guard.next_step(&self.memory, target, read, self.faulted, &mut self.refused);
            self.passed.push(step.upto);
            if let Some(effect) = step.effect.clone() {
                // The device's word for a user task in its root page, written as the device does.
                match effect {
                    Effect::DefineUserTask { id, .. } if !self.fails => {
                        let ours = (self.shadow.task_root(id) >> PAGE_SHIFT) as u32;
                        self.shadow.memory().set_word(self.shadow.root() + u64::from(id) * 4, ours);
                    }
                    Effect::DeleteUserTask { id } if !self.fails => {
                        self.shadow.memory().set_word(self.shadow.root() + u64::from(id) * 4, 0);
                    }
                    // And its bit in a child ring's descriptor.
                    Effect::DefineRing { id, stamp_cell, .. } if !self.fails => {
                        self.shadow.memory().set_word(self.child_descriptor(id) + 12, stamp_cell | 1 << 16);
                    }
                    Effect::DeleteRing { id } if !self.fails => {
                        self.shadow.memory().set_word(self.child_descriptor(id) + 12, 0);
                    }
                    _ => {}
                }
                let faulted = self.faulted || self.fails;
                let failed = self.guard.failed(&effect, self.faulted, faulted);
                // Where the device never says, as with a task refused behind a refused command,
                // the caller gives up after a while.
                self.guard.settle(&self.memory, effect, failed.unwrap_or(true));
                self.faulted |= self.fails;
                self.fails = false;
            }
            let done = step.upto == target || step.stopped.is_some();
            steps.push(step);
            if done {
                return Ok(steps);
            }
        }
    }

    fn send(&mut self, number: u32, payload: &[u32]) -> Vec<Step> {
        self.queue(number, payload);
        self.kick().expect("the kick is passed")
    }

    /// The reason the guard gave for the one command it refused since the last call.
    fn why(&mut self) -> Option<Why> {
        let whys: Vec<Why> = self.refused.drain(..).map(|refusal| refusal.why).collect();
        assert!(whys.len() <= 1, "more than one refusal: {whys:?}");
        whys.first().copied()
    }

    fn define_task(&mut self) -> Vec<Step> {
        // One level, and the glyph's page at the task's start.
        self.put(TASK_TABLE, &[page(GLYPH)]);
        self.put(TASK_ROOT, &[page(TASK_TABLE), 1]);
        let number = self.profile.commands.define_task.number;
        self.send(number, &[0, TASK_BYTES, 0, page(TASK_ROOT)])
    }

    /// A task for a program, as a driver defines one: the id shifted, a megabyte, its root page.
    fn define_user_task(&mut self, id: u32) -> Vec<Step> {
        // A table of one level, which names the glyph's page as the task's first.
        self.put(USER_TABLE, &[page(GLYPH) | self.profile.task.writable]);
        self.put(USER_ROOT, &[page(USER_TABLE), 1, 0, 0, 0x1000]);
        let number = self.profile.commands.define_task.number;
        self.send(number, &[id << 1, TASK_BYTES, 0, page(USER_ROOT)])
    }

    /// One command that hands over buffers in the newer form: these resources, as id and flags,
    /// and these buffers, as address and length.
    fn run(&mut self, task: u32, resources: &[(u32, u32)], buffers: &[(u64, u64)]) -> Vec<Step> {
        let number = self.profile.commands.run_buffers.number;
        let mut payload = vec![task, resources.len() as u32, buffers.len() as u32];
        payload.extend(resources.iter().flat_map(|&(id, flags)| [id, flags]));
        payload.extend(
            buffers.iter().flat_map(|&(at, length)| {
                [at as u32, (at >> 32) as u32, length as u32, (length >> 32) as u32]
            }),
        );
        self.send(number, &payload)
    }

    /// Where the device has a child ring's descriptor, in the host's root page.
    fn child_descriptor(&self, id: u32) -> u64 {
        self.shadow.root() + 0x400 + u64::from(id - 1) * 20
    }

    /// Writes a child ring's descriptor into the guest's root page as a driver does, with the
    /// ring's four pages not in a row, and sends the command that defines it.
    fn define_ring(&mut self, id: u32) -> Vec<Step> {
        let pages = [0u64, 2, 1, 5].map(|index| page(CHILD_RING + index * PAGE_BYTES));
        self.put(CHILD_LIST, &pages);
        self.put(ROOT_PAGE + 0x400 + u64::from(id - 1) * 20, &[0, 0, u32::MAX, id, page(CHILD_LIST)]);
        let number = self.profile.commands.define_ring.number;
        self.send(number, &[id])
    }

    /// Writes words at a position of the child ring, whose pages lie as `define_ring` listed
    /// them, sets the descriptor's written position, and wakes the ring.
    fn wake(&mut self, id: u32, position: u32, words: &[u32]) -> Result<(), Why> {
        for (index, &word) in words.iter().enumerate() {
            let at = u64::from(position) + index as u64 * 4;
            let page = [0u64, 2, 1, 5][(at / PAGE_BYTES % 4) as usize];
            self.put(CHILD_RING + page * PAGE_BYTES + at % PAGE_BYTES, &[word]);
        }
        self.put(ROOT_PAGE + 0x400 + u64::from(id - 1) * 20, &[position + words.len() as u32 * 4]);
        self.wake_written(id)
    }

    /// Wakes a child ring whose descriptor the guest has written. The device's read position in
    /// the host's descriptor is where it stopped, or all the guard passed.
    fn wake_written(&mut self, id: u32) -> Result<(), Why> {
        let stopped = self.child_read.iter().find(|&&(ring, _)| ring == id).map(|&(_, at)| at);
        let read = stopped.or_else(|| self.guard.children.get(&id).map(|child| child.passed)).unwrap_or(0);
        self.shadow.memory().set_word(self.child_descriptor(id) + 4, read);
        let wake = self.profile.registers.wake;
        match self.write(wake, id).map_err(|refusal| refusal.why)? {
            Pass::Wake => {}
            other => panic!("a wake was judged {other:?}"),
        }
        self.guard.wake(&self.memory, id, &mut self.refused).map_err(|refusal| refusal.why)
    }

    /// A word of a child ring as the device reads it.
    fn child_word(&self, id: u32, position: u32) -> u32 {
        self.shadow.memory().word(self.shadow.child_ring(id) + u64::from(position)).expect("the copy")
    }

    fn early(&mut self, offset: u64, value: u32) -> Result<Pass, Why> {
        self.write(offset, value).map_err(|refusal| refusal.why)
    }

    fn hand_over(&mut self, at: u64) -> Vec<Step> {
        let number = self.profile.commands.state_page.number;
        self.send(number, &[0, page(at)])
    }

    /// A cursor's picture at this address of the kernel task, 32 by 32 where the length mapped
    /// holds that and one pixel otherwise.
    fn glyph(&mut self, address: u64, mapped_length: u64) -> Vec<Step> {
        let side = if mapped_length >= 32 * 128 { 32 } else { 1 };
        self.glyph_rows(address, mapped_length, side * 4, side, side)
    }

    /// The same, with rows of `row_bytes` each.
    fn glyph_rows(
        &mut self,
        address: u64,
        mapped_length: u64,
        row_bytes: u32,
        width: u32,
        height: u32,
    ) -> Vec<Step> {
        let number = self.profile.commands.cursor_glyph.number;
        let words = [
            0,
            0,
            address as u32,
            (address >> 32) as u32,
            mapped_length as u32,
            (mapped_length >> 32) as u32,
        ];
        self.send(number, &[words.as_slice(), &[row_bytes, 0, width | height << 16, 0, 0]].concat())
    }

    fn mapper(&mut self, offset: u64, value: u64, written_now: u64) -> Result<(), Why> {
        self.mapper_told(offset, value, written_now).map(|_| ())
    }

    /// A write to the mapper's register, and the value the mapper is told in its place.
    fn mapper_told(&mut self, offset: u64, value: u64, written_now: u64) -> Result<u64, Why> {
        self.guard
            .mapper_write(&self.memory, offset, value, written_now, &mut self.refused)
            .map_err(|refusal| refusal.why)
    }

    /// Describes a picture of the pages at `SURFACE` for mapping 1, as a driver does.
    fn describe(&self, width: u32, height: u32) {
        self.describe_rows(width, height, width * 4);
    }

    /// The same, with rows of `row_bytes` each.
    fn describe_rows(&self, width: u32, height: u32, row_bytes: u32) {
        let length = u64::from(row_bytes * height).next_multiple_of(PAGE_BYTES);
        let pages: Vec<u32> =
            (0..length / PAGE_BYTES).map(|index| mapper_page(SURFACE + index * PAGE_BYTES)).collect();
        self.put(SURFACE_LIST, &pages);
        self.put(DESCRIPTION, &[0; 0x80]);
        self.put(DESCRIPTION, &[mapper_page(SURFACE_LIST), u32::from_be_bytes(*b"BGRA")]);
        self.put(DESCRIPTION + 0x10, &[length as u32, width << 8 | 1, height << 8 | 1, row_bytes, 4]);
        self.put(MAPPER_TABLE, &[mapper_page(DESCRIPTIONS)]);
    }

    /// Puts one command into the mapper's ring and kicks. Returns the number the mapper will find
    /// in the host's ring, which is what it reads.
    fn mapper_command(&mut self, count: u64, number: u32, id: u32) -> u32 {
        let slot = count % 1024 * 16;
        self.put(MAPPER_RING + slot, &[number, id, u32::MAX, 0]);
        assert_eq!(self.mapper(self.profile.mapper.written, count + 1, count), Ok(()));
        assert_eq!(self.memory.word(MAPPER_RING + slot), Some(number), "the guest's ring is the guest's");
        self.shadow.mapper_memory().word(self.shadow.mapper_ring() + slot).expect("the host's ring")
    }

    fn start_mapper(&mut self) {
        let mapper = &self.profile.mapper;
        assert_eq!(self.mapper(mapper.ring, MAPPER_RING, 0), Ok(()));
        assert_eq!(self.mapper(mapper.slots, mapper.slots_unit, 0), Ok(()));
        assert_eq!(self.mapper(mapper.table, MAPPER_TABLE, 0), Ok(()));
    }
}

// The four ways a guest can end the host's process, and the one that stops the device for good.

#[test]
fn a_mapper_ring_that_is_not_whole_pages_of_commands_is_not_passed() {
    let mut play = Play::new();
    let mapper = &play.profile.mapper;
    assert_eq!(play.mapper(mapper.ring, MAPPER_RING, 0), Ok(()));
    for length in [0, 1, 0x3FF, 0x401, 0x4010, 0x7FF, mapper.most_slots + mapper.slots_unit] {
        assert_eq!(play.mapper(mapper.slots, length, 0), Err(Why::MapperRingLength), "{length:#x} commands");
    }
    for length in [0x400, 0x1000, 0x6000] {
        assert_eq!(play.mapper(mapper.slots, length, 0), Ok(()), "{length:#x} commands");
    }
    // And no kick goes through before the ring has a length the guard passed.
    let mut play = Play::new();
    assert_eq!(play.mapper(mapper.ring, MAPPER_RING, 0), Ok(()));
    assert_eq!(play.mapper(mapper.slots, 0x4010, 0), Err(Why::MapperRingLength));
    assert_eq!(play.mapper(mapper.table, MAPPER_TABLE, 0), Ok(()));
    assert_eq!(play.mapper(mapper.written, 1, 0), Err(Why::MapperLayout));
}

#[test]
fn a_state_page_that_counts_too_many_modes_stops_what_would_make_the_device_read_it() {
    let mut play = Play::with_ring();
    let count_at = STATE_PAGE + play.profile.display.mode_count_at;
    let most = play.profile.display.most_modes;
    let acknowledge = play.profile.commands.acknowledge.number;

    // At the hand-over, as part of a page that isn't empty.
    play.put(count_at, &[most + 1]);
    play.hand_over(STATE_PAGE);
    assert_eq!(play.why(), Some(Why::StatePageNotEmpty));

    play.put(count_at, &[0]);
    play.hand_over(STATE_PAGE);
    assert_eq!(play.why(), None);

    // The device has written its list into the page it was given, which is the host's. What
    // the guest writes over the count in its own page is nothing the device reads.
    let ours = play.shadow.state_page(0) + play.profile.display.mode_count_at;
    play.shadow.memory().set_word(ours, most);
    play.put(count_at, &[most + 1]);
    assert_eq!(play.guard.mode_list(), Ok(()));
    play.send(acknowledge, &[0, 1]);
    assert_eq!(play.why(), None);
    assert_eq!(play.shadow.memory().word(ours), Some(most), "the device's count stands");

    // Should the device's own page ever count too many, nothing is passed that reads it.
    play.shadow.memory().set_word(ours, most + 1);
    assert_eq!(play.guard.mode_list(), Err(Refusal { what: What::ModeList, why: Why::TooManyModes }));
    let position = play.written;
    play.send(acknowledge, &[0, 1]);
    assert_eq!(play.why(), Some(Why::TooManyModes));
    assert_eq!(play.ring_word(position), play.profile.commands.refused, "the device finds a refused command");

    // The count is the word's lower half. What is above it is not the guard's.
    play.shadow.memory().set_word(ours, 0x0300_0000 | most);
    assert_eq!(play.guard.mode_list(), Ok(()));
}

#[test]
fn a_state_page_is_only_handed_over_empty_and_to_a_display_that_has_none() {
    let mut play = Play::with_ring();

    // One byte is enough: the device takes what the page holds for the display's state.
    play.put(STATE_PAGE + 0xFC0, &[0x0300_0400]);
    let position = play.written;
    let steps = play.hand_over(STATE_PAGE);
    assert_eq!(play.why(), Some(Why::StatePageNotEmpty));
    assert_eq!(
        steps.last().and_then(|step| step.effect.clone()),
        None,
        "it is passed as any refused command"
    );
    assert_eq!(play.ring_word(position), play.profile.commands.refused);

    play.hand_over(OUTSIDE);
    assert_eq!(play.why(), Some(Why::NotGuestMemory));
    let number = play.profile.commands.state_page.number;
    play.send(number, &[1, page(STATE_PAGE + PAGE_BYTES)]);
    assert_eq!(play.why(), Some(Why::NoSuchDisplay));
    play.send(number, &[0, 0]);
    assert_eq!(play.why(), Some(Why::DisplayNotInUse));

    let steps = play.hand_over(STATE_PAGE + PAGE_BYTES);
    assert_eq!(play.why(), None);
    assert_eq!(
        steps.last().and_then(|step| step.effect.clone()),
        Some(Effect::HandOver { port: 0, page: page(STATE_PAGE + PAGE_BYTES) })
    );
    play.hand_over(STATE_PAGE + 2 * PAGE_BYTES);
    assert_eq!(play.why(), Some(Why::DisplayInUse));

    // Taken back, the display takes a page again.
    play.send(number, &[0, 0]);
    assert_eq!(play.why(), None);
    play.hand_over(STATE_PAGE + 2 * PAGE_BYTES);
    assert_eq!(play.why(), None);
    assert!(!play.guard.spoiled());
}

#[test]
fn a_hand_over_that_fails_in_the_device_ends_hand_overs_and_resets() {
    let mut play = Play::with_ring();
    play.fails = true;
    play.hand_over(STATE_PAGE);
    assert_eq!(play.why(), None, "the guard knew no reason against it");
    assert!(play.guard.spoiled());

    // The device is resumed, and the guest tries again: the second hand-over would end the process.
    play.faulted = false;
    play.hand_over(STATE_PAGE);
    assert_eq!(play.why(), Some(Why::Spoiled));
    let number = play.profile.commands.state_page.number;
    play.send(number, &[0, 0]);
    assert_eq!(play.why(), Some(Why::Spoiled));
    // So would the reset, and with it letting the device go.
    assert_eq!(play.guard.reset(), Err(Refusal { what: What::Reset, why: Why::Spoiled }));
}

#[test]
fn a_reset_forgets_what_the_device_forgets() {
    let mut play = Play::with_ring();
    play.define_task();
    play.hand_over(STATE_PAGE);
    play.start_mapper();
    assert_eq!(play.guard.reset(), Ok(()));

    let written = play.profile.registers.written;
    assert_eq!(play.write(written, 12).map_err(|refusal| refusal.why), Err(Why::RingIsOff));
    assert_eq!(play.mapper(play.profile.mapper.written, 1, 0), Err(Why::MapperLayout));
    play.start_ring();
    play.define_task();
    assert_eq!(play.why(), None, "the task is gone, and can be defined");
    play.hand_over(STATE_PAGE);
    assert_eq!(play.why(), None, "the display has no page, and takes one");
}

#[test]
fn a_cursor_picture_the_kernel_tasks_table_does_not_map_is_refused() {
    let mut play = Play::with_ring();

    play.glyph(0, 4096);
    assert_eq!(play.why(), Some(Why::NoTask));

    play.define_task();
    assert_eq!(play.why(), None);
    play.glyph(0, 4096);
    assert_eq!(play.why(), None, "the first page is mapped");
    play.glyph(0, PAGE_BYTES);
    assert_eq!(play.why(), None);
    play.glyph(PAGE_BYTES - 4, 4);
    assert_eq!(play.why(), None);

    let position = play.written;
    play.glyph(0, PAGE_BYTES + 1);
    assert_eq!(play.why(), Some(Why::PictureNotMapped), "the second page is not");
    assert_eq!(play.ring_word(position), play.profile.commands.refused);
    play.glyph(PAGE_BYTES, 16);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
    play.glyph(u64::from(TASK_BYTES) - 16, 32);
    assert_eq!(play.why(), Some(Why::PictureNotMapped), "past the task's end");
    play.glyph(u64::MAX - 8, 16);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
}

#[test]
fn the_kernel_tasks_table_counts_as_it_was_when_the_task_was_defined() {
    let mut play = Play::with_ring();
    play.define_task();
    // Mapping a second page afterwards changes nothing for the device, which has read the table.
    play.put(TASK_TABLE + 4, &[page(GLYPH + PAGE_BYTES)]);
    play.glyph(PAGE_BYTES, 16);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));

    // A task that is there can't be defined again; deleted and defined, the new table counts.
    play.define_task();
    assert_eq!(play.why(), Some(Why::TaskExists));
    let delete = play.profile.commands.delete_task.number;
    play.send(delete, &[0]);
    assert_eq!(play.why(), None);
    play.put(TASK_TABLE + 4, &[page(GLYPH + PAGE_BYTES)]);
    play.define_task();
    assert_eq!(play.why(), None);
    play.glyph(PAGE_BYTES, 16);
    assert_eq!(play.why(), None);
}

#[test]
fn a_table_entry_that_names_no_guest_memory_maps_nothing() {
    let mut play = Play::with_ring();
    let writable = play.profile.task.writable;
    play.put(TASK_TABLE, &[page(OUTSIDE), 0, page(GLYPH) | writable]);
    play.put(TASK_ROOT, &[page(TASK_TABLE), 1]);
    let define = play.profile.commands.define_task.number;
    play.send(define, &[0, TASK_BYTES, 0, page(TASK_ROOT)]);
    assert_eq!(play.why(), None);
    for (task_page, mapped) in [(0, false), (1, false), (2, true), (3, false)] {
        play.glyph(task_page * PAGE_BYTES, 64);
        assert_eq!(play.why().is_none(), mapped, "the task's page {task_page}");
    }
}

#[test]
fn a_position_below_the_one_before_is_not_passed() {
    let mut play = Play::with_ring();
    let nothing = play.profile.commands.nothing.number;
    let written = play.profile.registers.written;
    play.send(nothing, &[]);
    play.send(nothing, &[]);
    assert_eq!(play.passed, [12, 24]);

    for below in [0, 12, 23] {
        assert_eq!(play.write(written, below).map_err(|refusal| refusal.why), Err(Why::PositionWentBack));
    }
    assert_eq!(play.write(written, 24), Ok(Pass::Nothing), "the same again tells the device nothing");
    assert_eq!(play.write(written, 24 + ROOM + 4).map_err(|refusal| refusal.why), Err(Why::PositionTooFar));

    // The mapper's count is the mapper's own to keep, and is asked each time.
    play.start_mapper();
    let mapper = &play.profile.mapper;
    assert_eq!(play.mapper(mapper.written, 6, 7), Err(Why::PositionWentBack));
    assert_eq!(play.mapper(mapper.written, 7, 7), Ok(()));
    assert_eq!(play.mapper(mapper.written, 7 + mapper.slots_unit, 7), Ok(()));
    assert_eq!(play.mapper(mapper.written, 8 + mapper.slots_unit, 7), Err(Why::PositionTooFar));
}

// What else the guard holds to.

#[test]
fn a_driver_s_commands_are_passed_as_they_are() {
    let mut play = Play::with_ring();
    let commands = play.profile.commands.clone();
    play.define_task();
    play.hand_over(STATE_PAGE);
    play.send(commands.acknowledge.number, &[0, 1]);
    play.send(commands.swap.number, &[0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
    play.send(commands.cursor_show.number, &[0, 1]);
    play.glyph(0, 4096);
    play.send(commands.nothing.number, &[0; 16]);
    play.send(commands.nothing_with_a_word.number, &[0x11]);
    play.send(commands.state_page.number, &[0, 0]);
    play.send(commands.delete_task.number, &[0]);
    assert_eq!(play.refused, []);
    assert_eq!(*play.passed.last().expect("kicks"), play.written);
}

#[test]
fn a_command_this_host_does_not_pass_becomes_a_refused_one() {
    let mut play = Play::with_ring();
    let refused = play.profile.commands.refused;
    // A second ring's, the one that waits for ever, commands of a user task that no driver of
    // ours sends, and ones for a display's properties.
    for number in [51, 58, 61, 69, 10, 12, 9] {
        let position = play.written;
        play.send(number, &[0; 12]);
        assert_eq!(play.why(), Some(Why::UnknownCommand), "command {number}");
        assert_eq!(play.ring_word(position), refused);
        assert_eq!(play.guest_ring_word(position), number, "the guest's own ring is not written");
        assert_eq!(
            play.ring_word(position + 4),
            HEADER + 48,
            "its length stays, for the device to read on by"
        );
    }
    let swap = play.profile.commands.swap.number;
    play.send(swap, &[0, 0, 1, 0, 0x4000, 0, 0, 0, 0, 0]);
    assert_eq!(play.why(), Some(Why::ColourTable));
    play.send(swap, &[0, 1, 1, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(play.why(), Some(Why::NotTheKernelTask));
    play.send(swap, &[0, 0, 1]);
    assert_eq!(play.why(), Some(Why::ShortCommand));
    // The bit below a task's id, which no driver sets.
    let define = play.profile.commands.define_task.number;
    play.put(TASK_ROOT, &[0, 1]);
    play.send(define, &[1, TASK_BYTES, 0, page(TASK_ROOT)]);
    assert_eq!(play.why(), Some(Why::TaskId));
    assert_eq!(*play.passed.last().expect("kicks"), play.written, "and the device is told of all of them");
}

#[test]
fn a_command_that_changes_what_the_guard_keeps_is_passed_alone() {
    let mut play = Play::with_ring();
    let nothing = play.profile.commands.nothing.number;
    let state_page = play.profile.commands.state_page.number;
    play.queue(nothing, &[]);
    play.queue(nothing, &[]);
    play.queue(state_page, &[0, page(STATE_PAGE)]);
    play.queue(nothing, &[]);
    let steps = play.kick().expect("the kick is passed");
    assert_eq!(play.passed, [24, 44, 56]);
    let effects: Vec<bool> = steps.iter().map(|step| step.effect.is_some()).collect();
    assert_eq!(effects, [false, true, false]);
    assert_eq!(play.refused, []);
}

#[test]
fn behind_a_refused_command_nothing_is_passed_that_the_device_must_answer_for() {
    let mut play = Play::with_ring();
    play.faulted = true;
    play.define_task();
    assert_eq!(play.why(), Some(Why::BehindARefusedCommand));
    play.hand_over(STATE_PAGE);
    assert_eq!(play.why(), Some(Why::BehindARefusedCommand));
    // Once the guest has resumed the ring they go through.
    play.faulted = false;
    play.define_task();
    play.hand_over(STATE_PAGE);
    assert_eq!(play.refused, []);
}

#[test]
fn a_task_the_device_refuses_is_not_kept() {
    let mut play = Play::with_ring();
    play.fails = true;
    play.define_task();
    play.faulted = false;
    play.glyph(0, 64);
    assert_eq!(play.why(), Some(Why::NoTask));
    assert!(!play.guard.spoiled());
}

#[test]
fn a_task_of_more_than_one_level_or_no_size_is_refused() {
    let mut play = Play::with_ring();
    let define = play.profile.commands.define_task.number;
    for depth in [0, 2, 3, 4] {
        play.put(TASK_ROOT, &[page(TASK_TABLE), depth]);
        play.send(define, &[0, TASK_BYTES, 0, page(TASK_ROOT)]);
        assert_eq!(play.why(), Some(Why::TaskLayout), "depth {depth}");
    }
    play.put(TASK_ROOT, &[page(TASK_TABLE), 1]);
    for (low, high) in [(0, 0), (0x0400_4000, 0), (0, 1)] {
        play.send(define, &[0, low, high, page(TASK_ROOT)]);
        assert_eq!(play.why(), Some(Why::TaskLayout), "length {high:#x}:{low:#x}");
    }
    play.send(define, &[0, TASK_BYTES, 0, page(OUTSIDE)]);
    assert_eq!(play.why(), Some(Why::NotGuestMemory));
    // A table that is nowhere maps nothing, and the task is there all the same.
    play.put(TASK_ROOT, &[0, 1]);
    play.send(define, &[0, TASK_BYTES, 0, page(TASK_ROOT)]);
    assert_eq!(play.why(), None);
    play.glyph(0, 4);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
}

#[test]
fn a_length_that_is_no_command_s_stops_the_kick_there() {
    for length in [0, 4, 8, 13, 0x1000, u32::MAX] {
        let mut play = Play::with_ring();
        let nothing = play.profile.commands.nothing.number;
        play.queue(nothing, &[]);
        let broken = play.queue(nothing, &[0; 4]);
        play.put(RING + u64::from(COMMANDS_AT + broken + 4), &[length]);
        play.queue(nothing, &[]);
        let steps = play.kick().expect("the kick is judged");
        assert_eq!(play.passed, [broken], "length {length:#x}: the device hears of what came before");
        let stopped = steps.last().and_then(|step| step.stopped).expect("it stopped");
        assert_eq!(stopped.why, Why::BrokenCommand);
        assert_eq!(stopped.what, What::Command { position: broken, number: nothing });
    }
}

#[test]
fn a_command_across_the_ring_s_end_is_read_word_by_word() {
    let mut play = Play::with_ring();
    let nothing = play.profile.commands.nothing.number;
    let filler = (ROOM - HEADER - 16) / 4 - 3;
    play.send(nothing, &vec![0; filler as usize]);
    assert_eq!(play.written, ROOM - 16 - HEADER);
    play.send(nothing, &[]);
    // Sixteen bytes are left before the end, so the command's last word, the page, wraps to the
    // ring's start. A guard that read it straight on would find no page of the guest's there.
    assert_eq!(play.written, ROOM - 16);
    play.hand_over(STATE_PAGE);
    assert_eq!(play.why(), None);
    assert_eq!(play.memory.word(RING + u64::from(COMMANDS_AT)), Some(page(STATE_PAGE)));
    // And one that the guard refuses is marked where its number lies.
    let position = play.written;
    play.send(61, &[0, 0]);
    assert_eq!(play.why(), Some(Why::UnknownCommand));
    assert_eq!(play.ring_word(position), play.profile.commands.refused);
}

#[test]
fn the_ring_is_only_laid_out_while_it_is_off_and_only_in_guest_memory() {
    let mut play = Play::with_ring();
    let registers = play.profile.registers.clone();
    let why = |result: Result<Pass, Refusal>| result.map_err(|refusal| refusal.why);
    for offset in [
        registers.version,
        registers.ring_bytes,
        registers.commands_at,
        registers.ring_page,
        registers.root_page,
    ] {
        let value = if offset == registers.version { play.profile.version } else { 0x4000 };
        assert_eq!(why(play.write(offset, value)), Err(Why::RingIsOn), "register {offset:#x}");
    }
    assert_eq!(why(play.write(registers.ring_on, 1)), Err(Why::RingIsOn));
    assert_eq!(why(play.write(registers.ring_on, 2)), Err(Why::UnknownRegister));

    assert_eq!(why(play.write(registers.ring_on, 0)), Ok(Pass::AsItIs));
    assert_eq!(why(play.write(registers.resume, 0)), Err(Why::RingIsOff));
    assert_eq!(why(play.write(registers.version, 100)), Err(Why::OtherVersion));
    let bad = [
        (registers.ring_page, page(OUTSIDE)),
        (registers.ring_page, page(OUTSIDE - PAGE_BYTES)),
        (registers.root_page, 0),
        (registers.ring_bytes, 0x1_0004),
        (registers.ring_bytes, 0x20_0000),
        (registers.commands_at, 0),
        (registers.commands_at, 0x1002),
        (registers.commands_at, RING_BYTES),
    ];
    let good = [
        (registers.ring_page, page(RING)),
        (registers.root_page, page(ROOT_PAGE)),
        (registers.ring_bytes, RING_BYTES),
        (registers.commands_at, COMMANDS_AT),
    ];
    for (offset, value) in bad {
        assert_eq!(why(play.write(offset, value)), Ok(Pass::AsItIs));
        assert_eq!(why(play.write(registers.ring_on, 1)), Err(Why::RingLayout), "{offset:#x} = {value:#x}");
        let (_, restore) = good.iter().find(|(good_offset, _)| *good_offset == offset).expect("a good value");
        assert_eq!(why(play.write(offset, *restore)), Ok(Pass::AsItIs));
    }
    assert_eq!(why(play.write(registers.ring_on, 1)), Ok(Pass::AsItIs));
}

#[test]
fn a_register_this_host_does_not_know_is_not_written() {
    let mut play = Play::with_ring();
    for offset in [0x0, 0x3000, 0x1038] {
        let refusal = play.write(offset, 1).expect_err("not passed");
        assert_eq!(
            refusal,
            Refusal { what: What::DeviceWrite { offset, value: 1 }, why: Why::UnknownRegister }
        );
    }
    let look = play.profile.registers.cursor_look;
    assert_eq!(play.write(look, 0).map_err(|refusal| refusal.why), Err(Why::DisplayNotInUse));
    play.hand_over(STATE_PAGE);
    assert_eq!(play.write(look, 0), Ok(Pass::AsItIs));
    assert_eq!(play.write(look, 1).map_err(|refusal| refusal.why), Err(Why::NoSuchDisplay));
    assert_eq!(play.mapper(0x1030, 1, 0), Err(Why::UnknownRegister));
}

#[test]
fn the_mapper_s_ring_and_table_have_to_lie_in_guest_memory() {
    let mut play = Play::new();
    let mapper = play.profile.mapper.clone();
    for address in [0, OUTSIDE, MAPPER_RING + 8, OUTSIDE - PAGE_BYTES + 0x1000] {
        assert_eq!(play.mapper(mapper.ring, address, 0), Err(Why::MapperLayout), "ring at {address:#x}");
        assert_eq!(play.mapper(mapper.table, address, 0), Err(Why::MapperLayout), "table at {address:#x}");
    }
    // A ring that fits with one page of commands and not with four.
    assert_eq!(play.mapper(mapper.ring, OUTSIDE - PAGE_BYTES, 0), Ok(()));
    assert_eq!(play.mapper(mapper.slots, 4 * mapper.slots_unit, 0), Err(Why::MapperLayout));
    assert_eq!(play.mapper(mapper.slots, mapper.slots_unit, 0), Ok(()));
    assert_eq!(play.mapper(mapper.ring, MAPPER_RING, 0), Ok(()));
    assert_eq!(play.mapper(mapper.slots, 4 * mapper.slots_unit, 0), Ok(()));
    assert_eq!(play.mapper(mapper.ring, OUTSIDE - PAGE_BYTES, 0), Err(Why::MapperLayout));
}

#[test]
fn only_a_plain_picture_in_the_guest_s_own_pages_is_mapped() {
    let mut play = Play::new();
    let map = play.profile.mapper.map;
    let unsupported = play.profile.mapper.unsupported;
    play.start_mapper();

    play.describe(1920, 1080);
    assert_eq!(play.mapper_command(0, map, 1), map, "a driver's surface is passed as it is");
    assert_eq!(play.why(), None);
    // Letting a mapping go is passed as it is. Any other number is one the mapper answers
    // "unsupported" to.
    assert_eq!(play.mapper_command(1, play.profile.mapper.unmap, 1), play.profile.mapper.unmap);
    assert_eq!(play.why(), None);
    assert_eq!(play.mapper_command(2, 7, 1), unsupported);
    assert_eq!(play.why(), Some(Why::UnknownCommand));
    for number in [3, 0x100, u32::MAX] {
        assert_eq!(play.mapper_command(2, number, 1), unsupported, "{number:#x}");
        assert_eq!(play.why(), Some(Why::UnknownCommand));
    }

    // One word of the description changed at a time, each to something a picture doesn't have.
    let changes: [(u64, u32); 12] = [
        (0x00, mapper_page(SURFACE_LIST) & !1),
        (0x00, mapper_page(OUTSIDE)),
        (0x04, u32::from_be_bytes(*b"420v")),
        (0x08, 1),
        (0x10, 0x4000),
        (0x10, u32::MAX),
        (0x14, 1920 << 8 | 2),
        (0x14, 1),
        (0x18, 16385 << 8 | 1),
        (0x1C, 1920 * 4 - 4),
        (0x20, 8),
        (0x24, 1),
    ];
    for (count, (at, value)) in changes.into_iter().enumerate() {
        play.describe(1920, 1080);
        play.put(DESCRIPTION + at, &[value]);
        assert_eq!(play.mapper_command(3 + count as u64, map, 1), unsupported, "{value:#x} at {at:#x}");
        assert_eq!(play.why(), Some(Why::SurfaceDescription), "{value:#x} at {at:#x}");
    }

    // A page of the surface that is not the guest's, an id the mapper doesn't have, and an id
    // whose page of descriptions the table doesn't name.
    play.describe(1920, 1080);
    play.put(SURFACE_LIST + 4 * 500, &[mapper_page(OUTSIDE)]);
    assert_eq!(play.mapper_command(20, map, 1), unsupported);
    play.describe(1920, 1080);
    assert_eq!(play.mapper_command(21, map, 0), unsupported);
    assert_eq!(play.mapper_command(22, map, 8192), unsupported);
    assert_eq!(play.mapper_command(23, map, 33), unsupported);
    assert_eq!(play.refused.len(), 4);
    play.refused.clear();

    // The ring wraps at its length, and the guard reads where the mapper will.
    play.describe(64, 64);
    play.put(DESCRIPTION + 0x24, &[1]);
    assert_eq!(play.mapper_command(1024 + 5, map, 1), unsupported);
    let host = play.shadow.mapper_memory();
    assert_eq!(host.word(play.shadow.mapper_ring() + 5 * 16), Some(unsupported));
    assert_eq!(play.memory.word(MAPPER_RING + 5 * 16), Some(map));
}

#[test]
fn a_surface_s_rows_may_be_rounded_up_to_64_bytes_and_to_nothing_else() {
    let mut play = Play::new();
    let map = play.profile.mapper.map;
    let unsupported = play.profile.mapper.unsupported;
    play.start_mapper();

    // 100 pixels are 400 bytes, 448 rounded up; 1920 are 7680, a multiple of 64 already.
    let passed = [(100, 400), (100, 448), (1918, 7680), (1920, 7680), (13, 64), (17, 68), (17, 128)];
    for (count, (width, row_bytes)) in passed.into_iter().enumerate() {
        play.describe_rows(width, 64, row_bytes);
        assert_eq!(play.mapper_command(count as u64, map, 1), map, "{width} pixels in {row_bytes} bytes");
        assert_eq!(play.why(), None, "{width} pixels in {row_bytes} bytes");
    }
    let start = passed.len() as u64;
    // Fewer bytes than the pixels, a row rounded to anything but 64, or rounded further.
    let refused =
        [(100, 396), (100, 416), (100, 512), (100, 464), (1920, 7744), (1918, 7676), (17, 72), (17, 192)];
    for (count, (width, row_bytes)) in refused.into_iter().enumerate() {
        play.describe_rows(width, 64, row_bytes);
        assert_eq!(
            play.mapper_command(start + count as u64, map, 1),
            unsupported,
            "{width} pixels in {row_bytes} bytes"
        );
        assert_eq!(play.why(), Some(Why::SurfaceDescription), "{width} pixels in {row_bytes} bytes");
    }

    // The length has to be that of the rows as they are, not of the pixels: 100 by 1000 is 25
    // pages in rows of 400 bytes, 28 in rows of 448.
    let at = start + refused.len() as u64;
    play.describe_rows(100, 1000, 448);
    assert_eq!(play.mapper_command(at, map, 1), map);
    play.describe_rows(100, 1000, 448);
    play.put(DESCRIPTION + 0x10, &[25 * PAGE_BYTES as u32]);
    assert_eq!(play.mapper_command(at + 1, map, 1), unsupported);
    assert_eq!(play.why(), Some(Why::SurfaceDescription));
}

// The early framebuffer.

#[test]
fn a_driver_s_early_framebuffer_is_passed() {
    let mut play = Play::new();
    let early = play.profile.registers.early.clone();
    let bytes = 1280 * 800 * 4;
    assert_eq!(play.early(early.on, 1), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.mode_index, 3), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.page, page(EARLY_BLOCK)), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.bytes, bytes), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.depth, 32), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.mode, 800 << 16 | 1280), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.on, 0), Ok(Pass::AsItIs));
}

#[test]
fn an_early_framebuffer_has_to_be_on_and_lie_in_guest_memory() {
    let mut play = Play::new();
    let early = play.profile.registers.early.clone();
    // Off, the bank keeps nothing, so nothing is passed to it.
    assert_eq!(play.early(early.page, page(EARLY_BLOCK)), Err(Why::EarlyIsOff));
    assert_eq!(play.early(early.mode, 480 << 16 | 640), Err(Why::EarlyIsOff));
    assert_eq!(play.early(early.on, 2), Err(Why::UnknownRegister));
    assert_eq!(play.early(early.on, 1), Ok(Pass::AsItIs));

    assert_eq!(play.early(early.page, page(OUTSIDE)), Err(Why::EarlyBlock));
    assert_eq!(play.early(early.page, 0), Err(Why::EarlyBlock));
    assert_eq!(play.early(early.bytes, 0), Err(Why::EarlyBlock));
    assert_eq!(play.early(early.bytes, 0x4100), Err(Why::EarlyBlock));
    // A block that starts in the guest's memory and ends past it, said in either order.
    let last_page = page(OUTSIDE) - 1;
    assert_eq!(play.early(early.page, last_page), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.bytes, 0x8000), Err(Why::EarlyBlock));
    assert_eq!(play.early(early.bytes, 0x4000), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.page, page(EARLY_BLOCK)), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.bytes, 0x40_0000), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.page, last_page), Err(Why::EarlyBlock));
}

#[test]
fn an_early_picture_has_to_fit_its_block() {
    let mut play = Play::new();
    let early = play.profile.registers.early.clone();
    play.early(early.on, 1).expect("on");
    // No block yet.
    assert_eq!(play.early(early.mode, 480 << 16 | 640), Err(Why::EarlyMode));
    play.early(early.page, page(EARLY_BLOCK)).expect("the page");
    play.early(early.bytes, 0x12_C000).expect("640 by 480");
    assert_eq!(play.early(early.depth, 16), Err(Why::EarlyMode));
    assert_eq!(play.early(early.mode, 480 << 16 | 640), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.mode, 768 << 16 | 1024), Err(Why::EarlyMode));
    assert_eq!(play.early(early.mode, 0), Err(Why::EarlyMode));
    assert_eq!(play.early(early.mode, 480 << 16), Err(Why::EarlyMode));
    // Off forgets the block, and a reset does.
    play.early(early.on, 0).expect("off");
    play.early(early.on, 1).expect("on");
    assert_eq!(play.early(early.mode, 480 << 16 | 640), Err(Why::EarlyMode));
    play.early(early.page, page(EARLY_BLOCK)).expect("the page");
    play.early(early.bytes, 0x12_C000).expect("640 by 480");
    play.guard.reset().expect("a reset");
    assert_eq!(play.early(early.mode, 480 << 16 | 640), Err(Why::EarlyIsOff));
}

#[test]
fn an_early_block_never_gets_shorter_than_the_picture_shown_from_it() {
    let mut play = Play::new();
    let early = play.profile.registers.early.clone();
    let (big, small) = (800 << 16 | 1280, 480 << 16 | 640);
    let (big_bytes, small_bytes) = (0x3E_8000, 0x12_C000);
    play.early(early.on, 1).expect("on");
    play.early(early.page, page(EARLY_BLOCK)).expect("the page");
    play.early(early.bytes, big_bytes).expect("1280 by 800");
    assert_eq!(play.early(early.mode, big), Ok(Pass::AsItIs));
    // The block as long as a smaller picture, with the larger still shown.
    assert_eq!(play.early(early.bytes, small_bytes), Err(Why::EarlyBlock));
    assert_eq!(play.early(early.bytes, PAGE_BYTES as u32), Err(Why::EarlyBlock));
    // On again while on keeps the picture, as the device may.
    play.early(early.on, 1).expect("on");
    assert_eq!(play.early(early.bytes, small_bytes), Err(Why::EarlyBlock));
    // Another page holds the block as long as it was.
    let last_page = page(MEMORY_AT + MEMORY_BYTES) - 1;
    assert_eq!(play.early(early.page, last_page), Err(Why::EarlyBlock));
    assert_eq!(play.early(early.page, page(EARLY_BLOCK + PAGE_BYTES)), Ok(Pass::AsItIs));

    // A driver's change to the smaller picture, in its order: on, page, bytes, depth, mode. The
    // shorter block waits for the smaller picture, and the picture changes all the same.
    play.early(early.on, 1).expect("on");
    play.early(early.page, page(EARLY_BLOCK)).expect("the page");
    assert_eq!(play.early(early.bytes, small_bytes), Err(Why::EarlyBlock));
    play.early(early.depth, 32).expect("the depth");
    assert_eq!(play.early(early.mode, small), Ok(Pass::AsItIs));
    assert_eq!(play.early(early.bytes, small_bytes), Ok(Pass::AsItIs));
    // And back to the larger one.
    play.early(early.on, 1).expect("on");
    play.early(early.page, page(EARLY_BLOCK)).expect("the page");
    assert_eq!(play.early(early.bytes, big_bytes), Ok(Pass::AsItIs));
    play.early(early.depth, 32).expect("the depth");
    assert_eq!(play.early(early.mode, big), Ok(Pass::AsItIs));

    // Off forgets the picture. Writes while off are not kept either.
    play.early(early.on, 0).expect("off");
    assert_eq!(play.early(early.page, page(EARLY_BLOCK)), Err(Why::EarlyIsOff));
    play.early(early.on, 1).expect("on");
    assert_eq!(play.early(early.mode, small), Err(Why::EarlyMode));
    assert_eq!(play.early(early.bytes, PAGE_BYTES as u32), Ok(Pass::AsItIs));
}

// A program's work: user tasks and the commands that name them.

#[test]
fn a_user_task_is_defined_alone_and_kept_when_the_device_says_so() {
    let mut play = Play::with_ring();
    let nothing = play.profile.commands.nothing.number;
    let define = play.profile.commands.define_task.number;
    play.put(USER_ROOT, &[page(USER_TABLE), 1]);
    play.queue(nothing, &[]);
    play.queue(define, &[1 << 1, TASK_BYTES, 0, page(USER_ROOT)]);
    play.queue(nothing, &[]);
    let steps = play.kick().expect("the kick is passed");
    assert_eq!(play.passed, [12, 40, 52]);
    assert_eq!(
        steps[1].effect,
        Some(Effect::DefineUserTask {
            id: 1,
            length: u64::from(TASK_BYTES),
            root_page: page(USER_ROOT),
            table: USER_TABLE,
            depth: 1
        })
    );
    play.run(1, &[(1, 2)], &[(0x4000, 0x274)]);
    assert_eq!(play.refused, []);
    // A second task of that id, and a task the device did not take.
    play.define_user_task(1);
    assert_eq!(play.why(), Some(Why::TaskExists));
    play.fails = true;
    play.define_user_task(2);
    play.faulted = false;
    play.run(2, &[], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::NoSuchTask));
}

#[test]
fn a_user_task_has_an_id_a_size_and_a_root_page_in_guest_memory() {
    let mut play = Play::with_ring();
    let define = play.profile.commands.define_task.number;
    play.put(USER_ROOT, &[page(USER_TABLE), 1]);
    let tries = [
        ([256 << 1, TASK_BYTES, 0, page(USER_ROOT)], Why::TaskId),
        // The bit below the id.
        ([1 << 1 | 1, TASK_BYTES, 0, page(USER_ROOT)], Why::TaskId),
        ([1 << 1, 0, 0, page(USER_ROOT)], Why::TaskLayout),
        // 64 GiB.
        ([1 << 1, 0, 16, page(USER_ROOT)], Why::TaskLayout),
        ([1 << 1, TASK_BYTES, 0, page(OUTSIDE)], Why::NotGuestMemory),
    ];
    for (payload, why) in tries {
        play.send(define, &payload);
        assert_eq!(play.why(), Some(why), "{payload:x?}");
    }
    // More than a table of one level names: 64 MiB and a page.
    play.send(define, &[1 << 1, 0x0400_4000, 0, page(USER_ROOT)]);
    assert_eq!(play.why(), Some(Why::TaskLayout));
    // With two levels a task may be 4 GiB, as a driver's are, and no more. Three are not taken.
    play.put(USER_ROOT, &[page(USER_TABLE), 2]);
    play.send(define, &[1 << 1, 0x4000, 1, page(USER_ROOT)]);
    assert_eq!(play.why(), Some(Why::TaskLayout));
    play.put(USER_ROOT, &[page(USER_TABLE), 3]);
    play.send(define, &[1 << 1, TASK_BYTES, 0, page(USER_ROOT)]);
    assert_eq!(play.why(), Some(Why::TaskLayout));
    play.put(USER_ROOT, &[page(USER_TABLE), 2]);
    play.send(define, &[255 << 1, 0, 1, page(USER_ROOT)]);
    assert_eq!(play.refused, []);
}

#[test]
fn a_user_task_of_up_to_4_gib_is_copied_and_one_leaf_more_is_refused() {
    let mut play = Play::with_ring();
    let shape = play.profile.task.clone();
    let number = |address: u64| (address >> PAGE_SHIFT) as u32;
    let leaves = shape.most_user_leaves;
    assert_eq!(leaves * 4096 * PAGE_BYTES, 4 << 30);
    // Every page of the top names the same page of the lowest level, whose first entry names a
    // page of the guest's and whose last a page of the host's.
    let top: Vec<u32> = (0..leaves).map(|_| page(USER_LEAF)).collect();
    play.put(USER_TABLE, &top);
    play.put(USER_LEAF, &[page(GLYPH) | shape.writable]);
    play.put(USER_LEAF + PAGE_BYTES - 4, &[number(play.shadow.root())]);
    play.put(USER_ROOT, &[page(USER_TABLE), 2]);
    let define = play.profile.commands.define_task.number;
    play.send(define, &[3 << 1, 0, 1, page(USER_ROOT)]);
    assert_eq!(play.refused, []);
    assert_eq!(play.memory.word64(USER_ROOT + shape.length_at), Some(4 << 30));

    // The host's top names a page of its own for each of the 64, and each is the guest's copied.
    let ours = play.shadow.memory().clone();
    let our_top = play.shadow.task_top(3);
    for leaf in [0, 15, 16, 63] {
        let our_leaf = play.shadow.task_leaf(3, leaf);
        assert_eq!(ours.word(our_top + leaf * 4), Some(number(our_leaf)), "leaf {leaf}");
        assert_eq!(ours.word(our_leaf), Some(page(GLYPH) | shape.writable), "leaf {leaf}");
        assert_eq!(ours.word(our_leaf + PAGE_BYTES - 4), Some(0), "leaf {leaf}");
    }
    assert_eq!(ours.word(our_top + leaves * 4), Some(0), "no leaf past 4 GiB");
    // Work in the last page of the task, which a buffer's address reaches by the last leaf.
    play.run(3, &[], &[((4 << 30) - 0x4000, 8)]);
    assert_eq!(play.refused, []);
    play.run(3, &[], &[(4 << 30, 8)]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));

    // A page more, or a leaf more, is a task this host can't copy.
    play.put(USER_TABLE + leaves * 4, &[page(USER_LEAF)]);
    for (low, high) in [(0x4000, 1), (0x400_0000, 1)] {
        play.send(define, &[4 << 1, low, high, page(USER_ROOT)]);
        assert_eq!(play.why(), Some(Why::TaskLayout), "{high:#x}{low:08x}");
    }
    // The next task's pages lie behind the 64 leaves, and the first task left them alone.
    assert_eq!(play.shadow.task_root(4), play.shadow.task_leaf(3, leaves));
}

#[test]
fn a_user_task_s_table_is_the_host_s_copy_and_names_only_the_guest_s_pages() {
    let mut play = Play::with_ring();
    let shape = play.profile.task.clone();
    let number = |address: u64| (address >> PAGE_SHIFT) as u32;
    let host_page = number(play.shadow.root());
    // Two levels, as a driver's: the top names one page of the lowest level, which names a page
    // of the guest's, a page of the host's, and a page outside all memory.
    play.put(USER_LEAF, &[page(GLYPH) | shape.writable, host_page | shape.writable, page(OUTSIDE)]);
    play.put(USER_TABLE, &[page(USER_LEAF), host_page]);
    play.put(USER_ROOT, &[page(USER_TABLE), 2, 0, 0, 0x3000]);
    play.put(USER_ROOT + shape.placements_at, &[0x4000, 0, 0x4000]);
    let define = play.profile.commands.define_task.number;
    let position = play.written;
    play.send(define, &[3 << 1, 0x800_0000, 0, page(USER_ROOT)]);
    assert_eq!(play.refused, []);

    let ours = play.shadow.memory().clone();
    let (root, top, leaf) = (play.shadow.task_root(3), play.shadow.task_top(3), play.shadow.task_leaf(3, 0));
    assert_eq!(play.ring_word(position + HEADER + 12), number(root), "the device gets the host's root page");
    assert_eq!([ours.word(root), ours.word(root + 4)], [Some(number(top)), Some(2)]);
    assert_eq!(ours.word(root + shape.objects_at + 8), Some(0x3000), "the lists are where the guest said");
    assert_eq!(ours.word(root + shape.placements_at), Some(0x4000));
    // The second page of the lowest level was a page of the host's: it names nothing.
    assert_eq!([ours.word(top), ours.word(top + 4)], [Some(number(leaf)), Some(0)]);
    assert_eq!(
        [ours.word(leaf), ours.word(leaf + 4), ours.word(leaf + 8)],
        [Some(page(GLYPH) | shape.writable), Some(0), Some(0)]
    );
    // The guest's root page has what the device would have written there.
    assert_eq!(play.memory.word(ROOT_PAGE + 3 * 4), Some(page(USER_ROOT)));
    assert_eq!(play.memory.word64(USER_ROOT + shape.length_at), Some(0x800_0000));

    // The driver places a buffer and takes one away: the next command that names the task
    // brings the copy up to date, and one for another task does not.
    play.put(USER_LEAF + 4, &[page(SURFACE), 0, page(SURFACE + PAGE_BYTES)]);
    play.run(4, &[], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::NoSuchTask));
    assert_eq!(ours.word(leaf + 4), Some(0));
    play.run(3, &[], &[(0x4000, 8)]);
    assert_eq!(play.refused, []);
    assert_eq!(
        [ours.word(leaf), ours.word(leaf + 4), ours.word(leaf + 8), ours.word(leaf + 12)],
        [Some(page(GLYPH) | shape.writable), Some(page(SURFACE)), Some(0), Some(page(SURFACE + PAGE_BYTES))]
    );
}

#[test]
fn one_program_s_refused_buffer_does_not_keep_another_from_starting() {
    let mut play = Play::with_ring();
    play.faulted = true;
    play.define_user_task(1);
    play.run(1, &[], &[(0x4000, 8)]);
    let delete = play.profile.commands.delete_task.number;
    play.send(delete, &[1]);
    assert_eq!(play.refused, []);
    play.run(1, &[], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::NoSuchTask));
}

#[test]
fn buffers_are_only_handed_over_for_a_user_task_and_inside_it() {
    let mut play = Play::with_ring();
    play.define_task();
    play.define_user_task(1);
    let end = u64::from(TASK_BYTES);
    play.run(1, &[(1, 2), (3, 0), (0, 0)], &[(0x4000, 0x274), (end - 8, 8)]);
    play.run(1, &[], &[]);
    assert_eq!(play.refused, []);

    play.run(0, &[], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::TaskId));
    play.run(2, &[], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::NoSuchTask));
    play.run(1, &[], &[(end - 4, 8)]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.run(1, &[], &[(u64::MAX - 3, 8)]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.run(1, &[], &[(0x4000, 0)]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    // An object whose entry of the object list would lie past the task's end.
    play.run(1, &[(TASK_BYTES / 12, 0)], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.run(1, &[(1, 1)], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::ResourceFlags));
    play.run(1, &[(1, 0x8000_0002)], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::ResourceFlags));
}

#[test]
fn a_command_s_counts_have_to_be_what_its_length_holds() {
    let mut play = Play::with_ring();
    play.define_user_task(1);
    let run = play.profile.commands.run_buffers.number;
    // One buffer said, none there; none said, one there; and counts that only fit when
    // multiplied in 32 bits.
    play.send(run, &[1, 0, 1]);
    assert_eq!(play.why(), Some(Why::CommandLayout));
    play.send(run, &[1, 0, 0, 0x4000, 0, 8, 0]);
    assert_eq!(play.why(), Some(Why::CommandLayout));
    play.send(run, &[1, 0x8000_0000, 0]);
    assert_eq!(play.why(), Some(Why::CommandLayout));
    play.send(run, &[1, 0, 0x1000_0000]);
    assert_eq!(play.why(), Some(Why::CommandLayout));
    play.send(run, &[1]);
    assert_eq!(play.why(), Some(Why::ShortCommand));
}

#[test]
fn the_older_form_takes_a_resource_of_six_words() {
    let mut play = Play::with_ring();
    play.define_user_task(1);
    let run = play.profile.commands.run_buffers_2.number;
    play.send(run, &[1, 2, 1, 1, 0, 0, 0, 0, 0, 2, 1 << 8, 0, 0, 0, 0, 0x4000, 0, 8, 0]);
    assert_eq!(play.refused, []);
    // Another of its four bytes, and one of the words that were only ever seen as zero.
    play.send(run, &[1, 1, 1, 1, 1, 0, 0, 0, 0, 0x4000, 0, 8, 0]);
    assert_eq!(play.why(), Some(Why::ResourceFlags));
    play.send(run, &[1, 1, 1, 1, 0, 0, 7, 0, 0, 0x4000, 0, 8, 0]);
    assert_eq!(play.why(), Some(Why::ResourceFlags));
    play.send(run, &[1, 1, 1, 1, 0, 0x4000, 0, 8, 0]);
    assert_eq!(play.why(), Some(Why::CommandLayout));
}

#[test]
fn the_other_commands_of_a_task_are_read_word_by_word() {
    let mut play = Play::with_ring();
    play.define_user_task(1);
    let commands = play.profile.commands.clone();
    let end = TASK_BYTES;

    play.send(commands.synchronize.number, &[1, 2, 1, 5]);
    play.send(commands.synchronize.number, &[1, 0]);
    assert_eq!(play.refused, []);
    play.send(commands.synchronize.number, &[1, 2, 1]);
    assert_eq!(play.why(), Some(Why::CommandLayout));
    play.send(commands.synchronize.number, &[1, 1, end / 12]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.send(commands.synchronize.number, &[3, 0]);
    assert_eq!(play.why(), Some(Why::NoSuchTask));

    // A stretch of the task unmapped, a resource deleted, and an object deleted by one of the
    // serializer's operations: its number, its length, the object's id.
    play.send(commands.unmap_memory.number, &[1, 0x8_0000, 0, 0x8000, 0]);
    play.send(commands.delete_resource.number, &[1, 1]);
    play.send(commands.delete_object.number, &[1, 1007, 12, 5]);
    assert_eq!(play.refused, []);
    play.send(commands.unmap_memory.number, &[1, end - 0x4000, 0, 0x8000, 0]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.send(commands.unmap_memory.number, &[1, 0, 0, 0x8000]);
    assert_eq!(play.why(), Some(Why::ShortCommand));
    play.send(commands.delete_resource.number, &[1, end / 12]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.send(commands.delete_resource.number, &[2, 1]);
    assert_eq!(play.why(), Some(Why::NoSuchTask));
    // An operation that deletes a heap, one with another length, and no operation at all.
    play.send(commands.delete_object.number, &[1, 1012, 12, 5]);
    assert_eq!(play.why(), Some(Why::CommandLayout));
    play.send(commands.delete_object.number, &[1, 1007, 16, 5]);
    assert_eq!(play.why(), Some(Why::CommandLayout));
    play.send(commands.delete_object.number, &[1, 12, 1007, 12, 5]);
    assert_eq!(play.why(), Some(Why::CommandLayout));

    // The task, a pipeline, a limit for the keys, room for 64 pairs, and where they go.
    play.send(commands.compute_info.number, &[1, 5, 0x100, 64, end - 512, 0]);
    assert_eq!(play.refused, []);
    play.send(commands.compute_info.number, &[1, 5, 0x100, 65, end - 512, 0]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.send(commands.compute_info.number, &[1, 5, 0x100, 64, 0x4000, 0, 0]);
    assert_eq!(play.why(), Some(Why::CommandLayout));

    // The task, the two lists' places, their lengths.
    play.send(commands.set_lists.number, &[1, 0, 0, 0x8000, 0, 0x1000, 0x4000]);
    play.send(commands.set_lists.number, &[1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(play.refused, []);
    play.send(commands.set_lists.number, &[1, 0x100, 0, 0x8000, 0, 0x1000, 0x4000]);
    assert_eq!(play.why(), Some(Why::TaskLayout));
    play.send(commands.set_lists.number, &[1, 0, 0, end - 0x4000, 0, 0x1000, 0x4004]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.send(commands.set_lists.number, &[1, end, 0, 0x8000, 0, 0x1000, 0x4000]);
    assert_eq!(play.why(), Some(Why::OutsideTheTask));
    play.send(commands.set_lists.number, &[0, 0, 0, 0x8000, 0, 0x1000, 0x4000]);
    assert_eq!(play.why(), Some(Why::TaskId));
}

#[test]
fn a_reset_forgets_the_user_tasks() {
    let mut play = Play::with_ring();
    play.define_user_task(1);
    play.guard.reset().expect("a reset");
    play.put(ROOT_PAGE, &[0; 256]);
    play.start_ring();
    play.run(1, &[], &[(0x4000, 8)]);
    assert_eq!(play.why(), Some(Why::NoSuchTask));
    play.define_user_task(1);
    assert_eq!(play.refused, []);
}

// The host's own pages.

#[test]
fn the_device_is_told_the_host_s_pages_for_the_ring_and_the_root() {
    let play = Play::with_ring();
    let registers = &play.profile.registers;
    let ours = |address: u64| (address >> PAGE_SHIFT) as u32;
    assert_eq!(
        play.told,
        [(registers.root_page, ours(play.shadow.root())), (registers.ring_page, ours(play.shadow.ring()))]
    );
    // The guest reads back what it wrote.
    assert_eq!(play.guard.reads_as(registers.root_page), Some(page(ROOT_PAGE)));
    assert_eq!(play.guard.reads_as(registers.ring_page), Some(page(RING)));
    assert_eq!(play.guard.reads_as(registers.read), None);
    assert!(!play.memory.holds(play.shadow.root(), PAGE_BYTES), "no page of the host's is the guest's");
}

#[test]
fn what_the_guest_writes_after_the_kick_is_not_what_the_device_reads() {
    let mut play = Play::with_ring();
    let nothing = play.profile.commands.nothing.number;
    let position = play.queue(nothing, &[]);
    play.kick().expect("the kick is passed");
    // The other processor writes the command that waits for ever over the one that was judged.
    play.put(RING + u64::from(COMMANDS_AT + position), &[61]);
    assert_eq!(play.ring_word(position), nothing);
    assert_eq!(play.guest_ring_word(position), 61);
}

#[test]
fn the_first_ring_s_copy_reaches_no_further_than_a_ring_past_the_device_s_read_position() {
    let mut play = Play::with_ring();
    let nothing = play.profile.commands.nothing.number;
    // A refused command at the start, where the device stops, and ten after it.
    play.queue(99, &[]);
    for _ in 0..10 {
        play.queue(nothing, &[]);
    }
    play.kick().expect("the kick is passed");
    play.why();
    play.read = Some(0);
    let passed = play.written;
    assert_eq!(play.ring_word(12), nothing);

    // A position a whole ring past what was passed would have the copy go over what the device
    // was told of and has not read.
    play.put(RING + u64::from(COMMANDS_AT + 12), &[61, 12, 0]);
    play.put(RING + u64::from(COMMANDS_AT + passed), &[nothing, 0, 0]);
    play.written = passed + ROOM;
    let steps = play.kick().expect("the kick is passed");
    let stopped = steps.last().and_then(|step| step.stopped).map(|refusal| refusal.why);
    assert_eq!(stopped, Some(Why::PositionTooFar));
    assert_eq!(steps.last().map(|step| step.upto), Some(passed));
    assert_eq!(play.ring_word(12), nothing, "what the device has yet to read is what was judged");

    // Up to a ring past the device's read position is the guest's to fill.
    play.written = passed;
    let left = (ROOM - passed - HEADER) / 4;
    play.queue(nothing, &vec![0; left as usize]);
    assert_eq!(play.written, ROOM);
    let steps = play.kick().expect("the kick is passed");
    assert_eq!(steps.last().map(|step| (step.upto, step.stopped)), Some((ROOM, None)));
    assert_eq!(play.ring_word(0), play.profile.commands.refused);
    assert_eq!(play.ring_word(12), nothing);
}

#[test]
fn a_child_ring_s_copy_reaches_no_further_than_a_ring_past_the_device_s_read_position() {
    let mut play = Play::with_ring();
    play.define_ring(1);
    let nothing = play.profile.commands.nothing.number;
    let mut words = vec![99, 12, 0];
    for _ in 0..10 {
        words.extend([nothing, 12, 0]);
    }
    assert_eq!(play.wake(1, 0, &words), Ok(()));
    play.why();
    play.child_read.push((1, 0));
    let passed = words.len() as u32 * 4;
    assert_eq!(play.child_word(1, 12), nothing);

    // The guest's words over what the device has not read, and a position a whole ring on.
    let theirs =
        |at: u32| CHILD_RING + [0u64, 2, 1, 5][(at / 0x1000) as usize] * PAGE_BYTES + u64::from(at % 0x1000);
    play.put(theirs(12), &[61, 12, 0]);
    play.put(theirs(passed), &[nothing, 0, 0]);
    play.put(ROOT_PAGE + 0x400, &[passed + 0x1_0000]);
    assert_eq!(play.wake_written(1), Err(Why::PositionTooFar));
    assert_eq!(play.child_word(1, 12), nothing, "what the device has yet to read is what was judged");
    assert_eq!(play.shadow.memory().word(play.child_descriptor(1)), Some(passed));

    // Once the device has read on, the room it read is the guest's again.
    play.child_read.clear();
    play.put(theirs(passed), &[nothing, 12, 0]);
    play.put(ROOT_PAGE + 0x400, &[passed + 12]);
    assert_eq!(play.wake_written(1), Ok(()));
    assert_eq!(play.refused, []);
}

#[test]
fn a_display_gets_a_page_of_the_host_s_and_the_guest_s_gets_what_the_device_writes() {
    let mut play = Play::with_ring();
    let display = play.profile.display.clone();
    let ours = play.shadow.state_page(0);
    let position = play.written;
    play.hand_over(STATE_PAGE);
    assert_eq!(play.refused, []);
    // The page's number in the command the device reads, and in the guest's own.
    assert_eq!(play.ring_word(position + HEADER + 4), (ours >> PAGE_SHIFT) as u32);
    assert_eq!(play.guest_ring_word(position + HEADER + 4), page(STATE_PAGE));
    assert_eq!(play.memory.word(ROOT_PAGE + display.state_pages_at), Some(page(STATE_PAGE)));
    let mirror = play.guard.mirror();
    let mut mirror = mirror.lock().expect("the mirror");

    // The device writes a list of three modes and says so, and puts a stamp in its cell.
    let device = play.shadow.memory();
    device.set_word(ours + display.mode_count_at, 3);
    device.set_word(ours + display.events_at, 4);
    device.set_word(play.shadow.ring(), 0x5001);
    mirror.to_guest();
    assert_eq!(play.memory.word(STATE_PAGE + display.mode_count_at), Some(3));
    assert_eq!(play.memory.word(STATE_PAGE + display.events_at), Some(4));
    assert_eq!(play.memory.word(RING), Some(0x5001));
    // An event the guest has not taken yet stays, beside a new one.
    device.set_word(ours + display.events_at, 2);
    mirror.to_guest();
    assert_eq!(play.memory.word(STATE_PAGE + display.events_at), Some(6));
    assert_eq!(device.word(ours + display.events_at), Some(0));

    // The guest's words reach the device's page only where the guest has words to say: the
    // cursor's place and whether it shows. Its mask stays its own, and so does a count.
    play.put(STATE_PAGE + display.cursor_place_at, &[0x0020_0010, 7]);
    play.put(STATE_PAGE + display.event_mask_at, &[2]);
    play.put(STATE_PAGE + display.mode_count_at, &[129]);
    mirror.from_guest();
    mirror.to_guest();
    assert_eq!(device.word(ours + display.cursor_place_at), Some(0x0020_0010));
    assert_eq!(device.word(ours + display.cursor_shown_at), Some(1));
    assert_eq!(device.word(ours + display.event_mask_at), Some(u32::MAX));
    assert_eq!(device.word(ours + display.mode_count_at), Some(3));
    assert_eq!(play.memory.word(STATE_PAGE + display.event_mask_at), Some(2));
}

#[test]
fn the_kernel_task_s_table_is_copied_for_the_device_entry_by_entry() {
    let mut play = Play::with_ring();
    let shape = play.profile.task.clone();
    // The glyph's page, a page that is not the guest's, and a page the host may write.
    play.put(TASK_TABLE, &[page(GLYPH), page(OUTSIDE), page(GLYPH) | shape.writable]);
    play.put(TASK_ROOT, &[page(TASK_TABLE), 1]);
    let define = play.profile.commands.define_task.number;
    let position = play.written;
    play.send(define, &[0, TASK_BYTES, 0, page(TASK_ROOT)]);
    assert_eq!(play.refused, []);

    let ours = play.shadow.memory();
    let number = |address: u64| (address >> PAGE_SHIFT) as u32;
    assert_eq!(play.ring_word(position + HEADER + 12), number(play.shadow.kernel_root()));
    assert_eq!(ours.word(play.shadow.kernel_root()), Some(number(play.shadow.kernel_table())));
    assert_eq!(ours.word(play.shadow.kernel_root() + shape.depth_at), Some(1));
    let table = play.shadow.kernel_table();
    assert_eq!(
        [ours.word(table), ours.word(table + 4), ours.word(table + 8)],
        [Some(page(GLYPH)), Some(0), Some(page(GLYPH) | shape.writable)]
    );
    // What the device would have written into the guest's pages, written for it.
    assert_eq!(play.memory.word(ROOT_PAGE), Some(page(TASK_ROOT)));
    assert_eq!(play.memory.word64(TASK_ROOT + shape.length_at), Some(u64::from(TASK_BYTES)));
}

// Child rings.

#[test]
fn a_child_ring_s_commands_are_copied_and_judged_as_the_first_ring_s() {
    let mut play = Play::with_ring();
    play.define_user_task(1);
    let position = play.written;
    let steps = play.define_ring(1);
    assert_eq!(play.refused, []);
    assert!(matches!(
        steps.last().and_then(|step| step.effect.clone()),
        Some(Effect::DefineRing { id: 1, .. })
    ));
    assert_eq!(play.guest_ring_word(position), play.profile.commands.define_ring.number);

    // The device's descriptor names the host's pages, in a row, and starts empty.
    let ours = play.shadow.memory().clone();
    let descriptor = play.child_descriptor(1);
    let number = |address: u64| (address >> PAGE_SHIFT) as u32;
    assert_eq!(ours.word(descriptor + 16), Some(number(play.shadow.child_list(1))));
    assert_eq!(
        ours.word(play.shadow.child_list(1) + 4),
        Some(number(play.shadow.child_ring(1) + PAGE_BYTES))
    );
    assert_eq!(ours.word(descriptor), Some(0));

    // A command that hands over a buffer, one this host doesn't pass, and one that does nothing.
    let run = play.profile.commands.run_buffers.number;
    let nothing = play.profile.commands.nothing.number;
    let commands = [run, 48, 0, 1, 1, 1, 1, 2, 0x4000, 0, 0x274, 0, 61, 20, 0, 0, 0, nothing, 12, 0];
    assert_eq!(play.wake(1, 0, &commands), Ok(()));
    assert_eq!(play.why(), Some(Why::UnknownCommand));
    assert_eq!(play.child_word(1, 0), run);
    assert_eq!(play.child_word(1, 48), play.profile.commands.refused);
    assert_eq!(play.child_word(1, 68), nothing);
    assert_eq!(ours.word(descriptor), Some(80), "the device is told how far the guest has written");
    // The guest's own pages are as the guest wrote them.
    assert_eq!(play.memory.word(CHILD_RING + 48), Some(61));
}

#[test]
fn a_child_ring_is_copied_across_its_pages_and_its_end() {
    let mut play = Play::with_ring();
    play.define_ring(3);
    let nothing = play.profile.commands.nothing.number;
    // One command of four pages less twelve bytes, so that the next lies across the ring's end.
    let long = 0x1_0000 - 12;
    let mut first = vec![nothing, long, 0];
    first.resize(long as usize / 4, 0x1111_1111);
    assert_eq!(play.wake(3, 0, &first), Ok(()));
    assert_eq!(play.wake(3, long, &[nothing, 20, 0, 0x2222_2222, 0x3333_3333]), Ok(()));
    assert_eq!(play.refused, []);
    assert_eq!(play.child_word(3, 0x4000), 0x1111_1111);
    assert_eq!(play.child_word(3, long + 8), 0);
    // The ring's first page again, after the end.
    assert_eq!(play.child_word(3, 0), 0x2222_2222);
    assert_eq!(play.child_word(3, 4), 0x3333_3333);
    assert_eq!(play.shadow.memory().word(play.child_descriptor(3)), Some(long + 20));
}

#[test]
fn a_child_ring_takes_no_command_that_changes_what_the_guard_keeps() {
    let mut play = Play::with_ring();
    play.define_ring(1);
    play.put(USER_ROOT, &[page(USER_TABLE), 1]);
    let commands = play.profile.commands.clone();
    assert_eq!(
        play.wake(1, 0, &[commands.define_task.number, 28, 0, 2 << 1, TASK_BYTES, 0, page(USER_ROOT)]),
        Ok(())
    );
    assert_eq!(play.why(), Some(Why::NotInAChildRing));
    assert_eq!(play.wake(1, 28, &[commands.state_page.number, 20, 0, 0, page(STATE_PAGE)]), Ok(()));
    assert_eq!(play.why(), Some(Why::NotInAChildRing));
    assert_eq!(play.child_word(1, 28), commands.refused);
    // And a task that was never defined is none in a child ring either.
    assert_eq!(play.wake(1, 48, &[commands.run_buffers.number, 24, 0, 2, 0, 0]), Ok(()));
    assert_eq!(play.why(), Some(Why::NoSuchTask));
}

#[test]
fn a_child_ring_s_position_is_read_once_and_only_goes_on() {
    let mut play = Play::with_ring();
    play.define_ring(1);
    let nothing = play.profile.commands.nothing.number;
    assert_eq!(play.wake(1, 0, &[nothing, 12, 0, nothing, 12, 0]), Ok(()));
    let descriptor = play.child_descriptor(1);
    assert_eq!(play.shadow.memory().word(descriptor), Some(24));
    // Back, and further on than the ring has room for.
    assert_eq!(play.wake(1, 0, &[nothing, 12, 0]), Err(Why::PositionWentBack));
    play.put(ROOT_PAGE + 0x400, &[24 + 0x1_0004]);
    assert_eq!(play.wake_written(1), Err(Why::PositionTooFar));
    assert_eq!(play.shadow.memory().word(descriptor), Some(24), "the device's position stands");
    // A length that is no command's: the device is told of what lies before it.
    assert_eq!(play.wake(1, 24, &[nothing, 12, 0, nothing, 7, 0]), Ok(()));
    assert_eq!(play.why(), Some(Why::BrokenCommand));
    assert_eq!(play.shadow.memory().word(descriptor), Some(36));
}

#[test]
fn a_child_ring_has_an_id_a_cell_and_pages_in_guest_memory_and_starts_empty() {
    let mut play = Play::with_ring();
    let define = play.profile.commands.define_ring.number;
    let wake = play.profile.registers.wake;
    let resume = play.profile.registers.resume_child;
    for id in [0, 64] {
        play.send(define, &[id]);
        assert_eq!(play.why(), Some(Why::RingId));
    }
    assert_eq!(play.write(wake, 1).map_err(|refusal| refusal.why), Err(Why::NoSuchRing));
    assert_eq!(play.write(resume, 1).map_err(|refusal| refusal.why), Err(Why::NoSuchRing));
    assert_eq!(play.write(resume, 0).map_err(|refusal| refusal.why), Err(Why::NoSuchRing));

    let descriptor = ROOT_PAGE + 0x400 + 20;
    play.put(CHILD_LIST, &[page(CHILD_RING); 4]);
    let tries = [
        // Commands that lie before the written position, the first ring's cell, a cell past the
        // last, a bit above the index, a list outside the guest's memory.
        ([12, 0, u32::MAX, 2, page(CHILD_LIST)], Why::ChildRingLayout),
        ([0, 0, u32::MAX, 0, page(CHILD_LIST)], Why::ChildRingLayout),
        ([0, 0, u32::MAX, 64, page(CHILD_LIST)], Why::ChildRingLayout),
        ([0, 0, u32::MAX, 2 | 1 << 16, page(CHILD_LIST)], Why::ChildRingLayout),
        ([0, 0, u32::MAX, 2, page(OUTSIDE)], Why::NotGuestMemory),
    ];
    for (words, why) in tries {
        play.put(descriptor, &words);
        play.send(define, &[2]);
        assert_eq!(play.why(), Some(why), "{words:x?}");
    }
    // A page of the ring outside the guest's memory.
    play.put(descriptor, &[0, 0, u32::MAX, 2, page(CHILD_LIST)]);
    play.put(CHILD_LIST + 8, &[page(OUTSIDE)]);
    play.send(define, &[2]);
    assert_eq!(play.why(), Some(Why::NotGuestMemory));

    play.define_ring(2);
    assert_eq!(play.refused, []);
    play.define_ring(2);
    assert_eq!(play.why(), Some(Why::RingExists));
    assert_eq!(play.write(resume, 2), Ok(Pass::AsItIs));
    // Deleted, it is woken no more, and can be defined again.
    let delete = play.profile.commands.delete_ring.number;
    play.send(delete, &[2]);
    assert_eq!(play.refused, []);
    assert_eq!(play.write(wake, 2).map_err(|refusal| refusal.why), Err(Why::NoSuchRing));
    play.send(delete, &[2]);
    assert_eq!(play.why(), Some(Why::NoSuchRing));
    play.define_ring(2);
    assert_eq!(play.refused, []);
}

#[test]
fn what_the_device_writes_of_a_child_ring_reaches_the_guest_s_descriptor() {
    let mut play = Play::with_ring();
    play.define_ring(5);
    let (ours, theirs) = (play.child_descriptor(5), ROOT_PAGE + 0x400 + 4 * 20);
    let mirror = play.guard.mirror();
    let mut mirror = mirror.lock().expect("the mirror");
    mirror.to_guest();
    assert_eq!(play.memory.word(theirs + 12), Some(5 | 1 << 16), "the bit that says it is defined");
    // The device has read 24 bytes, refused the command at 12, and stamped the ring's cell.
    let device = play.shadow.memory();
    device.set_word(ours + 4, 24);
    device.set_word(ours + 8, 12);
    device.set_word(play.shadow.ring() + 5 * 4, 0x77);
    mirror.to_guest();
    assert_eq!(play.memory.word(theirs + 4), Some(24));
    assert_eq!(play.memory.word(theirs + 8), Some(12));
    assert_eq!(play.memory.word(RING + 5 * 4), Some(0x77));
    assert_eq!(play.memory.word(theirs + 16), Some(page(CHILD_LIST)), "the guest's list stays the guest's");
}

#[test]
fn a_cursor_s_picture_is_read_for_the_host_as_the_command_names_it() {
    let mut play = Play::with_ring();
    play.define_task();
    // Rows of 128 bytes for 32 pixels: the second pixel of the third row.
    play.put(GLYPH + 2 * 128 + 4, &[0xFF11_2233]);
    play.glyph(0, 32 * 128);
    let glyph = play.guard.take_glyph().expect("a picture");
    assert_eq!((glyph.width, glyph.height, glyph.hot_x, glyph.hot_y), (32, 32, 0, 0));
    assert_eq!(glyph.pixels.len(), 32 * 32 * 4);
    assert_eq!(glyph.pixels[(2 * 32 + 1) * 4..][..4], 0xFF11_2233u32.to_le_bytes());
    assert_eq!(play.guard.take_glyph(), None, "taken once");
    // A picture longer than the length mapped is refused, at the row length the command gives.
    let position = play.written;
    play.glyph_rows(0, 31 * 128, 128, 32, 32);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
    assert_eq!(play.ring_word(position), play.profile.commands.refused);
    assert_eq!(play.guard.take_glyph(), None);
    play.glyph_rows(0, 32 * 128, 256, 32, 32);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
    play.glyph_rows(0, 32 * 128, u32::MAX, 32, 2);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
    // Rows with a gap between them that end within it are read, and so is no picture at all.
    play.glyph_rows(0, 31 * 128 + 64, 128, 16, 32);
    assert_eq!(play.why(), None);
    assert_eq!(play.guard.take_glyph().map(|glyph| (glyph.width, glyph.height)), Some((16, 32)));
    play.glyph_rows(0, 16, 0, 0, 0);
    assert_eq!(play.why(), None);
    assert_eq!(play.guard.take_glyph(), None);
}

#[test]
fn the_mapper_is_told_the_host_s_ring_and_table_and_the_guest_reads_its_own() {
    let mut play = Play::new();
    let mapper = play.profile.mapper.clone();
    assert_eq!(play.mapper_told(mapper.ring, MAPPER_RING, 0), Ok(play.shadow.mapper_ring()));
    assert_eq!(play.mapper_told(mapper.slots, mapper.slots_unit, 0), Ok(mapper.slots_unit));
    assert_eq!(play.mapper_told(mapper.table, MAPPER_TABLE, 0), Ok(play.shadow.mapper_table()));
    assert_eq!(play.guard.mapper_reads_as(mapper.ring), Some(MAPPER_RING));
    assert_eq!(play.guard.mapper_reads_as(mapper.table), Some(MAPPER_TABLE));
    assert_eq!(play.guard.mapper_reads_as(mapper.done), None);
    // The host's pages are the mapper's alone: neither the guest's memory nor the device's.
    let host = play.shadow.mapper_memory();
    assert!(!play.memory.holds(play.shadow.mapper_ring(), PAGE_BYTES));
    assert!(!play.shadow.memory().holds(play.shadow.mapper_ring(), PAGE_BYTES));
    assert!(host.holds(play.shadow.mapper_ring(), mapper.most_slots * mapper.slot_bytes));
    assert!(host.holds(play.shadow.page_list(mapper.most_id), PAGE_BYTES));
    // The host's table names the host's pages of descriptions, whatever the guest's names.
    let entry = host.word(play.shadow.mapper_table() + 4 * 255);
    assert_eq!(entry, Some(mapper_page(play.shadow.mapper_descriptions(255))));
}

#[test]
fn what_the_guest_writes_after_the_mapper_s_kick_is_not_what_the_mapper_reads() {
    let mut play = Play::new();
    let shape = play.profile.mapper.clone();
    play.start_mapper();
    play.describe(64, 64);
    assert_eq!(play.mapper_command(0, shape.map, 1), shape.map);
    assert_eq!(play.why(), None);
    let host = play.shadow.mapper_memory().clone();
    let description = play.shadow.mapper_descriptions(0) + 0x200;
    let list = play.shadow.page_list(1);

    // The mapper's copy names the host's page list, which names the guest's surface.
    assert_eq!(host.word(description), Some(mapper_page(list)));
    assert_eq!(host.word(description + 4), Some(u32::from_be_bytes(*b"BGRA")));
    assert_eq!(host.word(description + 0x14), Some(64 << 8 | 1));
    assert_eq!(host.word(list), Some(mapper_page(SURFACE)));
    assert_eq!(host.word(list + 4), Some(0), "64 by 64 is one page");

    // The other processor writes over the description and the list that were judged.
    play.put(DESCRIPTION, &[mapper_page(SURFACE_LIST) & !1]);
    play.put(DESCRIPTION + 0x14, &[16384 << 8 | 1]);
    play.put(SURFACE_LIST, &[mapper_page(OUTSIDE)]);
    play.put(MAPPER_TABLE, &[mapper_page(OUTSIDE)]);
    play.put(MAPPER_RING, &[7]);
    assert_eq!(host.word(description), Some(mapper_page(list)));
    assert_eq!(host.word(description + 0x14), Some(64 << 8 | 1));
    assert_eq!(host.word(list), Some(mapper_page(SURFACE)));
    assert_eq!(host.word(play.shadow.mapper_ring()), Some(shape.map));
    assert_eq!(host.word(play.shadow.mapper_table()), Some(mapper_page(play.shadow.mapper_descriptions(0))));

    // A larger surface after a smaller one: the list is the new one's, and nothing of the old.
    play.describe(1920, 1080);
    play.put(SURFACE_LIST + 4 * 507, &[mapper_page(SURFACE)]);
    assert_eq!(play.mapper_command(1, shape.map, 1), shape.map);
    assert_eq!(host.word(list + 4 * 506), Some(mapper_page(SURFACE + 506 * PAGE_BYTES)));
    assert_eq!(host.word(list + 4 * 507), Some(0), "507 pages, and the guest's next word is not copied");
    play.describe(64, 64);
    assert_eq!(play.mapper_command(2, shape.map, 1), shape.map);
    assert_eq!(host.word(list + 4), Some(0));
}

#[test]
fn the_mapper_s_outcome_reaches_the_guest_s_ring_before_the_guest_hears_it_is_done() {
    let mut play = Play::new();
    let shape = play.profile.mapper.clone();
    play.start_mapper();
    play.describe(64, 64);
    assert_eq!(play.mapper_command(0, shape.map, 1), shape.map);
    assert_eq!(play.mapper_command(1, 2, 1), 2);
    let host = play.shadow.mapper_memory().clone();
    let ring = play.shadow.mapper_ring();
    let mirror = play.guard.mirror();
    let mut mirror = mirror.lock().expect("the mirror");

    // Nothing done yet: the guest's outcome words stay as the guest wrote them.
    mirror.mapper_to_guest(None);
    assert_eq!(play.memory.word(MAPPER_RING + 8), Some(u32::MAX));
    // The mapper has done the first, and its interrupt comes.
    host.set_word(ring + 8, 0);
    mirror.mapper_to_guest(None);
    assert_eq!(play.memory.word(MAPPER_RING + 8), Some(0));
    // The second, and the guest reads the count of commands done.
    host.set_word(ring + 16 + 8, 0xE000_02C2);
    mirror.mapper_to_guest(Some(2));
    assert_eq!(play.memory.word(MAPPER_RING + 16 + 8), Some(0xE000_02C2));
    // Done with, both are forgotten: the guest may use the slots again for its own.
    play.put(MAPPER_RING + 8, &[0x1234]);
    host.set_word(ring + 8, 5);
    mirror.mapper_to_guest(None);
    assert_eq!(play.memory.word(MAPPER_RING + 8), Some(0x1234));
}

// The first ring turned off, and the positions' wrap at 2^32.

#[test]
fn turning_the_ring_off_forgets_what_the_device_forgets() {
    let mut play = Play::with_ring();
    play.define_task();
    play.hand_over(STATE_PAGE);
    play.define_user_task(1);
    play.define_ring(2);
    assert_eq!(play.refused, []);
    let ours = play.shadow.memory().clone();
    let roots = play.shadow.root();
    assert_eq!(play.memory.word(ROOT_PAGE + 0x900), Some(page(STATE_PAGE)));
    assert_ne!(ours.word(roots + 4), Some(0));
    ours.set_word(play.child_descriptor(2) + 12, 2 | 1 << 16);
    play.put(ROOT_PAGE + 0x400 + 20 + 12, &[2 | 1 << 16]);

    let ring_on = play.profile.registers.ring_on;
    assert_eq!(play.write(ring_on, 0), Ok(Pass::AsItIs));
    // The display's word and the ring's bit are cleared, in both root pages, as the device clears
    // them in its own. The task's word stays in the guest's page, as in the device's, and is
    // cleared in the host's, where the guard reads the device's answer to a 56.
    assert_eq!(play.memory.word(ROOT_PAGE + 0x900), Some(0));
    assert_eq!(ours.word(roots + 0x900), Some(0));
    assert_eq!(ours.word(play.child_descriptor(2) + 12), Some(2));
    assert_eq!(play.memory.word(ROOT_PAGE + 0x400 + 20 + 12), Some(2));
    assert_eq!(ours.word(roots), Some(0));
    assert_eq!(ours.word(roots + 4), Some(0));
    assert_eq!(play.wake(2, 0, &[play.profile.commands.nothing.number, 12, 0]), Err(Why::NoSuchRing));

    // A driver loaded again defines all of it anew, and the guard passes it.
    assert_eq!(play.write(ring_on, 1), Ok(Pass::AsItIs));
    play.written = 0;
    play.define_task();
    play.hand_over(STATE_PAGE);
    play.define_user_task(1);
    play.define_ring(2);
    assert_eq!(play.refused, [], "nothing is defined already");
    let run = play.profile.commands.run_buffers.number;
    play.send(run, &[1, 0, 1, 0x4000, 0, 8, 0]);
    assert_eq!(play.why(), None);
}

/// The driver's ring as it is now: 0x14000 bytes with commands at 0x4000, a room of 2^16.
const EVEN_BYTES: u32 = 0x1_4000;
const EVEN_AT: u32 = 0x4000;
const EVEN_ROOM: u32 = EVEN_BYTES - EVEN_AT;

impl Play {
    fn with_even_ring() -> Self {
        let mut play = Self::new();
        let registers = &play.profile.registers;
        let writes = [
            (registers.version, play.profile.version),
            (registers.root_page, page(ROOT_PAGE)),
            (registers.ring_page, page(RING)),
            (registers.ring_bytes, EVEN_BYTES),
            (registers.commands_at, EVEN_AT),
            (registers.ring_on, 1),
        ];
        for (offset, value) in writes {
            assert_eq!(play.write(offset, value), Ok(Pass::AsItIs), "register {offset:#x}");
        }
        play
    }

    /// Writes words into the even ring from a position on, each where it falls.
    fn put_even(&self, start: u32, words: &[u32]) {
        for (index, &word) in words.iter().enumerate() {
            let place = start.wrapping_add(index as u32 * 4) % EVEN_ROOM;
            self.put(RING + u64::from(EVEN_AT + place), &[word]);
        }
    }

    fn even_ring_word(&self, position: u32) -> u32 {
        let at = self.shadow.ring() + u64::from(EVEN_AT + position % EVEN_ROOM);
        self.shadow.memory().word(at).expect("the copy of the ring")
    }
}

#[test]
fn the_first_ring_s_positions_wrap_at_2_to_the_32() {
    let mut play = Play::with_even_ring();
    let nothing = play.profile.commands.nothing.number;
    let written = play.profile.registers.written;
    // The driver's count 24 bytes short of the wrap, and a command of 44 bytes across it.
    let start = 0u32.wrapping_sub(24);
    play.guard.ring.passed = start;
    play.put_even(start, &[nothing, 44, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
    play.written = start.wrapping_add(44);
    let steps = play.kick().expect("the kick is passed");
    assert_eq!(play.refused, []);
    assert_eq!(steps.last().map(|step| step.upto), Some(20));
    assert_eq!(play.even_ring_word(start), nothing);
    assert_eq!(play.even_ring_word(start.wrapping_add(20)), 3);
    assert_eq!(play.even_ring_word(0), 4);
    assert_eq!(play.even_ring_word(16), 8);

    // Back across the wrap is back, and the next command goes on from 20.
    assert_eq!(play.write(written, u32::MAX - 3).map_err(|refusal| refusal.why), Err(Why::PositionWentBack));
    assert_eq!(
        play.write(written, 20 + EVEN_ROOM + 4).map_err(|refusal| refusal.why),
        Err(Why::PositionTooFar)
    );
    play.put_even(20, &[nothing, 12, 0]);
    play.written = 32;
    assert_eq!(play.kick().expect("the kick is passed").last().map(|step| step.upto), Some(32));
    assert_eq!(play.refused, []);
}

#[test]
fn a_command_that_changes_what_the_guard_keeps_is_passed_alone_across_the_wrap() {
    let mut play = Play::with_even_ring();
    let start = 0u32.wrapping_sub(8);
    play.guard.ring.passed = start;
    play.put(USER_TABLE, &[page(GLYPH) | play.profile.task.writable]);
    play.put(USER_ROOT, &[page(USER_TABLE), 1, 0, 0, 0x1000]);
    let define = play.profile.commands.define_task.number;
    play.put_even(start, &[define, 28, 0, 1 << 1, TASK_BYTES, 0, page(USER_ROOT)]);
    play.written = start.wrapping_add(28);
    let steps = play.kick().expect("the kick is passed");
    assert_eq!(play.refused, []);
    assert!(matches!(
        steps.last().and_then(|step| step.effect.clone()),
        Some(Effect::DefineUserTask { id: 1, .. })
    ));
    assert_eq!(steps.last().map(|step| step.upto), Some(20));
    // The device is given the host's root page for the task, in the copy, past the wrap.
    assert_eq!(play.even_ring_word(16), page(play.shadow.task_root(1)));
}

#[test]
fn a_ring_whose_room_does_not_divide_2_to_the_32_does_not_wrap() {
    let mut play = Play::with_ring();
    let written = play.profile.registers.written;
    play.guard.ring.passed = 0u32.wrapping_sub(12);
    assert_eq!(play.write(written, 12).map_err(|refusal| refusal.why), Err(Why::UnevenWrap));
    assert_eq!(play.write(written, 0).map_err(|refusal| refusal.why), Err(Why::UnevenWrap));
    // Up to the wrap it goes as before.
    assert_eq!(play.write(written, u32::MAX - 3), Ok(Pass::Kick));
}

#[test]
fn a_child_ring_s_positions_wrap_at_2_to_the_32() {
    let mut play = Play::with_ring();
    play.define_ring(1);
    assert_eq!(play.refused, []);
    let nothing = play.profile.commands.nothing.number;
    let start = 0u32.wrapping_sub(12);
    play.guard.children.get_mut(&1).expect("the ring").passed = start;
    // A command of 24 bytes, half before the wrap and half after, in the ring's last and first
    // pages as `define_ring` listed them.
    for (index, word) in [nothing, 24, 0, 1, 2, 3].into_iter().enumerate() {
        let at = u64::from(start.wrapping_add(index as u32 * 4) % 0x1_0000);
        let page = [0u64, 2, 1, 5][(at / PAGE_BYTES) as usize];
        play.put(CHILD_RING + page * PAGE_BYTES + at % PAGE_BYTES, &[word]);
    }
    play.put(ROOT_PAGE + 0x400, &[start.wrapping_add(24)]);
    assert_eq!(play.wake_written(1), Ok(()));
    assert_eq!(play.refused, []);
    assert_eq!(play.child_word(1, 0x1_0000 - 12), nothing);
    assert_eq!(play.child_word(1, 0), 1);
    assert_eq!(play.child_word(1, 8), 3);
    assert_eq!(play.shadow.memory().word(play.child_descriptor(1)), Some(12));
    // And back across it is back.
    play.put(ROOT_PAGE + 0x400, &[u32::MAX - 3]);
    assert_eq!(play.wake_written(1), Err(Why::PositionWentBack));
}
