//! Giving way to the card's other users, so a guest doesn't take the engine from them.
//!
//! amdgpu's scheduler runs jobs in the order they were sent, whoever sent them: its default policy
//! is FIFO, also in 7.4, and the fair one is still an experiment. A guest sends long jobs, several
//! ahead, while a small app on the host, in a container or in another VM sends short ones, one at a
//! time. Each small job then waits behind everything the guests sent before it, and the small app
//! gets almost none of the engine's time: on a 780M two Windows guests left a container's vkmark
//! 163 frames a second of its 4624 (#276).
//!
//! So a guest gives way when it sees others waiting: it sends one run at a time on that engine, so
//! each of theirs waits for one of its runs at most, not for all it had sent. With nobody else on the
//! card, nothing changes: runs go as before, many at once.
//!
//! Each run notes the card's clock as it starts and ends. A run sent while the guest's own work was
//! still on the engine should start as soon as that work ends, and one sent soon after it ended finds
//! the card awake and should start at once. If either started later, the card ran someone else's
//! work first. Two such sightings close together make the space give way, as the card now and then
//! starts a run late with nobody else on it. It gives way until nobody has been seen for a while.
//!
//! The guest's own work is all of its processes', not only the space's. In a Windows guest the
//! compositor and the display run small jobs between a game's, and counting them as others had a
//! game alone on a 780M give way to itself, at under a third of its speed (#327). See [`OwnEnds`].
//!
//! A run sent to an engine that had been idle a while says little. The card takes longer to start it
//! the longer the engine sat idle: on a Radeon 610M 56 µs after 16 ms idle, on a 780M up to 1.2 ms.
//! That can't be told from a short job of someone else's, and taking it for one kept a guest alone on
//! the card giving way for good in 0.7.6 (#327). So only a wait of over 2 ms counts there.
//!
//! 0.7.6 also paused after each run while giving way, longer while others were still seen. That gave
//! a small app more of the engine, but two guests on one card each took the other's next run for
//! someone waiting on it, paused as long as it could, and left the engine idle. Stand-in clients on a
//! 610M's DMA engine, see the `amd-share` binary: a light client with 216 µs jobs got 142 jobs a
//! second beside one heavy client with no giving way, and 524 with it. Two heavy clients kept half
//! the engine each, and one alone 98 %.

use std::collections::{HashMap, VecDeque};

/// The daemon's switch that turns giving way on. Off without it since 0.7.7 (#327).
pub const VARIABLE: &str = "GUESTHOUSE_AMD_GIVE_WAY";

/// How much later than it should have a run must have started to have waited for someone else. A
/// client's queued runs alone start under 25 µs apart nearly always, and now and then up to 72 µs on
/// a Radeon 610M's graphics engine with nothing else on it: so it takes a second sighting.
pub const WAITED_NS: u64 = 50_000;
/// How soon after the guest's own work ended a run sent to the engine finds the card still awake.
/// On a Radeon 610M a run started within 23 µs after up to 500 µs idle, and in 40 to 70 µs from 2 ms
/// idle on.
pub const HOT_NS: u64 = 250_000;
/// The same as [`WAITED_NS`] for a run sent to an engine idle for longer, where the card may first
/// have to wake: 1.2 ms was the 90th percentile on a 780M.
pub const IDLE_WAITED_NS: u64 = 2_000_000;
/// How soon after one sighting of others waiting a second one makes the space give way.
pub const CONFIRM_NS: u64 = 50_000_000;
/// How long after others were last seen waiting the space still gives way.
pub const CONTENDED_FOR_NS: u64 = 200_000_000;

/// One engine of one space: whether it gives way. Times are the card's clock in nanoseconds.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Share {
    /// When others were last seen waiting.
    seen_at: Option<u64>,
    /// When a second sighting last confirmed it.
    contended_at: Option<u64>,
}

/// When a run was sent, after the call returned, and when the card started and ended it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTimes {
    pub sent: u64,
    pub started: u64,
    pub ended: u64,
}

impl Share {
    /// A run on this engine has retired. `own_end` is when the guest's own work on the engine, of
    /// any of its processes, last ended before the run started, 0 if never: see [`OwnEnds`].
    pub fn retired(&mut self, times: RunTimes, own_end: u64) {
        let RunTimes { sent, started, ended } = times;
        if started == 0 || ended < started {
            // The card didn't write both: the run was ended before it got there.
            return;
        }
        let others_waited = if own_end >= sent {
            // Sent while the guest's own work was on the engine: it should have followed it.
            started.saturating_sub(own_end) > WAITED_NS
        } else if own_end > 0 && sent - own_end <= HOT_NS {
            // Sent while the card was awake: it should have started at once.
            started.saturating_sub(sent) > WAITED_NS
        } else {
            started.saturating_sub(sent) > IDLE_WAITED_NS
        };
        if !others_waited {
            return;
        }
        if self.seen_at.is_some_and(|at| ended.saturating_sub(at) < CONFIRM_NS) {
            self.contended_at = Some(ended);
        }
        self.seen_at = Some(ended);
    }

    /// Whether others were seen waiting lately.
    pub fn contended(&self, now: u64) -> bool {
        self.contended_at.is_some_and(|at| now.saturating_sub(at) < CONTENDED_FOR_NS)
    }

    /// Whether a run may be sent at `now`, with `on_card` of this engine's runs still on the card.
    pub fn may_send(&self, now: u64, on_card: usize) -> bool {
        on_card == 0 || !self.contended(now)
    }
}

/// When the guest's runs ended, on each engine, over all its spaces: what a space's run is judged
/// against. A space finds its own runs retired only when it next looks, so the daemon notes every
/// space's ended runs, also those not yet retired, before it judges any.
#[derive(Debug, Default)]
pub struct OwnEnds {
    ends: HashMap<u32, VecDeque<u64>>,
}

/// How many ends are kept for each engine: far more than the guest has runs on it at once.
const KEPT_ENDS: usize = 256;

impl OwnEnds {
    /// Notes that a run of the guest on the engine `ip_type` ended at `ended`.
    pub fn noted(&mut self, ip_type: u32, ended: u64) {
        let ends = self.ends.entry(ip_type).or_default();
        let at = ends.partition_point(|&end| end < ended);
        if ends.get(at) == Some(&ended) {
            return;
        }
        ends.insert(at, ended);
        if ends.len() > KEPT_ENDS {
            ends.pop_front();
        }
    }

    /// When the guest's work on the engine last ended at or before `at`, 0 if it never did.
    pub fn last_by(&self, ip_type: u32, at: u64) -> u64 {
        let Some(ends) = self.ends.get(&ip_type) else { return 0 };
        let after = ends.partition_point(|&end| end <= at);
        after.checked_sub(1).map_or(0, |index| ends[index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;
    const US: u64 = 1_000;

    /// A run of `took` sent while the guest's work before it was on the engine, until `own_end`,
    /// starting `waited` after that.
    fn queued(own_end: u64, waited: u64, took: u64) -> (RunTimes, u64) {
        let started = own_end + waited;
        (RunTimes { sent: own_end.saturating_sub(MS), started, ended: started + took }, own_end)
    }

    /// A run of `took` sent `gap` after the guest's own work ended at `own_end`, starting `waited`
    /// after it was sent.
    fn after(own_end: u64, gap: u64, waited: u64, took: u64) -> (RunTimes, u64) {
        let sent = own_end + gap;
        (RunTimes { sent, started: sent + waited, ended: sent + waited + took }, own_end)
    }

    fn retire(share: &mut Share, (times, own_end): (RunTimes, u64)) -> u64 {
        share.retired(times, own_end);
        times.ended
    }

    #[test]
    fn alone_on_the_card_runs_go_as_before() {
        let mut share = Share::default();
        let mut now = 2 * MS;
        for _ in 0..100 {
            now = retire(&mut share, queued(now, 10 * US, 2 * MS));
        }
        assert!(!share.contended(now));
        assert!(share.may_send(now, 7), "many at once");
    }

    #[test]
    fn a_run_sent_to_an_idle_engine_may_wait_for_the_card_to_wake() {
        let mut share = Share::default();
        let mut now = MS;
        for waited in [40 * US, 62 * US, 300 * US, 1_200 * US, 1_900 * US] {
            now = retire(&mut share, after(now, 16 * MS, waited, 2 * MS));
        }
        assert!(!share.contended(now));
    }

    #[test]
    fn the_guest_s_other_processes_between_its_runs_are_not_others() {
        // A game's run each frame, sent long before, and the compositor's two small runs after it,
        // which end just before the game's next run starts.
        let mut share = Share::default();
        let mut own = OwnEnds::default();
        let mut game_end = 26 * MS;
        own.noted(0, game_end);
        for _ in 0..100 {
            let compositor_end = game_end + 518 * US;
            own.noted(0, compositor_end - 282 * US);
            own.noted(0, compositor_end);
            let started = compositor_end + 5 * US;
            let times = RunTimes { sent: game_end - 20 * MS, started, ended: started + 25 * MS };
            share.retired(times, own.last_by(0, started));
            game_end = times.ended;
            own.noted(0, game_end);
        }
        assert!(!share.contended(game_end));
    }

    #[test]
    fn runs_one_after_another_do_not_count_as_waiting_for_others() {
        let mut share = Share::default();
        // Sent together, each starts when the one before ends.
        share.retired(RunTimes { sent: 0, started: 5_000, ended: 2 * MS }, 0);
        share.retired(RunTimes { sent: 1_000, started: 2 * MS + 20_000, ended: 4 * MS }, 2 * MS);
        assert!(!share.contended(4 * MS));
    }

    #[test]
    fn one_late_start_alone_is_the_card_s_own_jitter() {
        let mut share = Share::default();
        let mut now = retire(&mut share, queued(MS, 72 * US, 2 * MS));
        for _ in 0..100 {
            now = retire(&mut share, queued(now, 10 * US, 2 * MS));
        }
        now = retire(&mut share, queued(now, 72 * US, 2 * MS));
        assert!(!share.contended(now));
    }

    #[test]
    fn seeing_others_wait_twice_makes_the_space_send_one_run_at_a_time() {
        let mut share = Share::default();
        let first = retire(&mut share, queued(MS, MS, 2 * MS));
        assert!(!share.contended(first));
        let ended = retire(&mut share, queued(first, 216 * US, 2 * MS));
        assert!(share.contended(ended));
        assert!(!share.may_send(ended + 1, 1), "one at a time");
        assert!(share.may_send(ended + 1, 0), "and no pause");
    }

    #[test]
    fn a_run_sent_while_the_card_is_awake_that_starts_late_waited_for_someone() {
        let mut share = Share::default();
        let mut now = MS;
        for _ in 0..2 {
            now = retire(&mut share, after(now, 30 * US, 200 * US, 2 * MS));
        }
        assert!(share.contended(now));
    }

    #[test]
    fn a_long_wait_on_an_idle_engine_is_someone_else_s_work() {
        let mut share = Share::default();
        let first = retire(&mut share, after(MS, 10 * MS, 5 * MS, 2 * MS));
        let ended = retire(&mut share, after(first, 10 * MS, 5 * MS, 2 * MS));
        assert!(share.contended(ended));
    }

    #[test]
    fn giving_way_ends_once_nobody_has_been_seen_for_a_while() {
        let mut share = Share::default();
        let first = retire(&mut share, queued(MS, MS, 2 * MS));
        let mut now = retire(&mut share, queued(first, MS, 2 * MS));
        let until = now + CONTENDED_FOR_NS;
        // One at a time, each sent as the last one ends, and starting at once.
        while now < until {
            assert!(share.may_send(now, 0) && !share.may_send(now, 1));
            now = retire(&mut share, after(now, 30 * US, 15 * US, 2 * MS));
        }
        assert!(!share.contended(now));
        assert!(share.may_send(now, 5));
    }

    #[test]
    fn a_run_the_card_never_started_changes_nothing() {
        let mut share = Share::default();
        share.retired(RunTimes { sent: 5, started: 0, ended: 0 }, 0);
        assert_eq!(share, Share::default());
    }

    #[test]
    fn own_ends_give_the_last_end_by_a_time_on_that_engine() {
        let mut own = OwnEnds::default();
        assert_eq!(own.last_by(0, 100), 0);
        for end in [30, 10, 20, 20] {
            own.noted(0, end);
        }
        own.noted(1, 25);
        assert_eq!(own.last_by(0, 5), 0);
        assert_eq!(own.last_by(0, 20), 20);
        assert_eq!(own.last_by(0, 29), 20);
        assert_eq!(own.last_by(0, 100), 30);
        assert_eq!(own.last_by(1, 100), 25);
        assert_eq!(own.last_by(2, 100), 0);
    }

    #[test]
    fn own_ends_keep_the_latest() {
        let mut own = OwnEnds::default();
        for end in 1..=(KEPT_ENDS as u64 + 10) {
            own.noted(0, end * 10);
        }
        assert_eq!(own.ends[&0].len(), KEPT_ENDS);
        assert_eq!(own.last_by(0, u64::MAX), (KEPT_ENDS as u64 + 10) * 10);
        assert_eq!(own.last_by(0, 50), 0, "the oldest are gone");
    }
}
