//! Where a hardware queue's channel belongs among the channels of its context.
//!
//! The guest's user-mode driver says it in the queue's private data, which NVIDIA's own kernel
//! driver reads when it makes the queue: one channel group for the context, the queue that draws on
//! the group's synchronous subcontext, and each queue that only computes on an asynchronous
//! subcontext and the group's second runqueue. Queues of one context wait on each other, and in one
//! group such a wait hands the GPU to the queue it waits for, where a group for each queue sits out
//! its timeslice.

use crate::channel::Seat;
use crate::pushbuffer::{ENTRY_SIZE, Header, RingEntry};
use guesthouse_backend::{HostServices, SpaceId};

/// The private data of a hardware queue: this size, starting with these fields.
const QUEUE_DATA_SIZE: usize = 0xB4;
const MAGIC: usize = 0x00;
const QUEUE_DATA_MAGIC: u32 = 0x4E56_4441;
const VERSION: usize = 0x06;
const QUEUE_DATA_VERSION: u16 = 1;
const SIZE: usize = 0x08;
const TAG: usize = 0x0C;
/// The word that places the queue.
const PLACE: usize = 0x10;

const PLACE_SHARE_INDEX: u32 = 0xFF;
const PLACE_DRAWS: u32 = 1 << 8;
const PLACE_COMPUTES: u32 = 1 << 9;
const PLACE_SECOND_RUNQUEUE: u32 = 1 << 10;
const PLACE_OWN_RING: u32 = 1 << 11;
const PLACE_PREFER_LOWER: u32 = 1 << 12;

/// The placing word of a hardware queue's private data, as the guest's driver wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueuePlace(pub(crate) u32);

impl QueuePlace {
    /// `None` for data NVIDIA's kernel driver would refuse, which places nothing.
    pub(crate) fn parse(private: &[u8]) -> Option<Self> {
        let word = |at: usize| {
            private.get(at..at + 4).map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
        };
        let version = private.get(VERSION..VERSION + 2)?;
        let version = u16::from_le_bytes(version.try_into().expect("two bytes"));
        let valid = private.len() >= QUEUE_DATA_SIZE
            && word(MAGIC)? == QUEUE_DATA_MAGIC
            && version == QUEUE_DATA_VERSION
            && word(SIZE)? as usize == QUEUE_DATA_SIZE
            && word(TAG)? != 0;
        valid.then_some(Self(word(PLACE)?))
    }

    /// Which subcontext of its context's group the queue names.
    pub(crate) fn share_index(self) -> u8 {
        (self.0 & PLACE_SHARE_INDEX) as u8
    }

    pub(crate) fn draws(self) -> bool {
        self.0 & PLACE_DRAWS != 0
    }

    /// Only read when the queue doesn't draw, as the driver reads it.
    pub(crate) fn computes(self) -> bool {
        !self.draws() && self.0 & PLACE_COMPUTES != 0
    }

    pub(crate) fn second_runqueue(self) -> bool {
        self.0 & PLACE_SECOND_RUNQUEUE != 0
    }

    pub(crate) fn own_ring(self) -> bool {
        self.0 & PLACE_OWN_RING != 0
    }

    pub(crate) fn prefer_lower(self) -> bool {
        self.0 & PLACE_PREFER_LOWER != 0
    }

    /// Where the queue's channel goes in its context's group, if anywhere.
    ///
    /// A queue that draws names the first subcontext, and gets every class. A queue that computes
    /// on the second runqueue gets the asynchronous subcontext it names, and no class that draws or
    /// copies: the RM takes compute beside a group's drawing only on the second runqueue, and has
    /// no copy engine there. Anything else keeps a group of its own.
    pub(crate) fn seat(self) -> Option<Seat> {
        if self.draws() {
            return (self.share_index() == 0).then_some(Seat::Drawing);
        }
        // Subcontext 0 is the synchronous one, which the RM refuses compute of the second runqueue on.
        if self.computes() && self.second_runqueue() && self.share_index() != 0 {
            return Some(Seat::Compute { index: self.share_index(), prefer_lower: self.prefer_lower() });
        }
        None
    }
}

impl std::fmt::Display for QueuePlace {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match (self.draws(), self.computes()) {
            (true, _) => "draws",
            (false, true) => "computes",
            (false, false) => "no kind",
        };
        write!(out, "{:#x} (subcontext {}, {kind}", self.0, self.share_index())?;
        for (set, name) in [
            (self.second_runqueue(), "second runqueue"),
            (self.own_ring(), "own ring"),
            (self.prefer_lower(), "prefer lower"),
        ] {
            if set {
                write!(out, ", {name}")?;
            }
        }
        write!(out, ")")
    }
}

/// Methods from here up go to the object bound on their subchannel, and below it to the channel.
const FIRST_OBJECT_METHOD: u32 = 0x100;

/// Which subchannels' objects a submission's commands use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SubchannelUse {
    /// A bit for each subchannel with a method for its object.
    pub(crate) used: u8,
    /// Some of the commands couldn't be read, so they may use any.
    pub(crate) unread: bool,
}

impl SubchannelUse {
    /// Walks the method headers of the commands a listing of ring entries points at.
    pub(crate) fn of_listing(host: &dyn HostServices, space: SpaceId, listing: &[u8]) -> Self {
        let mut found = Self::default();
        let (entries, _) = listing.as_chunks::<ENTRY_SIZE>();
        for entry in entries {
            let entry = RingEntry::parse(*entry);
            if entry.length == 0 {
                continue;
            }
            let mut bytes = vec![0; entry.length as usize * 4];
            if host.read_guest_span(space, entry.address, &mut bytes) != Some(bytes.len()) {
                found.unread = true;
                continue;
            }
            found.used |= subchannels_of(&bytes);
        }
        found
    }

    /// Whether the commands may use an object on one of the `missing` subchannels.
    pub(crate) fn needs(self, missing: u8) -> bool {
        self.unread || self.used & missing != 0
    }
}

/// What a watched queue's commands have used so far.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Watched {
    used: u8,
    unread: bool,
    submissions: u64,
}

impl Watched {
    /// Adds a submission, and says which subchannels it is the first to use.
    pub(crate) fn add(&mut self, found: SubchannelUse) -> Option<(u8, u64)> {
        self.submissions += 1;
        let new = found.used & !self.used;
        let first_unread = found.unread && !self.unread;
        self.used |= found.used;
        self.unread |= found.unread;
        (new != 0 || first_unread || self.submissions == 1).then_some((new, self.submissions))
    }
}

fn subchannels_of(commands: &[u8]) -> u8 {
    let (words, _) = commands.as_chunks::<4>();
    let (mut used, mut at) = (0, 0);
    while at < words.len() {
        let header = Header::parse(u32::from_le_bytes(words[at]));
        if header.has_data() && header.method >= FIRST_OBJECT_METHOD {
            used |= 1 << header.subchannel;
        }
        at += header.stride();
    }
    used
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pushbuffer::increasing;

    fn queue_data(place: u32) -> Vec<u8> {
        let mut data = vec![0; QUEUE_DATA_SIZE];
        data[MAGIC..MAGIC + 4].copy_from_slice(&QUEUE_DATA_MAGIC.to_le_bytes());
        data[VERSION..VERSION + 2].copy_from_slice(&QUEUE_DATA_VERSION.to_le_bytes());
        data[SIZE..SIZE + 4].copy_from_slice(&(QUEUE_DATA_SIZE as u32).to_le_bytes());
        data[TAG..TAG + 4].copy_from_slice(&0x4E56_4458u32.to_le_bytes());
        data[PLACE..PLACE + 4].copy_from_slice(&place.to_le_bytes());
        data
    }

    #[test]
    fn the_placing_word_is_read_from_data_the_kernel_driver_would_take() {
        assert_eq!(QueuePlace::parse(&queue_data(0x603)), Some(QueuePlace(0x603)));
        let mut padded = queue_data(0x100);
        padded.extend([0; 12]);
        assert_eq!(QueuePlace::parse(&padded), Some(QueuePlace(0x100)), "the answer's padding follows it");
    }

    #[test]
    fn data_the_kernel_driver_would_refuse_places_nothing() {
        let good = queue_data(0x100);
        assert_eq!(QueuePlace::parse(&good[..QUEUE_DATA_SIZE - 1]), None, "too short");
        for (at, what) in [(MAGIC, "magic"), (VERSION, "version"), (SIZE, "size")] {
            let mut data = good.clone();
            data[at] ^= 0x40;
            assert_eq!(QueuePlace::parse(&data), None, "{what}");
        }
        let mut data = good.clone();
        data[TAG..TAG + 4].fill(0);
        assert_eq!(QueuePlace::parse(&data), None, "no tag");
        assert_eq!(QueuePlace::parse(&[]), None);
    }

    #[test]
    fn the_fields_of_the_placing_word() {
        let place = QueuePlace(0x1603);
        assert_eq!(place.share_index(), 3);
        assert!(place.computes() && !place.draws());
        assert!(place.second_runqueue() && place.prefer_lower() && !place.own_ring());
        assert!(QueuePlace(0x900).own_ring());
        assert!(!QueuePlace(0x300).computes(), "the drawing bit is read first");
    }

    #[test]
    fn a_queue_that_draws_sits_on_the_first_subcontext() {
        assert_eq!(QueuePlace(0x100).seat(), Some(Seat::Drawing));
        assert_eq!(QueuePlace(0x900).seat(), Some(Seat::Drawing), "with a ring of its own");
        assert_eq!(QueuePlace(0x500).seat(), Some(Seat::Drawing), "the runqueue bit is the compute queues'");
    }

    #[test]
    fn a_queue_that_computes_on_the_second_runqueue_sits_on_the_subcontext_it_names() {
        assert_eq!(QueuePlace(0x601).seat(), Some(Seat::Compute { index: 1, prefer_lower: false }));
        assert_eq!(QueuePlace(0x1E02).seat(), Some(Seat::Compute { index: 2, prefer_lower: true }));
    }

    #[test]
    fn anything_else_keeps_a_group_of_its_own() {
        assert_eq!(QueuePlace(0).seat(), None, "no kind: a copy node's queue");
        assert_eq!(QueuePlace(0x200).seat(), None, "compute on the first runqueue");
        assert_eq!(QueuePlace(0x201).seat(), None);
        assert_eq!(QueuePlace(0x101).seat(), None, "the kernel driver refuses a drawing queue there");
        assert_eq!(QueuePlace(0x400).seat(), None, "the runqueue bit alone");
        assert_eq!(QueuePlace(0x600).seat(), None, "subcontext 0 is the drawing queue's");
    }

    #[test]
    fn the_word_prints_with_what_it_means() {
        assert_eq!(
            QueuePlace(0x1602).to_string(),
            "0x1602 (subcontext 2, computes, second runqueue, prefer lower)"
        );
        assert_eq!(QueuePlace(0x100).to_string(), "0x100 (subcontext 0, draws)");
    }

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[test]
    fn object_methods_are_counted_by_subchannel_and_channel_methods_are_not() {
        let commands = bytes(&[
            increasing(1, 0x2B4, 2),
            // Data that would read as a header for subchannel 3.
            increasing(3, 0x300, 1),
            0,
            increasing(4, 0x6C, 1),
            3,
            increasing(5, 0x100, 1),
            7,
        ]);
        assert_eq!(subchannels_of(&commands), 1 << 1 | 1 << 5);
    }

    #[test]
    fn only_commands_for_a_missing_object_or_unread_ones_need_it() {
        let missing = 0b01_1101;
        assert!(!SubchannelUse { used: 1 << 1 | 1 << 5, unread: false }.needs(missing));
        assert!(
            !SubchannelUse { used: 1 << 6 | 1 << 7, unread: false }.needs(missing),
            "never had an object"
        );
        assert!(!SubchannelUse::default().needs(missing));
        assert!(SubchannelUse { used: 1 << 1 | 1 << 4, unread: false }.needs(missing));
        assert!(SubchannelUse { used: 0, unread: true }.needs(missing), "unread may be anything");
    }
}
