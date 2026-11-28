use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn the_real_clock_starts_near_nothing_and_only_goes_forward() {
    let clock = RealClock::default();
    assert!(clock.now() < Duration::from_secs(1));
    let before = clock.now();
    clock.sleep(Duration::from_millis(20));
    assert!(clock.now() - before >= Duration::from_millis(20));
}

/// Time that only moves when something sleeps, as a host's tests make one.
#[derive(Default)]
struct Ticking(AtomicU64);

impl Clock for Ticking {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::Relaxed))
    }

    fn sleep(&self, time: Duration) {
        self.0.fetch_add(time.as_millis() as u64, Ordering::Relaxed);
    }
}

#[test]
fn code_that_waits_takes_any_clock() {
    fn wait_for_an_hour(clock: &dyn Clock) -> Duration {
        let start = clock.now();
        while clock.now() - start < Duration::from_secs(3600) {
            clock.sleep(Duration::from_secs(60));
        }
        clock.now() - start
    }
    let clock: Arc<dyn Clock> = Arc::new(Ticking::default());
    assert_eq!(wait_for_an_hour(clock.as_ref()), Duration::from_secs(3600));
}
