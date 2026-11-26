//! The guest's trust list across the installer's runs.
//!
//! remoteman's installer is given a list with `--trust FILE` and makes it the host's whole list at
//! every run, so a device paired since is dropped unless it is on the list given. This joins the
//! viewer's own line to the list the guest has now, so that nothing paired by hand is lost.
//!
//! The list is text, one device to a line: its fingerprint, a space, its name. The installer
//! refuses a line that is not that and a list with nothing in it, and writes fingerprints in lower
//! case, so this does the same.

use crate::ctl::{NOT_A_FINGERPRINT, check_fingerprint};

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

#[cfg(test)]
mod tests;
