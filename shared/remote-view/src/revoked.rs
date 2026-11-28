//! The devices the user removed from a machine's Remote View, kept on the host in the machine's own
//! folder. Which devices a guest trusts is on the guest's own disk, so a snapshot that is gone back
//! to, or a machine that was imported, brings back devices the user never paired there or took out
//! since. Each host tells the guest again, at each start, to stop trusting each device on this list.

use crate::ctl::check_fingerprint;
use std::fs;
use std::path::Path;

/// The list's file, in the machine's folder.
pub const FILE: &str = "remote-view-revoked.json";
/// The most fingerprints kept: a list of one machine's devices is short.
const MOST: usize = 256;

/// The fingerprints removed, in the order they were.
pub fn removed(folder: &Path) -> Vec<String> {
    let text = fs::read_to_string(folder.join(FILE)).unwrap_or_default();
    let listed: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
    listed.into_iter().filter(|fingerprint| check_fingerprint(fingerprint).is_ok()).take(MOST).collect()
}

fn write(folder: &Path, list: &[String]) -> std::io::Result<()> {
    let file = folder.join(FILE);
    if list.is_empty() {
        return match fs::remove_file(file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(list).unwrap_or_default();
    fs::write(file, text + "\n")
}

/// Notes that this device was removed.
pub fn add(folder: &Path, fingerprint: &str) -> std::io::Result<()> {
    let Ok(fingerprint) = check_fingerprint(fingerprint) else { return Ok(()) };
    let mut list = removed(folder);
    if !list.contains(&fingerprint) {
        list.push(fingerprint);
        list.drain(..list.len().saturating_sub(MOST));
    }
    write(folder, &list)
}

/// Notes that this device is trusted again, as it was paired again here.
pub fn forget(folder: &Path, fingerprint: &str) -> std::io::Result<()> {
    let Ok(fingerprint) = check_fingerprint(fingerprint) else { return Ok(()) };
    let mut list = removed(folder);
    let before = list.len();
    list.retain(|listed| *listed != fingerprint);
    if list.len() == before { Ok(()) } else { write(folder, &list) }
}
