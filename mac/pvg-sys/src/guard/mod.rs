// SPDX-License-Identifier: GPL-2.0-or-later
//! What a guest may say to the device.
//!
//! The framework trusts its guest in places. Four things a guest can say end the process the
//! device lives in, with an assertion inside the framework:
//!
//! - a length for the surface mapper's ring that isn't whole pages of commands;
//! - more modes than a display takes, in the word of the display's state page that counts them,
//!   which the device reads back when a page is handed over and whenever a new list is due;
//! - a state page that isn't empty when it is handed over: the hand-over then fails half done, and
//!   the next hand-over, and letting the display go, both end the process;
//! - a cursor picture at an address the kernel task's table doesn't map, where the host takes
//!   cursor pictures.
//!
//! A fifth stops the device for good and fills the system's log meanwhile: a written position
//! below the one before. And what a guest tells the mapper of a surface goes on to the host's
//! kernel, which makes the surface. So every word a guest says is read here first. A register write that
//! isn't known to be good is not passed on, and the guest finds a device that ignored it. A command
//! that isn't known to be good gets its number changed, in the guest's ring, to one the device
//! refuses in its ordinary way, so the guest sees a refused command and resumes the ring as its
//! driver would for any other.
//!
//! The guard keeps what the device was told, in order to judge what comes next: where the ring is,
//! whether the kernel task exists and what its table mapped when the device read it, which user
//! tasks exist and how long each is, and which page the display holds. A command that changes one of those is passed alone and the device is
//! asked how it went, through [`Guard::next_step`] and [`Guard::settle`].
//!
//! A program's work comes as command buffers in a user task's memory, which the framework hands
//! to a helper process of its own. The guard reads the ring's command that hands them over, every
//! word: the task, the counts, each resource's id and flags, each buffer's place in the task. It
//! copies the task's table, so that the task names the guest's own pages and no others. It does
//! not read what a buffer holds, nor the task's object list. Those the helper reads from the
//! guest's memory when it comes to them, and judging them here would mean copying a task's
//! memory for every command. The helper is the process Apple made to take a guest's words,
//! and its end is not this process's end.
//!
//! The guest's other processors run on while a word is checked here. So what the device reads of
//! the ring, the root page, a display's state page and the kernel task's table is not the
//! guest's memory: it is a copy in pages of the host's own, made before the check, which the
//! guest can't write. See [`Shadow`]. The device is told those pages' numbers in place of the
//! guest's, and what it writes there is copied back for the guest to read. The surface mapper's
//! ring, table, descriptions and page lists are copied the same way, into a range that only the
//! mapper is shown.

mod mapper;
mod ring;
mod shadow;
#[cfg(test)]
mod tests;

pub use shadow::{Mirror, Shadow};

use crate::memory::GuestMemory;
use crate::profile::{PAGE_BYTES, PAGE_SHIFT, Profile};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

/// This host gives the device one display.
pub const PORTS: u32 = 1;

/// Why something a guest said was not passed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    UnknownRegister,
    /// The register is only written while the ring is off, so that what is kept here is what the
    /// device goes by.
    RingIsOn,
    RingIsOff,
    OtherVersion,
    /// The ring's place or sizes are not ones a ring can have in this guest's memory.
    RingLayout,
    /// The device would read old commands again, without end.
    PositionWentBack,
    PositionTooFar,
    /// The position wraps at 2^32, and the ring's room does not divide 2^32.
    UnevenWrap,
    /// A length in the ring that is no command's.
    BrokenCommand,
    UnknownCommand,
    ShortCommand,
    NoSuchDisplay,
    DisplayInUse,
    DisplayNotInUse,
    NotGuestMemory,
    StatePageNotEmpty,
    TooManyModes,
    /// A command that changes what is kept here is only passed while no refused command holds the
    /// ring, as only then does the device say how it went.
    BehindARefusedCommand,
    NotTheKernelTask,
    TaskExists,
    NoTask,
    TaskLayout,
    PictureNotMapped,
    /// The swap's words for a table of colours, which the device would read through the task.
    ColourTable,
    /// A hand-over failed in the device, which leaves its display in a state no later call
    /// survives.
    Spoiled,
    MapperRingLength,
    MapperLayout,
    /// A surface that is not a plain picture in the guest's own pages. What the mapper is told
    /// of a surface goes to the host's kernel to make one.
    SurfaceDescription,
    EarlyIsOff,
    EarlyBlock,
    EarlyMode,
    TaskId,
    NoSuchTask,
    /// A command's length is not the one its own counts need.
    CommandLayout,
    OutsideTheTask,
    ResourceFlags,
    RingId,
    RingExists,
    NoSuchRing,
    ChildRingLayout,
    /// A command that changes what the guard keeps goes through the first ring, where it is
    /// passed alone.
    NotInAChildRing,
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownRegister => "no register this host passes on",
            Self::RingIsOn => "only written while the ring is off",
            Self::RingIsOff => "the ring is off",
            Self::OtherVersion => "not the version this host serves",
            Self::RingLayout => "no ring can lie there with those sizes",
            Self::PositionWentBack => "the position is below the one before",
            Self::PositionTooFar => "the position is further on than the ring has room for",
            Self::UnevenWrap => "the position wraps at 2^32, which the ring's room does not divide",
            Self::BrokenCommand => "the length there is no command's",
            Self::UnknownCommand => "no command this host passes on",
            Self::ShortCommand => "too short for that command",
            Self::NoSuchDisplay => "no display on that port",
            Self::DisplayInUse => "the display has a state page already",
            Self::DisplayNotInUse => "the display has no state page",
            Self::NotGuestMemory => "the page named is not the guest's memory",
            Self::StatePageNotEmpty => "a state page has to be handed over empty",
            Self::TooManyModes => "the state page counts more modes than a display takes",
            Self::BehindARefusedCommand => "not passed while a refused command holds the ring",
            Self::NotTheKernelTask => "not the kernel task",
            Self::TaskExists => "the task is defined already",
            Self::NoTask => "the kernel task is not defined",
            Self::TaskLayout => "the task's size or table is not one this host takes",
            Self::PictureNotMapped => "the kernel task's table doesn't map all of the picture",
            Self::ColourTable => "a swap with a table of colours",
            Self::Spoiled => "a hand-over failed in the device before",
            Self::MapperRingLength => "the mapper's ring is not whole pages of commands",
            Self::MapperLayout => "the mapper's ring or table can't lie there",
            Self::SurfaceDescription => "the surface described is no plain picture in the guest's memory",
            Self::EarlyIsOff => "the early framebuffer is off",
            Self::EarlyBlock => "the early framebuffer's block is not whole pages of the guest's memory",
            Self::EarlyMode => "no picture of that size and depth fits the early framebuffer's block",
            Self::TaskId => "no id a task of that kind can have",
            Self::NoSuchTask => "no task with that id is defined",
            Self::CommandLayout => "the command's length is not what its counts need",
            Self::OutsideTheTask => "names memory or an object beyond the task's end",
            Self::ResourceFlags => "a resource with flags this host doesn't know",
            Self::RingId => "no id a child ring can have",
            Self::RingExists => "the child ring is defined already",
            Self::NoSuchRing => "no child ring with that id is defined",
            Self::ChildRingLayout => "the child ring's descriptor is not one this host takes",
            Self::NotInAChildRing => "only passed in the first ring",
        })
    }
}

/// What it was that the guest said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    DeviceWrite {
        offset: u64,
        value: u32,
    },
    MapperWrite {
        offset: u64,
        value: u64,
    },
    /// A command in the first ring, by where it starts and the number it had.
    Command {
        position: u32,
        number: u32,
    },
    /// A command in a child ring.
    ChildCommand {
        ring: u32,
        position: u32,
        number: u32,
    },
    /// A command in the mapper's ring, by its place in the count of all commands and the mapping
    /// it names.
    MapperCommand {
        count: u64,
        id: u32,
    },
    /// The host's own new mode list, held back for what the guest wrote into the state page.
    ModeList,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    pub what: What,
    pub why: Why,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.what {
            What::DeviceWrite { offset, value } => write!(f, "write of {value:#x} to register {offset:#x}")?,
            What::MapperWrite { offset, value } => {
                write!(f, "write of {value:#x} to the mapper's register {offset:#x}")?
            }
            What::Command { position, number } => write!(f, "command {number} at {position:#x}")?,
            What::ChildCommand { ring, position, number } => {
                write!(f, "command {number} at {position:#x} of child ring {ring}")?
            }
            What::MapperCommand { count, id } => write!(f, "the mapper's command {count}, for mapping {id}")?,
            What::ModeList => f.write_str("a new mode list")?,
            What::Reset => f.write_str("a reset")?,
        }
        write!(f, ": {}", self.why)
    }
}

/// What to do with a write to one of the device's registers that the guard lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Write it to the device as it is.
    AsItIs,
    /// Write this to the device in its place: the number of the host's own page for what the
    /// guest named.
    Other(u32),
    /// It tells the device of new commands. Pass it in the steps [`Guard::next_step`] gives.
    Kick,
    /// It tells the device of new commands in a child ring. Call [`Guard::wake`] first, and
    /// write it if that says so.
    Wake,
    /// Nothing to tell the device: the guest has written no further.
    Nothing,
}

/// A cursor's picture, as a guest handed it over: four bytes a pixel, blue first, rows from the
/// top with no padding, and the place in it that points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub pixels: Vec<u8>,
}

/// A stretch of the ring to tell the device of in one write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The position to write. It may be the one before, where there is nothing to pass.
    pub upto: u32,
    /// Set where the stretch is one command that changes what the guard keeps. The caller looks
    /// at the device's fault register after the write and calls [`Guard::settle`].
    pub effect: Option<Effect>,
    /// Set where the guard can read no further. The rest of the guest's kick is not passed.
    pub stopped: Option<Refusal>,
}

/// What a command will have changed in the device if the device takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    HandOver {
        port: u32,
        page: u32,
    },
    TakeBack {
        port: u32,
    },
    DefineTask(KernelTask),
    DeleteTask,
    /// A task for a program's work. `root_page` is what the device writes into its root page
    /// for the task once it is defined.
    DefineUserTask {
        id: u32,
        length: u64,
        root_page: u32,
        /// Where the guest has the top of the task's table, and how many levels it has.
        table: u64,
        depth: u32,
    },
    DeleteUserTask {
        id: u32,
    },
    /// A child ring, with the guest's four pages for it and the index of its stamp cell.
    DefineRing {
        id: u32,
        pages: Vec<u32>,
        stamp_cell: u32,
    },
    DeleteRing {
        id: u32,
    },
}

impl Effect {
    /// Whether the fault register is what says how the command went, as soon as the device has
    /// read it. A user task comes and goes in the framework's helper process, some time after
    /// the device has read the command, and the device then says so in its root page. That also
    /// says it behind a refused command: one program's refused buffer must not keep another
    /// from starting.
    fn told_by_the_fault(&self) -> bool {
        matches!(self, Self::HandOver { .. } | Self::TakeBack { .. } | Self::DefineTask(_) | Self::DeleteTask)
    }
}

/// The kernel task, as the device read it when it was defined: it reads the table once, then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelTask {
    length: u64,
    /// The guest's root page for it.
    root_page: u32,
    /// For each of the task's pages, whether the table named a page of the guest's memory.
    mapped: Vec<bool>,
}

impl KernelTask {
    /// Whether the table mapped every byte of this stretch of the task.
    pub fn maps(&self, address: u64, length: u64) -> bool {
        let Some(end) = address.checked_add(length).filter(|&end| end <= self.length) else {
            return false;
        };
        if length == 0 {
            return true;
        }
        let pages = (address >> PAGE_SHIFT)..=((end - 1) >> PAGE_SHIFT);
        pages.into_iter().all(|page| self.mapped.get(page as usize).copied().unwrap_or(false))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Ring {
    on: bool,
    version: u32,
    bytes: u32,
    commands_at: u32,
    page: u32,
    root_page: u32,
    /// How far the device has been told the guest has written.
    passed: u32,
}

impl Ring {
    /// The bytes commands lie in. The ring wraps there, not at its memory's end.
    fn room(&self) -> u32 {
        self.bytes - self.commands_at
    }
}

/// A user task the device has.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UserTask {
    length: u64,
    /// Where the guest has the top of the task's table, and how many levels it has. The device
    /// and the helper process have a copy in the host's pages.
    table: u64,
    depth: u32,
}

/// A child ring, as the device was told.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Child {
    /// The guest's pages for the ring. The device has the host's.
    pages: Vec<u32>,
    /// How far the device has been told the guest has written.
    passed: u32,
}

/// The early framebuffer's bank, as the device was told.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Early {
    on: bool,
    page: u32,
    bytes: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Mapper {
    ring: u64,
    slots: u64,
    table: u64,
}

/// Reads a guest's words before the device does. One for each device.
pub struct Guard {
    profile: &'static Profile,
    shadow: Shadow,
    mirror: Arc<Mutex<Mirror>>,
    ring: Ring,
    kernel_task: Option<KernelTask>,
    user_tasks: BTreeMap<u32, UserTask>,
    children: BTreeMap<u32, Child>,
    early: Early,
    /// For each display, the number of the guest's page it was handed for its state, or 0. The
    /// device holds the host's page in its place.
    state_pages: [u32; PORTS as usize],
    spoiled: bool,
    mapper: Mapper,
    /// The picture of the last cursor command that was passed, until someone takes it.
    glyph: RefCell<Option<Glyph>>,
}

fn page_address(page: u32) -> u64 {
    u64::from(page) << PAGE_SHIFT
}

/// The number of a page of the host's own. [`Shadow`] keeps them where a number fits.
fn page_number(address: u64) -> u32 {
    (address >> PAGE_SHIFT) as u32
}

impl Guard {
    /// A guard for a device over this guest's memory, which is shown `shadow` beside it.
    pub fn new(profile: &'static Profile, guest: &GuestMemory, shadow: Shadow) -> Self {
        let mirror = Arc::new(Mutex::new(Mirror::new(profile, guest.clone(), shadow.clone())));
        Self {
            profile,
            shadow,
            mirror,
            ring: Ring::default(),
            kernel_task: None,
            user_tasks: BTreeMap::new(),
            children: BTreeMap::new(),
            early: Early::default(),
            state_pages: [0; PORTS as usize],
            spoiled: false,
            mapper: Mapper::default(),
            glyph: RefCell::new(None),
        }
    }

    /// The cursor's picture, if a command with a new one was passed since the last call. The
    /// framework's declared interface hands a host no cursor picture on this macOS, so a host that
    /// shows one reads it here, from the same words the device is given.
    pub fn take_glyph(&self) -> Option<Glyph> {
        self.glyph.borrow_mut().take()
    }

    /// Whether a hand-over failed in the device. From then on the device may be neither reset
    /// nor let go: see [`crate::Device`].
    pub fn spoiled(&self) -> bool {
        self.spoiled
    }

    /// What copies the device's words to the guest's pages. Whoever raises the device's
    /// interrupts for the guest calls [`Mirror::to_guest`] first.
    pub fn mirror(&self) -> Arc<Mutex<Mirror>> {
        Arc::clone(&self.mirror)
    }

    fn mirror_now(&self) -> MutexGuard<'_, Mirror> {
        self.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What a register reads as that the device was told another value for than the guest wrote:
    /// the guest finds its own page numbers there.
    pub fn reads_as(&self, offset: u64) -> Option<u32> {
        let registers = &self.profile.registers;
        if offset == registers.root_page {
            Some(self.ring.root_page)
        } else if offset == registers.ring_page {
            Some(self.ring.page)
        } else {
            None
        }
    }

    /// Judges a write to one of the device's registers.
    pub fn device_write(&mut self, memory: &GuestMemory, offset: u64, value: u32) -> Result<Pass, Refusal> {
        let registers = &self.profile.registers;
        let no = |why| Err(Refusal { what: What::DeviceWrite { offset, value }, why });
        let on = self.ring.on;

        if offset == registers.ring_on {
            return match value {
                0 => {
                    // Off puts both positions back to the start. The ring's place, sizes, version
                    // and root page stay, and the device forgets its tasks, child rings and the
                    // display's page.
                    self.ring.on = false;
                    self.ring.passed = 0;
                    self.mirror_now().set_ring(None);
                    self.forget_with_the_ring(memory);
                    Ok(Pass::AsItIs)
                }
                1 if on => no(Why::RingIsOn),
                1 => match self.ring_can_start(memory) {
                    Ok(()) => {
                        self.ring.on = true;
                        self.ring.passed = 0;
                        self.mirror_now().set_ring(Some(page_address(self.ring.page)));
                        Ok(Pass::AsItIs)
                    }
                    Err(why) => no(why),
                },
                _ => no(Why::UnknownRegister),
            };
        }
        if offset == registers.written {
            if !on {
                return no(Why::RingIsOff);
            }
            // The positions are counters that wrap at 2^32.
            let ahead = value.wrapping_sub(self.ring.passed);
            if ahead > self.ring.room() {
                return no(if ahead > u32::MAX / 2 { Why::PositionWentBack } else { Why::PositionTooFar });
            }
            // Where the room does not divide 2^32 the place would jump at the wrap, and what the
            // device does then is not known.
            if value < self.ring.passed && !(1u64 << 32).is_multiple_of(u64::from(self.ring.room())) {
                return no(Why::UnevenWrap);
            }
            return Ok(if value == self.ring.passed { Pass::Nothing } else { Pass::Kick });
        }
        if offset == registers.resume {
            return if on { Ok(Pass::AsItIs) } else { no(Why::RingIsOff) };
        }
        if offset == registers.wake || offset == registers.resume_child {
            if !self.children.contains_key(&value) {
                return no(Why::NoSuchRing);
            }
            return Ok(if offset == registers.wake { Pass::Wake } else { Pass::AsItIs });
        }
        if offset == registers.cursor_look {
            return match self.state_pages.get(value as usize) {
                None => no(Why::NoSuchDisplay),
                Some(0) => no(Why::DisplayNotInUse),
                Some(_) => Ok(Pass::AsItIs),
            };
        }
        if let Some(judged) = self.early_write(memory, offset, value) {
            return match judged {
                Ok(()) => Ok(Pass::AsItIs),
                Err(why) => no(why),
            };
        }

        // The device is told where the host's pages for the ring and the root are, whatever
        // the guest names: the guest's own are kept here, to copy from and to.
        let (kept, pass) = if offset == registers.version {
            if value != self.profile.version {
                return no(Why::OtherVersion);
            }
            (&mut self.ring.version, Pass::AsItIs)
        } else if offset == registers.ring_bytes {
            (&mut self.ring.bytes, Pass::AsItIs)
        } else if offset == registers.commands_at {
            (&mut self.ring.commands_at, Pass::AsItIs)
        } else if offset == registers.ring_page {
            (&mut self.ring.page, Pass::Other(page_number(self.shadow.ring())))
        } else if offset == registers.root_page {
            (&mut self.ring.root_page, Pass::Other(page_number(self.shadow.root())))
        } else {
            return no(Why::UnknownRegister);
        };
        if on {
            return no(Why::RingIsOn);
        }
        *kept = value;
        Ok(pass)
    }

    /// Judges a write to the early framebuffer's bank, or `None` for another register. The
    /// device shows the block of guest memory these words name, and reads it for every frame.
    fn early_write(&mut self, memory: &GuestMemory, offset: u64, value: u32) -> Option<Result<(), Why>> {
        let registers = &self.profile.registers.early;
        let shape = &self.profile.early;
        let early = &mut self.early;
        let block_is_the_guests =
            |page: u32, bytes: u32| memory.holds(page_address(page), u64::from(bytes).max(PAGE_BYTES));
        if offset == registers.on {
            return Some(match value {
                // Off, the bank keeps nothing.
                0 | 1 => {
                    *early = Early { on: value == 1, ..Early::default() };
                    Ok(())
                }
                _ => Err(Why::UnknownRegister),
            });
        }
        let judged = if offset == registers.mode_index {
            // Past the list's end the device says so in what it reads next.
            Ok(())
        } else if offset == registers.page {
            if value != 0 && block_is_the_guests(value, early.bytes) {
                early.page = value;
                Ok(())
            } else {
                Err(Why::EarlyBlock)
            }
        } else if offset == registers.bytes {
            let whole_pages = value != 0 && u64::from(value).is_multiple_of(PAGE_BYTES);
            if whole_pages && (early.page == 0 || block_is_the_guests(early.page, value)) {
                early.bytes = value;
                Ok(())
            } else {
                Err(Why::EarlyBlock)
            }
        } else if offset == registers.depth {
            if value == shape.depth { Ok(()) } else { Err(Why::EarlyMode) }
        } else if offset == registers.mode {
            let (width, height) = (value & 0xFFFF, value >> 16);
            let sides = 1..=shape.most_side;
            let needed = u64::from(width) * u64::from(height) * u64::from(shape.pixel_bytes);
            let fits = sides.contains(&width)
                && sides.contains(&height)
                && early.page != 0
                && needed <= u64::from(early.bytes);
            if fits { Ok(()) } else { Err(Why::EarlyMode) }
        } else {
            return None;
        };
        Some(if early.on { judged } else { Err(Why::EarlyIsOff) })
    }

    fn ring_can_start(&self, memory: &GuestMemory) -> Result<(), Why> {
        let shape = &self.profile.ring;
        let ring = &self.ring;
        if ring.version != self.profile.version {
            return Err(Why::OtherVersion);
        }
        let sizes_fit = ring.bytes <= shape.most_bytes
            && u64::from(ring.bytes).is_multiple_of(PAGE_BYTES)
            && ring.commands_at >= shape.least_commands_at
            && ring.commands_at.is_multiple_of(4)
            && ring.commands_at < ring.bytes
            && ring.room() >= shape.header_bytes;
        if !sizes_fit
            || !memory.holds(page_address(ring.page), u64::from(ring.bytes))
            || !memory.holds(page_address(ring.root_page), PAGE_BYTES)
        {
            return Err(Why::RingLayout);
        }
        Ok(())
    }

    /// Judges the host's own wish to give the display a new mode list. The device reads the
    /// count of the list before from the display's page when it writes a new one. That page is
    /// the host's, so the count is the device's own, and this holds unless the device itself
    /// wrote a count it can't read back.
    pub fn mode_list(&self) -> Result<(), Refusal> {
        for port in 0..PORTS {
            if self.state_pages[port as usize] != 0 && !self.mode_count_fits(port) {
                return Err(Refusal { what: What::ModeList, why: Why::TooManyModes });
            }
        }
        Ok(())
    }

    fn mode_count_fits(&self, port: u32) -> bool {
        let display = &self.profile.display;
        // The count is the word's lower half.
        self.shadow
            .memory()
            .word(self.shadow.state_page(port) + display.mode_count_at)
            .is_some_and(|word| word & 0xFFFF <= display.most_modes)
    }

    /// Judges a reset of the device and its mapper, and forgets what a reset makes the device
    /// forget: the ring, the task, the display's page and the mapper's ring.
    pub fn reset(&mut self) -> Result<(), Refusal> {
        if self.spoiled {
            return Err(Refusal { what: What::Reset, why: Why::Spoiled });
        }
        self.ring = Ring::default();
        self.kernel_task = None;
        self.user_tasks.clear();
        self.children.clear();
        self.early = Early::default();
        self.state_pages = [0; PORTS as usize];
        self.mapper = Mapper::default();
        self.mirror_now().clear();
        self.shadow.clear();
        Ok(())
    }
}
