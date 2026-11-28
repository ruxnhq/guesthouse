//! Reads a note a jailed program left in the run folder. What is there is whatever the program
//! wrote, so no more than a few kilobytes of it is read, which is far more than any note holds.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The most of a note that is read, in bytes.
pub const MOST_NOTE_BYTES: u64 = 4096;

/// The note's first [`MOST_NOTE_BYTES`], as text, or `None` if there is no file to read.
pub fn read_capped(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    File::open(path).ok()?.take(MOST_NOTE_BYTES).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs, process};

    #[test]
    fn a_huge_note_is_read_as_far_as_the_limit_only() {
        let folder = env::temp_dir().join(format!("guesthouse-note-file-{}", process::id()));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("note");
        assert_eq!(read_capped(&path), None);
        fs::write(&path, "short\n").unwrap();
        assert_eq!(read_capped(&path).as_deref(), Some("short\n"));
        File::create(&path).unwrap().set_len(64 << 20).unwrap();
        assert_eq!(read_capped(&path).unwrap().len() as u64, MOST_NOTE_BYTES);
        let _ = fs::remove_dir_all(folder);
    }
}
