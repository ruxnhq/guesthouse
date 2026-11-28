//! The guest's serial port. QEMU writes it to a pipe it's handed, at [`SERIAL_FD`], and the app keeps
//! the file the port's words end up in, held to a size (`capped_log`). Any QEMU takes a port this way.

use super::VmPlan;
use super::arguments::words;
use std::os::fd::RawFd;
use std::path::PathBuf;

/// Where QEMU finds the port's pipe: clear of the drives', the throwaway disk's and the sockets'.
pub const SERIAL_FD: RawFd = 56;
/// Begins the note on the pipe's set, after which comes the file in the run folder it goes to.
const SERIAL_MARK: &str = "guesthouse-serial:";

impl VmPlan {
    /// The port's words, for words in the run folder named `file`. A plan printed for the user to run
    /// (one that opens its drives by name) has QEMU write the file itself, as nothing keeps it there.
    pub(super) fn serial_arguments(&self, file: &str) -> Vec<String> {
        if self.drives_by_path.is_some() {
            return words(&["-serial", &format!("file:{}", self.in_run_folder(file))]);
        }
        words(&[
            "-add-fd",
            &format!("fd={SERIAL_FD},set={SERIAL_FD},opaque={SERIAL_MARK}{file}"),
            "-serial",
            &format!("file:/dev/fdset/{SERIAL_FD}"),
        ])
    }

    /// The file this machine's serial port goes to, if it has one.
    pub fn serial_log(&self) -> Option<PathBuf> {
        let note = format!("fd={SERIAL_FD},set={SERIAL_FD},opaque={SERIAL_MARK}");
        let arguments = self.qemu_arguments();
        let file = arguments.iter().find_map(|word| word.strip_prefix(&note))?;
        Some(self.run_folder.join(file))
    }
}
