//! Time, so that a test of a two-hour wait doesn't take two hours. The trait and the real clock are
//! every host's, in `guesthouse_basics::clock`.

pub use guesthouse_basics::clock::{Clock, RealClock};

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
