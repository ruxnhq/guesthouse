//! The Linux host's clock as it is: time from when the clock was made, which only goes forward, and a
//! sleep that takes at least as long as it was asked to. Held before the clock's trait moves, so code
//! that waits by it keeps waiting the same way.

#![cfg(target_os = "linux")]

use guesthouse_app_core::clock::{Clock, RealClock};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn the_real_clock_starts_near_nothing_and_only_goes_forward() {
    let clock = RealClock::default();
    let first = clock.now();
    assert!(first < Duration::from_secs(1), "{first:?} from a clock made just now");
    let mut last = first;
    for _ in 0..1000 {
        let now = clock.now();
        assert!(now >= last);
        last = now;
    }
}

#[test]
fn a_sleep_takes_at_least_its_time() {
    let clock = RealClock::default();
    let before = clock.now();
    clock.sleep(Duration::from_millis(20));
    assert!(clock.now() - before >= Duration::from_millis(20));
}

#[test]
fn the_clock_is_shared_between_threads_as_the_context_holds_it() {
    let clock: Arc<dyn Clock> = Arc::new(RealClock::default());
    let other = Arc::clone(&clock);
    let seen = std::thread::spawn(move || other.now()).join().unwrap();
    assert!(clock.now() >= seen);
}
