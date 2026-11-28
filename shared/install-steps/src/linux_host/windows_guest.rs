//! A Windows install on the Linux host: the answer disc is made, the VM starts from Windows' disc
//! with a key pressed at its prompt, Setup is watched until the guest's agent says it has finished,
//! the desktop is waited for, the guest is checked, and Windows is shut down.

use std::time::Duration;

/// The longest Setup is waited for.
pub const INSTALL_HOURS: u64 = 2;
/// Setup writes all the time until the desktop, so a disk that stops growing means it's waiting for
/// someone, or stuck. The longest quiet stretch in a normal install is well under this.
pub const INSTALL_QUIET_MINUTES: u64 = 10;
/// How long a key is pressed for at the disc's prompt, once the firmware has named it.
pub const BOOT_PROMPT_SECONDS: u64 = 6;
/// The longest the firmware is given to name the disc before the keys go anyway.
pub const FIRMWARE_WAIT: Duration = Duration::from_secs(30);
/// How long the script that asks whether Setup has finished is given.
pub const SETUP_SCRIPT_SECONDS: u64 = 30;
/// How long the check of the finished guest is given.
pub const CHECK_SCRIPT_SECONDS: u64 = 60;
/// How long Windows is given to shut down at the end.
pub const INSTALL_STOP_SECONDS: u64 = 120;
/// How often Setup is looked at.
pub const SETUP_POLL: Duration = Duration::from_secs(5);
/// How often the firmware's lines are looked at.
pub const SHORT_POLL: Duration = Duration::from_millis(200);
/// The screen kept when Setup is stuck, in the run folder.
pub const STUCK_SCREEN: &str = "stuck-install.ppm";
/// What the finish script answers once Setup has finished.
pub const FINISHED: &str = "finished";
/// What the check answers when the guest has everything.
pub const ALL_THERE: &str = "all there";

/// The steps the install says, in the order it says them, around the steps read off the firmware's
/// lines ([`super::INSTALL_STEPS`]), which come between [`STARTING`] and [`WAITING_FOR_DESKTOP`].
pub const PREPARING: &str = "Preparing the install disc";
pub const STARTING: &str = "Starting the installer";
pub const WAITING_FOR_DESKTOP: &str = "Waiting for the desktop";
pub const CHECKING: &str = "Checking Windows";
pub const SHUTTING_DOWN: &str = "Shutting down Windows";

pub const NO_PASSWORD: &str = "The answer file has no password in it, so nothing was installed.";
pub const NO_DESKTOP: &str = "Windows installed, but its desktop never came up.";
pub const WOULD_NOT_SHUT_DOWN: &str =
    "Windows is installed but would not shut down. Try Force Off, then Start.";

/// Said when the VM went away during Setup, with its last words if it had any.
pub fn stopped_during(last_words: &str) -> String {
    format!("The VM stopped during the install. {last_words}").trim().to_owned()
}

/// Said when Setup hasn't written to the disk for [`INSTALL_QUIET_MINUTES`].
pub fn stuck() -> String {
    format!(
        "Windows Setup has not written to the disk for {INSTALL_QUIET_MINUTES} minutes, so it is \
         stuck or waiting for an answer. The VM is still up: Show Screen has what it says."
    )
}

/// Said when Setup hasn't finished in [`INSTALL_HOURS`].
pub fn not_finished() -> String {
    format!("Windows setup did not finish in {INSTALL_HOURS} hours. Show Screen says how far it got.")
}

/// Said when the check of the finished guest didn't answer [`ALL_THERE`]: the last 300 characters of
/// what it answered, or that the agent went quiet.
pub fn missing_something(answer: &str) -> String {
    let said = answer.trim();
    let tail = said.char_indices().rev().nth(299).map_or(0, |found| found.0);
    let said = if said.is_empty() { "the agent went quiet" } else { &said[tail..] };
    format!("Windows is missing something: {said}")
}

/// Whether Setup is stuck, from how much its disk holds at each look: it is when the disk hasn't
/// grown for longer than [`INSTALL_QUIET_MINUTES`]. The time is the host's clock's, from any start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietDisk {
    grew_at: Duration,
    written: u64,
}

impl QuietDisk {
    /// Watching from `now`, with nothing seen written yet.
    pub fn new(now: Duration) -> QuietDisk {
        QuietDisk { grew_at: now, written: 0 }
    }

    /// Notes that the disk holds `written` at `now`, and gives whether it is stuck.
    pub fn stuck(&mut self, now: Duration, written: u64) -> bool {
        let quiet = Duration::from_secs(INSTALL_QUIET_MINUTES * 60);
        if written > self.written {
            (self.grew_at, self.written) = (now, written);
        }
        now.saturating_sub(self.grew_at) > quiet
    }
}
