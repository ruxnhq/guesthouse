//! How often the window is told things. Every message to the window is followed by a render of the
//! whole window on its thread, and Windows runs that thread's queue inside its loop for moving the
//! window too, so a stream of messages makes a dragged window lag behind the mouse. These hold the
//! messages back to what the window needs. They have nothing of Windows in them, so their tests run
//! on any system.

// Off Windows only the tests use these.
#![cfg_attr(not(windows), allow(dead_code))]

use std::time::{Duration, Instant};

/// Items gathered so that they go to the window together, at most once every `apart`. The first
/// after a quiet spell goes at once, so a lone line isn't held back, and none are dropped or
/// reordered.
pub struct Batch<T> {
    apart: Duration,
    waiting: Vec<T>,
    /// When the last of them went.
    sent: Option<Instant>,
}

impl<T> Batch<T> {
    pub fn new(apart: Duration) -> Batch<T> {
        Batch { apart, waiting: Vec::new(), sent: None }
    }

    /// Takes an item that came at `now`, and returns it with those waiting before it if it's time
    /// for them to go.
    pub fn add(&mut self, item: T, now: Instant) -> Option<Vec<T>> {
        self.waiting.push(item);
        self.take_due(now)
    }

    /// When the items waiting are to go, if any are.
    pub fn due(&self) -> Option<Instant> {
        if self.waiting.is_empty() {
            return None;
        }
        // Something waits only because something went before it.
        self.sent.map(|sent| sent + self.apart)
    }

    /// The items waiting, if it's time at `now` for them to go.
    pub fn take_due(&mut self, now: Instant) -> Option<Vec<T>> {
        if self.waiting.is_empty() || self.sent.is_some_and(|sent| now < sent + self.apart) {
            return None;
        }
        self.sent = Some(now);
        Some(std::mem::take(&mut self.waiting))
    }

    /// Every item waiting, at once, as when the stream ends and nothing more will come.
    pub fn take_all(&mut self) -> Vec<T> {
        std::mem::take(&mut self.waiting)
    }
}

/// The last value sent, so the same again isn't sent.
pub struct Changes<T> {
    last: Option<T>,
}

impl<T: Clone + PartialEq> Changes<T> {
    pub fn new() -> Changes<T> {
        Changes { last: None }
    }

    /// `value` if it's the first, or differs from the last one sent, which it then becomes.
    pub fn news(&mut self, value: T) -> Option<T> {
        if self.last.as_ref() == Some(&value) {
            return None;
        }
        self.last = Some(value.clone());
        Some(value)
    }

    /// Lets the next value go whatever it is.
    pub fn forget(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APART: Duration = Duration::from_millis(200);

    fn ms(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn the_first_item_goes_at_once() {
        let start = Instant::now();
        let mut batch = Batch::new(APART);
        assert_eq!(batch.add(1, start), Some(vec![1]));
        assert_eq!(batch.due(), None);
    }

    #[test]
    fn items_close_together_wait_and_go_together_in_order() {
        let start = Instant::now();
        let mut batch = Batch::new(APART);
        assert_eq!(batch.add(1, start), Some(vec![1]));
        assert_eq!(batch.add(2, ms(start, 10)), None);
        assert_eq!(batch.add(3, ms(start, 50)), None);
        assert_eq!(batch.due(), Some(ms(start, 200)));
        assert_eq!(batch.take_due(ms(start, 199)), None);
        assert_eq!(batch.take_due(ms(start, 200)), Some(vec![2, 3]));
        assert_eq!(batch.due(), None);
    }

    #[test]
    fn a_steady_stream_goes_at_most_once_each_interval() {
        let start = Instant::now();
        let mut batch = Batch::new(APART);
        let mut sends = Vec::new();
        let mut seen = Vec::new();
        // A line every millisecond for two seconds, as a chatty guest might write, with the timer
        // taking what's due as it fires.
        for at in 0..2000u64 {
            let now = ms(start, at);
            if let Some(due) = batch.due()
                && now >= due
                && let Some(items) = batch.take_due(now)
            {
                sends.push(at);
                seen.extend(items);
            }
            if let Some(items) = batch.add(at, now) {
                sends.push(at);
                seen.extend(items);
            }
        }
        seen.extend(batch.take_all());
        assert_eq!(seen, (0..2000).collect::<Vec<_>>());
        assert!(sends.windows(2).all(|pair| pair[1] - pair[0] >= 200), "{sends:?}");
        assert_eq!(sends.len(), 10);
    }

    #[test]
    fn an_item_after_a_quiet_spell_goes_at_once() {
        let start = Instant::now();
        let mut batch = Batch::new(APART);
        assert!(batch.add(1, start).is_some());
        assert_eq!(batch.add(2, ms(start, 500)), Some(vec![2]));
    }

    #[test]
    fn the_end_takes_what_waits() {
        let start = Instant::now();
        let mut batch = Batch::new(APART);
        assert!(batch.add(1, start).is_some());
        assert_eq!(batch.add(2, ms(start, 1)), None);
        assert_eq!(batch.take_all(), vec![2]);
        assert_eq!(batch.due(), None);
        assert_eq!(batch.take_all(), Vec::<i32>::new());
    }

    #[test]
    fn the_first_value_goes_and_the_same_again_does_not() {
        let mut changes = Changes::new();
        assert_eq!(changes.news(Ok::<_, String>(1)), Some(Ok(1)));
        assert_eq!(changes.news(Ok(1)), None);
        assert_eq!(changes.news(Ok(2)), Some(Ok(2)));
    }

    #[test]
    fn the_service_going_away_and_coming_back_both_go() {
        let mut changes = Changes::new();
        assert!(changes.news(Ok::<_, String>(1)).is_some());
        assert_eq!(changes.news(Err("away".to_owned())), Some(Err("away".to_owned())));
        assert_eq!(changes.news(Err("away".to_owned())), None);
        // Back as it was before it went, which is still news to the window.
        assert_eq!(changes.news(Ok(1)), Some(Ok(1)));
    }

    #[test]
    fn a_forgotten_value_goes_again() {
        let mut changes = Changes::new();
        assert!(changes.news(1).is_some());
        changes.forget();
        assert_eq!(changes.news(1), Some(1));
    }
}
