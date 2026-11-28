//! Whether a running machine looks idle, measured and not acted on. Each signal is read on its own
//! and can be missing: a reading that could not be taken says why, and is never taken for quiet. The
//! judging is a plain function of the reading, and the guest's text is parsed by layout and by our own
//! markers, never by a word Windows says in its own language.
//!
//! Taking a reading (asking the guest and reading the host's process) stays with the host, as it draws
//! on the system. Only the Linux host reads idleness today.

mod judge;
mod parse;
mod words;

pub use judge::{Busy, Limits, Verdict, judge};
pub use parse::{parse_power, parse_report, parse_sessions};
pub use words::{describe, limits_text};

use std::fmt;

/// A reading that could not be taken, and why. It is a signal we know nothing about, which is not
/// the same as a quiet one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unread(pub String);

impl Unread {
    pub fn new(why: impl Into<String>) -> Self {
        Self(why.into())
    }
}

impl fmt::Display for Unread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A signal that was read, or why not.
pub type Signal<T> = Result<T, Unread>;

/// The last keyboard or mouse in the user's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Seconds since Windows last saw any.
    Seconds(u64),
    /// Nobody is logged in, so there is no session to have input. That is an answer, unlike a helper
    /// that did not answer.
    NobodyLoggedIn,
}

/// The kinds of request a program makes to keep Windows awake, in the order `powercfg /requests` lists
/// them. The ones after these are not a reason to stay up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerKind {
    Display,
    System,
    AwayMode,
    Execution,
}

impl PowerKind {
    /// In the order the sections come, which is how they are told apart: their names are Windows' words.
    pub const IN_ORDER: [Self; 4] = [Self::Display, Self::System, Self::AwayMode, Self::Execution];

    pub fn name(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::System => "system",
            Self::AwayMode => "away mode",
            Self::Execution => "execution",
        }
    }
}

/// One program's request, with the line Windows gave for it, cut short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerRequest {
    pub kind: PowerKind,
    pub who: String,
}

/// What kind of guest it is, as far as what can be read from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestKind {
    /// Has our agent, so input, sessions and power requests can be read.
    Windows,
    /// Ubuntu, a plain VM or macOS. Nothing in them answers us, so only the host's side is read.
    Other,
}

/// Everything read about one machine at one time.
#[derive(Debug, Clone, PartialEq)]
pub struct IdleReading {
    pub kind: GuestKind,
    pub input: Signal<Input>,
    /// Remote Desktop sessions that are connected, not those that were left disconnected.
    pub remote_sessions: Signal<u32>,
    pub power_requests: Signal<Vec<PowerRequest>>,
    /// How much of one core the guest's vCPUs used over `vcpu_window_seconds`.
    pub vcpu_cores: Signal<f64>,
    pub vcpu_window_seconds: f64,
}

#[cfg(test)]
mod tests;
