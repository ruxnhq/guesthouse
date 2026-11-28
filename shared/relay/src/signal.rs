//! Ending on SIGTERM and SIGINT, with the program's own loop noticing, rather than the signal's default
//! cutting the program off half way through putting its files away.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

static ASKED_TO_END: AtomicBool = AtomicBool::new(false);

extern "C" fn note_signal(_signal: libc::c_int) {
    // Only an atomic store, which is safe in a signal handler.
    ASKED_TO_END.store(true, Ordering::Release);
}

/// Starts watching for SIGTERM and SIGINT. Afterwards [`asked_to_end`] says whether one has come.
pub fn watch_for_end() -> io::Result<()> {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the action is zeroed and then given a handler that only stores to an atomic, with
        // no flags, so nothing interrupted by the signal is left half done. The signal is a valid one.
        let failed = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = note_signal as extern "C" fn(libc::c_int) as usize;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut()) != 0
        };
        if failed {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub fn asked_to_end() -> bool {
    ASKED_TO_END.load(Ordering::Acquire)
}
