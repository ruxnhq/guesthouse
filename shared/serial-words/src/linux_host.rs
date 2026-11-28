//! The Linux host's serial words: what an Ubuntu install says once it's done, the steps of a Windows
//! install read off the firmware's lines, and in [`guest_says`] what a guest says about its GPU and
//! its desktop. The host reads them from the serial log QEMU writes.

pub mod guest_says;

/// Only a finished install writes this line. The VM powers off either way.
pub const INSTALL_FINISHED: &[u8] = b"nvfwd-install-finished";

pub fn installed(serial: &[u8]) -> bool {
    serial.split(|byte| *byte == b'\n').any(|line| line.starts_with(INSTALL_FINISHED))
}

pub const INSTALL_STEPS: [&str; 4] = [
    "Copying Windows to the disk",
    "Windows is setting up its devices",
    "Windows is getting ready",
    "Windows is finishing its setup",
];
/// The firmware names it twice a boot.
pub const WINDOWS_BOOT_LINE: &[u8] = b"\"Windows Boot Manager\"";

/// What Windows setup is doing, from how often it has started from the disk.
pub fn install_step(serial: &[u8]) -> &'static str {
    let started = count(serial, WINDOWS_BOOT_LINE);
    INSTALL_STEPS[(started / 2).min(INSTALL_STEPS.len() - 1)]
}

pub fn count(data: &[u8], wanted: &[u8]) -> usize {
    data.windows(wanted.len()).filter(|part| *part == wanted).count()
}

#[cfg(test)]
mod tests;
