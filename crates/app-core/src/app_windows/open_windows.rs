//! What the viewer of a machine has open, for the app's Library to read. The viewer writes a small file
//! in the machine's run folder whenever the set of programs with windows changes, and removes it when
//! it ends. The file counts only while a viewer holds the machine's viewer lock, so one that a crashed
//! viewer left behind says nothing.

use super::viewer_lock;
use crate::context::Context;
use guesthouse_daemon::slot::Slot;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const FILE: &str = "app-windows-open.json";

/// One program with windows open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenProgram {
    /// Which program it is: the cells' `app`, or their `exe` from a helper that does not say.
    pub key: String,
    /// What it is called. Empty if the helper does not know, and then `shown_name` says something.
    pub name: String,
    /// How many windows it has, popups not counted. Always at least one.
    pub windows: u32,
    /// Where its icon is saved on this host, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<PathBuf>,
}

impl OpenProgram {
    /// The name to show: its own, else the stem of its file name, else a general word.
    pub fn shown_name(&self) -> String {
        let named = self.name.trim();
        if !named.is_empty() {
            return named.to_owned();
        }
        let file = self.key.rsplit(['\\', '/', ':']).next().unwrap_or("");
        let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file);
        if stem.is_empty() { "Windows program".to_owned() } else { stem.to_owned() }
    }
}

/// The file in this run folder.
pub fn file_in(run_folder: &Path) -> PathBuf {
    run_folder.join(FILE)
}

/// Writes what is open, whole or not at all. Nothing open removes the file.
pub fn write_to(run_folder: &Path, programs: &[OpenProgram]) -> io::Result<()> {
    if programs.is_empty() {
        return remove_from(run_folder);
    }
    let file = file_in(run_folder);
    let new = run_folder.join(format!(".{FILE}.new"));
    fs::write(&new, serde_json::to_vec(programs).map_err(io::Error::other)?)?;
    fs::rename(&new, file)
}

/// Removes the file, if there is one.
pub fn remove_from(run_folder: &Path) -> io::Result<()> {
    match fs::remove_file(file_in(run_folder)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// What is open, as the file says, if a viewer holds the lock of this run folder. No file, a file
/// nobody can read, or no viewer is nothing open.
pub fn read_from(run_folder: &Path) -> Vec<OpenProgram> {
    let Ok(text) = fs::read(file_in(run_folder)) else { return Vec::new() };
    if !viewer_lock::held_in(run_folder).unwrap_or(false) {
        return Vec::new();
    }
    let mut programs: Vec<OpenProgram> = serde_json::from_slice(&text).unwrap_or_default();
    programs.retain(|program| program.windows > 0);
    programs
}

/// What the machine in this slot has open.
pub fn open_in(context: &Context, slot: Slot) -> Vec<OpenProgram> {
    read_from(&context.run_folder(Some(slot)))
}

/// The number of windows in all.
pub fn window_count(programs: &[OpenProgram]) -> u32 {
    programs.iter().map(|program| program.windows).sum()
}

/// "1 app window open." or "3 app windows open.", or nothing for none.
pub fn count_text(programs: &[OpenProgram]) -> Option<String> {
    match window_count(programs) {
        0 => None,
        1 => Some("1 app window open.".to_owned()),
        count => Some(format!("{count} app windows open.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::time::{Duration, Instant};

    fn program(key: &str, name: &str, windows: u32) -> OpenProgram {
        OpenProgram { key: key.to_owned(), name: name.to_owned(), windows, icon: None }
    }

    fn folder(scratch: &Scratch) -> PathBuf {
        let folder = scratch.path("run");
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn what_is_written_is_read_while_the_viewer_holds_its_lock() {
        let scratch = Scratch::new();
        let folder = folder(&scratch);
        let _lock = viewer_lock::take_in(&folder).unwrap().unwrap();
        let mut edge = program("C:\\Edge\\msedge.exe", "Microsoft Edge", 2);
        edge.icon = Some(PathBuf::from("/icons/edge.png"));
        let written = vec![edge, program("notepad.exe", "Notepad", 1)];
        write_to(&folder, &written).unwrap();
        assert_eq!(read_from(&folder), written);
    }

    #[test]
    fn a_file_left_by_a_viewer_that_is_gone_counts_for_nothing() {
        let scratch = Scratch::new();
        let folder = folder(&scratch);
        let lock = viewer_lock::take_in(&folder).unwrap().unwrap();
        write_to(&folder, &[program("notepad.exe", "Notepad", 1)]).unwrap();
        drop(lock);
        assert!(file_in(&folder).is_file());
        // A lock belongs to the open file, and a child that another test forks has a copy of every file
        // this process has open until it runs its program, so the lock can outlive the drop by that much.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !read_from(&folder).is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(read_from(&folder).is_empty());
    }

    #[test]
    fn nothing_open_or_nothing_written_reads_as_nothing() {
        let scratch = Scratch::new();
        let folder = folder(&scratch);
        let _lock = viewer_lock::take_in(&folder).unwrap().unwrap();
        assert!(read_from(&folder).is_empty());
        write_to(&folder, &[program("a.exe", "A", 1)]).unwrap();
        write_to(&folder, &[]).unwrap();
        assert!(!file_in(&folder).exists());
        remove_from(&folder).unwrap();
        fs::write(file_in(&folder), "not json").unwrap();
        assert!(read_from(&folder).is_empty());
    }

    #[test]
    fn the_name_falls_back_to_the_file() {
        assert_eq!(program("C:\\Windows\\notepad.exe", "", 1).shown_name(), "notepad");
        assert_eq!(
            program("aumid:Microsoft.Photos_8wekyb3d8bbwe!App", "", 1).shown_name(),
            "Microsoft.Photos_8wekyb3d8bbwe!App"
        );
        assert_eq!(program("", " ", 1).shown_name(), "Windows program");
        assert_eq!(program("x.exe", " Paint ", 1).shown_name(), "Paint");
    }

    #[test]
    fn the_count_is_singular_or_plural() {
        assert_eq!(count_text(&[]), None);
        assert_eq!(count_text(&[program("a", "A", 1)]).as_deref(), Some("1 app window open."));
        assert_eq!(
            count_text(&[program("a", "A", 1), program("b", "B", 2)]).as_deref(),
            Some("3 app windows open.")
        );
    }
}
