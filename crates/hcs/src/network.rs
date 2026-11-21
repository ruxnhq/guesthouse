//! computenetwork.dll: a network the host shares its connection on, and an endpoint on it for each
//! machine's network card.

use crate::error::{Error, Result};
use crate::schema::network::{Endpoint as EndpointDocument, NETWORK_SCHEMA_VERSION, Network};
use crate::text::{guid_text, take_co_task, wide};
use std::ffi::c_void;
use std::ptr::null_mut;
use windows_sys::Win32::System::HostComputeNetwork::{
    HcnCloseEndpoint, HcnCloseNetwork, HcnCreateEndpoint, HcnCreateNetwork, HcnDeleteEndpoint, HcnOpenNetwork,
};
use windows_sys::core::PWSTR;

/// A network's or an endpoint's ID.
pub use windows_sys::core::GUID;

/// Makes the ICS network `id` unless HCN has it already. HCN keeps a network after its maker exits,
/// until the host restarts, so one network serves every run.
pub fn ensure_ics_network(id: &GUID, name: &str) -> Result<()> {
    let mut network = null_mut();
    let mut record: PWSTR = null_mut();
    // SAFETY: `id` is a GUID that outlives the call, and the other two are places for HCN's answers.
    let opened = unsafe { HcnOpenNetwork(id, &mut network, &mut record) };
    // SAFETY: HCN allocates its error record with the COM allocator for the caller to free.
    drop(unsafe { take_co_task(record) });
    if opened >= 0 {
        close_network(network);
        return Ok(());
    }

    let settings = wide(serde_json::to_string(&Network::ics(name)).expect("a network always serialises"));
    let mut record: PWSTR = null_mut();
    // SAFETY: as above, and the settings end in a zero and outlive the call.
    let created = unsafe { HcnCreateNetwork(id, settings.as_ptr(), &mut network, &mut record) };
    // SAFETY: as above.
    let record = unsafe { take_co_task(record) };
    if created < 0 {
        return Err(Error::new(format!("Making the network {name}"), created, record));
    }
    close_network(network);
    Ok(())
}

/// A machine's place on a network, deleted again when dropped.
pub struct Endpoint {
    id: GUID,
}

impl Endpoint {
    /// `mac` is in the form 00-15-5D-12-34-56.
    pub fn create(network_id: &GUID, id: GUID, mac: &str) -> Result<Self> {
        let doing = "Plugging the machine into the network";
        let mut network = null_mut();
        let mut record: PWSTR = null_mut();
        // SAFETY: as in `ensure_ics_network`.
        let opened = unsafe { HcnOpenNetwork(network_id, &mut network, &mut record) };
        // SAFETY: as in `ensure_ics_network`.
        let record = unsafe { take_co_task(record) };
        if opened < 0 {
            return Err(Error::new(doing, opened, record));
        }

        let document = EndpointDocument {
            schema_version: NETWORK_SCHEMA_VERSION,
            host_compute_network: guid_text(network_id),
            mac_address: Some(mac.to_owned()),
        };
        let settings = wide(serde_json::to_string(&document).expect("an endpoint always serialises"));
        let mut endpoint = null_mut();
        let mut record: PWSTR = null_mut();
        // SAFETY: the network is open, the GUID and the settings outlive the call, and the rest are
        // places for HCN's answers.
        let created =
            unsafe { HcnCreateEndpoint(network, &id, settings.as_ptr(), &mut endpoint, &mut record) };
        // SAFETY: as in `ensure_ics_network`.
        let record = unsafe { take_co_task(record) };
        close_network(network);
        if created < 0 {
            return Err(Error::new(doing, created, record));
        }
        // The endpoint lives on in HCN until it's deleted, so its handle isn't needed.
        // SAFETY: the handle came from HcnCreateEndpoint and isn't used after this.
        unsafe { HcnCloseEndpoint(endpoint) };
        Ok(Endpoint { id })
    }

    /// As HCS's machine document names it.
    pub fn id_string(&self) -> String {
        guid_text(&self.id)
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let mut record: PWSTR = null_mut();
        // SAFETY: the GUID outlives the call, and `record` is a place for HCN's answer.
        unsafe { HcnDeleteEndpoint(&self.id, &mut record) };
        // SAFETY: as in `ensure_ics_network`.
        drop(unsafe { take_co_task(record) });
    }
}

fn close_network(network: *mut c_void) {
    if !network.is_null() {
        // SAFETY: the handle came from HcnOpenNetwork or HcnCreateNetwork and isn't used after this.
        unsafe { HcnCloseNetwork(network) };
    }
}

/// A MAC address in Hyper-V's own range, from six bytes of a GUID, in the form HCN takes.
pub fn mac_from(guid: &GUID) -> String {
    /// Microsoft's prefix for the cards of its virtual machines.
    const HYPER_V_PREFIX: [u8; 3] = [0x00, 0x15, 0x5D];
    let tail = &guid.data4[5..8];
    HYPER_V_PREFIX.iter().chain(tail).map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mac_is_in_hyper_vs_range_with_dashes() {
        let guid = GUID::from_u128(0xabcdef);
        assert_eq!(mac_from(&guid), "00-15-5D-AB-CD-EF");
    }
}
