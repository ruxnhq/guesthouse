//! The Windows host's serial words: the report's last line, the line that asks a guest to shut down,
//! the address in a report line, what an Ubuntu guest says once its install is done (in [`linux`]),
//! and in [`printable`] each guest line made safe to print. The host reads a guest's lines one by
//! one from its serial port's pipe, each made printable first. A Windows guest's `report.ps1` and an
//! Ubuntu guest's `gpu.sh` write `guesthouse: ` before each of their lines.

pub mod printable;

/// The report's last line, on either system's serial port.
pub const REPORT_DONE: &str = "guesthouse: report done";

/// What the host writes on a guest's serial port to ask it to shut down. HCS's own request needs
/// Hyper-V's integration services in the guest, which a machine HCS makes doesn't have, so the guest
/// listens for this and shuts itself down, as App Sandbox's guest agent does when asked.
/// `report.ps1` listens for it in Windows and `shutdown.sh` in Ubuntu.
pub const SHUTDOWN_LINE: &str = "guesthouse: shutdown";

/// Whether a line from the guest is its report's last.
pub fn report_done(line: &str) -> bool {
    line.starts_with(REPORT_DONE)
}

/// The IPv4 address in one of a guest's report lines, other than loopback. A Windows guest writes
/// `address: 192.0.2.2`; an Ubuntu guest writes what `ip -o address` says, such as
/// `address: 2: eth0    inet 192.0.2.2/24 ...`.
pub fn address_in(line: &str) -> Option<String> {
    let rest = line.strip_prefix("guesthouse: address: ").or_else(|| line.strip_prefix("address: "))?;
    rest.split(|c: char| c.is_whitespace() || c == '/')
        .find(|word| word.parse::<std::net::Ipv4Addr>().is_ok_and(|address| !address.is_loopback()))
        .map(str::to_owned)
}

/// What a Linux guest of the Windows host says about its install.
pub mod linux {
    /// What the guest says on its serial port when its install is done, as `user-data` writes it.
    pub const INSTALL_FINISHED: &str = "guesthouse-install-finished";

    /// Whether a line from the guest is the one saying its install is done. Only a line that starts
    /// with it counts, so the installer showing the command that says it doesn't.
    pub fn install_finished(line: &str) -> bool {
        line.starts_with(INSTALL_FINISHED)
    }
}

#[cfg(test)]
mod tests;
