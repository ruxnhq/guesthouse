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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkType {
    /// The host shares its own connection, as Windows' Internet Connection Sharing does. WSL's
    /// network is one, and it needs nothing beyond Virtual Machine Platform.
    #[serde(rename = "ICS")]
    Ics,
    #[serde(rename = "NAT")]
    Nat,
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
        }
    }
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}
