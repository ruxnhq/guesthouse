//! Which slots each buffer still holds for the compositor, and which buffers wait for their release
//! before they can go.
//!
//! A hold belongs to the buffer that was attached, not to its slot: one slot can be on more than one
//! buffer (cut to different parts of the frame), and each is given back on its own `release`. A buffer
//! that still holds a slot is not destroyed when it is no longer wanted, as a destroyed buffer hears no
//! release and its slot would never go back to the producer; it waits, and goes when the release comes.

use std::collections::HashMap;
use std::hash::Hash;

pub type Release = Box<dyn FnOnce() + Send>;

pub struct BufferHolds<K, B> {
    held: HashMap<K, Vec<Release>>,
    /// Buffers no longer wanted, kept until the compositor lets go of them.
    retiring: HashMap<K, B>,
}

impl<K, B> Default for BufferHolds<K, B> {
    fn default() -> Self {
        Self { held: HashMap::new(), retiring: HashMap::new() }
    }
}

impl<K: Eq + Hash + Clone, B> BufferHolds<K, B> {
    /// The buffer was attached with this hold on its slot. A buffer attached again before it was
    /// released keeps both, and gives both back on its one release.
    pub fn hold(&mut self, buffer: K, release: Release) {
        self.held.entry(buffer).or_default().push(release);
    }

    pub fn is_held(&self, buffer: &K) -> bool {
        self.held.contains_key(buffer)
    }

    /// The buffer is no longer wanted. Gives it back to destroy now if it holds nothing, or keeps it
    /// until its release.
    #[must_use]
    pub fn retire(&mut self, key: K, buffer: B) -> Option<B> {
        if self.is_held(&key) {
            self.retiring.insert(key, buffer);
            None
        } else {
            Some(buffer)
        }
    }

    /// The compositor let go of the buffer: its slots go back, and a retiring buffer is given back to
    /// destroy.
    #[must_use]
    pub fn released(&mut self, buffer: &K) -> Option<B> {
        for release in self.held.remove(buffer).into_iter().flatten() {
            release();
        }
        self.retiring.remove(buffer)
    }

    /// Gives back every slot, for a surface that shows nothing now or is going, and every retiring
    /// buffer to destroy.
    #[must_use]
    pub fn release_all(&mut self) -> Vec<B> {
        for (_, releases) in self.held.drain() {
            for release in releases {
                release();
            }
        }
        self.retiring.drain().map(|(_, buffer)| buffer).collect()
    }

    #[cfg(test)]
    fn retiring(&self) -> usize {
        self.retiring.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A stand-in ring: how many slots are back with the producer.
    fn ring() -> Arc<AtomicUsize> {
        Arc::new(AtomicUsize::new(0))
    }

    fn hold_of(back: &Arc<AtomicUsize>) -> Release {
        let back = back.clone();
        Box::new(move || {
            back.fetch_add(1, Ordering::SeqCst);
        })
    }

    #[test]
    fn a_buffer_dropped_before_its_release_keeps_its_slot_until_the_release() {
        let back = ring();
        let mut holds = BufferHolds::<u32, &str>::default();
        holds.hold(1, hold_of(&back));
        // The cell moved: buffer 1 is not wanted, but the compositor may still show it.
        assert_eq!(holds.retire(1, "buffer 1"), None);
        assert_eq!(back.load(Ordering::SeqCst), 0);
        assert_eq!(holds.released(&1), Some("buffer 1"));
        assert_eq!(back.load(Ordering::SeqCst), 1);
        assert_eq!(holds.retiring(), 0);
    }

    #[test]
    fn a_buffer_that_holds_nothing_goes_at_once() {
        let mut holds = BufferHolds::<u32, &str>::default();
        assert_eq!(holds.retire(7, "never shown"), Some("never shown"));
        let back = ring();
        holds.hold(7, hold_of(&back));
        assert_eq!(holds.released(&7), None);
        assert_eq!(holds.retire(7, "shown and released"), Some("shown and released"));
        assert_eq!(back.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn two_buffers_of_one_slot_each_give_back_only_their_own_hold() {
        let back = ring();
        let mut holds = BufferHolds::<u32, ()>::default();
        // The same slot cut two ways, both attached in turn.
        holds.hold(1, hold_of(&back));
        holds.hold(2, hold_of(&back));
        let _ = holds.released(&1);
        assert_eq!(back.load(Ordering::SeqCst), 1);
        assert!(holds.is_held(&2));
        let _ = holds.released(&2);
        assert_eq!(back.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_buffer_attached_twice_gives_both_holds_back_on_its_release() {
        let back = ring();
        let mut holds = BufferHolds::<u32, ()>::default();
        holds.hold(3, hold_of(&back));
        holds.hold(3, hold_of(&back));
        let _ = holds.released(&3);
        assert_eq!(back.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_release_that_never_comes_strands_nothing_once_the_surface_lets_go() {
        let back = ring();
        let mut holds = BufferHolds::<u32, &str>::default();
        holds.hold(1, hold_of(&back));
        holds.hold(2, hold_of(&back));
        assert_eq!(holds.retire(1, "retiring"), None);
        assert_eq!(holds.release_all(), vec!["retiring"]);
        assert_eq!(back.load(Ordering::SeqCst), 2);
        assert!(!holds.is_held(&2));
    }

    /// A resize at frame rate: each frame comes in a new slot cut to a new cell, and the last buffer is
    /// dropped before its release is read. With three slots no slot may stay out.
    #[test]
    fn a_resize_at_frame_rate_never_runs_the_ring_dry() {
        let back = ring();
        let mut holds = BufferHolds::<u64, u64>::default();
        let mut on_surface: Option<u64> = None;
        let mut out = 0usize;
        for buffer in 0..1000u64 {
            holds.hold(buffer, hold_of(&back));
            out += 1;
            // The cell changed, so the buffer that was up is not wanted any more, and goes before the
            // compositor's release of it is read.
            if let Some(previous) = on_surface.replace(buffer) {
                assert_eq!(holds.retire(previous, previous), None);
                // The release, read in the next tick.
                assert_eq!(holds.released(&previous), Some(previous));
            }
            assert!(out - back.load(Ordering::SeqCst) <= 1, "slots stranded at buffer {buffer}");
        }
    }
}
