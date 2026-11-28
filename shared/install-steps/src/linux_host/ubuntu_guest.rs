//! An Ubuntu install on the Linux host: the installer's kernel and an answer disc are taken from
//! Ubuntu's server disc, and its installer runs until it powers the VM off, having said
//! [`super::INSTALL_FINISHED`] on the serial port if it finished.

/// The longest the installer is waited for.
pub const INSTALL_HOURS: u64 = 2;

/// The first step the install says.
pub const PREPARING: &str = "Preparing the install";

/// Said when the installer hasn't powered the VM off in [`INSTALL_HOURS`].
pub fn not_finished() -> String {
    format!("The install did not finish in {INSTALL_HOURS} hours. Show Screen says how far it got.")
}

/// Said when the disc has no file `name` the install takes from it.
pub fn not_a_server_disc(iso: &str, name: &str) -> String {
    format!("{iso} has no {name}, so it is not an Ubuntu server ISO.")
}

/// Said when the disc's pool has no generic kernel.
pub fn no_generic_kernel(iso: &str) -> String {
    format!("{iso} holds no generic kernel in its pool, so it is not an Ubuntu server ISO.")
}
