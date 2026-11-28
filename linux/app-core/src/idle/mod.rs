//! Whether a running machine looks idle, measured and not acted on. Each signal is read on its own
//! and can be missing: a reading that could not be taken says why, and is never taken for quiet. The
//! judging is a plain function of the reading, and the guest's text is parsed by layout and by our own
//! markers, never by a word Windows says in its own language.
//!
//! The rules, the reading of the guest's answer and the words are in the crate every host can share;
//! taking a reading stays here.

mod take;
mod vcpu;

pub use guesthouse_idle_core::{
    Busy, GuestKind, IdleReading, Input, Limits, PowerKind, PowerRequest, Signal, Unread, Verdict, describe,
    judge, limits_text, parse_power, parse_report, parse_sessions,
};
pub use take::take;

#[cfg(test)]
pub use vcpu::stand_in::write_guest_stat;
