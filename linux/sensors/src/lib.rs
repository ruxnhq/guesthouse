//! The host card's newest readings, shared between processes.
//!
//! One process reads the card and publishes a [`Snapshot`] in a small file in shared memory. Any
//! process of the same user can read it: a daemon answering a guest's performance queries, or the
//! app showing the host's figures over a guest's screen. A reader takes no lock and never waits, and
//! reads the file with positional reads, so it is never faulted by the file changing length.
//! What reads the card, and when, is the reader's own business and lives elsewhere.
//!
//! [`guest_use`] is the same kind of file for one guest: the video memory its daemon's books say it
//! holds, kept in its run folder for the app's share panel. [`card_processes`] is each process's part
//! of one card, for a driver that counts that per process.

// Linux only: it uses flock and getuid. Other systems get an empty crate.
#![cfg(target_os = "linux")]

pub mod card_processes;
pub mod guest_use;
mod shared;
pub mod words;

pub use shared::SharedSnapshot;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A snapshot this old means the reading thread has stopped, so it is not used.
pub const OLDEST_USABLE: Duration = Duration::from_secs(5);
/// The file's layout. It is in the file's name, so a daemon and an app of different builds never
/// read each other's fields as something else: each layout has a file, a lock and a reader of its own.
pub const LAYOUT: u32 = 2;

/// What was last read from the card. `None` is a value that couldn't be read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// When it was taken, in milliseconds since the Unix epoch.
    pub taken_at: u64,
    pub temperature_celsius: Option<u32>,
    /// How much of the last moment the card spent busy.
    pub load_percent: Option<u32>,
    /// The performance state, 0 for the fastest.
    pub performance_state: Option<u32>,
    pub power_milliwatts: Option<u32>,
    pub power_limit_milliwatts: Option<u32>,
    pub graphics_megahertz: Option<u32>,
    pub graphics_most_megahertz: Option<u32>,
    pub memory_megahertz: Option<u32>,
    pub memory_most_megahertz: Option<u32>,
    pub fan_percent: Option<u32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
}

/// Where the newest snapshot can be found. This runs on the message path, so it must not wait.
pub trait SnapshotReader {
    /// The newest snapshot, if there is one recent enough to be true.
    fn latest(&self) -> Option<Snapshot>;
}

pub fn milliseconds_since_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}

/// Clocks can step, so a reading stamped up to this far ahead still counts.
pub const AHEAD: Duration = Duration::from_secs(1);

pub fn is_fresh(snapshot: &Snapshot, now: u64) -> bool {
    snapshot.taken_at <= now.saturating_add(AHEAD.as_millis() as u64)
        && now.saturating_sub(snapshot.taken_at) <= OLDEST_USABLE.as_millis() as u64
}

/// The file for this user's readings, in this layout.
pub fn snapshot_path() -> PathBuf {
    // SAFETY: this asks for a number and can't fail.
    let user = unsafe { libc::getuid() };
    PathBuf::from(format!("/dev/shm/gpupv-sensors-{user}-{LAYOUT}"))
}

/// The file for the readings of the card at `card`, an address as `0000:01:00.0`, which the guests on
/// that card share. Each card has its own, so a guest on one is never told another's figures. With no
/// address, as a setup from before addresses were kept has, it's the one file there always was.
pub fn snapshot_path_for(card: Option<&str>) -> PathBuf {
    let shared = snapshot_path();
    match card {
        Some(card) => PathBuf::from(format!("{}-{card}", shared.display())),
        None => shared,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_is_in_the_files_name() {
        let name = snapshot_path().file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(&format!("-{LAYOUT}")), "{name}");
    }

    #[test]
    fn each_card_has_a_file_and_no_address_is_the_shared_one() {
        assert_eq!(snapshot_path_for(None), snapshot_path());
        let first = snapshot_path_for(Some("0000:01:00.0"));
        let second = snapshot_path_for(Some("0000:02:00.0"));
        assert_ne!(first, second);
        let name = second.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(&format!("-{LAYOUT}-0000:02:00.0")), "{name}");
    }
}
