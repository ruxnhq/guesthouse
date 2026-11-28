// SPDX-License-Identifier: GPL-2.0-or-later
//! The guest's frames for other processes, with `--frames NAME`: the Guesthouse app's screen and
//! remoteman's host piece read them by that bootstrap name, up to four readers at once.
//!
//! This is QEMU's `frame-export` for the device in this process, and speaks the same protocol
//! through the same files (`shim/rmframes/`, whose `rmframes.h` says how it works). Each frame
//! the display shows is drawn by the display itself, on the GPU, into one of a few IOSurfaces no
//! reader is reading, and every reader is told which one. A frame finds no free surface only when
//! the readers hold them all between them: it is skipped and counted, and never drawn over a
//! frame that is being read.
//!
//! The work runs on a thread of its own, so the device's thread only puts a word on a channel.
//! What readers say back is taken on rmframes' own queue, which also hears when one dies and
//! frees what it held.
//!
//! A reader that comes late is rmframes' business: frames are drawn into slots with no reader
//! there too, the newest is kept, and rmframes sends it to the newcomer alone, after its slots.
//! What is left here is a frame that found no free slot: the newest picture is then owed to the
//! readers, and handed out as soon as a slot is free again, [`LOOK_EVERY`] at most, even if the
//! guest shows nothing new.

use guesthouse_pvg_sys::FrameSource;
use std::ffi::{CString, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

/// How often the thread tries again to hand out a picture that found no free slot, while the
/// guest shows nothing new.
pub const LOOK_EVERY: Duration = Duration::from_millis(20);
const ERROR_ROOM: usize = 256;

unsafe extern "C" {
    fn pvg_export_open(name: *const c_char, error: *mut c_char, room: usize) -> *mut c_void;
    fn pvg_export_resize(export: *mut c_void, metal: *mut c_void, usage: u64, width: u32, height: u32)
    -> u32;
    fn pvg_export_acquire(export: *mut c_void, slot: *mut i32, generation: *mut u32) -> *mut c_void;
    fn pvg_export_publish(export: *mut c_void, slot: i32, generation: u32);
    fn pvg_export_cancel(export: *mut c_void, slot: i32, generation: u32);
    fn pvg_export_counts(export: *mut c_void, sent: *mut u64, dropped: *mut u64);
    fn pvg_export_readers(export: *mut c_void) -> u32;
    fn pvg_export_close(export: *mut c_void);
}

/// What the device tells the export, from its own threads.
pub enum Shown {
    /// The guest showed a new picture.
    Frame,
    /// The guest's picture has this size from now on.
    Size(u32, u32),
    /// The process is ending: the thread lets go of the display's picture and ends.
    Stop,
}

/// The bootstrap name, checked in, and the slots behind it.
pub struct Publisher(*mut c_void);

// SAFETY: the shim's calls other than open are made from the export's thread only, which takes
// the publisher with it; rmframes keeps what its own queue shares in atomics.
unsafe impl Send for Publisher {}

/// A slot that is the export's to draw into until it is published or cancelled.
struct Slot {
    index: i32,
    generation: u32,
    texture: *mut c_void,
}

impl Publisher {
    /// Checks in `name`. Fails if another process has it, so a second VM can't take a first
    /// one's screen.
    pub fn open(name: &str) -> Result<Self, String> {
        let name = CString::new(name).map_err(|_| "a frame name can't hold a zero byte".to_owned())?;
        let mut error = [0 as c_char; ERROR_ROOM];
        // SAFETY: the name ends in a zero, and the error buffer is as long as it says.
        let export = unsafe { pvg_export_open(name.as_ptr(), error.as_mut_ptr(), ERROR_ROOM) };
        if export.is_null() {
            // SAFETY: the shim wrote a string that ends in a zero into the buffer.
            let why = unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) };
            return Err(format!("no frame export: {}", why.to_string_lossy()));
        }
        Ok(Self(export))
    }

    /// New slots of this size on the display's own device. Returns how many there are.
    fn resize(&self, source: &FrameSource, width: u32, height: u32) -> u32 {
        // SAFETY: the publisher is live, and the device is the display's, alive while `source`
        // is.
        unsafe { pvg_export_resize(self.0, source.metal_device(), source.texture_usage(), width, height) }
    }

    fn acquire(&self) -> Option<Slot> {
        let (mut index, mut generation) = (0, 0);
        // SAFETY: the publisher is live, and both numbers are the call's to write.
        let texture = unsafe { pvg_export_acquire(self.0, &raw mut index, &raw mut generation) };
        (!texture.is_null()).then_some(Slot { index, generation, texture })
    }

    fn publish(&self, slot: Slot) {
        // SAFETY: the publisher is live, and the slot was handed out by it.
        unsafe { pvg_export_publish(self.0, slot.index, slot.generation) };
    }

    fn cancel(&self, slot: Slot) {
        // SAFETY: as above.
        unsafe { pvg_export_cancel(self.0, slot.index, slot.generation) };
    }

    /// Frame messages sent to readers, those a reader's queue had no room for, and how many
    /// readers there are.
    fn counts(&self) -> (u64, u64, u32) {
        let (mut sent, mut dropped) = (0, 0);
        // SAFETY: the publisher is live, and both numbers are the call's to write.
        let readers = unsafe {
            pvg_export_counts(self.0, &raw mut sent, &raw mut dropped);
            pvg_export_readers(self.0)
        };
        (sent, dropped, readers)
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        // SAFETY: the publisher is live and is not used again.
        unsafe { pvg_export_close(self.0) };
    }
}

/// What the export has done, for the control socket's `status`.
#[derive(Default)]
pub struct Counts {
    /// How many readers there are.
    pub readers: AtomicU64,
    /// Frame messages readers were sent: a frame sent to two readers counts two.
    pub published: AtomicU64,
    /// Frames of the guest's that found no free slot: the readers held them all.
    pub skipped: AtomicU64,
    /// Frame messages a reader's queue had no room for.
    pub dropped: AtomicU64,
    /// Frames the display would not draw into a slot, such as one of the size before.
    pub failed: AtomicU64,
}

impl Counts {
    /// The counts as the `status` line shows them.
    pub fn line(&self) -> String {
        format!(
            " readers {} published {} skipped {} dropped {} failed {}",
            self.readers.load(Ordering::Relaxed),
            self.published.load(Ordering::Relaxed),
            self.skipped.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed),
            self.failed.load(Ordering::Relaxed),
        )
    }
}

/// How a try at handing the picture out went.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Drawn into a slot, and sent to every reader there is.
    Published,
    /// The readers hold every slot.
    NoSlot,
    /// The display drew nothing: the picture changed size, or there is none yet.
    NotDrawn,
}

/// When the newest picture is owed to the readers. No calls in it, so it can be tested alone.
#[derive(Default)]
struct Plan {
    /// The display has a picture of the slots' size to give.
    picture: bool,
    /// The newest picture found no free slot, and has reached nobody.
    owed: bool,
}

impl Plan {
    fn resized(&mut self) {
        // The display's current picture may be of the old size until the guest shows a new one.
        self.picture = false;
        self.owed = false;
    }

    fn frame(&mut self) {
        self.picture = true;
    }

    /// Whether to try again now, while the guest shows nothing new.
    fn try_again(&self) -> bool {
        self.picture && self.owed
    }

    fn tried(&mut self, outcome: &Outcome) {
        match outcome {
            Outcome::Published => self.owed = false,
            Outcome::NoSlot => self.owed = true,
            Outcome::NotDrawn => {
                // Not again until the guest shows something: the display would say no again.
                self.picture = false;
                self.owed = false;
            }
        }
    }
}

/// The export's thread: the publisher, the display's picture and the counts.
struct Export {
    publisher: Publisher,
    source: FrameSource,
    counts: Arc<Counts>,
    plan: Plan,
}

impl Export {
    /// Draws the newest picture into a free slot and tells the readers.
    fn hand_out(&mut self) -> Outcome {
        let Some(slot) = self.publisher.acquire() else {
            return Outcome::NoSlot;
        };
        // SAFETY: the texture is the slot's, made on the display's device with the usage it
        // asked for, BGRA, and alive until the slots change, which only this thread does.
        if unsafe { self.source.draw_into(slot.texture) } {
            self.publisher.publish(slot);
            Outcome::Published
        } else {
            self.publisher.cancel(slot);
            Outcome::NotDrawn
        }
    }

    fn try_now(&mut self, from_guest: bool) {
        let outcome = self.hand_out();
        match outcome {
            Outcome::NoSlot if from_guest => self.counts.skipped.fetch_add(1, Ordering::Relaxed),
            Outcome::NotDrawn => self.counts.failed.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
        self.plan.tried(&outcome);
    }

    /// Takes rmframes' own counts, which its queue moves on after a frame is published.
    fn count(&self) {
        let (sent, dropped, readers) = self.publisher.counts();
        self.counts.published.store(sent, Ordering::Relaxed);
        self.counts.dropped.store(dropped, Ordering::Relaxed);
        self.counts.readers.store(u64::from(readers), Ordering::Relaxed);
    }

    fn resize(&mut self, width: u32, height: u32) {
        let made = self.publisher.resize(&self.source, width, height);
        eprintln!("pvg-host: {made} slots of {width} by {height} for the frame export");
        self.plan.resized();
    }

    /// Serves until the device has gone and the channel with it.
    fn run(mut self, shown: &Receiver<Shown>) {
        loop {
            match shown.recv_timeout(LOOK_EVERY) {
                Ok(Shown::Size(width, height)) => self.resize(width, height),
                Ok(Shown::Stop) => return,
                Ok(Shown::Frame) => {
                    // Of frames that piled up only the newest matters: they are all the
                    // display's current picture by now.
                    let mut next = None;
                    while let Ok(event) = shown.try_recv() {
                        if let Shown::Size(..) | Shown::Stop = event {
                            next = Some(event);
                            break;
                        }
                    }
                    match next {
                        Some(Shown::Stop) => return,
                        Some(Shown::Size(width, height)) => {
                            self.plan.frame();
                            self.try_now(true);
                            self.resize(width, height);
                        }
                        _ => {
                            self.plan.frame();
                            self.try_now(true);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.plan.try_again() {
                        self.try_now(false);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.count();
        }
    }
}

/// Starts the export's thread over the display's picture. It ends when it is told
/// [`Shown::Stop`], or when the device has gone; the process tells it first, so that the
/// display's picture is let go before the device.
pub fn start(
    publisher: Publisher,
    source: FrameSource,
    shown: Receiver<Shown>,
) -> (Arc<Counts>, std::thread::JoinHandle<()>) {
    let counts = Arc::new(Counts::default());
    let export = Export { publisher, source, counts: Arc::clone(&counts), plan: Plan::default() };
    let thread = std::thread::spawn(move || export.run(&shown));
    (counts, thread)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_tried_again_before_the_guest_shows_a_picture() {
        let mut plan = Plan::default();
        assert!(!plan.try_again());
        plan.tried(&Outcome::NoSlot);
        assert!(!plan.try_again(), "no picture of the slots' size yet");
        plan.frame();
        assert!(plan.try_again());
    }

    #[test]
    fn a_frame_that_found_no_slot_is_owed_until_it_is_published() {
        let mut plan = Plan::default();
        plan.frame();
        assert!(!plan.try_again(), "a frame handed out with the guest's word is owed nothing");
        plan.tried(&Outcome::NoSlot);
        assert!(plan.try_again());
        plan.tried(&Outcome::NoSlot);
        assert!(plan.try_again(), "still owed after another try");
        plan.tried(&Outcome::Published);
        assert!(!plan.try_again());
    }

    #[test]
    fn a_picture_the_display_would_not_draw_waits_for_the_next_frame() {
        let mut plan = Plan::default();
        plan.frame();
        plan.tried(&Outcome::NoSlot);
        plan.tried(&Outcome::NotDrawn);
        assert!(!plan.try_again(), "no retry the display would refuse again");
        plan.frame();
        plan.tried(&Outcome::NoSlot);
        assert!(plan.try_again(), "and owed once there is one");
    }

    #[test]
    fn new_slots_wait_for_a_picture_of_their_size() {
        let mut plan = Plan::default();
        plan.frame();
        plan.tried(&Outcome::NoSlot);
        plan.resized();
        assert!(!plan.try_again(), "the old size's picture is owed to nobody");
        plan.frame();
        assert!(!plan.try_again());
    }

    #[test]
    fn the_status_line_has_every_count() {
        let counts = Counts::default();
        counts.readers.store(2, Ordering::Relaxed);
        counts.published.store(3, Ordering::Relaxed);
        counts.skipped.store(2, Ordering::Relaxed);
        assert_eq!(counts.line(), " readers 2 published 3 skipped 2 dropped 0 failed 0");
    }
}
