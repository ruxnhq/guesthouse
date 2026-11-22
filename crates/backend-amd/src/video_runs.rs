//! The video runs of one space that are on the card, and the places in its video buffer they hold.
//!
//! A video run isn't waited for. It is sent, and the kernel signals a sync object when it has
//! retired. The video ring writes no fence of the guest's (the firmware's commands have no packet of
//! the daemon's at their end, as the graphics engines' do), so when the kernel says a run has
//! retired the host writes the queue's fence itself. Until then the run keeps its place in the
//! buffer its commands were copied to: see [`crate::video_ib`]. Kept apart from the sync objects so
//! it can be tested without a card.

use crate::fence_ib::Slots;
use crate::space_vm::GuestFence;
use crate::video_ib::SLOTS;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// One run on the card: the place its commands are in, what says it has retired, and the fence the
/// host writes then.
#[derive(Debug)]
struct OnCard<T> {
    slot: usize,
    retired: T,
    queue: u32,
    /// None for the daemon's own work, as a closing, and once its queue has gone, as the fence's
    /// page may then be another queue's.
    fence: Option<GuestFence>,
    sent: Instant,
}

/// A run the kernel has said has retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retired {
    pub queue: u32,
    /// The fence the host is to write for it, if any.
    pub fence: Option<GuestFence>,
    /// From the run being sent to it being seen to have retired.
    pub took: Duration,
}

/// See the module documentation. `T` is what the kernel signals, a sync object on a card.
#[derive(Debug)]
pub struct VideoRuns<T> {
    slots: Slots,
    on_card: VecDeque<OnCard<T>>,
}

impl<T> Default for VideoRuns<T> {
    fn default() -> Self {
        Self { slots: Slots::new(SLOTS), on_card: VecDeque::new() }
    }
}

impl<T> VideoRuns<T> {
    /// A free place for a run's commands, or `None` while every place's run is on the card.
    pub fn take_slot(&mut self) -> Option<usize> {
        self.slots.take()
    }

    /// Frees a place whose run won't be sent.
    pub fn give_back(&mut self, slot: usize) {
        self.slots.give_back(slot);
    }

    /// Notes a run that was sent from `slot`.
    pub fn sent(&mut self, slot: usize, retired: T, queue: u32, fence: Option<GuestFence>, now: Instant) {
        self.on_card.push_back(OnCard { slot, retired, queue, fence, sent: now });
    }

    /// Whether any run is still on the card.
    pub fn any_on_card(&self) -> bool {
        !self.on_card.is_empty()
    }

    /// Takes off the runs `signalled` says have retired, oldest first, freeing their places.
    /// Returns each with what signalled it, which the caller lets go. A run that is still on the
    /// card keeps its place, also when a later one has retired.
    pub fn retire(&mut self, mut signalled: impl FnMut(&T) -> bool, now: Instant) -> Vec<(T, Retired)> {
        let mut retired = Vec::new();
        let mut still = VecDeque::with_capacity(self.on_card.len());
        for run in self.on_card.drain(..) {
            if signalled(&run.retired) {
                self.slots.give_back(run.slot);
                let took = now.saturating_duration_since(run.sent);
                retired.push((run.retired, Retired { queue: run.queue, fence: run.fence, took }));
            } else {
                still.push_back(run);
            }
        }
        self.on_card = still;
        retired
    }

    /// Forgets the fences of a queue that has gone. Its runs keep their places until they retire.
    pub fn forget_queue(&mut self, queue: u32) {
        for run in self.on_card.iter_mut().filter(|run| run.queue == queue) {
            run.fence = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEUE: u32 = 0x4000_0010;
    const OTHER: u32 = 0x4000_0020;

    fn fence(value: u64) -> Option<GuestFence> {
        Some(GuestFence { queue: QUEUE, address: 0x1000, value })
    }

    #[test]
    fn a_place_is_taken_by_each_run_on_the_card_and_none_is_left_when_all_are() {
        let mut runs = VideoRuns::<u32>::default();
        let now = Instant::now();
        let mut taken = Vec::new();
        while let Some(slot) = runs.take_slot() {
            runs.sent(slot, slot as u32, QUEUE, fence(slot as u64 + 1), now);
            taken.push(slot);
        }
        assert_eq!(taken.len(), SLOTS);
        taken.sort_unstable();
        taken.dedup();
        assert_eq!(taken.len(), SLOTS, "no place was given twice");
        assert!(runs.any_on_card());
        // Nothing has retired, so nothing is freed.
        assert!(runs.retire(|_| false, now).is_empty());
        assert_eq!(runs.take_slot(), None);
    }

    #[test]
    fn a_retired_run_frees_its_place_and_says_its_fence_and_how_long_it_took() {
        let mut runs = VideoRuns::default();
        let sent = Instant::now();
        let slot = runs.take_slot().unwrap();
        runs.sent(slot, "first", QUEUE, fence(7), sent);
        let later = sent + Duration::from_micros(1900);
        let retired = runs.retire(|_| true, later);
        assert_eq!(
            retired,
            [("first", Retired { queue: QUEUE, fence: fence(7), took: Duration::from_micros(1900) })]
        );
        assert!(!runs.any_on_card());
        assert_eq!(runs.take_slot(), Some(slot), "the place is free again");
    }

    #[test]
    fn a_run_still_on_the_card_keeps_its_place_when_a_later_one_retires() {
        let mut runs = VideoRuns::default();
        let now = Instant::now();
        let (first, second, third) =
            (runs.take_slot().unwrap(), runs.take_slot().unwrap(), runs.take_slot().unwrap());
        runs.sent(first, 1, QUEUE, fence(1), now);
        runs.sent(second, 2, OTHER, None, now);
        runs.sent(third, 3, QUEUE, fence(2), now);
        let retired: Vec<u32> =
            runs.retire(|&token| token != 1, now).into_iter().map(|(token, _)| token).collect();
        assert_eq!(retired, [2, 3], "oldest first");
        let free: Vec<usize> = std::iter::from_fn(|| runs.take_slot()).collect();
        assert!(!free.contains(&first) && free.contains(&second) && free.contains(&third));
        assert_eq!(runs.retire(|_| true, now).len(), 1);
    }

    #[test]
    fn a_queue_that_has_gone_gets_no_fence_written_and_its_runs_keep_their_places_until_they_retire() {
        let mut runs = VideoRuns::default();
        let now = Instant::now();
        let mine = runs.take_slot().unwrap();
        runs.sent(mine, 1, QUEUE, fence(4), now);
        let theirs = runs.take_slot().unwrap();
        runs.sent(theirs, 2, OTHER, Some(GuestFence { queue: OTHER, address: 0x2000, value: 9 }), now);
        runs.forget_queue(QUEUE);
        let retired = runs.retire(|_| true, now);
        assert_eq!(retired[0].1.fence, None);
        assert_eq!(retired[1].1.fence.map(|fence| fence.value), Some(9));
    }

    #[test]
    fn a_place_whose_run_was_not_sent_is_free_again() {
        let mut runs = VideoRuns::<u32>::default();
        let slot = runs.take_slot().unwrap();
        runs.give_back(slot);
        assert_eq!(runs.take_slot(), Some(slot));
        assert!(!runs.any_on_card());
    }
}
