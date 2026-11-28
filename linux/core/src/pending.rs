//! Things that wait on fences, and queue gates.
//!
//! None of this has a thread of its own. The daemon calls `Session::poll` while anything here is
//! waiting, and a condition is simply checked again.

use crate::session::ReplyTicket;
use guesthouse_backend::SpaceId;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long the GPU gets before the host stops waiting for a queue, unless the backend says.
pub const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a queue may wait behind a fence before going on regardless.
pub const GATE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a submission's reply may be held for ring room. It is bounded, because what the GPU is
/// waiting for may be in a later submission from the same guest thread. A submission's own work never
/// holds its reply past the limit rule, so a hold that runs this long is for work ahead of it.
pub const RING_ROOM_TIMEOUT: Duration = Duration::from_millis(100);
/// How many of a process's queues may stop answering before the whole process comes off the GPU. A
/// card that stalled for someone else can cost one queue, so one isn't enough to give up on a process.
/// A fault usually takes all of a process's queues with it, which gets here at once.
pub const MOST_FAILED_QUEUES: u32 = 2;
/// How many address updates and CPU waits may wait for their fences at once in a session. A guest
/// keeps a few in flight per thread, so this is far past what one uses.
pub const MOST_WAITING: usize = 4096;
/// How many bytes of operations the waiting address updates may hold between them.
pub const MOST_WAITING_UPDATE_BYTES: usize = 32 << 20;
/// How long an update or a CPU wait is looked at again at once. One that waits longer is waiting on
/// work that takes time, and is looked at with the background's pace.
pub const LONG_WAIT: Duration = Duration::from_secs(1);

/// (fence address, value).
pub type Target = (u64, u64);
/// (sync object handle, value).
pub type Signal = (u32, u64);

/// What to do when some queues finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAction {
    WakeEvent {
        event: u64,
        release: u64,
    },
    WriteFences(Vec<Signal>),
    PresentDone([u64; 3]),
    /// Handles a message again, whose reply has been held for this.
    Retry {
        message: Vec<u8>,
        ticket: Option<ReplyTicket>,
        started: Instant,
    },
}

/// One thing that is waiting.
#[derive(Debug)]
pub enum Pending {
    AfterQueues {
        targets: Vec<Target>,
        deadline: Instant,
        action: QueueAction,
    },
    CpuWait {
        signals: Vec<Signal>,
        wake_on_any: bool,
        event: u64,
        release: u64,
        since: Instant,
    },
    Gate {
        queue: u32,
    },
    Update {
        fence: u32,
        value: u64,
        space: SpaceId,
        operations: Vec<u8>,
        since: Instant,
    },
    /// A submission's reply, held until its queue has room.
    RingRoom {
        queue: u32,
        fence: u64,
        work: u32,
        ticket: ReplyTicket,
        started: Instant,
    },
    /// A submission the backend had no room for, to try again.
    Retry {
        message: Vec<u8>,
        ticket: Option<ReplyTicket>,
        started: Instant,
    },
    /// Only here to notice a GPU that has stopped, so nobody is in a hurry for it.
    Watch {
        queue: u32,
        fence: u64,
        value: u64,
        rung: Instant,
    },
}

impl Pending {
    /// Whether it is checked again at once. A watch isn't, nor a queue's gate that `resting` says
    /// waits for a doorbell's time to come.
    fn is_urgent(&self, resting: &impl Fn(u32) -> bool, now: Instant) -> bool {
        match self {
            Self::Watch { .. } => false,
            Self::Gate { queue } => !resting(*queue),
            Self::CpuWait { since, .. } | Self::Update { since, .. } => {
                now.saturating_duration_since(*since) < LONG_WAIT
            }
            _ => true,
        }
    }

    /// Whether this is an address update or a CPU wait, which a session holds a limited number of.
    pub(crate) fn is_fence_wait(&self) -> bool {
        matches!(self, Self::CpuWait { .. } | Self::Update { .. })
    }
}

/// What the caller of `Session::poll` should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// Nothing is waiting, so sleep until a message comes.
    Nothing,
    /// Only the GPU's health is being watched, so look again before long.
    Background,
    /// The guest is waiting on a fence, so look again at once.
    Urgent,
}

impl Waiting {
    pub(crate) fn of<'a>(
        pending: impl Iterator<Item = &'a Pending>,
        copies_waiting: bool,
        resting: impl Fn(u32) -> bool,
        now: Instant,
    ) -> Self {
        let mut found = if copies_waiting { Self::Urgent } else { Self::Nothing };
        for item in pending {
            if item.is_urgent(&resting, now) {
                return Self::Urgent;
            }
            found = Self::Background;
        }
        found
    }

    pub fn any(self) -> bool {
        self != Self::Nothing
    }
}

/// What a queue held back, in order.
#[derive(Debug)]
pub enum GateStep {
    Wait {
        signals: Vec<Signal>,
        deadline: Instant,
        /// Its place in the line for objects that are held.
        id: u64,
    },
    Ring {
        doorbell: u64,
        fence: u64,
        value: u64,
    },
    /// A copy that reads this queue's last frame, which the next frame must not draw over.
    Copy {
        id: u64,
    },
}

pub type Gate = VecDeque<GateStep>;

/// A wait that more than one queue of a context is held behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedWait {
    /// The queues still behind it.
    pub queues_left: usize,
    /// One queue has gone through, and took what the wait holds for the others.
    pub went: bool,
}
