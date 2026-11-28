//! What one guest holds of its card by its daemon's own books: the video memory its allocations hold,
//! the system memory those the card had no room for spilled into, and the cap it's held to. These are
//! the figures the cap is checked against, so the app shows the same ones.
//!
//! The daemon keeps them in a small file in the guest's run folder, which its jail already lets it
//! write, and stores into it without a call to the kernel. The record names the daemon that wrote it,
//! so a file a daemon left behind is never read as a running guest's.

use crate::words::SharedWords;
use std::io;
use std::path::Path;

/// The file's name in the run folder. The layout is in it, as with the sensors' file.
pub const FILE: &str = "gpu-use-1";
const NO_CAP: u64 = u64::MAX;
const FIELD_COUNT: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuestUse {
    /// The process id of the daemon that wrote it.
    pub daemon: u32,
    pub video_bytes: u64,
    pub spilled_bytes: u64,
    /// `None` when the guest may have all the card has.
    pub cap_bytes: Option<u64>,
}

fn to_words(used: &GuestUse) -> [u64; FIELD_COUNT] {
    [u64::from(used.daemon), used.video_bytes, used.spilled_bytes, used.cap_bytes.unwrap_or(NO_CAP)]
}

fn from_words(words: [u64; FIELD_COUNT]) -> Option<GuestUse> {
    Some(GuestUse {
        daemon: u32::try_from(words[0]).ok()?,
        video_bytes: words[1],
        spilled_bytes: words[2],
        cap_bytes: (words[3] != NO_CAP).then_some(words[3]),
    })
}

/// See the module documentation.
pub struct SharedGuestUse(SharedWords<FIELD_COUNT>);

impl SharedGuestUse {
    /// The daemon's end, which makes the file.
    pub fn open(run_folder: &Path) -> io::Result<Self> {
        Ok(Self(SharedWords::open(&run_folder.join(FILE))?))
    }

    /// The app's end, which leaves a run folder without the file as it is.
    pub fn open_to_read(run_folder: &Path) -> io::Result<Self> {
        Ok(Self(SharedWords::open_to_read(&run_folder.join(FILE))?))
    }

    pub fn publish(&self, used: &GuestUse) {
        self.0.publish(to_words(used));
    }

    /// The record as it stands, whoever wrote it. The caller checks [`GuestUse::daemon`] is the
    /// running guest's before it believes a figure.
    pub fn read(&self) -> Option<GuestUse> {
        self.0.read().and_then(from_words).filter(|used| used.daemon != 0)
    }

    /// What `daemon` last wrote. Anything else, a record of another daemon's or none, is no reading.
    pub fn read_from(&self, daemon: u32) -> Option<GuestUse> {
        self.read().filter(|used| used.daemon == daemon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn folder(name: &str) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("guesthouse-use-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn the_app_reads_what_the_daemon_wrote() {
        let run = folder("both");
        let daemon = SharedGuestUse::open(&run).unwrap();
        let app = SharedGuestUse::open_to_read(&run).unwrap();
        let used =
            GuestUse { daemon: 4242, video_bytes: 3 << 30, spilled_bytes: 1 << 20, cap_bytes: Some(4 << 30) };
        daemon.publish(&used);
        assert_eq!(app.read_from(4242), Some(used));
        let uncapped = GuestUse { cap_bytes: None, ..used };
        daemon.publish(&uncapped);
        assert_eq!(app.read_from(4242), Some(uncapped));
        fs::remove_dir_all(run).unwrap();
    }

    #[test]
    fn another_daemons_record_or_none_is_no_reading() {
        let run = folder("other");
        assert!(SharedGuestUse::open_to_read(&run).is_err(), "no file, and none is made");
        let daemon = SharedGuestUse::open(&run).unwrap();
        let app = SharedGuestUse::open_to_read(&run).unwrap();
        assert_eq!(app.read_from(0), None, "nothing written yet");
        daemon.publish(&GuestUse { daemon: 1, ..GuestUse::default() });
        assert_eq!(app.read_from(2), None);
        fs::remove_dir_all(run).unwrap();
    }
}
