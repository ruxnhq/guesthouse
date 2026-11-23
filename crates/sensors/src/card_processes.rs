//! Each process's part of one card, as the card's own driver counts it: how busy it kept the card
//! in the driver's last sample of it, and the video memory the driver charges it with. The daemon
//! that reads the card's snapshot fills this too, for every process on the card, and the app looks
//! up each machine's processes in it. It is for drivers that keep these figures per process and not
//! per client, as NVIDIA's does.
//!
//! The daemon that writes it serves a guest, so the app takes nothing in it on trust: the count is
//! held to the table's room and every share to a whole.

use crate::words::SharedWords;
use crate::{milliseconds_since_epoch, snapshot_path};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The most processes the table holds. A desktop and a few guests are a few dozen at most.
pub const ROOM: usize = 64;
/// The writer reads each second, so a table older than this means it has stopped, and a figure in it
/// is no longer the last second's.
pub const OLDEST_USABLE: Duration = Duration::from_secs(3);
/// Clocks can step, so a table stamped up to this far ahead still counts.
const AHEAD: Duration = Duration::from_secs(1);
/// The table's layout, in the file's name as with the snapshot's.
pub const LAYOUT: u32 = 1;

const HEADER: usize = 2;
const WORDS_PER_PROCESS: usize = 2;
const FIELD_COUNT: usize = HEADER + ROOM * WORDS_PER_PROCESS;
const NOT_READ: u64 = u64::MAX;
const WHOLE: u32 = 100;
const SAMPLES_READ: u64 = 1;
const PROCESSES_READ: u64 = 1 << 1;
const LEFT_OUT: u64 = 1 << 2;
const COUNT_SHIFT: u32 = 32;
const SAMPLED: u64 = 1 << 32;
const SM_SHIFT: u32 = 40;
const ENCODER_SHIFT: u32 = 48;
const DECODER_SHIFT: u32 = 56;
const PERCENT_MASK: u64 = 0xff;
const PID_MASK: u64 = 0xffff_ffff;

/// What a process used in the driver's last sample of it, in percent of that sample's time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Use {
    /// The shader cores, which draw and compute.
    pub sm: u32,
    pub encoder: u32,
    pub decoder: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    /// Its sample, if the driver has one from the last moment. A process the driver lists with no
    /// such sample used none of the card then, as the driver only samples a process that did.
    pub used: Option<Use>,
    /// The video memory the driver charges it with, if it listed it.
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessTable {
    /// When it was read, in milliseconds since the Unix epoch.
    pub taken_at: u64,
    /// The driver's samples were read, so a listed process without one used nothing.
    pub samples_read: bool,
    /// The driver's lists of processes were read, so a process not in the table isn't on the card.
    pub processes_read: bool,
    /// There were more processes than room, so one not in the table may still be on the card.
    pub left_out: bool,
    pub processes: Vec<Process>,
}

impl ProcessTable {
    /// The process with this pid, if the table has it.
    pub fn process(&self, pid: u32) -> Option<&Process> {
        self.processes.iter().find(|process| process.pid == pid)
    }

    /// Whether the table has every process on the card, so one not in it is on another card or none.
    pub fn is_whole(&self) -> bool {
        self.processes_read && !self.left_out
    }
}

fn percent(word: u64, shift: u32) -> u32 {
    ((word >> shift) & PERCENT_MASK).min(u64::from(WHOLE)) as u32
}

fn to_words(table: &ProcessTable) -> [u64; FIELD_COUNT] {
    let mut words = [0; FIELD_COUNT];
    let shown = &table.processes[..table.processes.len().min(ROOM)];
    let flag = |set: bool, bit: u64| if set { bit } else { 0 };
    words[0] = table.taken_at;
    words[1] = flag(table.samples_read, SAMPLES_READ)
        | flag(table.processes_read, PROCESSES_READ)
        | flag(table.left_out || shown.len() < table.processes.len(), LEFT_OUT)
        | (shown.len() as u64) << COUNT_SHIFT;
    for (index, process) in shown.iter().enumerate() {
        let used = process.used.map_or(0, |used| {
            let share = |value: u32| u64::from(value.min(WHOLE));
            SAMPLED
                | share(used.sm) << SM_SHIFT
                | share(used.encoder) << ENCODER_SHIFT
                | share(used.decoder) << DECODER_SHIFT
        });
        words[HEADER + index * WORDS_PER_PROCESS] = u64::from(process.pid) | used;
        words[HEADER + index * WORDS_PER_PROCESS + 1] = process.bytes.unwrap_or(NOT_READ);
    }
    words
}

fn from_words(words: &[u64; FIELD_COUNT]) -> ProcessTable {
    let count = ((words[1] >> COUNT_SHIFT) as usize).min(ROOM);
    let processes = (0..count).map(|index| {
        let first = words[HEADER + index * WORDS_PER_PROCESS];
        let bytes = words[HEADER + index * WORDS_PER_PROCESS + 1];
        Process {
            pid: (first & PID_MASK) as u32,
            used: (first & SAMPLED != 0).then(|| Use {
                sm: percent(first, SM_SHIFT),
                encoder: percent(first, ENCODER_SHIFT),
                decoder: percent(first, DECODER_SHIFT),
            }),
            bytes: (bytes != NOT_READ).then_some(bytes),
        }
    });
    ProcessTable {
        taken_at: words[0],
        samples_read: words[1] & SAMPLES_READ != 0,
        processes_read: words[1] & PROCESSES_READ != 0,
        left_out: words[1] & LEFT_OUT != 0,
        processes: processes.collect(),
    }
}

/// The file for the processes of the card at `card`, an address as `0000:01:00.0`, beside its
/// snapshot's. With no address it's the one beside the snapshot that has none.
pub fn table_path_for(card: Option<&str>) -> PathBuf {
    // SAFETY: this asks for a number and can't fail.
    let user = unsafe { libc::getuid() };
    let shared = snapshot_path().with_file_name(format!("gpupv-processes-{user}-{LAYOUT}"));
    match card {
        Some(card) => PathBuf::from(format!("{}-{card}", shared.display())),
        None => shared,
    }
}

/// See the module documentation.
pub struct SharedProcessTable(SharedWords<FIELD_COUNT>);

impl SharedProcessTable {
    /// The writer's end, which makes the file if needed. Anyone else's file at this path is refused.
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(SharedWords::open(path)?))
    }

    /// The app's end, which leaves a missing file missing.
    pub fn open_to_read(path: &Path) -> io::Result<Self> {
        Ok(Self(SharedWords::open_existing(path)?))
    }

    /// Only the daemon that reads the card writes, as with the snapshot.
    pub fn publish(&self, table: &ProcessTable) {
        self.0.publish(to_words(table));
    }

    /// The table as it stands, if it was read in the last few seconds. One stamped later than now is
    /// not believed either, as it would never grow old.
    pub fn latest(&self) -> Option<ProcessTable> {
        let table = from_words(&self.0.read()?);
        let now = milliseconds_since_epoch();
        let fresh = table.taken_at <= now.saturating_add(AHEAD.as_millis() as u64)
            && now.saturating_sub(table.taken_at) <= OLDEST_USABLE.as_millis() as u64;
        fresh.then_some(table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temporary(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("guesthouse-processes-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    /// As a real guest's daemon was seen under a light load, beside an idle one.
    fn measured() -> ProcessTable {
        ProcessTable {
            taken_at: milliseconds_since_epoch(),
            samples_read: true,
            processes_read: true,
            left_out: false,
            processes: vec![
                Process {
                    pid: 2_462_537,
                    used: Some(Use { sm: 12, encoder: 0, decoder: 0 }),
                    bytes: Some(1848 << 20),
                },
                Process { pid: 56_207, used: None, bytes: Some(264_183_808) },
                Process { pid: 9, used: Some(Use::default()), bytes: None },
            ],
        }
    }

    #[test]
    fn the_app_reads_what_the_daemon_wrote() {
        let path = temporary("both");
        let daemon = SharedProcessTable::open(&path).unwrap();
        let app = SharedProcessTable::open_to_read(&path).unwrap();
        assert_eq!(app.latest(), None, "nothing written yet");
        daemon.publish(&measured());
        assert_eq!(app.latest(), Some(measured()));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_old_table_or_one_from_later_is_not_used() {
        let path = temporary("old");
        let daemon = SharedProcessTable::open(&path).unwrap();
        for taken_at in [milliseconds_since_epoch() - 10_000, milliseconds_since_epoch() + 60_000] {
            daemon.publish(&ProcessTable { taken_at, ..measured() });
            assert_eq!(daemon.latest(), None);
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn more_processes_than_room_keep_the_first_and_say_some_are_left_out() {
        let many = ProcessTable {
            processes: (1..=ROOM as u32 + 5).map(|pid| Process { pid, used: None, bytes: None }).collect(),
            ..measured()
        };
        let read = from_words(&to_words(&many));
        assert_eq!(read.processes.len(), ROOM);
        assert!(read.left_out && !read.is_whole());
        assert!(measured().is_whole());
    }

    #[test]
    fn a_table_written_wrong_is_held_to_its_room_and_to_a_whole() {
        let mut words = to_words(&measured());
        words[1] |= 0xffff << COUNT_SHIFT;
        words[HEADER] |= PERCENT_MASK << SM_SHIFT;
        let read = from_words(&words);
        assert_eq!(read.processes.len(), ROOM);
        assert_eq!(read.processes[0].used.unwrap().sm, WHOLE);
    }

    #[test]
    fn each_card_has_its_own_table_beside_its_snapshot() {
        let first = table_path_for(Some("0000:01:00.0"));
        assert_ne!(first, table_path_for(Some("0000:02:00.0")));
        assert_eq!(first.parent(), snapshot_path().parent());
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        // SAFETY: this asks for a number and can't fail.
        let user = unsafe { libc::getuid() };
        assert_eq!(name, format!("gpupv-processes-{user}-{LAYOUT}-0000:01:00.0"));
    }
}
