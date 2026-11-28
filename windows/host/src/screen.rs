//! A Windows guest's screen, through remoteman. What has no Windows in it lives in
//! `guesthouse-remote-view-core`, which every host builds on; this keeps the names the Windows host
//! has always used for it, and the one thing that is about its own disks.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

pub use guesthouse_remote_view_core::{
    GUEST_FOLDER, GUEST_SETUP, PORT, SCRIPT, SETUP, encoder_for, fingerprint, fingerprint_in, trust_list,
    viewer_identity,
};

/// The viewer's state folder for a machine whose disk is `disk`, beside it.
pub fn state_beside(disk: &Path) -> PathBuf {
    disk.with_extension("screen")
}

/// The file in the guest's screen folder that says who may reach remoteman's port.
pub const HOSTS_FILE: &str = "hosts.txt";

/// What [`HOSTS_FILE`] holds: the network the machine is on, `shared` or `bridged`, then this PC's
/// addresses, a line each. On the shared network the guest also lets in its default gateway, which
/// is this PC on that network and may not have an address yet when the file is written, as Windows
/// makes the network's adapter as the machine starts.
pub fn hosts_file(bridged: bool, addresses: &[Ipv4Addr]) -> String {
    let mut text = String::from(if bridged { "bridged\n" } else { "shared\n" });
    for address in addresses {
        text.push_str(&format!("{address}\n"));
    }
    text
}

/// Whether `address` can be one of this PC's own addresses that a guest sees packets come from.
pub fn own_address(address: Ipv4Addr) -> bool {
    !(address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_multicast()
        || address.is_broadcast())
}

/// This PC's IPv4 addresses on every network it's on now, those [`own_address`] keeps, in order and
/// once each.
pub fn this_pcs_addresses() -> Vec<Ipv4Addr> {
    let mut found: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| match interface.ip() {
            std::net::IpAddr::V4(address) if own_address(address) => Some(address),
            _ => None,
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The address of this PC's that a packet to `guest` leaves from, by the route the system would
/// take. Nothing is sent.
pub fn source_towards(guest: Ipv4Addr) -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((guest, PORT)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(address) if !address.is_unspecified() => Some(address),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guest_is_told_its_network_and_this_pcs_addresses_a_line_each() {
        let addresses = [Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(172, 30, 0, 1)];
        assert_eq!(hosts_file(true, &addresses), "bridged\n192.168.1.20\n172.30.0.1\n");
        assert_eq!(hosts_file(false, &[]), "shared\n");
        assert!(SCRIPT.contains(&format!("'{HOSTS_FILE}'")), "the guest reads the file by this name");
    }

    #[test]
    fn only_addresses_a_packet_can_come_from_are_this_pcs() {
        assert!(own_address(Ipv4Addr::new(192, 168, 1, 5)));
        for address in ["127.0.0.1", "169.254.1.1", "0.0.0.0", "224.0.0.251", "255.255.255.255"] {
            assert!(!own_address(address.parse().unwrap()), "{address}");
        }
        assert!(this_pcs_addresses().iter().all(|address| own_address(*address)));
    }
}
