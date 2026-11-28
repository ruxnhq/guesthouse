//! A jail for a process that faces a guest, with nothing but what an unprivileged user has.
//!
//! On Linux, three parts, all of which carry across `execve` and can't be undone:
//!
//! - [`landlock`]: which files and folders it may open, and how, and which ports it may use.
//! - [`seccomp`]: which system calls it may make.
//! - no new privileges, which seccomp needs anyway, so a setuid program gains nothing.
//!
//! The daemon applies these to itself after its setup. QEMU gets them from `guesthouse-jail`, which
//! applies them and then runs it. What each of them may do is in [`filters`] and in the caller's
//! [`Plan`]. A kernel that has only some of Landlock's rights gets the ones it has, and [`Applied`]
//! says which.
//!
//! On Windows a process can't jail itself, so [`windows`] starts it jailed, with the same three
//! promises kept by an AppContainer, process mitigations and a job.

#[cfg(target_os = "linux")]
pub mod filters;
#[cfg(target_os = "linux")]
pub mod landlock;
#[cfg(target_os = "linux")]
pub mod launch;
#[cfg(target_os = "linux")]
mod plan;
#[cfg(target_os = "linux")]
pub mod seccomp;
#[cfg(target_os = "linux")]
mod support;
#[cfg(target_os = "linux")]
pub mod system;
#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "linux")]
pub use plan::{Grant, Network, Plan, Ports};
#[cfg(target_os = "linux")]
pub use support::{Support, only_thread};

#[cfg(target_os = "linux")]
use std::io;

/// What a jail really got, for the log and for the machine's row.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    pub landlock_abi: u32,
}

#[cfg(target_os = "linux")]
impl Applied {
    /// The parts a kernel below the newest known ABI leaves out, in words.
    pub fn missing(&self) -> Vec<&'static str> {
        landlock::missing_at(self.landlock_abi)
    }
}

/// Stops this process and everything it starts from gaining privileges, through setuid programs or
/// file capabilities. seccomp needs it before an unprivileged process may install a filter.
#[cfg(target_os = "linux")]
pub fn no_new_privileges() -> io::Result<()> {
    // SAFETY: this sets one flag the kernel keeps for this thread, and takes no pointers.
    let done = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if done == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}
