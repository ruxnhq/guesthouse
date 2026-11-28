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
    fn the_screen_reads_the_hosts_file_where_the_host_writes_it() {
        assert!(SCRIPT.contains(&format!(r"'C:\{}'", crate::guest::HOSTS_PATH)));
    }

    /// The screen's script changes firewall rules only through the report's functions, which make a
    /// rule with its addresses and turn it off when none is known, and the report defines each one
    /// before it runs the script.
    #[test]
    fn the_screen_changes_its_rules_only_through_the_reports_functions() {
        for cmdlet in ["New-NetFirewallRule", "Set-NetFirewallRule", "Enable-NetFirewallRule"] {
            assert!(!SCRIPT.contains(cmdlet), "{cmdlet}");
        }
        let runs = crate::guest::REPORT.find(". $screenScript").unwrap();
        for function in ["Get-HostAddresses", "Get-Gateways", "Set-HostRule", "Limit-HostRule"] {
            assert!(SCRIPT.contains(&format!("{function} ")), "{function}");
            let defined = crate::guest::REPORT.find(&format!("function {function}(")).unwrap();
            assert!(defined < runs, "{function}");
        }
        let script = SCRIPT.replace("\r\n", "\n");
        let first_rule = script.find("Set-HostRule $rule").unwrap();
        for way_out in ["\n    return\n", "\n        return\n"] {
            assert!(script.find(way_out).is_none_or(|at| at > first_rule), "a return before the rule is set");
        }
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
