//! Why the daemon gave an Intel guest the software desktop when the hardware desktop was asked, or
//! that nothing runs on the card on a software desktop asked for, in the run folder for the app. The
//! app then keeps the display driver off and doesn't watch a desktop on the card for this start, and
//! keeps the Intel device on in the guest only for work that runs.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

/// The note's file in the run folder.
pub const NOTE_FILE: &str = "desktop-in-software";

/// Says why the desktop is in software, or takes the note away when it isn't. Written at every start
/// of an Intel guest, so no note outlives its daemon's choice.
pub fn write_note(run_folder: &Path, why: Option<&str>) -> io::Result<()> {
    let path = run_folder.join(NOTE_FILE);
    if let Some(why) = why {
        return fs::write(path, format!("{why}\n"));
    }
    match fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Why the desktop is in software, if the daemon said.
pub fn read_note(run_folder: &Path) -> Option<String> {
    Some(fs::read_to_string(run_folder.join(NOTE_FILE)).ok()?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::process;

    #[test]
    fn a_note_is_read_back_and_taken_away() {
        let folder = env::temp_dir().join(format!("guesthouse-desktop-note-{}", process::id()));
        fs::create_dir_all(&folder).unwrap();
        assert_eq!(read_note(&folder), None);
        write_note(&folder, Some("stage=Listen layer=card")).unwrap();
        assert_eq!(read_note(&folder).as_deref(), Some("stage=Listen layer=card"));
        write_note(&folder, None).unwrap();
        assert_eq!(read_note(&folder), None);
        write_note(&folder, None).unwrap();
        fs::remove_dir_all(&folder).unwrap();
    }
}
