//! Giving way to the card's other users, so a guest doesn't take the engine from them.
//!
//! amdgpu's scheduler runs jobs in the order they were sent, whoever sent them: its default policy
//! is FIFO, also in 7.4, and the fair one is still an experiment. A guest sends long jobs, several
//! ahead, while a small app on the host, in a container or in another VM sends short ones, one at a
//! time. Each small job then waits behind everything the guests sent before it, and the small app
//! gets almost none of the engine's time: on a 780M two Windows guests left a container's vkmark
//! 163 frames a second of its 4624 (#276).
//!
//! So a guest gives way when it sees others waiting. Each run notes the card's clock as it starts
//! and ends. If the card ran someone else's work between the run's being sent, or the space's last
//! run on that engine ending, and the run starting, others are waiting. Then the space sends one run
//! at a time on that engine, and after each one leaves the engine to the others for a while: some
//! part of the run's own time, which grows while others are still found waiting and shrinks when
//! they aren't. With nobody else on the card, nothing changes: runs go as before, many at once.
//!
//! The numbers come from stand-in clients on the DMA engine of a Radeon iGPU, see the `amd-share`
//! binary. A light client went from 72 jobs a second beside two heavy ones to over 1200, the three
//! then had about a third of the engine each, and the two heavy ones alone lost nothing.

/// The daemon's switch that turns giving way on. It is off without it: as built, a guest alone on
/// the card can take its own other processes, or the card waking from idle, for others (#327).
pub const VARIABLE: &str = "GUESTHOUSE_AMD_GIVE_WAY";

/// How much longer than its own sending a run must have waited to have waited for someone else.
/// A space's runs alone wait under 25 µs.
pub const WAITED_NS: u64 = 50_000;
/// How long after others were last seen waiting the space still gives way.
pub const CONTENDED_FOR_NS: u64 = 200_000_000;
/// How much of a run's time is added to the pause after it while others are still waiting.
const GROW: f64 = 0.25;
/// What the pause is multiplied by when nobody was waiting.
const SHRINK: f64 = 0.25;
/// The longest pause, in runs' times: a space keeps at least a quarter of a busy engine.
const MOST: f64 = 3.0;

/// One engine of one space: how much it gives way. Times are the card's clock in nanoseconds.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Share {
    factor: f64,
    contended_at: Option<u64>,
    last_end: u64,
    not_before: u64,
}

/// When a run was sent, after the call returned, and when the card started and ended it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTimes {
    pub sent: u64,
    pub started: u64,
    pub ended: u64,
}

impl Share {
    /// A run on this engine has retired. Runs are given in the order they were sent.
    pub fn retired(&mut self, times: RunTimes) {
        let RunTimes { sent, started, ended } = times;
        if started == 0 || ended < started {
            // The card didn't write both: the run was ended before it got there.
            return;
        }
        let waited = started.saturating_sub(sent.max(self.last_end));
        self.last_end = self.last_end.max(ended);
        let others_waiting = waited > WAITED_NS;
        if others_waiting {
            self.contended_at = Some(ended);
        }
        if !self.contended(ended) {
            self.factor = 0.0;
            self.not_before = 0;
            return;
        }
        self.factor = if others_waiting { (self.factor + GROW).min(MOST) } else { self.factor * SHRINK };
        self.not_before = ended + ((ended - started) as f64 * self.factor) as u64;
    }

    /// Whether others were seen waiting lately.
    pub fn contended(&self, now: u64) -> bool {
        self.contended_at.is_some_and(|at| now.saturating_sub(at) < CONTENDED_FOR_NS)
    }

    /// Whether a run may be sent at `now`, with `on_card` of this engine's runs still on the card.
    pub fn may_send(&self, now: u64, on_card: usize) -> bool {
        !self.contended(now) || (on_card == 0 && now >= self.not_before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    /// A run of `took` sent at `sent`, starting `waited` after that.
    fn run(sent: u64, waited: u64, took: u64) -> RunTimes {
        RunTimes { sent, started: sent + waited, ended: sent + waited + took }
    }

    #[test]
    fn alone_on_the_card_runs_go_as_before() {
        let mut share = Share::default();
        let mut now = 0;
        for _ in 0..100 {
            share.retired(run(now, 10_000, 2 * MS));
            now += 2 * MS + 10_000;
        }
        assert!(!share.contended(now));
        assert!(share.may_send(now, 7), "many at once");
    }

    #[test]
    fn runs_one_after_another_do_not_count_as_waiting_for_others() {
        let mut share = Share::default();
        // Sent together, each starts when the one before ends.
        share.retired(RunTimes { sent: 0, started: 5_000, ended: 2 * MS });
        share.retired(RunTimes { sent: 1_000, started: 2 * MS + 20_000, ended: 4 * MS });
        assert!(!share.contended(4 * MS));
    }

    #[test]
    fn seeing_others_wait_makes_the_space_send_one_run_at_a_time_and_pause_after_it() {
        let mut share = Share::default();
        share.retired(run(0, MS, 2 * MS));
        let ended = 3 * MS;
        assert!(share.contended(ended));
        assert!(!share.may_send(ended + 1, 1), "one at a time");
        assert!(!share.may_send(ended + 1, 0), "and a pause of a quarter of the run first");
        assert!(share.may_send(ended + MS / 2, 0));
    }

    #[test]
    fn the_pause_grows_while_others_keep_waiting_up_to_three_runs_long() {
        let mut share = Share::default();
        let mut now = 0;
        for _ in 0..50 {
            share.retired(run(now, MS, 2 * MS));
            now += 3 * MS;
        }
        assert!(!share.may_send(now + 6 * MS - 1, 0));
        assert!(share.may_send(now + 6 * MS, 0));
    }

    #[test]
    fn the_pause_shrinks_when_nobody_was_waiting_and_ends_once_nobody_has_been_for_a_while() {
        let mut share = Share::default();
        let mut now = 0;
        for _ in 0..8 {
            share.retired(run(now, MS, 2 * MS));
            now += 3 * MS;
        }
        share.retired(run(now, 0, 2 * MS));
        now += 2 * MS;
        assert!(share.may_send(now + MS, 0), "the pause shrank to half a run");
        let later = now + CONTENDED_FOR_NS;
        share.retired(run(later, 0, 2 * MS));
        assert!(!share.contended(later + 2 * MS));
        assert!(share.may_send(later + 2 * MS, 5));
    }

    #[test]
    fn a_run_the_card_never_started_changes_nothing() {
        let mut share = Share::default();
        share.retired(RunTimes { sent: 5, started: 0, ended: 0 });
        assert_eq!(share, Share::default());
    }
}
