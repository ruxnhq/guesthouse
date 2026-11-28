//! The Linux host's installs: Windows from its disc ([`windows_guest`]), Ubuntu from its server disc
//! ([`ubuntu_guest`]) and a macOS machine's boot image and Apple's installer ([`macos_guest`]).
//!
//! A machine being installed says so in its file (`installing`, with the disc it installs from), and
//! the task's file says the step the install is on, as the app reads them. A Windows or an Ubuntu
//! install that stops is run again from nothing; a macOS machine's goes on where it stopped, as each
//! part it made is kept. Each host keeps those files itself.

pub mod macos_guest;
pub mod ubuntu_guest;
pub mod windows_guest;

/// The steps of a Windows install read off the firmware's lines, and the line an Ubuntu install ends
/// with, which a guest says on its serial port: `guesthouse_serial_words_core` holds them.
pub use guesthouse_serial_words_core::linux_host::{
    INSTALL_FINISHED, INSTALL_STEPS, install_step, installed,
};

/// Said before an install over a machine whose disk is somewhere else.
pub const NOT_IN_FOLDER: &str = "This machine's disk is not in its folder, so nothing will install over it.";

/// Said before an install whose disc isn't there.
pub fn disc_missing(iso: &str) -> String {
    format!("{iso} is missing.")
}

#[cfg(test)]
mod tests;
