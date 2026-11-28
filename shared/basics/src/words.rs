//! Small wording the hosts have, each formatter as the host that had it says it: sizes, spans of time
//! and the `ssh` line. The hosts don't agree on sizes, so each way is a function of its own, named
//! for what it does, and none stands in for another:
//!
//! - [`size_in_gb_or_mb_up`]: GiB called GB, with a decimal, from 1 GiB, and MiB called MB rounded
//!   up below it. The Linux host, before a download.
//! - [`size_in_gb_or_mb_down`]: the same, but MiB rounded down. The Windows service, in a refusal.
//! - [`size_as_maker_says`]: decimal TB, GB and MB, as a disk's maker sells it. The Linux host's
//!   drives.
//! - [`memory_as_given`] and [`card_memory_to_nearest_gb`]: MB in, GB of 1024 MB out. The Windows
//!   app.

/// A size in GiB or MiB, as said before a download.
pub fn size_in_gb_or_mb_up(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= 1024 * MIB {
        format!("{:.1} GB", bytes as f64 / (1024 * MIB) as f64)
    } else {
        format!("{} MB", bytes.div_ceil(MIB))
    }
}

/// A size in words, in GB when it's large and MB when it isn't.
pub fn size_in_gb_or_mb_down(bytes: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    if bytes >= 1 << 30 { format!("{:.1} GB", bytes as f64 / GB) } else { format!("{} MB", bytes >> 20) }
}

/// A size as the maker would say it, as `1.0 TB` or `500 GB`.
pub fn size_as_maker_says(bytes: u64) -> String {
    const KB: f64 = 1000.0;
    let bytes = bytes as f64;
    if bytes >= KB.powi(4) {
        format!("{:.1} TB", bytes / KB.powi(4))
    } else if bytes >= KB.powi(3) {
        format!("{:.0} GB", bytes / KB.powi(3))
    } else {
        format!("{:.0} MB", bytes / KB.powi(2))
    }
}

const MB_PER_GB: u64 = 1024;

/// A machine's memory, as it was given: in GB when it's a whole number of them, and in MB otherwise.
pub fn memory_as_given(mb: u64) -> String {
    if mb >= MB_PER_GB && mb.is_multiple_of(MB_PER_GB) {
        format!("{} GB", mb / MB_PER_GB)
    } else {
        format!("{mb} MB")
    }
}

/// A card's memory, to the nearest GB, as a card's driver gives a little under its size.
pub fn card_memory_to_nearest_gb(mb: u64) -> String {
    if mb >= MB_PER_GB { format!("{} GB", (mb + MB_PER_GB / 2) / MB_PER_GB) } else { format!("{mb} MB") }
}

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;

/// "45 seconds", "12 minutes", "3 hours 5 minutes".
pub fn span(seconds: u64) -> String {
    let plural = |count: u64, unit: &str| format!("{count} {unit}{}", if count == 1 { "" } else { "s" });
    match seconds {
        0..MINUTE => plural(seconds, "second"),
        MINUTE..HOUR => plural(seconds / MINUTE, "minute"),
        _ => match seconds % HOUR / MINUTE {
            0 => plural(seconds / HOUR, "hour"),
            minutes => format!("{} {}", plural(seconds / HOUR, "hour"), plural(minutes, "minute")),
        },
    }
}

/// The `ssh` line that reaches an Ubuntu guest through the port the service listens on, on this PC's
/// loopback, which needs no network in the guest.
pub fn ssh_line(port: u16, user: &str) -> String {
    format!("ssh -p {port} {user}@127.0.0.1")
}

#[cfg(test)]
mod tests;
