//! Each process's part of the card, as the vendor's library counts it, made into the table the app
//! reads. The library gives each process's newest sample, of one second of the driver's, stamped
//! with the CPU's clock, and keeps old ones, so a sample is only believed when it ended within the
//! last moment. A process the driver lists with no such sample used none of the card in it, as the
//! driver only samples a process that used some. Where the samples can't be placed in time, nothing
//! is said about any process's use.

pub use guesthouse_sensors::card_processes::{Process, ProcessTable, Use};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::time::Duration;

/// One process's use of the card in one of the driver's sample periods, in percent of that period.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sample {
    pub pid: u32,
    /// When the period ended, in the driver's microseconds.
    pub at: u64,
    /// The shader cores, which draw and compute.
    pub sm: u32,
    /// How busy the memory was with its reads and writes: not how much it holds.
    pub memory: u32,
    pub encoder: u32,
    pub decoder: u32,
}

/// What asking for the samples since a moment gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samples {
    Read(Vec<Sample>),
    /// The driver has no sample newer than that, which it says when no process used the card since.
    NoneSince,
    /// The library can't say: an old one, a card it doesn't sample, or a failure.
    Unavailable,
}

/// A process the card holds objects for, and the video memory the driver charges it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMemory {
    pub pid: u32,
    pub bytes: Option<u64>,
}

/// Something that can read each process's part of the card.
pub trait ProcessSource {
    /// The samples whose periods ended after `since`, in the driver's microseconds, or all it keeps
    /// for 0.
    fn samples_since(&mut self, since: u64) -> Samples;
    /// The processes on the card. A process in both of the driver's lists is in this twice.
    fn processes(&mut self) -> Option<Vec<ProcessMemory>>;
}

/// The driver samples each process once a second, so a sample that ended longer ago than this is not
/// the last second's.
const FRESH: Duration = Duration::from_secs(2);
/// The driver's samples are stamped with the CPU's clock in microseconds since the epoch. One stamped
/// more than this ahead of now, or further behind than the driver keeps a sample for, says its clock
/// is another, and then no sample can be placed in time.
const AHEAD: Duration = Duration::from_secs(1);
const BEHIND: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MICROS_PER_MILLI: u64 = 1000;

fn micros(duration: Duration) -> u64 {
    duration.as_micros() as u64
}

/// The newest sample of each process from the last moment, or `None` if the samples can't be
/// trusted to say anything about it. `now` is the CPU's clock in microseconds since the epoch.
fn fresh_samples(samples: &Samples, now: u64) -> Option<BTreeMap<u32, Sample>> {
    let list = match samples {
        Samples::Read(list) => list.as_slice(),
        Samples::NoneSince => &[],
        Samples::Unavailable => return None,
    };
    let placed = |sample: &Sample| {
        sample.at <= now.saturating_add(micros(AHEAD)) && sample.at.saturating_add(micros(BEHIND)) >= now
    };
    if !list.iter().all(placed) {
        return None;
    }
    let mut newest: BTreeMap<u32, Sample> = BTreeMap::new();
    for sample in list.iter().filter(|sample| sample.at.saturating_add(micros(FRESH)) >= now) {
        let kept = newest.entry(sample.pid).or_insert(*sample);
        if sample.at > kept.at {
            *kept = *sample;
        }
    }
    Some(newest)
}

/// The table for one look at the card. The busiest processes come first, so a card with more
/// processes than the table has room for loses the idle ones.
pub fn table(samples: &Samples, processes: Option<&[ProcessMemory]>, now: u64) -> ProcessTable {
    let fresh = fresh_samples(samples, now);
    let mut memory: BTreeMap<u32, Option<u64>> = BTreeMap::new();
    // A process in both of the driver's lists is charged the same in each.
    for process in processes.into_iter().flatten() {
        let bytes = memory.entry(process.pid).or_default();
        *bytes = (*bytes).max(process.bytes);
    }
    let mut sampled: Vec<&Sample> = fresh.iter().flat_map(BTreeMap::values).collect();
    sampled.sort_by_key(|sample| (Reverse(sample.sm), sample.pid));
    let busy = sampled.iter().map(|sample| Process {
        pid: sample.pid,
        used: Some(Use { sm: sample.sm, encoder: sample.encoder, decoder: sample.decoder }),
        bytes: memory.get(&sample.pid).copied().flatten(),
    });
    let has_sample = |pid: &u32| fresh.as_ref().is_some_and(|fresh| fresh.contains_key(pid));
    let quiet = memory.iter().filter(|(pid, _)| !has_sample(pid));
    let quiet = quiet.map(|(pid, bytes)| Process { pid: *pid, used: None, bytes: *bytes });
    ProcessTable {
        taken_at: now / MICROS_PER_MILLI,
        samples_read: fresh.is_some(),
        processes_read: processes.is_some(),
        left_out: false,
        processes: busy.chain(quiet).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A moment as the driver stamps it, from the reading taken on an RTX 5090 with driver 620.73.04.
    const NOW: u64 = 1_795_465_129_822_361;
    const SECOND: u64 = 1_000_000;
    const DAEMON: u32 = 2_462_537;
    const DESKTOP: u32 = 56_207;

    fn sample(pid: u32, at: u64, sm: u32) -> Sample {
        Sample { pid, at, sm, memory: 16, encoder: 0, decoder: 0 }
    }

    fn listed(pid: u32, mib: u64) -> ProcessMemory {
        ProcessMemory { pid, bytes: Some(mib << 20) }
    }

    #[test]
    fn a_guest_under_load_has_its_last_second_and_its_charge() {
        let samples =
            Samples::Read(vec![sample(DAEMON, NOW - SECOND / 3, 12), sample(DESKTOP, NOW - 80_000, 2)]);
        // The driver lists a process that draws and computes in both of its lists.
        let processes = [listed(DAEMON, 1848), listed(DESKTOP, 252), listed(DAEMON, 1848)];
        let table = table(&samples, Some(&processes), NOW);
        assert!(table.samples_read && table.processes_read);
        assert_eq!(table.taken_at, NOW / 1000);
        let daemon = table.process(DAEMON).unwrap();
        assert_eq!(daemon.used, Some(Use { sm: 12, encoder: 0, decoder: 0 }));
        assert_eq!(daemon.bytes, Some(1848 << 20));
        assert_eq!(table.processes.len(), 2);
        assert_eq!(table.processes[0].pid, DAEMON, "the busiest first");
    }

    #[test]
    fn an_idle_guest_is_listed_with_no_sample() {
        let table = table(&Samples::NoneSince, Some(&[listed(DAEMON, 940)]), NOW);
        assert!(table.samples_read);
        assert_eq!(table.process(DAEMON).unwrap().used, None);
        assert_eq!(table.process(DAEMON).unwrap().bytes, Some(940 << 20));
    }

    #[test]
    fn a_sample_from_before_the_last_moment_is_not_the_last_seconds() {
        // As the first reading gave, with a sample of one process more than four seconds old.
        let samples =
            Samples::Read(vec![sample(DAEMON, NOW - 4_280_315, 1), sample(DESKTOP, NOW - 80_023, 1)]);
        let table = table(&samples, Some(&[listed(DAEMON, 940)]), NOW);
        assert_eq!(table.process(DAEMON).unwrap().used, None);
        assert!(table.process(DESKTOP).unwrap().used.is_some(), "a sample counts even if the list missed it");
    }

    #[test]
    fn the_newest_of_several_samples_is_the_one() {
        let samples = Samples::Read(vec![sample(DAEMON, NOW - SECOND, 10), sample(DAEMON, NOW - 1000, 11)]);
        assert_eq!(table(&samples, None, NOW).process(DAEMON).unwrap().used.unwrap().sm, 11);
    }

    #[test]
    fn samples_on_another_clock_say_nothing() {
        let ahead = Samples::Read(vec![sample(DAEMON, NOW + 5 * SECOND, 50)]);
        let since_boot = Samples::Read(vec![sample(DAEMON, 86_400 * SECOND, 50)]);
        for samples in [ahead, since_boot] {
            let table = table(&samples, Some(&[listed(DAEMON, 940)]), NOW);
            assert!(!table.samples_read);
            assert_eq!(table.process(DAEMON).unwrap().used, None);
        }
    }

    #[test]
    fn a_library_that_cant_say_reads_nothing() {
        let table = table(&Samples::Unavailable, None, NOW);
        assert!(!table.samples_read && !table.processes_read);
        assert!(table.processes.is_empty());
    }
}
