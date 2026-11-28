//! The guest's video memory by the session's own books, kept in the run folder for the app, which
//! shows each machine's part of the card. Counting it goes through every allocation, so it's done
//! at most once a second, and only after the guest has said something since. Telling it is a few
//! stores into a mapped file, with no call to the kernel, so it costs the message thread nothing more.

use guesthouse_core::VideoUse;
use guesthouse_sensors::guest_use::{GuestUse, SharedGuestUse};
use std::path::Path;
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(1);

pub struct UseNote {
    /// `None` if the file couldn't be made, which costs the app the figure and nothing else.
    file: Option<SharedGuestUse>,
    /// This process, which the record names so the app can tell it from one a daemon left behind.
    daemon: u32,
    told_at: Option<Instant>,
    /// The guest has sent something since the last telling.
    behind: bool,
}

impl UseNote {
    /// Makes the file. This has to be before the jail's filter goes on, as it maps the file.
    pub fn open(run_folder: &Path) -> Self {
        let file = SharedGuestUse::open(run_folder)
            .inspect_err(|error| log::warn!("no file for the app's video memory figures: {error}"))
            .ok();
        Self { file, daemon: std::process::id(), told_at: None, behind: true }
    }

    pub fn guest_spoke(&mut self) {
        self.behind = true;
    }

    /// Tells the app what `video` says, if it's due.
    pub fn tell(&mut self, now: Instant, video: impl FnOnce() -> VideoUse) {
        let Some(file) = &self.file else { return };
        let due = self.told_at.is_none_or(|told| now.saturating_duration_since(told) >= INTERVAL);
        if !self.behind || !due {
            return;
        }
        let video = video();
        file.publish(&GuestUse {
            daemon: self.daemon,
            video_bytes: video.on_card,
            spilled_bytes: video.spilled,
            cap_bytes: video.cap,
        });
        self.told_at = Some(now);
        self.behind = false;
    }

    /// How long the loop may sleep before a telling falls due, if one is waiting.
    pub fn due_in(&self, now: Instant) -> Option<Duration> {
        if self.file.is_none() || !self.behind {
            return None;
        }
        let told = self.told_at?;
        Some(INTERVAL.saturating_sub(now.saturating_duration_since(told)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn folder(name: &str) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("guesthouse-use-note-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn use_of(bytes: u64) -> VideoUse {
        VideoUse { on_card: bytes, spilled: 0, cap: Some(4 << 30) }
    }

    #[test]
    fn the_first_telling_is_at_once_and_the_next_waits_its_second() {
        let run = folder("pace");
        let mut note = UseNote::open(&run);
        let app = SharedGuestUse::open_to_read(&run).unwrap();
        let start = Instant::now();
        note.tell(start, || use_of(1));
        assert_eq!(app.read_from(std::process::id()).unwrap().video_bytes, 1);
        note.guest_spoke();
        assert_eq!(note.due_in(start), Some(INTERVAL));
        note.tell(start + INTERVAL / 2, || panic!("not counted before its time"));
        note.tell(start + INTERVAL, || use_of(2));
        assert_eq!(app.read_from(std::process::id()).unwrap().cap_bytes, Some(4 << 30));
        assert_eq!(app.read_from(std::process::id()).unwrap().video_bytes, 2);
        fs::remove_dir_all(run).unwrap();
    }

    #[test]
    fn a_quiet_guest_is_not_counted_again() {
        let run = folder("quiet");
        let mut note = UseNote::open(&run);
        let start = Instant::now();
        note.tell(start, || use_of(1));
        assert_eq!(note.due_in(start), None);
        note.tell(start + 10 * INTERVAL, || panic!("nothing changed"));
        fs::remove_dir_all(run).unwrap();
    }
}
