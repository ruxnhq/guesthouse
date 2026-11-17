//! Copies of the guest's last command buffers, for a report of a run that hung the card. Off unless
//! the daemon is started with `GUESTHOUSE_AMD_IB_DUMP`; when it's off nothing here exists.
//!
//! A hang shows in the log only as a fence the card never wrote, so the commands it stopped on are
//! copied when each run is sent, while the guest's pages still hold them. The last few runs of each
//! queue are held in memory. Every ten seconds, when there is something new, they go to
//! `ib-dump-latest.bin` in the run folder, which is in the jail. When a run is lost to a reset, or
//! the kernel refuses a submission with ETIME, they also go to the first free
//! `ib-dump-hung-N.bin`, which nothing here overwrites.
//!
//! The file is text lines, each command buffer's line followed by its bytes:
//!
//! ```text
//! guesthouse ib dump 1
//! card device=0x164e revision=0xc1 family=0x91 gfx=10.3
//! written at=42.590 why=hung queue=0x40021400 fence=0x3
//! run queue=0x40021400 space=4635 engine=0 fence=0x3 at=38.388 hung=1
//! ib address=0x800100000 bytes=0xfc00 copied=0xfc00 from=listed
//! <0xfc00 bytes>
//! end
//! ```
//!
//! `at` is seconds since the daemon started, as in its verbose log. `from=chained` is a command
//! buffer an INDIRECT_BUFFER packet in another one names; `copied=0` is one the CPU couldn't read, or past the
//! limit of a run.

use crate::submit::Ib;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The daemon's switch.
pub const VARIABLE: &str = "GUESTHOUSE_AMD_IB_DUMP";

/// Runs held for each queue. A hang is the last run of its queue, as nothing after it finishes.
const RUNS_PER_QUEUE: usize = 8;
/// Bytes held across every queue, which is also about the most a file holds. The run folder is in
/// memory on most systems, so this is memory too.
const HELD_BYTES: usize = 24 << 20;
/// Bytes copied of one run, its chained buffers included.
const RUN_BYTES: usize = 4 << 20;
/// How deep chained command buffers are followed, and how many are followed in one run.
const CHAIN_DEPTH: usize = 4;
const CHAINED_PER_RUN: usize = 64;
/// How often the held runs are written, when there are new ones.
const WRITE_EVERY: Duration = Duration::from_secs(10);
/// Sets kept for hung runs. After a reset the kernel cancels the queue's other runs, which would
/// each ask for one, so a set isn't kept again within this time.
const KEPT_SETS: usize = 4;
const KEPT_APART: Duration = Duration::from_secs(10);
const LATEST: &str = "ib-dump-latest.bin";

/// PM4 type 3 packets that run another command buffer.
const INDIRECT_BUFFER: u32 = 0x3f;
const INDIRECT_BUFFER_CONST: u32 = 0x33;
/// A NOP with the largest count, which the card takes as one dword of padding. AMD's driver pads
/// its command buffers with it.
const NOP_PAD: u32 = 0xffff_1000;

/// One command buffer as copied, or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    pub ib: Ib,
    pub chained: bool,
    /// None when the CPU couldn't read it.
    pub bytes: Option<Vec<u8>>,
}

/// What the log says of a run, so the two can be matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunFacts {
    pub queue: u32,
    pub space: u64,
    pub engine: u32,
    pub fence: u64,
}

#[derive(Debug)]
struct Run {
    facts: RunFacts,
    at: Duration,
    ibs: Vec<Copied>,
}

impl Run {
    fn bytes(&self) -> usize {
        self.ibs.iter().map(|ib| ib.bytes.as_ref().map_or(0, Vec::len)).sum()
    }
}

/// See the module documentation.
#[derive(Debug)]
pub struct Keeper {
    folder: PathBuf,
    started: Instant,
    card: String,
    queues: BTreeMap<u32, VecDeque<Run>>,
    held: usize,
    written: Option<Instant>,
    new_runs: bool,
    kept: Option<Instant>,
    said_full: bool,
}

impl Keeper {
    /// Writes into `folder`. `started` is when the log's clock started, and `card` names the card
    /// for the decoder.
    pub fn new(folder: &Path, started: Instant, card: String) -> Self {
        log::warn!(
            "{VARIABLE} is on: the last {RUNS_PER_QUEUE} graphics and compute runs of each queue are copied, \
             at most {} MiB, and written to {} every {} s, and to ib-dump-hung-N.bin there when a run hangs",
            HELD_BYTES >> 20,
            folder.join(LATEST).display(),
            WRITE_EVERY.as_secs()
        );
        Self {
            folder: folder.to_owned(),
            started,
            card,
            queues: BTreeMap::new(),
            held: 0,
            written: None,
            new_runs: false,
            kept: None,
            said_full: false,
        }
    }

    /// Copies a run's command buffers through `read`, which gives the bytes at a GPU address, and
    /// those they chain to.
    pub fn note_run(
        &mut self,
        facts: RunFacts,
        listed: &[Ib],
        now: Instant,
        read: &mut dyn FnMut(&Ib) -> Option<Vec<u8>>,
    ) {
        let run = Run { facts, at: now.saturating_duration_since(self.started), ibs: copy_run(listed, read) };
        self.held += run.bytes();
        self.queues.entry(facts.queue).or_default().push_back(run);
        let queue = &mut self.queues.get_mut(&facts.queue).expect("just added");
        if queue.len() > RUNS_PER_QUEUE {
            let old = queue.pop_front().expect("longer than the limit");
            self.held -= old.bytes();
        }
        self.make_room();
        self.new_runs = true;
    }

    /// Takes old runs off the queue holding the most bytes. Not the oldest queue: a queue that hung
    /// sends nothing more, and its runs are the ones wanted.
    fn make_room(&mut self) {
        while self.held > HELD_BYTES {
            let biggest =
                self.queues.iter().max_by_key(|(_, runs)| runs.iter().map(Run::bytes).sum::<usize>());
            let Some((&queue, _)) = biggest else { return };
            let runs = self.queues.get_mut(&queue).expect("just found");
            let Some(old) = runs.pop_front() else { return };
            self.held -= old.bytes();
            if runs.is_empty() {
                self.queues.remove(&queue);
            }
        }
    }

    /// Writes the held runs to the latest file, if there are new ones and it's time.
    pub fn now_and_then(&mut self, now: Instant) {
        if !self.new_runs
            || self.written.is_some_and(|last| now.saturating_duration_since(last) < WRITE_EVERY)
        {
            return;
        }
        self.written = Some(now);
        self.new_runs = false;
        let file = self.folder.join(LATEST);
        if let Err(error) = write_file(&file, &self.contents(now, "latest", None)) {
            log::warn!("writing {}: {error}", file.display());
        }
    }

    /// Keeps the held runs in a file of their own, for the run on `queue` with fence value `fence`.
    pub fn keep_for(&mut self, queue: u32, fence: u64, why: &str, now: Instant) {
        if self.kept.is_some_and(|last| now.saturating_duration_since(last) < KEPT_APART) {
            return;
        }
        self.kept = Some(now);
        let free = (1..=KEPT_SETS)
            .map(|n| self.folder.join(format!("ib-dump-hung-{n}.bin")))
            .find(|file| !file.exists());
        let Some(file) = free else {
            if !self.said_full {
                self.said_full = true;
                log::warn!(
                    "{KEPT_SETS} command buffer sets are kept in {} already, so no more are; take them out to keep more",
                    self.folder.display()
                );
            }
            return;
        };
        match write_file(&file, &self.contents(now, why, Some((queue, fence)))) {
            Ok(()) => log::warn!(
                "kept the last command buffers of every queue in {}, for the run on queue {queue:#x} with fence \
                 value {fence:#x} ({why})",
                file.display()
            ),
            Err(error) => log::warn!("writing {}: {error}", file.display()),
        }
    }

    fn contents(&self, now: Instant, why: &str, hung: Option<(u32, u64)>) -> Vec<u8> {
        let at = seconds(now.saturating_duration_since(self.started));
        let mut out = format!("guesthouse ib dump 1\n{}\nwritten at={at} why={why}", self.card).into_bytes();
        if let Some((queue, fence)) = hung {
            out.extend(format!(" queue={queue:#x} fence={fence:#x}").as_bytes());
        }
        out.push(b'\n');
        for run in self.queues.values().flatten() {
            let RunFacts { queue, space, engine, fence } = run.facts;
            let is_hung = u8::from(hung == Some((queue, fence)));
            let at = seconds(run.at);
            out.extend(
                format!("run queue={queue:#x} space={space} engine={engine} fence={fence:#x} at={at} hung={is_hung}\n")
                    .as_bytes(),
            );
            for copied in &run.ibs {
                let bytes = copied.bytes.as_deref().unwrap_or_default();
                let from = if copied.chained { "chained" } else { "listed" };
                out.extend(
                    format!(
                        "ib address={:#x} bytes={:#x} copied={:#x} from={from}\n",
                        copied.ib.address,
                        copied.ib.bytes,
                        bytes.len()
                    )
                    .as_bytes(),
                );
                out.extend(bytes);
                out.push(b'\n');
            }
        }
        out.extend(b"end\n");
        out
    }
}

fn seconds(time: Duration) -> String {
    format!("{:.3}", time.as_secs_f64())
}

/// Writes beside the file and moves it in, so a reader never finds half of one.
fn write_file(file: &Path, contents: &[u8]) -> io::Result<()> {
    let part = file.with_extension("part");
    fs::write(&part, contents)?;
    fs::rename(&part, file)
}

/// The listed buffers, then those they chain to, as far as the limits go.
fn copy_run(listed: &[Ib], read: &mut dyn FnMut(&Ib) -> Option<Vec<u8>>) -> Vec<Copied> {
    let mut copied: Vec<Copied> = Vec::new();
    let mut total = 0usize;
    let mut wanted: VecDeque<(Ib, bool, usize)> = listed.iter().map(|&ib| (ib, false, 0)).collect();
    let mut chained = 0;
    while let Some((ib, is_chained, depth)) = wanted.pop_front() {
        let bytes = (total + ib.bytes as usize <= RUN_BYTES).then(|| read(&ib)).flatten();
        total += bytes.as_ref().map_or(0, Vec::len);
        if let Some(bytes) = &bytes
            && depth < CHAIN_DEPTH
        {
            for next in chains_in(bytes) {
                let seen = next == ib
                    || copied.iter().any(|done| done.ib == next)
                    || wanted.iter().any(|(ib, ..)| *ib == next);
                if !seen && chained < CHAINED_PER_RUN {
                    chained += 1;
                    wanted.push_back((next, true, depth + 1));
                }
            }
        }
        copied.push(Copied { ib, chained: is_chained, bytes });
    }
    copied
}

/// The command buffers a PM4 stream runs through INDIRECT_BUFFER packets. The walk stops at
/// anything that isn't a type 3 or type 2 packet, as past that the lengths can't be trusted.
pub fn chains_in(bytes: &[u8]) -> Vec<Ib> {
    let words: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|&word| u32::from_le_bytes(word)).collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let header = words[at];
        if header == NOP_PAD {
            at += 1;
            continue;
        }
        match header >> 30 {
            3 => {
                let opcode = (header >> 8) & 0xff;
                let count = ((header >> 16) & 0x3fff) as usize + 1;
                if matches!(opcode, INDIRECT_BUFFER | INDIRECT_BUFFER_CONST) && at + 3 < words.len() {
                    let address = u64::from(words[at + 1] & !3) | (u64::from(words[at + 2] & 0xffff) << 32);
                    let dwords = words[at + 3] & 0xf_ffff;
                    found.push(Ib { address, bytes: dwords * 4 });
                }
                at += 1 + count;
            }
            2 => at += 1,
            _ => break,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    /// A NOP with one dword, then an INDIRECT_BUFFER to 0x1_2345_6000 for 0x40 dwords.
    fn with_chain() -> Vec<u8> {
        le(&[0xc000_1000, 0xdead_beef, 0xc002_3f00, 0x2345_6000, 0x0001, 0x0010_0040])
    }

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gh-ib-dump-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn facts(queue: u32, fence: u64) -> RunFacts {
        RunFacts { queue, space: 9, engine: 0, fence }
    }

    #[test]
    fn an_indirect_buffer_packet_is_found_past_other_packets() {
        assert_eq!(chains_in(&with_chain()), vec![Ib { address: 0x1_2345_6000, bytes: 0x100 }]);
    }

    #[test]
    fn padding_is_one_dword_and_not_a_long_nop() {
        let mut words = le(&[NOP_PAD, NOP_PAD]);
        words.extend(with_chain());
        assert_eq!(chains_in(&words).len(), 1);
    }

    #[test]
    fn a_type_0_packet_ends_the_walk() {
        let mut words = le(&[0x0000_0000]);
        words.extend(with_chain());
        assert!(chains_in(&words).is_empty());
    }

    #[test]
    fn a_chained_buffer_is_copied_after_the_listed_ones() {
        let listed = [Ib { address: 0x1000, bytes: 24 }];
        let mut read =
            |ib: &Ib| Some(if ib.address == 0x1000 { with_chain() } else { vec![7; ib.bytes as usize] });
        let copied = copy_run(&listed, &mut read);
        assert_eq!(copied.len(), 2);
        assert!(copied[1].chained);
        assert_eq!(copied[1].ib.address, 0x1_2345_6000);
        assert_eq!(copied[1].bytes.as_ref().map(Vec::len), Some(0x100));
    }

    #[test]
    fn a_buffer_that_chains_to_itself_is_copied_once() {
        let looped = le(&[0xc002_3f00, 0x1000, 0, 6]);
        let mut read = |_: &Ib| Some(looped.clone());
        assert_eq!(copy_run(&[Ib { address: 0x1000, bytes: 24 }], &mut read).len(), 1);
    }

    #[test]
    fn only_the_last_runs_of_a_queue_are_held() {
        let mut keeper = Keeper::new(&folder("last"), Instant::now(), "card".into());
        let mut read = |ib: &Ib| Some(vec![0; ib.bytes as usize]);
        for fence in 1..=20 {
            keeper.note_run(facts(1, fence), &[Ib { address: 0x1000, bytes: 64 }], Instant::now(), &mut read);
        }
        let runs = &keeper.queues[&1];
        assert_eq!(runs.len(), RUNS_PER_QUEUE);
        assert_eq!(runs[0].facts.fence, 20 - RUNS_PER_QUEUE as u64 + 1);
        assert_eq!(keeper.held, RUNS_PER_QUEUE * 64);
    }

    #[test]
    fn a_busy_queue_gives_way_before_a_quiet_one() {
        let mut keeper = Keeper::new(&folder("room"), Instant::now(), "card".into());
        let mut read = |ib: &Ib| Some(vec![0; ib.bytes as usize]);
        let big = [Ib { address: 0x1000, bytes: 4 << 20 }];
        keeper.note_run(facts(1, 1), &big, Instant::now(), &mut read);
        for fence in 1..=8 {
            keeper.note_run(facts(2, fence), &big, Instant::now(), &mut read);
        }
        assert!(keeper.held <= HELD_BYTES);
        assert_eq!(keeper.queues[&1].len(), 1);
    }

    #[test]
    fn a_hung_run_keeps_a_set_that_is_not_overwritten() {
        let folder = folder("hung");
        let started = Instant::now();
        let mut keeper = Keeper::new(&folder, started, "card device=0x164e".into());
        let mut read = |_: &Ib| Some(with_chain());
        keeper.note_run(facts(0x4002_1400, 3), &[Ib { address: 0x1000, bytes: 24 }], started, &mut read);
        keeper.keep_for(0x4002_1400, 3, "hung", started);
        // The runs the reset cancels ask again at once, and get nothing.
        keeper.keep_for(0x4002_1400, 4, "hung", started + Duration::from_secs(1));
        let kept = fs::read(folder.join("ib-dump-hung-1.bin")).unwrap();
        assert!(!folder.join("ib-dump-hung-2.bin").exists());
        let text = String::from_utf8_lossy(&kept);
        assert!(text.starts_with("guesthouse ib dump 1\ncard device=0x164e\nwritten at=0.000 why=hung"));
        assert!(text.contains("run queue=0x40021400 space=9 engine=0 fence=0x3 at=0.000 hung=1\n"));
        assert!(text.contains("ib address=0x1000 bytes=0x18 copied=0x18 from=listed\n"));
        assert!(text.contains("from=chained"));
        assert!(text.ends_with("end\n"));
        // A later daemon finds the first name taken and uses the next.
        let mut later = Keeper::new(&folder, started, "card".into());
        later.keep_for(1, 1, "ETIME", started);
        assert!(folder.join("ib-dump-hung-2.bin").exists());
        assert_eq!(fs::read(folder.join("ib-dump-hung-1.bin")).unwrap(), kept);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn the_latest_file_is_written_only_when_there_is_something_new() {
        let folder = folder("latest");
        let started = Instant::now();
        let mut keeper = Keeper::new(&folder, started, "card".into());
        keeper.now_and_then(started);
        assert!(!folder.join(LATEST).exists());
        let mut read = |_: &Ib| None;
        keeper.note_run(facts(1, 1), &[Ib { address: 0x1000, bytes: 24 }], started, &mut read);
        keeper.now_and_then(started);
        let text = fs::read_to_string(folder.join(LATEST)).unwrap();
        assert!(text.contains("ib address=0x1000 bytes=0x18 copied=0x0 from=listed\n"));
        fs::remove_dir_all(folder).unwrap();
    }
}
