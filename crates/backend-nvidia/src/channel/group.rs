//! The channel groups that the queues of one guest context share.
//!
//! NVIDIA's own kernel driver makes one channel group for a context, and every hardware queue of
//! the context is a channel in it, on the subcontext its private data names. This keeps who is in
//! each group, so that the last channel to leave takes its subcontext and the group with it.

use crate::rm::Handle;
use std::collections::{HashMap, HashSet};

/// A channel's place in its context's group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seat {
    /// The group's synchronous subcontext, with every class.
    Drawing,
    /// An asynchronous subcontext, on the second runqueue, with the compute class alone.
    Compute { index: u8, prefer_lower: bool },
}

impl Seat {
    /// Which of the group's subcontexts this is: the synchronous one is the first.
    pub(crate) fn slot(self) -> u8 {
        match self {
            Self::Compute { index, .. } => index,
            Self::Drawing => 0,
        }
    }
}

#[derive(Debug, Default)]
struct ContextGroup {
    group: Handle,
    members: u32,
    /// Each subcontext made so far, and how many channels are on it.
    shares: HashMap<u8, (Handle, u32)>,
}

/// What a channel that left was the last to use, for the caller to free, in this order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Left {
    pub share: Option<Handle>,
    pub group: Option<Handle>,
}

/// The groups, by the guest's handle of the context.
#[derive(Debug, Default)]
pub struct ContextGroups {
    /// Off, every channel has a group of its own.
    pub enabled: bool,
    groups: HashMap<u32, ContextGroup>,
    /// Contexts with a queue the RM refused a seat, whose queues are then spread over groups after
    /// all and wait for each other across them.
    split: HashSet<u32>,
}

impl ContextGroups {
    /// The context's group, once a channel has made it.
    pub fn group(&self, context: u32) -> Option<Handle> {
        self.groups.get(&context).map(|found| found.group)
    }

    /// One of the group's subcontexts, once a channel has made it.
    pub fn share(&self, context: u32, slot: u8) -> Option<Handle> {
        self.groups.get(&context)?.shares.get(&slot).map(|&(share, _)| share)
    }

    /// How many channels the context's group has.
    pub fn members(&self, context: u32) -> u32 {
        self.groups.get(&context).map_or(0, |found| found.members)
    }

    /// A channel is in the group now, on this subcontext.
    pub fn join(&mut self, context: u32, group: Handle, slot: u8, share: Handle) {
        let found = self.groups.entry(context).or_default();
        found.group = group;
        found.members += 1;
        found.shares.entry(slot).or_insert((share, 0)).1 += 1;
    }

    /// One of the context's queues was refused its seat, and has a group of its own.
    pub fn note_split(&mut self, context: u32) {
        self.split.insert(context);
    }

    /// Whether the context's queues are in more than one group, for all that it has a shared one.
    pub fn is_split(&self, context: u32) -> bool {
        self.split.contains(&context)
    }

    /// A channel has left.
    pub fn leave(&mut self, context: u32, slot: u8) -> Left {
        let Some(found) = self.groups.get_mut(&context) else { return Left::default() };
        let mut left = Left::default();
        if let Some((share, members)) = found.shares.get_mut(&slot) {
            *members = members.saturating_sub(1);
            if *members == 0 {
                left.share = Some(*share);
                found.shares.remove(&slot);
            }
        }
        found.members = found.members.saturating_sub(1);
        if found.members == 0 {
            left.group = Some(found.group);
            self.groups.remove(&context);
            self.split.remove(&context);
        }
        left
    }

    pub fn len(&self) -> usize {
        self.groups.len()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_has_no_group_until_a_channel_joins() {
        let mut groups = ContextGroups::default();
        assert_eq!(groups.group(7), None);
        groups.join(7, 100, 0, 101);
        assert_eq!(groups.group(7), Some(100));
        assert_eq!(groups.share(7, 0), Some(101));
        assert_eq!(groups.share(7, 1), None);
        assert_eq!(groups.group(8), None, "another context has its own");
    }

    #[test]
    fn channels_that_name_one_subcontext_share_it() {
        let mut groups = ContextGroups::default();
        groups.join(7, 100, 0, 101);
        groups.join(7, 100, 1, 102);
        groups.join(7, 100, 1, 102);
        assert_eq!(groups.members(7), 3);
        assert_eq!(groups.leave(7, 1), Left::default(), "another channel is still on it");
        assert_eq!(groups.leave(7, 1), Left { share: Some(102), group: None });
        assert_eq!(groups.share(7, 1), None);
    }

    #[test]
    fn the_group_goes_with_its_last_channel_whichever_that_is() {
        let mut groups = ContextGroups::default();
        groups.join(7, 100, 0, 101);
        groups.join(7, 100, 2, 103);
        assert_eq!(groups.leave(7, 0), Left { share: Some(101), group: None }, "the drawing channel first");
        assert_eq!(groups.leave(7, 2), Left { share: Some(103), group: Some(100) });
        assert!(groups.is_empty());
        assert_eq!(groups.group(7), None, "a later channel starts a new group");
    }

    #[test]
    fn a_context_stays_split_until_its_group_goes() {
        let mut groups = ContextGroups::default();
        groups.join(7, 100, 0, 101);
        assert!(!groups.is_split(7));
        groups.note_split(7);
        assert!(groups.is_split(7) && !groups.is_split(8));
        groups.leave(7, 0);
        assert!(!groups.is_split(7));
    }

    #[test]
    fn leaving_a_group_that_is_not_there_frees_nothing() {
        assert_eq!(ContextGroups::default().leave(7, 0), Left::default());
    }

    #[test]
    fn the_drawing_seat_is_the_first_subcontext_and_a_compute_seat_the_one_it_names() {
        assert_eq!(Seat::Drawing.slot(), 0);
        assert_eq!(Seat::Compute { index: 3, prefer_lower: true }.slot(), 3);
    }
}
