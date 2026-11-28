//! A few numbers in a small file in shared memory, written by one process and read by any.
//!
//! The file is read and written with positional reads and writes, never mapped, so another process
//! changing the file's length can't fault a reader: a file that is too short reads as nothing, and
//! the writer's next publish puts it back.
//!
//! A reader takes no lock, as it can be on the message path. Each write makes a counter odd, stores
//! the numbers, and makes it even, and a reader that saw it change gives up rather than retrying for
//! long.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const ONLY_THE_OWNER: u32 = 0o600;
const WORD: usize = size_of::<u64>();
/// The counter is the file's first word, and the numbers follow it.
const FIELDS: usize = 1;
/// How often a reader looks again if a write was under way. Writes take well under a microsecond.
const READ_TRIES: usize = 3;

/// Makes the folder, for the owner alone, and refuses one that is anyone else's or open to others.
pub fn make_private_folder(folder: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(folder)?;
    let details = std::fs::symlink_metadata(folder)?;
    // SAFETY: this asks for a number and can't fail.
    if !details.is_dir() || details.uid() != unsafe { libc::getuid() } || details.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "someone else's folder, or one open to others",
        ));
    }
    Ok(())
}

/// What a writer's turn is kept on: a file of its own beside the shared one.
const TURN_SUFFIX: &str = ".writer";

/// The file a writer's turn for `path` is kept on.
pub fn turn_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(TURN_SUFFIX);
    PathBuf::from(name)
}

/// Waits until no other process has the writer's turn for the file at `path`, and returns what holds
/// it: this process is the one writer while it keeps that. The turn is a lock on a file of its own
/// beside `path`, which a reader is never given, so only a writer can hold it.
pub fn wait_for_writers_turn(path: &Path) -> io::Result<File> {
    if let Some(folder) = path.parent().filter(|folder| *folder == crate::shared_folder()) {
        make_private_folder(folder)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    let file = options.mode(ONLY_THE_OWNER).custom_flags(libc::O_NOFOLLOW).open(turn_path(path))?;
    let details = file.metadata()?;
    // SAFETY: this asks for a number and can't fail.
    if details.uid() != unsafe { libc::getuid() } || details.mode() & 0o077 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "someone else's shared file"));
    }
    // SAFETY: a plain call on a file this owns.
    match unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } {
        0 => Ok(file),
        _ => Err(io::Error::last_os_error()),
    }
}

/// `COUNT` numbers after the counter. See the module documentation.
pub struct SharedWords<const COUNT: usize> {
    /// Each open of the file has a lock of its own, which is what lets processes take turns.
    file: File,
}

impl<const COUNT: usize> SharedWords<COUNT> {
    const SIZE: usize = (FIELDS + COUNT) * WORD;

    /// The writer's open, which makes the file if needed and writes it in place, so a reader's open
    /// of it goes on reading what is published. Anyone else's file at this path is refused.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_with(path, true)
    }

    /// Opens a file for reading only, never making it. The file may still be empty, as one made
    /// ready for a reader is until the writer's first publish, and then reads as nothing. A file of
    /// any other size is some other layout's, and is refused.
    pub fn open_to_read(path: &Path) -> io::Result<Self> {
        Self::open_with(path, false)
    }

    fn open_with(path: &Path, make: bool) -> io::Result<Self> {
        // The shared readings' folder is made here, by whichever of the app and the daemon comes first.
        if make && let Some(folder) = path.parent().filter(|folder| *folder == crate::shared_folder()) {
            make_private_folder(folder)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(make).create(make).truncate(false);
        let file = options.mode(ONLY_THE_OWNER).custom_flags(libc::O_NOFOLLOW).open(path)?;
        let details = file.metadata()?;
        // SAFETY: this asks for a number and can't fail.
        if details.uid() != unsafe { libc::getuid() } || details.mode() & 0o077 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "someone else's shared file"));
        }
        if make {
            if details.len() != Self::SIZE as u64 {
                file.set_len(Self::SIZE as u64)?;
            }
        } else if details.len() != 0 && details.len() != Self::SIZE as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a shared file of another layout"));
        }
        Ok(Self { file })
    }

    /// The counter, or `None` if the file is too short to hold it.
    fn counter(&self) -> Option<u64> {
        let mut word = [0u8; WORD];
        self.file.read_exact_at(&mut word, 0).ok()?;
        Some(u64::from_ne_bytes(word))
    }

    fn store_counter(&self, value: u64) {
        // A file that can't be written leaves the readers with what was there.
        let _ = self.file.write_all_at(&value.to_ne_bytes(), 0);
    }

    /// Only one process writes: the one holding the writer's turn, see [`wait_for_writers_turn`].
    pub fn publish(&self, words: [u64; COUNT]) {
        // A writer that died part way leaves the counter odd, and this just carries on from there. A
        // file cut short has no counter, and starts again from nothing.
        let writing = self.counter().unwrap_or(0) | 1;
        self.store_counter(writing);
        let mut bytes = vec![0u8; Self::SIZE - FIELDS * WORD];
        for (index, value) in words.into_iter().enumerate() {
            bytes[index * WORD..][..WORD].copy_from_slice(&value.to_ne_bytes());
        }
        let _ = self.file.write_all_at(&bytes, (FIELDS * WORD) as u64);
        self.store_counter(writing + 1);
    }

    /// The numbers as they stand, however old, or `None` if a write kept getting in the way or the
    /// file is too short.
    pub fn read(&self) -> Option<[u64; COUNT]> {
        for _ in 0..READ_TRIES {
            let before = self.counter()?;
            let mut bytes = vec![0u8; Self::SIZE - FIELDS * WORD];
            self.file.read_exact_at(&mut bytes, (FIELDS * WORD) as u64).ok()?;
            let steady = before.is_multiple_of(2) && self.counter() == Some(before);
            if steady {
                let mut words = [0u64; COUNT];
                for (index, word) in words.iter_mut().enumerate() {
                    *word =
                        u64::from_ne_bytes(bytes[index * WORD..][..WORD].try_into().expect("a whole word"));
                }
                return Some(words);
            }
        }
        None
    }

    #[cfg(test)]
    pub(crate) fn cut_a_write_short(&self) {
        self.store_counter(self.counter().unwrap_or(0) + 1);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_folder_open_to_others_is_refused_and_a_new_one_is_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("gh-sensor-folder-{}", std::process::id()));
        let open = base.join("open");
        std::fs::create_dir_all(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(make_private_folder(&open).is_err());
        let new = base.join("new");
        make_private_folder(&new).unwrap();
        assert_eq!(std::fs::metadata(&new).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(base).unwrap();
    }

    use super::*;
    use std::fs;

    fn temporary(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("guesthouse-words-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn a_reader_can_open_an_empty_file_and_reads_the_first_publish() {
        let path = temporary("empty");
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let reader = SharedWords::<2>::open_to_read(&path).unwrap();
        assert_eq!(reader.read(), None);
        SharedWords::<2>::open(&path).unwrap().publish([4, 5]);
        assert_eq!(reader.read(), Some([4, 5]));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_readers_open_cannot_write() {
        let path = temporary("read-only");
        SharedWords::<2>::open(&path).unwrap().publish([1, 2]);
        let reader = SharedWords::<2>::open_to_read(&path).unwrap();
        assert!(reader.file.write_all_at(&[0xff; 8], 8).is_err());
        reader.publish([6, 6]);
        assert_eq!(reader.read(), Some([1, 2]));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn only_one_writer_has_the_turn_until_it_lets_go() {
        let path = temporary("turn");
        let first = wait_for_writers_turn(&path).unwrap();
        let second = fs::File::open(turn_path(&path)).unwrap();
        // SAFETY: a plain call on a file this owns.
        let taken = unsafe { libc::flock(second.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(taken, -1, "the turn is held");
        drop(first);
        drop(wait_for_writers_turn(&path).unwrap());
        fs::remove_file(turn_path(&path)).unwrap();
    }

    #[test]
    fn a_reader_that_must_not_make_the_file_finds_none() {
        let path = temporary("missing");
        assert!(SharedWords::<2>::open_to_read(&path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn a_reader_sees_the_writers_numbers_in_its_own_open() {
        let path = temporary("both");
        let writer = SharedWords::<2>::open(&path).unwrap();
        let reader = SharedWords::<2>::open_to_read(&path).unwrap();
        writer.publish([7, 9]);
        assert_eq!(reader.read(), Some([7, 9]));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_file_of_another_layout_is_refused() {
        let path = temporary("other");
        SharedWords::<3>::open(&path).unwrap();
        assert!(SharedWords::<2>::open_to_read(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_file_cut_short_under_a_reader_reads_as_nothing() {
        let path = temporary("cut-under");
        let writer = SharedWords::<2>::open(&path).unwrap();
        let reader = SharedWords::<2>::open_to_read(&path).unwrap();
        writer.publish([7, 9]);
        assert_eq!(reader.read(), Some([7, 9]));
        // Another process's open of the file, which cuts it to nothing.
        fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(0).unwrap();
        assert_eq!(reader.read(), None);
        // The writer's next publish puts the file back, and the reader reads it again.
        writer.publish([8, 10]);
        assert_eq!(reader.read(), Some([8, 10]));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_writer_whose_file_was_cut_short_goes_on_publishing() {
        let path = temporary("cut-writer");
        let writer = SharedWords::<2>::open(&path).unwrap();
        writer.publish([1, 2]);
        fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(0).unwrap();
        writer.publish([3, 4]);
        assert_eq!(SharedWords::<2>::open_to_read(&path).unwrap().read(), Some([3, 4]));
        fs::remove_file(path).unwrap();
    }
}
