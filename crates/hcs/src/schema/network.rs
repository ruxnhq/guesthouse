//! The documents the Host Compute Network takes: a network, and an endpoint on it for one machine's
//! network card.

use super::Version;
use serde::{Deserialize, Serialize};

/// HCN's schema version. Its documents have their own numbering, apart from the compute system's.
pub const NETWORK_SCHEMA_VERSION: Version = Version { major: 2, minor: 0 };

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Network {
    pub schema_version: Version,
    pub name: String,
    pub r#type: NetworkType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkType {
    /// The host shares its own connection, as Windows' Internet Connection Sharing does, and gives
    /// each guest an address by DHCP. WSL's network is one, and it needs nothing beyond Virtual
    /// Machine Platform.
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
    pub fn ics(name: &str) -> Self {
        Network { schema_version: NETWORK_SCHEMA_VERSION, name: name.to_owned(), r#type: NetworkType::Ics }
    }
}
