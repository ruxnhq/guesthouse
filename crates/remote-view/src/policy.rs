//! What differs between hosts in how pairing is run, and what a host keeps out of sight.
//!
//! remoteman's host in a guest serves a clipboard and files itself, so on a host that has no other
//! use for the host-wide switches for those they are turned off while a device is being let in.
//! On a host whose own viewer depends on them (Guesthouse's on Windows) they are never touched,
//! and that viewer's trust entry is kept out of the list of devices and out of reach of commands.

use crate::ctl::{self, Words, check_fingerprint};

/// What is said when a command names the viewer that is kept out of sight.
pub const HIDDEN_DEVICE: &str = "That is Guesthouse's own viewer, which cannot be changed here.";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// Turn the host-wide clipboard and files off while pairing is open and a device waits, and
    /// put them back once the new device's own are off, so that a device has neither in the moment
    /// between its being let in and being limited. Only for a host where nothing else uses them.
    pub hold_host_wide: bool,
    /// The fingerprint of a viewer that is on the host's list but is not a person's device, in
    /// lower case: it is left out of the devices and no command may name it.
    pub hide: Option<String>,
}

impl Policy {
    /// A Linux host, where the machine's remoteman is for other devices only.
    pub fn linux() -> Policy {
        Policy { hold_host_wide: true, hide: None }
    }

    /// A Windows host, where Guesthouse's own viewer uses the host-wide switches and has `viewer`
    /// as its fingerprint.
    pub fn windows(viewer: &str) -> Policy {
        Policy { hold_host_wide: false, hide: Some(viewer.to_ascii_lowercase()) }
    }

    /// Whether this is the fingerprint that is kept out of sight.
    pub fn hides(&self, fingerprint: &str) -> bool {
        self.hide.as_deref().is_some_and(|hidden| hidden.eq_ignore_ascii_case(fingerprint))
    }

    /// The fingerprint a command may name, in lower case, or why not.
    pub fn target(&self, fingerprint: &str) -> Result<String, String> {
        let fingerprint = check_fingerprint(fingerprint)?;
        if self.hides(&fingerprint) {
            return Err(HIDDEN_DEVICE.to_string());
        }
        Ok(fingerprint)
    }

    /// [`ctl::device_allow`], refused for the viewer that is kept out of sight.
    pub fn device_allow(&self, fingerprint: &str, what: &str, on: bool) -> Result<Words, String> {
        ctl::device_allow(&self.target(fingerprint)?, what, on)
    }

    /// [`ctl::device_revoke`], refused for the viewer that is kept out of sight.
    pub fn device_revoke(&self, fingerprint: &str) -> Result<Words, String> {
        ctl::device_revoke(&self.target(fingerprint)?)
    }
}

#[cfg(test)]
mod tests;
