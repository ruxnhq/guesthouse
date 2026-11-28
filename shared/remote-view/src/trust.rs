//! The guest's trust list across the installer's runs.
//!
//! remoteman's installer is given a list with `--trust FILE` and makes it the host's whole list at
//! every run, so a device paired since is dropped unless it is on the list given. This joins the
//! viewer's own line to the list the guest has now, so that nothing paired by hand is lost.
//!
//! The list is text, one device to a line: its fingerprint, a space, its name. The installer
//! refuses a line that is not that and a list with nothing in it, and writes fingerprints in lower
//! case, so this does the same.
//!
//! The list the guest has now is read off its disk while the machine is off and its disk is on the
//! host, as remoteman's host keeps it: it reads its file only when it starts and writes it again at
//! each pairing, rename and revoke. So a revoked device is gone from it, and stays gone, as the list
//! given is only ever made from that file and never from the copy given at an earlier boot.

use crate::ctl::{NOT_A_FINGERPRINT, check_fingerprint};
use std::io::Read;
use std::path::Path;

/// remoteman's host's own trust list in a Windows guest, from the root of its Windows volume.
pub const GUEST_TRUST_FILE: &str = r"ProgramData\remoteman\host\host\trusted";
/// The most of the guest's list that is read: some hundreds of devices, far more than anyone pairs.
/// Past it, only the whole lines before it are kept.
pub const MOST_LIST_BYTES: u64 = 64 * 1024;
/// A byte order mark, which remoteman never writes but an editor might.
const BYTE_ORDER_MARK: char = '\u{feff}';

/// The name the viewer's own line has, which the other lines in a list never take.
pub const VIEWER_NAME: &str = "Guesthouse";

/// One line of a list: a fingerprint, then a name that may be empty or have spaces in it. `None`
/// for a line that is empty or is not a fingerprint and a name.
fn entry(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let (fingerprint, name) = match line.split_once(char::is_whitespace) {
        Some((fingerprint, name)) => (fingerprint, name.trim()),
        None => (line, ""),
    };
    let fingerprint = check_fingerprint(fingerprint).ok()?;
    Some((fingerprint, name.to_string()))
}

/// The devices on a list, in order, each once. The first line for a fingerprint is the one kept.
pub fn devices(list: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for (fingerprint, name) in list.lines().filter_map(entry) {
        if !found.iter().any(|(seen, _)| *seen == fingerprint) {
            found.push((fingerprint, name));
        }
    }
    found
}

/// The list to give the installer: the viewer first, as `trust_list` makes it, then every other
/// device on `current`, the guest's list now (`None` when it has none yet), in the order they were
/// there. A line that is not a fingerprint and a name is left out, since the installer would stop
/// at it. A device that is on `current` under the viewer's fingerprint is the viewer, whatever
/// name it has.
pub fn merge_trust(viewer: &str, current: Option<&str>) -> Result<String, String> {
    let viewer = check_fingerprint(viewer).map_err(|_| NOT_A_FINGERPRINT.to_string())?;
    let mut list = format!("{viewer} {VIEWER_NAME}\n");
    for (fingerprint, name) in current.map(devices).unwrap_or_default() {
        if fingerprint == viewer {
            continue;
        }
        list.push_str(&fingerprint);
        if !name.is_empty() {
            list.push(' ');
            list.push_str(&name);
        }
        list.push('\n');
    }
    Ok(list)
}

/// The guest's list in the file at `path`, `None` when it has none or it can't be read, which leaves
/// the viewer alone on the list given, as before devices could be paired. A name that isn't UTF-8 is
/// kept as well as it reads, rather than losing its device.
pub fn read_list(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MOST_LIST_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MOST_LIST_BYTES {
        bytes.truncate(MOST_LIST_BYTES as usize);
        let whole = bytes.iter().rposition(|&byte| byte == b'\n').map_or(0, |last| last + 1);
        bytes.truncate(whole);
    }
    let text = String::from_utf8_lossy(&bytes);
    Some(text.strip_prefix(BYTE_ORDER_MARK).unwrap_or(&text).to_string())
}

/// What the host gives a Windows guest about whom to trust, in its screen folder.
#[derive(Debug, PartialEq, Eq)]
pub struct Given {
    /// The viewer's own line, as the whole list was before devices could be paired, which decides
    /// whether the installer runs again. Being the same bytes, a guest installed before keeps the
    /// note of its install.
    pub viewer: String,
    /// The list the installer is given when it runs: the viewer, then every device the guest has.
    pub trusted: String,
}

/// The lists for a guest whose Windows volume is at `root`, with the guest's own list read there.
pub fn given(viewer: &str, root: &Path) -> Result<Given, String> {
    let current = read_list(&root.join(GUEST_TRUST_FILE));
    Ok(Given { viewer: crate::trust_list(viewer), trusted: merge_trust(viewer, current.as_deref())? })
}

#[cfg(test)]
mod tests;
