//! Runs the kernel retired before the card wrote their fence.

use crate::space_vm::GuestFence;
use guesthouse_backend::{HostServices, SpaceId};
use std::collections::VecDeque;

/// Who a look at the runs on the card blames. A space that alone lost runs had a run that hung or
/// faulted the card: a fault of that process. Several at once are a reset that cancelled the runs
/// of all of them, and nothing here says whose run caused it, so none is blamed.
#[derive(Debug, Default)]
pub(crate) struct Blame {
    this_look: Vec<SpaceId>,
    found: VecDeque<SpaceId>,
}

impl Blame {
    pub(crate) fn lost(&mut self, space: SpaceId) {
        self.this_look.push(space);
    }

    pub(crate) fn end_look(&mut self) {
        if let [space] = self.this_look[..] {
            self.found.push_back(space);
        }
        self.this_look.clear();
    }

    pub(crate) fn take(&mut self) -> Option<SpaceId> {
        self.found.pop_front()
    }
}

/// Stands in at the fence of each run the kernel retired before the card wrote it: one it reset,
/// or cancelled after a reset. A run that went through wrote its fence before the kernel saw it end.
/// Returns the fences it stood in at.
pub(crate) fn stand_in_for_lost_runs(
    host: &mut dyn HostServices,
    retired: Vec<GuestFence>,
) -> Vec<GuestFence> {
    let mut lost = Vec::new();
    for fence in retired {
        let Ok(reached) = host.fence_value(fence.address) else { continue };
        if reached >= fence.value {
            continue;
        }
        lost.push(fence);
        log::warn!(
            "queue {:#x}: the kernel ended a run before it wrote fence value {:#x}, so the host writes it",
            fence.queue,
            fence.value
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

    #[test]
    fn only_a_space_that_alone_lost_runs_is_blamed() {
        let mut blame = Blame::default();
        blame.lost(SpaceId(3));
        blame.end_look();
        blame.lost(SpaceId(4));
        blame.lost(SpaceId(5));
        blame.end_look();
        blame.end_look();
        assert_eq!(blame.take(), Some(SpaceId(3)));
        assert_eq!(blame.take(), None, "a reset that cancelled several blames none of them");
    }
}
