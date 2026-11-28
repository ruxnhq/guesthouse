//! The registers Intel's Windows driver sets from its own render batches, and whether a batch on
//! this card can set them under xe.
//!
//! Windows lets a user batch set these registers. xe lets a user batch reach only the few it lists
//! and drops a write to any other without a word, so a register the guest's driver counts on may
//! stay as xe left it. That changes how the card caches and flushes for the guest, which shows as a
//! wrong picture rather than a failure. The self-test sets each one as the guest does, reads it
//! before and after, and says what happened.

/// One register write the guest's driver makes, with the same dwords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestWrite {
    pub register: u32,
    pub value: u32,
    /// The upper half of the value says which bits of the lower half are written.
    pub masked: bool,
}

const fn masked(register: u32, value: u32) -> GuestWrite {
    GuestWrite { register, value, masked: true }
}

const fn plain(register: u32, value: u32) -> GuestWrite {
    GuestWrite { register, value, masked: false }
}

/// What the guest's driver writes at the start of its render work, in its order. The register at
/// 0x6204 it also sets to 0x20202020 in most later batches.
pub const RENDER_WRITES: [GuestWrite; 17] = [
    masked(0x20D8, 0x0010_0010),
    masked(0x20E4, 0x0006_0000),
    masked(0x20EC, 0x8000_0000),
    masked(0x6204, 0x0400_0400),
    masked(0x6204, 0x0040_0040),
    masked(0x6204, 0x2020_2020),
    masked(0x6208, 0x0020_0020),
    masked(0x7000, 0x0004_0000),
    masked(0x7004, 0x0040_0040),
    masked(0x7004, 0x0004_0004),
    masked(0x7010, 0x4000_4000),
    masked(0x7010, 0x2000_2000),
    masked(0x7044, 0x0020_0020),
    masked(0x7300, 0x1000_1000),
    plain(0x221C, 0x0000_0001),
    plain(0x23B4, 0x0000_0010),
    // With the card's whole feature table in the record the driver sets its preemption choice
    // here, where it set 0x20E4 and 0x20EC before.
    masked(0x2580, 0x8006_8004),
];

/// What a read leaves in memory when the register can't be read: nothing is written over this.
pub const UNREAD: u32 = 0x6E6F_6E65;

/// What became of one write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The register reads back as written, and it read otherwise before.
    Took,
    /// It read as written before the write too, so whether the write reached it isn't known.
    AlreadySo,
    /// It reads back unchanged, and not as written: the write was dropped.
    Dropped,
    /// It reads back as something else again.
    Other,
    /// A batch can't read it, so nothing is known.
    Unreadable,
}

impl GuestWrite {
    /// Whether `read` holds what this write asks for.
    fn holds(&self, read: u32) -> bool {
        if self.masked {
            let bits = self.value >> 16;
            read & bits == self.value & bits
        } else {
            read == self.value
        }
    }

    /// What became of the write, from the register read just before and just after it.
    pub fn outcome(&self, before: u32, after: u32) -> Outcome {
        if before == UNREAD || after == UNREAD {
            Outcome::Unreadable
        } else if !self.holds(after) {
            if after == before { Outcome::Dropped } else { Outcome::Other }
        } else if self.holds(before) {
            Outcome::AlreadySo
        } else {
            Outcome::Took
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Took => "took",
            Self::AlreadySo => "already so",
            Self::Dropped => "dropped",
            Self::Other => "changed otherwise",
            Self::Unreadable => "unreadable",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_masked_write_is_judged_by_the_bits_it_names() {
        let set = masked(0x7004, 0x0040_0040);
        assert_eq!(set.outcome(0x0000, 0x0040), Outcome::Took);
        assert_eq!(set.outcome(0x0001, 0x0041), Outcome::Took, "other bits stay out of it");
        assert_eq!(set.outcome(0x0040, 0x0040), Outcome::AlreadySo);
        assert_eq!(set.outcome(0x0001, 0x0001), Outcome::Dropped);
        assert_eq!(set.outcome(0x0001, 0x0002), Outcome::Other);
        let clear = masked(0x7000, 0x0004_0000);
        assert_eq!(clear.outcome(0x0004, 0x0000), Outcome::Took);
        assert_eq!(clear.outcome(0x0004, 0x0004), Outcome::Dropped);
    }

    #[test]
    fn a_plain_write_must_read_back_whole() {
        let write = plain(0x221C, 1);
        assert_eq!(write.outcome(0, 1), Outcome::Took);
        assert_eq!(write.outcome(0, 0), Outcome::Dropped);
        assert_eq!(write.outcome(1, 1), Outcome::AlreadySo);
    }

    #[test]
    fn a_register_that_cant_be_read_tells_nothing() {
        let write = plain(0x23B4, 0x10);
        assert_eq!(write.outcome(UNREAD, UNREAD), Outcome::Unreadable);
        assert_eq!(write.outcome(0, UNREAD), Outcome::Unreadable);
    }

    #[test]
    fn every_masked_write_names_the_bits_it_writes() {
        for write in RENDER_WRITES.iter().filter(|write| write.masked) {
            let (bits, value) = (write.value >> 16, write.value & 0xFFFF);
            assert_ne!(bits, 0, "{write:?}");
            assert_eq!(value & !bits, 0, "{write:?} writes only bits it names");
        }
    }
}
