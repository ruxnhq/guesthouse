//! Time, so that a test of a two-hour wait doesn't take two hours.

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
pub mod stand_in {
    use super::Clock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    /// Time that only moves when something sleeps.
    #[derive(Default)]
    pub struct Ticking(AtomicU64);

    impl Clock for Ticking {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::Relaxed))
        }

        fn sleep(&self, time: Duration) {
            self.0.fetch_add(time.as_millis() as u64, Ordering::Relaxed);
        }
    }
}
