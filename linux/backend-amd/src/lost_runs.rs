//! Runs the kernel retired before the card wrote their fence, and whose fault each one was.

use crate::space_vm::GuestFence;
use guesthouse_backend::{HostServices, SpaceId};
use std::collections::VecDeque;

/// A run the kernel ended before the card wrote its fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LostRun {
    pub fence: GuestFence,
    /// The kernel ended this run because the run itself hung, not because a reset others caused
    /// cancelled it: see [`hung`].
    pub hung: bool,
}

/// Whether the kernel's word on how a run ended (a sync file's status) says the run itself hung.
/// amdgpu ends a run that ran out of time with ETIME, and with ENODATA one whose waves it killed to
/// get the engine going again. A run that was only caught up in a reset, someone else's or another
/// run's of the same context, ends with ECANCELED, and is no fault of its own. A run whose end
/// couldn't be asked about is not counted either.
pub(crate) fn hung(status: Option<i32>) -> bool {
    matches!(status, Some(status) if status == -libc::ETIME || status == -libc::ENODATA)
}

/// The spaces whose own runs hung the card, oldest first, each once for each look at the runs that
/// found it so.
#[derive(Debug, Default)]
pub(crate) struct Blame {
    found: VecDeque<SpaceId>,
}

impl Blame {
    /// Takes in what one look at a space's runs found lost.
    pub(crate) fn looked_at(&mut self, space: SpaceId, lost: &[LostRun]) {
        if lost.iter().any(|run| run.hung) {
            self.found.push_back(space);
        }
    }

    pub(crate) fn take(&mut self) -> Option<SpaceId> {
        self.found.pop_front()
    }
}

/// Whether a run that retired never wrote its fence. A fence that can't be read is left alone.
pub(crate) fn lost(host: &dyn HostServices, fence: &GuestFence) -> bool {
    host.fence_value(fence.address).is_ok_and(|reached| reached < fence.value)
}

/// Stands in at the fence of each run the kernel retired before the card wrote it: one it reset, or
/// cancelled after a reset. `lost` are those runs, as [`lost`] finds them, each with the kernel's
/// word on how it ended. Returns them, with whether each hung.
pub(crate) fn stand_in_for_lost_runs(
    host: &mut dyn HostServices,
    lost: Vec<(GuestFence, Option<i32>)>,
) -> Vec<LostRun> {
    let lost: Vec<LostRun> =
        lost.into_iter().map(|(fence, status)| LostRun { fence, hung: hung(status) }).collect();
    for run in &lost {
        let fence = run.fence;
        log::warn!(
            "queue {:#x}: the kernel ended a run before it wrote fence value {:#x} ({}), so the host \
             writes it",
            fence.queue,
            fence.value,
            if run.hung { "the run hung" } else { "a reset cancelled it" }
        );
        if let Err(error) = host.write_u64(fence.address, fence.value) {
            log::warn!("standing in for a lost run: {error}");
        }
    }
    lost
}

#[cfg(test)]
mod tests {
    use super::*;

    const FENCE: GuestFence = GuestFence { queue: 1, address: 0x1000, value: 4 };

    fn lost_with(status: Option<i32>) -> Vec<LostRun> {
        vec![LostRun { fence: FENCE, hung: hung(status) }]
    }

    #[test]
    fn a_run_is_its_own_fault_only_when_the_kernel_says_it_hung() {
        assert!(hung(Some(-libc::ETIME)));
        assert!(hung(Some(-libc::ENODATA)));
        for status in [Some(-libc::ECANCELED), Some(-libc::EHWPOISON), Some(1), Some(0), None] {
            assert!(!hung(status), "{status:?}");
        }
    }

    #[test]
    fn a_lone_space_whose_runs_a_reset_cancelled_is_not_blamed() {
        let mut blame = Blame::default();
        blame.looked_at(SpaceId(3), &lost_with(Some(-libc::ECANCELED)));
        assert_eq!(blame.take(), None);
    }

    #[test]
    fn the_space_whose_run_hung_is_blamed_among_others_a_reset_cancelled() {
        let mut blame = Blame::default();
        blame.looked_at(SpaceId(3), &lost_with(Some(-libc::ECANCELED)));
        blame.looked_at(SpaceId(4), &lost_with(Some(-libc::ETIME)));
        blame.looked_at(SpaceId(5), &lost_with(Some(-libc::ECANCELED)));
        blame.looked_at(SpaceId(6), &[]);
        assert_eq!(blame.take(), Some(SpaceId(4)));
        assert_eq!(blame.take(), None);
    }

    #[test]
    fn a_space_is_blamed_once_for_each_look_that_found_its_run_hung() {
        let mut blame = Blame::default();
        let two = vec![LostRun { fence: FENCE, hung: true }, LostRun { fence: FENCE, hung: true }];
        blame.looked_at(SpaceId(7), &two);
        blame.looked_at(SpaceId(7), &lost_with(Some(-libc::ENODATA)));
        assert_eq!((blame.take(), blame.take(), blame.take()), (Some(SpaceId(7)), Some(SpaceId(7)), None));
    }
}
