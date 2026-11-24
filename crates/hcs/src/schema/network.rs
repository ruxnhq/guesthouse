//! The documents the Host Compute Network takes: a network, and an endpoint on it for one machine's
//! network card.

use super::Version;
use serde::{Deserialize, Serialize};

/// HCN's schema version. Its documents have their own numbering, apart from the compute system's.
pub const NETWORK_SCHEMA_VERSION: Version = Version { major: 2, minor: 0 };

/// A network's flags, one bit each in the order HCN's schema lists them. hcsshim names
/// EnableNonPersistent as 8, the fourth bit, which fixes the order's numbering.
pub mod flags {
    /// The host answers the guests' DNS queries.
    pub const ENABLE_DNS_PROXY: u32 = 1;
    /// The host gives each guest its endpoint's address by DHCP.
    pub const ENABLE_DHCP_SERVER: u32 = 2;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Network {
    pub schema_version: Version,
    pub name: String,
    pub r#type: NetworkType,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<NetworkPolicy>,
}

/// One of a network's policies: its type, and the settings that type takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkPolicy {
    pub r#type: NetworkPolicyType,
    pub settings: NetAdapterName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkPolicyType {
    /// Binds a Transparent network to a physical adapter, by its name.
    NetAdapterName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetAdapterName {
    /// The adapter's name in Windows' network settings (Get-NetAdapter's Name).
    pub network_adapter_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkType {
    /// The host shares its own connection, as Windows' Internet Connection Sharing does. WSL's
    /// network is one, and it needs nothing beyond Virtual Machine Platform.
    #[serde(rename = "ICS")]
    Ics,
    #[serde(rename = "NAT")]
    Nat,
    /// A virtual switch bound to a physical adapter, so a guest is on the host's own network and
    /// takes its address from that network's DHCP server. hcsshim's `Transparent`.
    Transparent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Endpoint {
    pub schema_version: Version,
    /// The network's GUID, with no braces.
    pub host_compute_network: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
}

impl Network {
    /// A shared connection that gives each guest its address by DHCP and answers its DNS, as a
    /// guest that installs or updates itself needs both. HCN gives an endpoint its address either
    /// way, but without the DHCP server nothing tells the guest.
    pub fn ics(name: &str) -> Self {
        Network {
            schema_version: NETWORK_SCHEMA_VERSION,
            name: name.to_owned(),
            r#type: NetworkType::Ics,
            flags: flags::ENABLE_DNS_PROXY | flags::ENABLE_DHCP_SERVER,
            policies: Vec::new(),
        }
    }

    /// A switch on the physical adapter `adapter`, which the router's DHCP serves the guests of.
    /// There are no flags and no address space: the host's network is the guest's, so nothing here
    /// gives the guest an address.
    pub fn transparent(name: &str, adapter: &str) -> Self {
        Network {
            schema_version: NETWORK_SCHEMA_VERSION,
            name: name.to_owned(),
            r#type: NetworkType::Transparent,
            flags: 0,
            policies: vec![NetworkPolicy {
                r#type: NetworkPolicyType::NetAdapterName,
                settings: NetAdapterName { network_adapter_name: adapter.to_owned() },
            }],
        }
    }
}

/// What the network bridged onto `adapter` is called, one per adapter.
pub fn bridged_network_name(adapter: &str) -> String {
    format!("guesthouse-bridged-{adapter}")
}

/// The GUID of the network bridged onto `adapter`, the same every time, so HCN's own copy of the
/// network, which outlives every run, is found again and not made twice. It's a 128-bit FNV-1a hash
/// of the adapter's name, in a GUID's layout with its version and variant bits set.
pub fn bridged_network_id(adapter: &str) -> u128 {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013b;
    let hash = adapter
        .to_lowercase()
        .bytes()
        .fold(OFFSET, |hash, byte| (hash ^ u128::from(byte)).wrapping_mul(PRIME));
    // Version 8 (custom) and the RFC variant.
    (hash & !(0xf << 76) | (0x8 << 76)) & !(0b11 << 62) | (0b10 << 62)
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_bridged_network_is_a_transparent_one_bound_to_its_adapter() {
        let network = Network::transparent("guesthouse-bridged-Ethernet", "Ethernet");
        assert_eq!(
            serde_json::to_value(&network).unwrap(),
            json!({
                "SchemaVersion": {"Major": 2, "Minor": 0},
                "Name": "guesthouse-bridged-Ethernet",
                "Type": "Transparent",
                "Policies": [{"Type": "NetAdapterName", "Settings": {"NetworkAdapterName": "Ethernet"}}]
            })
        );
        assert_eq!(bridged_network_name("Ethernet"), "guesthouse-bridged-Ethernet");
    }

    #[test]
    fn the_shared_network_has_no_policies() {
        let value = serde_json::to_value(Network::ics("Guesthouse network")).unwrap();
        assert_eq!(value["Type"], json!("ICS"));
        assert_eq!(value["Flags"], json!(3));
        assert!(value.get("Policies").is_none());
    }

    #[test]
    fn an_adapters_network_has_one_id_whatever_the_case_and_no_other_adapter_has_it() {
        assert_eq!(bridged_network_id("Ethernet"), bridged_network_id("ETHERNET"));
        assert_ne!(bridged_network_id("Ethernet"), bridged_network_id("Ethernet 2"));
        let id = bridged_network_id("Wi-Fi");
        assert_eq!((id >> 76) & 0xf, 0x8, "version");
        assert_eq!((id >> 62) & 0b11, 0b10, "variant");
    }
}
