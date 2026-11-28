//! A slot that more than one reader shows at once, as when GTK draws a frame and the app's own
//! surface then takes the same one over. The producer gets it back when the last of them is done.

use super::frames::SlotFrame;
use super::own_surface::PQ_TRANSFER;
use super::socket_frames::LINEAR;
use guesthouse_frame_socket::{Descriptor, Format};
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

type Release = Box<dyn FnOnce() + Send>;

/// Runs the slot's release when the last hold on it goes, however it goes: a reader that is dropped
/// without ever saying it has finished still gives its hold back.
struct LastOut(Mutex<Option<Release>>);

impl Drop for LastOut {
    fn drop(&mut self) {
        let release = match self.0.get_mut() {
            Ok(release) => release.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(release) = release {
            release();
        }
    }
}

pub struct SlotHold(Arc<LastOut>);

impl SlotHold {
    pub fn new(release: Release) -> Self {
        Self(Arc::new(LastOut(Mutex::new(Some(release)))))
    }

    /// One reader's own release, to run when that reader has finished with the slot.
    pub fn for_reader(&self) -> Release {
        let held = self.0.clone();
        Box::new(move || drop(held))
    }
}

/// A frame kept with a hold on its slot, to give to a reader now or later, and as often as it
/// takes: each reader gets a hold of its own, and one that is turned away just gives it back.
pub struct HeldFrame {
    file: Arc<OwnedFd>,
    descriptor: Descriptor,
    generation: u32,
    slot: u32,
    hold: SlotHold,
}

impl HeldFrame {
    pub fn new(frame: SlotFrame) -> Self {
        let SlotFrame { file, descriptor, generation, slot, release } = frame;
        Self { file, descriptor, generation, slot, hold: SlotHold::new(release) }
    }

    /// The picture's width and height in pixels.
    pub fn size(&self) -> (i32, i32) {
        (self.descriptor.width as i32, self.descriptor.height as i32)
    }

    pub fn format(&self) -> Format {
        self.descriptor.format
    }

    /// Whether the frame is laid out in a card's tiles rather than in rows.
    pub fn is_tiled(&self) -> bool {
        self.descriptor.format.modifier != LINEAR
    }

    /// Whether the frame is HDR, as PQ.
    pub fn is_pq(&self) -> bool {
        self.descriptor.colour.transfer == PQ_TRANSFER
    }

    pub fn for_reader(&self) -> SlotFrame {
        SlotFrame {
            file: self.file.clone(),
            descriptor: self.descriptor,
            generation: self.generation,
            slot: self.slot,
            release: self.hold.for_reader(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn counted() -> (SlotHold, Arc<AtomicUsize>) {
        let released = Arc::new(AtomicUsize::new(0));
        let count = released.clone();
        let hold = SlotHold::new(Box::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        }));
        (hold, released)
    }

    #[test]
    fn the_slot_goes_back_once_when_the_last_reader_has_finished() {
        let (hold, released) = counted();
        let (gtk, compositor) = (hold.for_reader(), hold.for_reader());
        drop(hold);
        gtk();
        assert_eq!(released.load(Ordering::SeqCst), 0);
        compositor();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_order_the_readers_finish_in_does_not_matter() {
        let (hold, released) = counted();
        let (gtk, compositor) = (hold.for_reader(), hold.for_reader());
        compositor();
        gtk();
        assert_eq!(released.load(Ordering::SeqCst), 0);
        drop(hold);
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_reader_dropped_without_finishing_still_gives_the_slot_back() {
        let (hold, released) = counted();
        let (gtk, waiting) = (hold.for_reader(), hold.for_reader());
        drop(hold);
        drop(waiting);
        assert_eq!(released.load(Ordering::SeqCst), 0);
        drop(gtk);
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn readers_that_come_and_go_leave_the_slot_held_until_the_last() {
        let (hold, released) = counted();
        for _ in 0..10_000 {
            drop(hold.for_reader());
            hold.for_reader()();
        }
        assert_eq!(released.load(Ordering::SeqCst), 0);
        drop(hold);
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_slot_with_one_reader_goes_back_when_it_has_finished() {
        let (hold, released) = counted();
        let only = hold.for_reader();
        drop(hold);
        only();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }
}
