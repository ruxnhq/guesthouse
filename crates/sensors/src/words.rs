//! A few numbers in a small file in shared memory, written by one process and read by any.
//!
//! A reader takes no lock, as it can be on the message path. Each write makes a counter odd, stores
//! the numbers, and makes it even, and a reader that saw it change gives up rather than retrying for
//! long.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};

const ONLY_THE_OWNER: u32 = 0o600;
const COUNTER: usize = 0;
const FIELDS: usize = 1;
/// How often a reader looks again if a write was under way. Writes take well under a microsecond.
const READ_TRIES: usize = 3;

struct Mapping {
    start: *mut AtomicU64,
    size: usize,
}

// SAFETY: every access through the pointer is atomic, and the mapping lasts as long as this does.
unsafe impl Send for Mapping {}
// SAFETY: as above.
unsafe impl Sync for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this is the mapping made in `open`, and this is its last owner.
        unsafe { libc::munmap(self.start.cast(), self.size) };
    }
}

/// `COUNT` numbers after the counter. See the module documentation.
pub struct SharedWords<const COUNT: usize> {
    /// Each open of the file has a lock of its own, which is what lets processes take turns.
    file: File,
    mapping: Arc<Mapping>,
}

impl<const COUNT: usize> SharedWords<COUNT> {
    const SIZE: usize = (FIELDS + COUNT) * size_of::<u64>();

    /// Opens the file, making it if needed. Anyone else's file at this path is refused.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_with(path, true)
    }

    /// Opens a file the writer has made, for a reader that must not make one. A file of any other
    /// size is some other layout's, and is refused.
    pub fn open_existing(path: &Path) -> io::Result<Self> {
        Self::open_with(path, false)
    }

    fn open_with(path: &Path, make: bool) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(make).truncate(false);
        let file = options.mode(ONLY_THE_OWNER).custom_flags(libc::O_NOFOLLOW).open(path)?;
        let details = file.metadata()?;
        // SAFETY: this asks for a number and can't fail.
        if details.uid() != unsafe { libc::getuid() } || details.mode() & 0o077 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "someone else's shared file"));
        }
        if make {
            file.set_len(Self::SIZE as u64)?;
        } else if details.len() != Self::SIZE as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a shared file of another layout"));
        }
        let protection = libc::PROT_READ | libc::PROT_WRITE;
        // SAFETY: a fresh shared mapping of a file sized to fit, at an address the kernel picks.
        let start = unsafe {
            libc::mmap(ptr::null_mut(), Self::SIZE, protection, libc::MAP_SHARED, file.as_raw_fd(), 0)
        };
        if start == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file, mapping: Arc::new(Mapping { start: start.cast(), size: Self::SIZE }) })
    }

    /// Another handle on the same file, with a lock of its own, as it is a new open of the file.
    /// Take the lock on one of these: the mapping keeps the first open alive, and a lock on that
    /// would outlast the handle.
    pub fn try_clone(&self) -> io::Result<Self> {
        let own_path = format!("/proc/self/fd/{}", self.file.as_raw_fd());
        let file = OpenOptions::new().read(true).write(true).open(own_path)?;
        Ok(Self { file, mapping: Arc::clone(&self.mapping) })
    }

    /// Waits until no other process holds the lock. It lasts until this process exits.
    pub fn wait_for_lock(&self) -> io::Result<()> {
        // SAFETY: a plain call on a file this owns.
        match unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_EX) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn word(&self, index: usize) -> &AtomicU64 {
        assert!(index < FIELDS + COUNT);
        // SAFETY: the assert keeps it inside the mapping, which lives as long as `self`.
        unsafe { &*self.mapping.start.add(index) }
    }

    /// Only one process writes: the holder of the lock, or the file's only writer.
    pub fn publish(&self, words: [u64; COUNT]) {
        // A writer that died part way leaves the counter odd, and this just carries on from there.
        let writing = self.word(COUNTER).load(Ordering::Relaxed) | 1;
        self.word(COUNTER).store(writing, Ordering::Relaxed);
        fence(Ordering::Release);
        for (index, value) in words.into_iter().enumerate() {
            self.word(FIELDS + index).store(value, Ordering::Relaxed);
        }
        self.word(COUNTER).store(writing + 1, Ordering::Release);
    }

    /// The numbers as they stand, however old, or `None` if a write kept getting in the way.
    pub fn read(&self) -> Option<[u64; COUNT]> {
        (0..READ_TRIES).find_map(|_| {
            let before = self.word(COUNTER).load(Ordering::Acquire);
            let words: [u64; COUNT] =
                std::array::from_fn(|index| self.word(FIELDS + index).load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            let steady = before.is_multiple_of(2) && self.word(COUNTER).load(Ordering::Relaxed) == before;
            steady.then_some(words)
        })
    }

    #[cfg(test)]
    pub(crate) fn cut_a_write_short(&self) {
        self.word(COUNTER).fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn file(&self) -> &File {
        &self.file
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temporary(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("guesthouse-words-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn a_reader_that_must_not_make_the_file_finds_none() {
        let path = temporary("missing");
        assert!(SharedWords::<2>::open_existing(&path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn a_reader_sees_the_writers_numbers_in_its_own_open() {
        let path = temporary("both");
        let writer = SharedWords::<2>::open(&path).unwrap();
        let reader = SharedWords::<2>::open_existing(&path).unwrap();
        writer.publish([7, 9]);
        assert_eq!(reader.read(), Some([7, 9]));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_file_of_another_layout_is_refused() {
        let path = temporary("other");
        SharedWords::<3>::open(&path).unwrap();
        assert!(SharedWords::<2>::open_existing(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
