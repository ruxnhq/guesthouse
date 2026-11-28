// SPDX-License-Identifier: GPL-2.0-or-later
//! A guest played by hand: memory of this process's own, a ring, and the few commands a display
//! driver sends. `tests/picture.rs` and `examples/present.rs` both drive the device with it.

use guesthouse_pvg_sys::profile::{PAGE_BYTES, PAGE_SHIFT};
use guesthouse_pvg_sys::{Config, Device, Event, GuestMemory, Mode};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const MEMORY_AT: u64 = 0x4000_0000;
pub const MEMORY_BYTES: u64 = 64 << 20;

// Where this guest keeps things. Each is on a page of its own, clear of the others.
pub const ROOT_PAGE: u64 = 0x4100_0000;
pub const TASK_ROOT: u64 = 0x4120_0000;
pub const TASK_TABLE: u64 = 0x4130_0000;
pub const STATE_PAGE: u64 = 0x4140_0000;
pub const GLYPH: u64 = 0x4150_0000;
pub const MAPPER_RING: u64 = 0x4180_0000;
pub const MAPPER_TABLE: u64 = 0x4190_0000;
pub const DESCRIPTORS: u64 = 0x41A0_0000;
pub const PAGE_LIST: u64 = 0x41A0_4000;
pub const RING: u64 = 0x4200_0000;
pub const SURFACE: u64 = 0x4300_0000;

pub const VERSION: u32 = 252;
pub const RING_BYTES: u32 = 0x1_0000;
pub const COMMANDS_AT: u32 = 0x1000;
const HEADER_WORDS: u32 = 3;

// The device's registers.
const RING_ON: u64 = 0x1000;
const RING_LENGTH: u64 = 0x1004;
pub const WRITTEN: u64 = 0x1008;
const COMMANDS_START: u64 = 0x1010;
const ROOT: u64 = 0x101C;
const RESUME: u64 = 0x1024;
pub const FAULT: u64 = 0x102C;
// The early framebuffer's bank.
pub const EARLY_ON: u64 = 0x1200;
pub const EARLY_MODE_COUNT: u64 = 0x1204;
pub const EARLY_PAGE: u64 = 0x1210;
pub const EARLY_BYTES: u64 = 0x1214;
pub const EARLY_DEPTH: u64 = 0x1218;
pub const EARLY_MODE: u64 = 0x121C;
const RING_PAGE: u64 = 0x1030;
const VERSION_AT: u64 = 0x1034;

// The mapper's registers, and its one command here.
const MAPPER_RING_AT: u64 = 0x1000;
pub const MAPPER_SLOTS: u64 = 0x1008;
const MAPPER_TABLE_AT: u64 = 0x1010;
const MAPPER_WRITTEN: u64 = 0x1018;
pub const MAPPER_DONE: u64 = 0x1020;
const MAPPER_SLOT_BYTES: u64 = 16;
pub const SLOTS: u64 = 1024;
const MAP: u32 = 1;
/// What the mapper says of a command it doesn't support.
pub const UNSUPPORTED: u32 = 0xE000_02C7;
const DESCRIPTOR_BYTES: u64 = 0x200;
const IDS_PER_PAGE: u64 = 32;
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const BYTES_PER_PIXEL: u32 = 4;

// Commands.
pub const STATE_PAGE_COMMAND: u32 = 1;
pub const ACKNOWLEDGE: u32 = 2;
pub const CURSOR_GLYPH: u32 = 4;
pub const SWAP: u32 = 8;
pub const NOTHING: u32 = 30;
pub const DEFINE_TASK: u32 = 56;
pub const KERNEL_TASK_BYTES: u32 = 0x10_0000;

pub fn page_number(address: u64) -> u32 {
    (address >> PAGE_SHIFT) as u32
}

/// How the mapper names a page: its number shifted left by two, with the lowest bit for "there".
fn mapper_page(address: u64) -> u32 {
    page_number(address) << 2 | 1
}

/// How a command went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    /// Whether the device wrote the command's stamp into the ring's first word.
    pub stamped: bool,
    /// What the fault register read after.
    pub faulted: bool,
}

pub struct Guest {
    pub memory: GuestMemory,
    pub device: Device,
    pub events: Arc<Mutex<Vec<Event>>>,
    written: u32,
    stamp: u32,
    mapper_written: u64,
    /// Held for as long as the device lives: a process has room for one. Last, so it goes last.
    _alone: MutexGuard<'static, ()>,
}

impl Guest {
    pub fn new() -> Self {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let alone = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let memory = GuestMemory::anonymous(MEMORY_AT, MEMORY_BYTES).expect("memory for the guest");
        let events = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&events);
        guesthouse_pvg_sys::watch_reasons(|reason| eprintln!("the framework refused: {reason}"));
        let config = Config {
            name: "guest".to_owned(),
            millimetres: (400, 300),
            serial: 1,
            modes: [(640, 480), (1024, 768), (1920, 1080)]
                .map(|(width, height)| Mode { width, height, hertz: 60 })
                .to_vec(),
        };
        let device =
            Device::new(memory.clone(), &config, move |event| heard.lock().expect("events").push(event))
                .expect("the device");
        Self { memory, device, events, written: 0, stamp: 0x5000, mapper_written: 0, _alone: alone }
    }

    pub fn put(&self, address: u64, words: &[u32]) {
        for (index, &word) in words.iter().enumerate() {
            assert!(self.memory.set_word(address + index as u64 * 4, word), "{address:#x} is the guest's");
        }
    }

    pub fn fill(&self, address: u64, bytes: u64, word: u32) {
        let chunk: Vec<u8> = word.to_le_bytes().repeat(PAGE_BYTES as usize / 4);
        for at in (0..bytes).step_by(chunk.len()) {
            let part = (bytes - at).min(chunk.len() as u64) as usize;
            assert!(self.memory.write(address + at, &chunk[..part]), "{address:#x} is the guest's");
        }
    }

    /// States the version and turns the first ring on, as a driver does.
    pub fn start_ring(&mut self) {
        self.device.write(RING_ON, 0);
        self.device.write(VERSION_AT, VERSION);
        self.device.write(ROOT, page_number(ROOT_PAGE));
        self.device.write(RING_PAGE, page_number(RING));
        self.device.write(RING_LENGTH, RING_BYTES);
        self.device.write(COMMANDS_START, COMMANDS_AT);
        self.device.write(RING_ON, 1);
        self.written = 0;
    }

    /// Writes one command behind the last, without telling the device.
    pub fn queue(&mut self, number: u32, payload: &[u32]) {
        let room = RING_BYTES - COMMANDS_AT;
        let length = (HEADER_WORDS + payload.len() as u32) * 4;
        self.stamp += 1;
        let words = [number, length, self.stamp].into_iter().chain(payload.iter().copied());
        for (index, word) in words.enumerate() {
            let place = (self.written + index as u32 * 4) % room;
            self.put(RING + u64::from(COMMANDS_AT + place), &[word]);
        }
        self.written += length;
    }

    /// Tells the device how far the guest has written, and says how the last command went. A swap
    /// is stamped a moment after the write, by the display's own work, so this looks for a while.
    pub fn kick(&mut self) -> Sent {
        self.device.write(WRITTEN, self.written);
        let since = Instant::now();
        loop {
            let sent = Sent {
                stamped: self.memory.word(RING) == Some(self.stamp),
                faulted: self.device.read(FAULT) != 0,
            };
            if sent.stamped || sent.faulted || since.elapsed() > Duration::from_millis(500) {
                return sent;
            }
            std::thread::sleep(Duration::from_micros(100));
        }
    }

    pub fn command(&mut self, number: u32, payload: &[u32]) -> Sent {
        self.queue(number, payload);
        self.kick()
    }

    /// Has the ring go on after a refused command.
    pub fn resume(&mut self) {
        self.device.write(RESUME, 0);
    }

    /// Defines the kernel task, with a table of one level that maps the glyph's page at address 0.
    pub fn define_kernel_task(&mut self) -> Sent {
        self.put(TASK_TABLE, &[page_number(GLYPH)]);
        self.put(TASK_ROOT, &[page_number(TASK_TABLE), 1]);
        self.command(DEFINE_TASK, &[0, KERNEL_TASK_BYTES, 0, page_number(TASK_ROOT)])
    }

    pub fn hand_over_state_page(&mut self) -> Sent {
        self.command(STATE_PAGE_COMMAND, &[0, page_number(STATE_PAGE)])
    }

    /// Sets the mapper's ring up.
    pub fn start_mapper(&mut self) {
        self.device.mapper_write(MAPPER_RING_AT, MAPPER_RING);
        self.device.mapper_write(MAPPER_SLOTS, SLOTS);
        self.device.mapper_write(MAPPER_TABLE_AT, MAPPER_TABLE);
        self.mapper_written = self.device.mapper_read(MAPPER_WRITTEN);
    }

    /// Describes the pages at `SURFACE` as a picture and has the mapper give it this id. Returns
    /// the word the mapper wrote for how it went: 0 is good.
    pub fn map_surface(&mut self, id: u32, width: u32, height: u32) -> u32 {
        self.map_surface_with(id, width, height, |_, _| {})
    }

    /// As [`Guest::map_surface`], with a last word on the description before the mapper is told.
    pub fn map_surface_with(
        &mut self,
        id: u32,
        width: u32,
        height: u32,
        change: impl FnOnce(&Self, u64),
    ) -> u32 {
        let bytes = (u64::from(width * height * BYTES_PER_PIXEL)).next_multiple_of(PAGE_BYTES);
        let descriptor = DESCRIPTORS + u64::from(id) % IDS_PER_PAGE * DESCRIPTOR_BYTES;
        self.fill(descriptor, DESCRIPTOR_BYTES, 0);
        self.put(descriptor, &[mapper_page(PAGE_LIST), BGRA]);
        // The length; the width and the height, each above the size of one element; a row's
        // bytes; a pixel's bytes.
        self.put(
            descriptor + 0x10,
            &[bytes as u32, width << 8 | 1, height << 8 | 1, width * BYTES_PER_PIXEL],
        );
        self.put(descriptor + 0x20, &[BYTES_PER_PIXEL]);
        let pages: Vec<u32> =
            (0..bytes / PAGE_BYTES).map(|page| mapper_page(SURFACE + page * PAGE_BYTES)).collect();
        self.put(PAGE_LIST, &pages);
        self.put(MAPPER_TABLE + u64::from(id) / IDS_PER_PAGE * 4, &[mapper_page(DESCRIPTORS)]);

        change(self, descriptor);

        let slot = MAPPER_RING + self.mapper_written % SLOTS * MAPPER_SLOT_BYTES;
        self.put(slot, &[MAP, id, u32::MAX, 0]);
        self.mapper_written += 1;
        self.device.mapper_write(MAPPER_WRITTEN, self.mapper_written);
        let since = Instant::now();
        while self.device.mapper_read(MAPPER_DONE) < self.mapper_written
            && since.elapsed() < Duration::from_secs(2)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.memory.word(slot + 8).expect("the slot is the guest's")
    }

    /// Gives the device a moment for what it does on a thread of its own, such as writing a new
    /// mode list and saying so. The device's words reach the guest's pages when it raises an
    /// interrupt or the guest reads a register, as here.
    pub fn wait_for(&mut self, done: impl Fn(&Self) -> bool) {
        let since = Instant::now();
        while !done(self) && since.elapsed() < Duration::from_secs(2) {
            self.device.read(FAULT);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Shows a mapping on the display.
    pub fn swap(&mut self, id: u32) -> Sent {
        self.command(SWAP, &[0, 0, id, 0, 0, 0, 0, 0, 0, 0])
    }

    /// The display's picture, once the display has one of this size.
    pub fn frame(&mut self, width: u32, height: u32) -> Option<guesthouse_pvg_sys::Frame> {
        let since = Instant::now();
        loop {
            match self.device.frame().expect("the picture is read") {
                Some(frame) if frame.width == width && frame.height == height => return Some(frame),
                _ if since.elapsed() > Duration::from_secs(2) => return None,
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    pub fn refusals(&self) -> Vec<guesthouse_pvg_sys::Refusal> {
        let events = self.events.lock().expect("events");
        events
            .iter()
            .filter_map(|event| match event {
                Event::Refused(refusal) => Some(*refusal),
                _ => None,
            })
            .collect()
    }

    pub fn interrupts(&self) -> usize {
        self.events
            .lock()
            .expect("events")
            .iter()
            .filter(|event| matches!(event, Event::Interrupt { .. }))
            .count()
    }
}

pub const UPPER: u32 = 0xFFC0_3020;
pub const LOWER: u32 = 0xFF20_60C0;

/// The whole path: a picture of two colours in the guest's pages, given a mapping id by the
/// mapper, and shown with the kernel task, the display's state page, and a swap.
pub fn present(guest: &mut Guest, width: u32, height: u32) -> Option<guesthouse_pvg_sys::Frame> {
    let row = u64::from(width * BYTES_PER_PIXEL);
    let upper = u64::from(height / 2);
    guest.fill(SURFACE, row * upper, UPPER);
    guest.fill(SURFACE + row * upper, row * (u64::from(height) - upper), LOWER);
    guest.start_mapper();
    assert_eq!(guest.map_surface(1, width, height), 0, "the mapper takes the surface");
    guest.start_ring();
    assert!(guest.define_kernel_task().stamped, "the kernel task is defined");
    assert!(guest.hand_over_state_page().stamped, "the display takes its state page");
    assert!(guest.swap(1).stamped, "the swap is taken");
    guest.frame(width, height)
}
