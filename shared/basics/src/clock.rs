//! Time, so that a test of a two-hour wait doesn't take two hours. Code that waits takes a clock as a
//! value, and a test hands it one that only moves when something sleeps.
//!
//! The Linux host's tasks wait by it. [`RealClock`] uses only the standard library's monotonic clock
//! and sleep, so every host can use it as it is.

use std::thread;
use std::time::{Duration, Instant};

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep(&self, time: Duration);
}

pub struct RealClock(Instant);

impl Default for RealClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&self, time: Duration) {
        thread::sleep(time);
    }
}

#[cfg(test)]
mod tests;
