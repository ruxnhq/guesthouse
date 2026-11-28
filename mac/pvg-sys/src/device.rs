// SPDX-License-Identifier: GPL-2.0-or-later
//! The device a guest sees: Apple's, behind the guard.
//!
//! Every word a guest says goes through [`Device::write`] or [`Device::mapper_write`], is judged by
//! the [`Guard`], and reaches the framework only if the guard knows it to be good. What the guard
//! held back is told to the device's listener as [`Event::Refused`].

use crate::Error;
use crate::framework::FrameSource;
use crate::framework::{self, Framework};
use crate::guard::{Glyph, Guard, Mirror, Pass, Refusal, Shadow};
use crate::memory::GuestMemory;
use crate::profile::Profile;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const WORD_BYTES: u64 = 4;
const MAPPER_WORD_BYTES: u64 = 8;
/// The device reads a ring on a thread of its own after the write that tells it of new commands.
/// It gets this long before a command counts as failed.
const READ_WAIT: Duration = Duration::from_millis(500);
const READ_LOOK: Duration = Duration::from_micros(50);
/// How long a kick waits for a mode list's setter to be done before it tells the device all the same.
const GATE_WAIT: Duration = Duration::from_millis(100);
/// The first of the lines that say the device did not read the first ring in time.
const UNREAD_LINES: u64 = 8;
/// A user task is made in the framework's helper process, which is started for the first of them.
/// The device says in its root page when the task is there, and gets this long to.
const EFFECT_WAIT: Duration = Duration::from_secs(1);

/// One of a display's modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u16,
    pub height: u16,
    pub hertz: u16,
}

/// The display the device is made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// What the guest may show its user as the display's name.
    pub name: String,
    /// The display's width and height.
    pub millimetres: (u32, u32),
    pub serial: u32,
    /// The first is the one a guest should choose.
    pub modes: Vec<Mode>,
}

/// Which of the two devices raised an interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Device,
    Mapper,
}

/// What the device has to tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Interrupt {
        source: Source,
        vector: u32,
    },
    /// The guest's picture has a new size. `format` is its pixel format, as four characters.
    ModeChanged {
        width: u32,
        height: u32,
        format: u32,
    },
    /// The guest has shown a new picture, which [`Device::frame`] reads.
    NewFrame,
    CursorShown(bool),
    CursorMoved,
    /// The guest has a new picture for its cursor.
    CursorGlyph(Glyph),
    /// The guard did not pass something the guest said.
    Refused(Refusal),
}

/// A picture read from the display: four bytes a pixel, blue first, rows from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Whether this process has a device.
static ALIVE: AtomicBool = AtomicBool::new(false);

/// Apple's device, its display and its surface mapper for one guest.
///
/// A process has room for one at a time. The framework keeps something of the surface mapper for
/// the whole process: with two devices at work at once, one's display stops finding its mappings.
/// A program that serves several guests is several processes.
pub struct Device {
    profile: &'static Profile,
    guard: Guard,
    /// Copies what the device writes into the host's pages to the guest's. Shared with the
    /// framework's threads, which call it before they raise an interrupt.
    mirror: Arc<Mutex<Mirror>>,
    /// The guest's own memory. The device has it and the host's pages beside it.
    memory: GuestMemory,
    /// `None` only while the device is being dropped.
    framework: Option<Framework>,
    listener: Arc<dyn Fn(Event) + Send + Sync>,
    register_bytes: u64,
    /// How often the device did not read the first ring in time, for the first lines of it.
    unread: u64,
}

/// Two words of a display's state page that only a line reads: the mode list's number, and
/// whether it waits for the guest to acknowledge it, as they were seen on the device.
const STATE_SEQUENCE: u64 = 0x200;
const STATE_LIST_STATE: u64 = 0xfc0;

impl Device {
    /// Makes the device over a guest's memory, with one display.
    ///
    /// `listener` hears of interrupts, of the display, and of what the guard refused. It is called
    /// from threads of the framework's and from inside this device's own calls, so it must not
    /// call the device, and should not take long.
    ///
    /// A macOS build with no profile is refused before the framework is called. So is a second
    /// device while this process has one: see [`Device`].
    pub fn new(
        memory: GuestMemory,
        config: &Config,
        listener: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        if ALIVE.swap(true, Ordering::SeqCst) {
            return Err(Error::Framework("this process has a device already".to_owned()));
        }
        Self::make(memory, config, listener).inspect_err(|_| ALIVE.store(false, Ordering::SeqCst))
    }

    fn make(
        memory: GuestMemory,
        config: &Config,
        listener: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let build = framework::host_build()
            .ok_or_else(|| Error::Framework("no macOS build to be read".to_owned()))?;
        let profile = Profile::for_build(&build)?;
        if config.modes.is_empty() || config.modes.len() > profile.display.most_modes as usize {
            return Err(Error::Framework(format!(
                "a display takes 1 to {} modes",
                profile.display.most_modes
            )));
        }
        let listener: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(listener);
        let sink = Arc::clone(&listener);
        let shadow = Shadow::above(&memory, profile)?;
        let seen_by_the_device = memory.joined(shadow.memory())?;
        let seen_by_the_mapper = memory.joined(shadow.mapper_memory())?;
        let guard = Guard::new(profile, &memory, shadow);
        let mirror = guard.mirror();
        let before_interrupts = Arc::clone(&mirror);
        let framework = Framework::new(
            &seen_by_the_device,
            // The mapper makes surfaces of the guest's pages. Its ring, table, descriptions and
            // page lists are the host's, in a range of its own.
            &seen_by_the_mapper,
            config,
            profile.mapper.register_bytes,
            Box::new(move |event| {
                // A stamp, or a display's event, has to be in the guest's memory before the
                // guest hears of it.
                // So does what the mapper wrote of how a command went.
                if let Event::Interrupt { source, .. } = event {
                    let mut mirror =
                        before_interrupts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    match source {
                        Source::Device => mirror.to_guest(),
                        Source::Mapper => mirror.mapper_to_guest(None),
                    }
                }
                sink(event);
            }),
        )?;
        Ok(Self {
            profile,
            guard,
            mirror,
            memory,
            framework: Some(framework),
            listener,
            register_bytes: Framework::register_bytes(),
            unread: 0,
        })
    }

    /// How many bytes of registers the device has, and how many the mapper.
    pub fn register_bytes(&self) -> u64 {
        self.register_bytes
    }

    pub fn mapper_register_bytes(&self) -> u64 {
        self.profile.mapper.register_bytes
    }

    fn framework(&mut self) -> &mut Framework {
        self.framework.as_mut().expect("the framework is there until the device is dropped")
    }

    fn refused(&self, refusal: Refusal) {
        (self.listener)(Event::Refused(refusal));
    }

    fn is_register(offset: u64, word: u64, bytes: u64) -> bool {
        offset.is_multiple_of(word) && offset.checked_add(word).is_some_and(|end| end <= bytes)
    }

    /// Reads one of the device's registers. A place that is no register reads as zero.
    pub fn read(&mut self, offset: u64) -> u32 {
        if !Self::is_register(offset, WORD_BYTES, self.register_bytes) {
            return 0;
        }
        if let Some(value) = self.guard.reads_as(offset) {
            return value;
        }
        let value = self.framework().read(offset);
        // What the register says is there may only just have been written by the device.
        self.mirror().to_guest();
        value
    }

    fn mirror(&self) -> MutexGuard<'_, Mirror> {
        self.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Writes one of the device's registers, if the guard lets it through.
    pub fn write(&mut self, offset: u64, value: u32) {
        if !Self::is_register(offset, WORD_BYTES, self.register_bytes) {
            return;
        }
        // The words a guest writes into a display's page for the device to read.
        self.mirror().from_guest();
        match self.guard.device_write(&self.memory, offset, value) {
            Ok(Pass::AsItIs) => self.framework().write(offset, value),
            Ok(Pass::Other(ours)) => self.framework().write(offset, ours),
            Ok(Pass::Kick) => self.kick(value),
            Ok(Pass::Wake) => {
                let mut refused = Vec::new();
                let woken = self.guard.wake(&self.memory, value, &mut refused);
                for refusal in refused {
                    self.refused(refusal);
                }
                match woken {
                    Ok(()) => self.framework().write(offset, value),
                    Err(refusal) => self.refused(refusal),
                }
                self.tell_of_a_glyph();
            }
            Ok(Pass::Nothing) => {}
            Err(refusal) => self.refused(refusal),
        }
    }

    fn tell_of_a_glyph(&self) {
        if let Some(glyph) = self.guard.take_glyph() {
            (self.listener)(Event::CursorGlyph(glyph));
        }
    }

    /// A handle on the display's picture for another thread: one that draws it into a window
    /// while this device goes on serving its guest.
    pub fn frame_source(&mut self) -> FrameSource {
        self.framework().frame_source()
    }

    /// Whether a refused command holds the first ring. The fault register alone is up for any
    /// ring's, and a child ring's refused command is its own sender's business: it must not
    /// count against a command in the first ring.
    fn first_ring_faulted(&mut self) -> bool {
        let registers = &self.profile.registers;
        self.framework().read(registers.fault) != 0
            && self.framework().read(registers.fault_at) != self.profile.child.no_fault
    }

    /// Tells the device of the guest's new commands, a stretch at a time.
    fn kick(&mut self, target: u32) {
        let registers = &self.profile.registers;
        let mut refused = Vec::new();
        loop {
            let faulted = self.first_ring_faulted();
            let at = self.framework().read(registers.read);
            let step = self.guard.next_step(&self.memory, target, at, faulted, &mut refused);
            for refusal in refused.drain(..) {
                self.refused(refusal);
            }
            let mut read = self.framework().read(registers.read) == step.upto;
            if !read {
                // The device is not told of new commands while a mode list is being set: about
                // once in 100 lists the setter then did not come back and the device stopped
                // before the guest's next swap (2026-11-27, macOS 27.2.1). A setter that does not
                // come back holds the gate; then the commands go after GATE_WAIT all the same.
                let gated = self.framework().take_gate(GATE_WAIT);
                self.framework().write(registers.written, step.upto);
                let since = Instant::now();
                loop {
                    read = self.framework().read(registers.read) == step.upto;
                    if read || since.elapsed() > READ_WAIT {
                        break;
                    }
                    std::thread::sleep(READ_LOOK);
                }
                if gated {
                    self.framework().give_gate();
                }
                if !read {
                    let at = self.framework().read(registers.read);
                    self.unread += 1;
                    if self.unread <= UNREAD_LINES {
                        let numbers = self.guard.first_ring_numbers(at, step.upto, 8);
                        eprintln!(
                            "pvg-sys: the device did not read the first ring up to {:#x} in {} ms: it is at {at:#x}, before commands {numbers:?}",
                            step.upto,
                            READ_WAIT.as_millis()
                        );
                    }
                }
            }
            if let Some(effect) = step.effect {
                // A command the device never came to counts as failed, and so does one whose
                // outcome the device doesn't show in time.
                let since = Instant::now();
                let failed = !read
                    || loop {
                        let now = self.first_ring_faulted();
                        if let Some(failed) = self.guard.failed(&effect, faulted, now) {
                            break failed;
                        }
                        if since.elapsed() > EFFECT_WAIT {
                            break true;
                        }
                        std::thread::sleep(READ_LOOK);
                    };
                self.guard.settle(&self.memory, effect, failed);
            }
            self.mirror().to_guest();
            self.tell_of_a_glyph();
            if let Some(refusal) = step.stopped {
                self.refused(refusal);
                break;
            }
            if step.upto == target {
                break;
            }
        }
    }

    /// Reads one of the mapper's registers, which are 64 bits.
    pub fn mapper_read(&mut self, offset: u64) -> u64 {
        if !Self::is_register(offset, MAPPER_WORD_BYTES, self.profile.mapper.register_bytes) {
            return 0;
        }
        if let Some(value) = self.guard.mapper_reads_as(offset) {
            return value;
        }
        let value = self.framework().mapper_read(offset);
        // A guest learns from this count that a command is done, and then reads its outcome.
        if offset == self.profile.mapper.done {
            self.mirror().mapper_to_guest(Some(value));
        }
        value
    }

    /// Writes one of the mapper's registers, if the guard lets it through.
    pub fn mapper_write(&mut self, offset: u64, value: u64) {
        if !Self::is_register(offset, MAPPER_WORD_BYTES, self.profile.mapper.register_bytes) {
            return;
        }
        let written = self.profile.mapper.written;
        let written_now = self.framework().mapper_read(written);
        let mut refused = Vec::new();
        let judged = self.guard.mapper_write(&self.memory, offset, value, written_now, &mut refused);
        for refusal in refused {
            self.refused(refusal);
        }
        match judged {
            Ok(ours) => self.framework().mapper_write(offset, ours),
            Err(refusal) => self.refused(refusal),
        }
    }

    /// Whether a hand-over failed in the device. Such a device can't be reset, and dropping it
    /// keeps the framework's objects instead of letting them go, either of which would end the
    /// process. The guest's graphics are over until its host process is started anew.
    pub fn spoiled(&self) -> bool {
        self.guard.spoiled()
    }

    /// Resets the device and its mapper, as a guest's reset must.
    pub fn reset(&mut self) -> Result<(), Error> {
        if self.guard.reset().is_err() {
            return Err(Error::Spoiled);
        }
        self.framework().reset()
    }

    /// Gives the display a new mode list. The guest hears of it and reads it from its state page.
    /// The guard's part is done here, before it returns; the framework's on a serial queue of the
    /// host's own, and `done` hears there whether the framework took the list. The framework's
    /// setter may wait for the guest, so neither the thread that serves the guest nor the
    /// display's queue runs it or waits for it.
    pub fn set_modes(&mut self, modes: &[Mode], done: crate::framework::ModesDone) -> Result<(), Error> {
        if modes.is_empty() || modes.len() > self.profile.display.most_modes as usize {
            return Err(Error::Framework(format!(
                "a display takes 1 to {} modes",
                self.profile.display.most_modes
            )));
        }
        self.guard.mode_list().map_err(Error::Refused)?;
        self.framework().set_modes(modes, done)
    }

    /// What a line says of the display's state page when a mode list stands: the words that
    /// count events, number and count the list and say whether it waits for the guest, in the
    /// host's page and the guest's, from any thread.
    pub fn state_look(&self) -> impl Fn() -> String + Send + 'static {
        let mirror = Arc::clone(&self.mirror);
        let display = &self.profile.display;
        let places = [display.events_at, STATE_SEQUENCE, display.mode_count_at, STATE_LIST_STATE];
        move || {
            let mirror = mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match mirror.state_words(0, &places) {
                None => "no state page".to_owned(),
                Some((host, guest)) => format!(
                    "state page events, sequence, count, list state: host's {host:x?}, guest's {guest:x?}"
                ),
            }
        }
    }

    /// The display's current picture, or `None` before the guest has shown one.
    pub fn frame(&mut self) -> Result<Option<Frame>, Error> {
        self.framework().frame()
    }

    /// How many pictures the guest has presented, and how many were read here.
    pub fn presents(&mut self) -> (u64, u64) {
        self.framework().presents()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some(framework) = self.framework.take() {
            // Letting a display go that a failed hand-over left half attached ends the process.
            // What is kept stays this process's one device: there is no making another.
            if self.guard.spoiled() {
                framework.leak();
            } else {
                framework.free();
                ALIVE.store(false, Ordering::SeqCst);
            }
        }
    }
}
