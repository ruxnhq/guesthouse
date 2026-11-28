//! Notices when frames are handed to the compositor and none of them reaches the screen.
//!
//! The compositor answers every frame it's given: shown, or dropped for a newer one. A picture
//! that stands still while frames keep going out is seen here and nowhere else, as a count of
//! shown frames that has stopped. It goes in the app's log when it starts and when it ends, so a
//! report of a frozen picture says whether the frames were sent.
//!
//! A window nobody can see is shown no frames either, and that's no fault. The screen says when
//! it knows that is so. A compositor may say so late: GNOME names a minimised window suspended
//! some seconds after it went, and the log has a line for those seconds.

use std::time::{Duration, Instant};

/// Frames sent this long ago with none shown since is a stall. A healthy compositor shows one
/// within a refresh, which on the slowest screens is a few tens of milliseconds.
const STALLED_AFTER: Duration = Duration::from_millis(250);

/// What the watch found, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stall {
    /// No frame has reached the screen for `waited`, with `sent` handed over since.
    Began { waited: Duration, sent: u64 },
    /// Frames reach the screen again, this long after the stall was seen.
    Over { after: Duration },
}

#[derive(Debug, Default)]
pub struct StallWatch {
    /// How many frames the compositor had shown when last looked.
    shown: u64,
    /// When the oldest frame went out that no shown frame has followed.
    waiting_since: Option<Instant>,
    /// Frames sent since then.
    sent: u64,
    /// When the stall under way was seen.
    stalled_at: Option<Instant>,
}

impl StallWatch {
    /// A frame went to the compositor.
    pub fn sent(&mut self, now: Instant) {
        self.waiting_since.get_or_insert(now);
        self.sent += 1;
    }

    /// Nothing is on the surface, or nobody can see it, so no frame is awaited.
    pub fn nothing_awaited(&mut self) {
        self.waiting_since = None;
        self.sent = 0;
        self.stalled_at = None;
    }

    /// Looks at how many frames the compositor has shown by now.
    pub fn check(&mut self, now: Instant, shown: u64) -> Option<Stall> {
        if shown != self.shown {
            self.shown = shown;
            let stalled_at = self.stalled_at;
            self.nothing_awaited();
            return stalled_at.map(|at| Stall::Over { after: now.duration_since(at) });
        }
        let waited = now.duration_since(self.waiting_since?);
        if waited < STALLED_AFTER || self.stalled_at.is_some() {
            return None;
        }
        self.stalled_at = Some(now);
        Some(Stall::Began { waited, sent: self.sent })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(count: u64) -> Duration {
        Duration::from_millis(count)
    }

    #[test]
    fn frames_that_are_shown_are_no_stall() {
        let start = Instant::now();
        let mut watch = StallWatch::default();
        for frame in 0..1_000 {
            let now = start + ms(frame * 4);
            watch.sent(now);
            assert_eq!(watch.check(now + ms(3), frame + 1), None);
        }
    }

    #[test]
    fn a_still_guest_is_no_stall() {
        let start = Instant::now();
        let mut watch = StallWatch::default();
        watch.sent(start);
        assert_eq!(watch.check(start + ms(10), 1), None);
        assert_eq!(watch.check(start + ms(60_000), 1), None);
    }

    #[test]
    fn frames_sent_with_none_shown_are_a_stall_told_once() {
        let start = Instant::now();
        let mut watch = StallWatch::default();
        for frame in 0..62 {
            watch.sent(start + ms(frame * 4));
            assert_eq!(watch.check(start + ms(frame * 4), 0), None);
        }
        watch.sent(start + ms(250));
        assert_eq!(watch.check(start + ms(250), 0), Some(Stall::Began { waited: ms(250), sent: 63 }));
        for later in 251..10_000 {
            watch.sent(start + ms(later));
            assert_eq!(watch.check(start + ms(later), 0), None);
        }
    }

    #[test]
    fn one_frame_that_is_never_shown_is_a_stall_too() {
        let start = Instant::now();
        let mut watch = StallWatch::default();
        watch.sent(start);
        assert_eq!(watch.check(start + ms(249), 0), None);
        assert_eq!(watch.check(start + ms(250), 0), Some(Stall::Began { waited: ms(250), sent: 1 }));
    }

    #[test]
    fn the_end_of_a_stall_is_told_once() {
        let start = Instant::now();
        let mut watch = StallWatch::default();
        watch.sent(start);
        assert!(watch.check(start + ms(250), 0).is_some());
        watch.sent(start + ms(300));
        assert_eq!(watch.check(start + ms(310), 1), Some(Stall::Over { after: ms(60) }));
        watch.sent(start + ms(314));
        assert_eq!(watch.check(start + ms(318), 2), None);
    }

    #[test]
    fn a_surface_nobody_sees_awaits_nothing() {
        let start = Instant::now();
        let mut watch = StallWatch::default();
        watch.sent(start);
        watch.nothing_awaited();
        assert_eq!(watch.check(start + ms(10_000), 0), None);
        // And a stall under way is forgotten, with no word of its end.
        watch.sent(start + ms(10_000));
        assert!(watch.check(start + ms(10_250), 0).is_some());
        watch.nothing_awaited();
        assert_eq!(watch.check(start + ms(10_300), 1), None);
    }
}
