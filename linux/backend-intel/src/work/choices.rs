//! The choices about running a guest's work that the card has not yet confirmed, each in one named
//! place so that changing one is one edit.
//!
//! Nothing here has run on a card.

use crate::xe::abi;

/// The flags a guest process's address space is made with. A scratch page: an address nothing is
/// mapped at reads zeros and swallows writes. The card reads a little past the end of a buffer it
/// was given, and without the scratch page such a read faults, xe bans the queue and the guest's
/// program loses its device, though nothing was wrong with its work.
pub const VM_FLAGS: u32 = abi::VM_SCRATCH_PAGE;
/// The flags of the second address space a process gets for work that never ends: see [`Rings`].
pub const RING_VM_FLAGS: u32 = abi::VM_LONG_RUNNING;

/// What becomes of a submission whose batch parks in a semaphore wait instead of ending. Intel's
/// compute driver submits such a ring once and then feeds it from user mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rings {
    /// It runs on a queue in a long-running address space of the same process, which xe gives no
    /// job timeout. The card is asked to write the queue's fence after the batch, which is when the
    /// guest's driver has stopped the ring, and a lost ring is noticed by asking xe whether its
    /// queue was banned.
    LongRunning,
    /// It is refused, and its queue stops. With this, the guest's driver must be kept from making
    /// rings some other way, such as by not offering it the engines it makes them on.
    Refused,
}

pub const RINGS: Rings = Rings::LongRunning;

/// The unit of a submission's command length, as the message gives it: bytes.
pub const COMMAND_LENGTH_UNIT: u64 = 1;

/// What a new hardware queue's private data gets written back at +8: nothing. Intel's kernel
/// driver writes there only for a queue made with a native progress fence, which this backend never
/// offers, so the qword stays as the guest sent it.
pub const WRITES_QUEUE_TOKEN: bool = false;

/// Context state a Windows guest's batches may expect that xe's own context image may not have.
/// Nothing is done about any of it: a wrong picture or a slow kernel on a real card is to be read
/// against this list first.
pub const MAY_DIFFER_FROM_WINDOWS: [&str; 3] = [
    "the Windows driver's workarounds for this stepping, which xe may not match",
    "the tiled-resource translation table, which xe never turns on; the record leaves its two feature bits clear",
    "the cache-policy table: the guest's indexes mean what Windows programs, and xe programs its own",
];

/// How often work on the card is looked at for runs xe ended early.
pub const LOOK_AT_RUNS_EVERY: std::time::Duration = std::time::Duration::from_millis(50);
