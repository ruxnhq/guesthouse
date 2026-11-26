//! What a screen shows of a machine's sharing, made from the last reading of the host's status
//! and from where pairing stands.
//!
//! The reading says who the devices are and who is connected. Pairing says what the host does not
//! say: when the open pairing ends by our clock, and how the last one closed. The state, the port
//! and a failure's reason are the program's, since only it knows whether the machine is up and the
//! relay is bound.

use crate::pairing::Pairing;
use crate::policy::Policy;
use crate::share::{DeviceInfo, ShareInfo, ShareState};
use crate::status::Status;

/// What only the program knows.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub machine: &'a str,
    pub state: ShareState,
    pub port: Option<u16>,
    /// The host's fingerprint when no reading has it yet, from the line the guest says on its
    /// serial port.
    pub fingerprint: Option<&'a str>,
    pub reason: Option<&'a str>,
}

/// The devices a person sees: the trusted ones, without the viewer that is kept out of sight.
pub fn visible_devices(status: &Status, policy: &Policy) -> Vec<DeviceInfo> {
    status.devices.iter().filter(|device| !policy.hides(&device.fingerprint)).cloned().collect()
}

/// The name of the viewer connected, if it is a person's. The host names only the client; when the
/// device connected is the hidden one and no other is, that client is not shown.
///
/// The host marks a device connected a moment before it names the client, and a reading taken then
/// (the one in which a newly paired device first appears) has a connected device and no client. The
/// device's own name stands in for it then.
pub fn viewer(status: &Status, policy: &Policy) -> Option<String> {
    let Some(client) = status.client.as_ref() else {
        let connected =
            status.devices.iter().find(|device| device.connected && !policy.hides(&device.fingerprint));
        return connected.map(|device| device.name.clone());
    };
    let hidden_connected =
        status.devices.iter().any(|device| device.connected && policy.hides(&device.fingerprint));
    let other_connected =
        status.devices.iter().any(|device| device.connected && !policy.hides(&device.fingerprint));
    if hidden_connected && !other_connected {
        return None;
    }
    Some(client.name.clone())
}

/// The state to show. `reading` is the last status, if the host has answered at all.
pub fn share_info(facts: &Facts, reading: Option<&Status>, pairing: &Pairing) -> ShareInfo {
    let view = pairing.view();
    let policy = pairing.policy();
    ShareInfo {
        machine: facts.machine.to_string(),
        state: facts.state,
        port: facts.port,
        fingerprint: reading
            .and_then(|status| status.fingerprint.clone())
            .or_else(|| facts.fingerprint.map(str::to_string)),
        pairing: view.open,
        pairing_until_ms: view.until_ms,
        viewer: reading.and_then(|status| viewer(status, policy)),
        viewer_placement: reading
            .filter(|status| viewer(status, policy).is_some())
            .and_then(|status| status.client.as_ref()?.placement.clone()),
        paired: view.paired,
        closed_reason: view.closed_reason,
        pending: view.pending,
        devices: reading.map(|status| visible_devices(status, policy)).unwrap_or_default(),
        reason: facts.reason.map(str::to_string),
    }
}

#[cfg(test)]
mod tests;
