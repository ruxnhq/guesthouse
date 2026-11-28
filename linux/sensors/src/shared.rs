//! The snapshot in a small file in shared memory, written by one process and read by any. See
//! [`crate::words`] for how a reader never waits.

use crate::words::SharedWords;
use crate::{Snapshot, SnapshotReader, is_fresh, milliseconds_since_epoch};
use std::io;
use std::path::Path;

const NOT_READ: u64 = u64::MAX;
const FIELD_COUNT: usize = 13;

fn encode(value: Option<u32>) -> u64 {
    value.map_or(NOT_READ, u64::from)
}

fn decode(word: u64) -> Option<u32> {
    (word != NOT_READ).then_some(word as u32)
}

fn encode_long(value: Option<u64>) -> u64 {
    value.unwrap_or(NOT_READ)
}

fn decode_long(word: u64) -> Option<u64> {
    (word != NOT_READ).then_some(word)
}

fn to_words(snapshot: &Snapshot) -> [u64; FIELD_COUNT] {
    [
        snapshot.taken_at,
        encode(snapshot.temperature_celsius),
        encode(snapshot.load_percent),
        encode(snapshot.performance_state),
        encode(snapshot.power_milliwatts),
        encode(snapshot.power_limit_milliwatts),
        encode(snapshot.graphics_megahertz),
        encode(snapshot.graphics_most_megahertz),
        encode(snapshot.memory_megahertz),
        encode(snapshot.memory_most_megahertz),
        encode(snapshot.fan_percent),
        encode_long(snapshot.memory_used_bytes),
        encode_long(snapshot.memory_total_bytes),
    ]
}

fn from_words(words: [u64; FIELD_COUNT]) -> Snapshot {
    Snapshot {
        taken_at: words[0],
        temperature_celsius: decode(words[1]),
        load_percent: decode(words[2]),
        performance_state: decode(words[3]),
        power_milliwatts: decode(words[4]),
        power_limit_milliwatts: decode(words[5]),
        graphics_megahertz: decode(words[6]),
        graphics_most_megahertz: decode(words[7]),
        memory_megahertz: decode(words[8]),
        memory_most_megahertz: decode(words[9]),
        fan_percent: decode(words[10]),
        memory_used_bytes: decode_long(words[11]),
        memory_total_bytes: decode_long(words[12]),
    }
}

/// See the module documentation.
pub struct SharedSnapshot {
    words: SharedWords<FIELD_COUNT>,
}

impl SharedSnapshot {
    /// Opens the file, making it if needed. Anyone else's file at this path is refused.
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self { words: SharedWords::open(path)? })
    }

    /// Opens a file a daemon has made, for a reader that must not make one.
    pub fn open_to_read(path: &Path) -> io::Result<Self> {
        Ok(Self { words: SharedWords::open_existing(path)? })
    }

    /// Another handle on the same snapshot, with a lock of its own, as it is a new open of the file.
    /// Take the lock on one of these: the mapping keeps the first open alive, and a lock on that
    /// would outlast the handle.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { words: self.words.try_clone()? })
    }

    /// Waits until no other process is reading the card. The lock lasts until this process exits.
    pub fn wait_for_lock(&self) -> io::Result<()> {
        self.words.wait_for_lock()
    }

    /// Only the holder of the lock writes.
    pub fn publish(&self, snapshot: &Snapshot) {
        self.words.publish(to_words(snapshot));
    }

    /// The snapshot as it stands, however old, or `None` if a write kept getting in the way.
    fn read(&self) -> Option<Snapshot> {
        self.words.read().map(from_words)
    }
}

impl SnapshotReader for SharedSnapshot {
    fn latest(&self) -> Option<Snapshot> {
        self.read().filter(|snapshot| is_fresh(snapshot, milliseconds_since_epoch()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::path::PathBuf;

    fn temporary(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("guesthouse-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    fn now_snapshot() -> Snapshot {
        Snapshot {
            taken_at: milliseconds_since_epoch(),
            temperature_celsius: Some(40),
            load_percent: Some(7),
            performance_state: Some(0),
            memory_total_bytes: Some(32 << 30),
            ..Snapshot::default()
        }
    }

    #[test]
    fn an_empty_file_has_nothing_fresh() {
        let path = temporary("empty");
        assert_eq!(SharedSnapshot::open(&path).unwrap().latest(), None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_second_process_sees_what_the_first_wrote() {
        let path = temporary("two");
        let writer = SharedSnapshot::open(&path).unwrap();
        let reader = SharedSnapshot::open(&path).unwrap();
        let snapshot = now_snapshot();
        writer.publish(&snapshot);
        assert_eq!(reader.latest(), Some(snapshot));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_value_not_read_stays_not_read() {
        let path = temporary("unread");
        let shared = SharedSnapshot::open(&path).unwrap();
        shared.publish(&now_snapshot());
        assert_eq!(shared.latest().unwrap().fan_percent, None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_old_snapshot_is_not_used() {
        let path = temporary("old");
        let shared = SharedSnapshot::open(&path).unwrap();
        let old = Snapshot { taken_at: milliseconds_since_epoch() - 60_000, ..now_snapshot() };
        shared.publish(&old);
        assert_eq!(shared.latest(), None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_write_cut_short_reads_as_nothing_until_the_next_one() {
        let path = temporary("cut");
        let shared = SharedSnapshot::open(&path).unwrap();
        shared.publish(&now_snapshot());
        shared.words.cut_a_write_short();
        assert_eq!(shared.latest(), None);
        shared.publish(&now_snapshot());
        assert!(shared.latest().is_some());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_clone_has_a_lock_of_its_own() {
        let path = temporary("lock");
        let opened = SharedSnapshot::open(&path).unwrap();
        let first = opened.try_clone().unwrap();
        let second = opened.try_clone().unwrap();
        first.wait_for_lock().unwrap();
        // SAFETY: a plain call on a file this owns.
        let taken = unsafe { libc::flock(second.words.file().as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(taken, -1, "the lock is held");
        drop(first);
        second.wait_for_lock().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_file_others_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let path = temporary("open");
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(SharedSnapshot::open(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_link_is_not_followed() {
        let (target, link) = (temporary("target"), temporary("link"));
        fs::write(&target, b"kept").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(SharedSnapshot::open(&link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"kept");
        fs::remove_file(link).unwrap();
        fs::remove_file(target).unwrap();
    }
}
