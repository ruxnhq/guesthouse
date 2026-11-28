//! What a Linux guest says about its GPU, on its serial port. gpu-up.sh writes one line at each boot,
//! `nvfwd-gpu on VERSION` or `nvfwd-gpu off REASON`, and the app shows the newest while the VM runs. A
//! guest installed by hand then says where its desktop went, `nvfwd-desktop gpu` or
//! `nvfwd-desktop plain REASON`, and says it again if it later goes back to the plain screen.
//! The guest writes these, so only those prefixes count, and only plain words of a sane length are shown.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub(super) use guesthouse_serial_words_core::linux_host::guest_says::said;
/// The parser and the words for the machine's row are in the crate every host can share.
pub use guesthouse_serial_words_core::linux_host::guest_says::{GuestDesktop, GuestGpu, GuestSays, newest};

pub const SERIAL_LOG: &str = "serial.log";
/// The line is written early in the boot, and a guest's console can say a lot after it.
pub(super) const TAIL_BYTES: u64 = 256 * 1024;

/// What the newest boot said in a run folder's serial log. A guest that never wrote a line says nothing.
pub fn read(run_folder: &Path) -> Option<GuestSays> {
    newest(&serial_tail(run_folder)?)
}

/// The end of a run folder's serial log, where the newest boot's lines are.
pub(super) fn serial_tail(run_folder: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(run_folder.join(SERIAL_LOG)).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES))).ok()?;
    let mut tail = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut tail).ok()?;
    Some(tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn only_the_end_of_a_long_log_is_read() {
        let scratch = Scratch::new();
        let mut log = b"nvfwd-gpu on 615.78.08\n".to_vec();
        log.extend(std::iter::repeat_n(b'.', TAIL_BYTES as usize));
        std::fs::write(scratch.path("serial.log"), &log).unwrap();
        assert_eq!(read(&scratch.path("")), None);
        log.extend(b"\nnvfwd-gpu off the host gave no image\n");
        std::fs::write(scratch.path("serial.log"), &log).unwrap();
        let off = GuestGpu::Off { reason: "the host gave no image".into() };
        assert_eq!(read(&scratch.path("")).map(|says| says.gpu), Some(off));
        assert_eq!(read(&scratch.path("nowhere")), None);
    }
}
