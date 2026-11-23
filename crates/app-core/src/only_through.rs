//! A NAT machine that goes out through one host interface, a VPN's, and nothing else. What it asks
//! of the host is here, as functions that read text, so they can be tried without a VPN.

use std::fs;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::Path;
use std::process::Command;

const INTERFACES_FOLDER: &str = "/sys/class/net";
const IPV6_FILE: &str = "/proc/net/if_inet6";
const RESOLVER_FILE: &str = "/etc/resolv.conf";
/// The scope of an address in `if_inet6` that leads beyond the link.
const GLOBAL_SCOPE: &str = "00";
/// The loopback interface, which is no way out.
const LOOPBACK: &str = "lo";
pub const NO_DNS_HELP: &str = "Set a DNS server in the machine's network settings.";

/// The interface a machine's traffic is held to, and what it can carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlyThrough {
    pub interface: String,
    /// It has an IPv6 address that leads beyond the link. Without one, passt may not be told to
    /// bind IPv6 to it.
    pub ipv6: bool,
}

impl OnlyThrough {
    /// The setting as the host has it now, or `None` if the name is empty.
    pub fn read(interface: &str) -> Option<Self> {
        let interface = interface.trim();
        (!interface.is_empty()).then(|| Self {
            interface: interface.to_owned(),
            ipv6: has_global_ipv6(&fs::read_to_string(IPV6_FILE).unwrap_or_default(), interface),
        })
    }
}

/// Whether the host has this interface.
pub fn interface_exists(name: &str) -> bool {
    exists_in(Path::new(INTERFACES_FOLDER), name)
}

fn exists_in(folder: &Path, name: &str) -> bool {
    !name.is_empty() && folder.join(name).exists()
}

/// The host's interfaces a machine can be held to, by name.
pub fn interface_names() -> Vec<String> {
    names_in(Path::new(INTERFACES_FOLDER))
}

fn names_in(folder: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name != LOOPBACK)
        .collect();
    names.sort();
    names
}

/// Whether `if_inet6`'s text gives this interface an address with global scope. Each line is the
/// address, then the interface's index, the prefix length, the scope and the flags, then the name.
pub fn has_global_ipv6(if_inet6: &str, interface: &str) -> bool {
    if_inet6.lines().any(|line| {
        let columns: Vec<&str> = line.split_whitespace().collect();
        columns.len() == 6 && columns[3] == GLOBAL_SCOPE && columns[5] == interface
    })
}

/// The first IPv4 address `resolvectl dns LINK` prints, as in `Link 7 (wg0): 10.64.0.1 fc00::1`.
pub fn link_dns(resolvectl_output: &str) -> Option<String> {
    let (_, servers) = resolvectl_output.split_once("):")?;
    servers.split_whitespace().find_map(|word| {
        let address = word.split('#').next()?;
        address.parse::<Ipv4Addr>().ok().map(|found| found.to_string())
    })
}

/// The first IPv4 nameserver of a `resolv.conf` that isn't an address of this PC, as `local` says.
pub fn resolv_conf_dns(text: &str, local: impl Fn(&str) -> bool) -> Option<String> {
    text.lines().find_map(|line| {
        let address = line.strip_prefix("nameserver ")?.trim();
        let parsed = address.parse::<Ipv4Addr>().ok()?;
        (!parsed.is_loopback() && !local(address)).then(|| address.to_owned())
    })
}

/// Whether this address is one this PC has: a socket can be bound to it. Through a VPN a server on
/// this PC, a local resolver's stub, can't be reached.
fn is_of_this_pc(address: &str) -> bool {
    UdpSocket::bind((address, 0)).is_ok()
}

/// The DNS server the guest's names go to: the one typed, else the link's own, else the host's own
/// that isn't on this PC. Never a public default.
pub fn choose_dns(typed: &str, link: Option<String>, host: Option<String>) -> Option<String> {
    Some(typed.trim()).filter(|text| !text.is_empty()).map(str::to_owned).or(link).or(host)
}

fn link_dns_now(interface: &str) -> Option<String> {
    let output = Command::new("resolvectl").args(["dns", interface]).output().ok()?;
    output.status.success().then(|| link_dns(&String::from_utf8_lossy(&output.stdout)))?
}

fn host_dns_now() -> Option<String> {
    resolv_conf_dns(&fs::read_to_string(RESOLVER_FILE).ok()?, is_of_this_pc)
}

/// The DNS server for a machine held to this interface, as the host has it now.
pub fn dns_now(typed: &str, interface: &str) -> Option<String> {
    let typed = typed.trim();
    if !typed.is_empty() {
        return Some(typed.to_owned());
    }
    choose_dns(typed, link_dns_now(interface), host_dns_now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    const IF_INET6: &str = "\
fe800000000000000000000000000001 07 40 20 80    wg0
fc000000000000000000000000000002 07 80 00 80    wg0
00000000000000000000000000000001 01 80 10 80     lo
20010db8000000000000000000000001 02 40 00 80   eth0
";

    #[test]
    fn only_an_address_beyond_the_link_makes_ipv6() {
        assert!(has_global_ipv6(IF_INET6, "wg0"));
        assert!(has_global_ipv6(IF_INET6, "eth0"));
        assert!(!has_global_ipv6(IF_INET6, "lo"), "host scope");
        assert!(!has_global_ipv6(IF_INET6, "tun0"));
        assert!(!has_global_ipv6("fe80 07 40 20 80 tun0\n", "tun0"), "link scope");
        assert!(!has_global_ipv6("", "wg0"));
    }

    #[test]
    fn the_links_first_ipv4_server_is_read_from_resolvectl() {
        assert_eq!(link_dns("Link 7 (wg0): 10.64.0.1 fc00::1\n").as_deref(), Some("10.64.0.1"));
        assert_eq!(link_dns("Link 7 (wg0): fc00::1 10.8.0.1\n").as_deref(), Some("10.8.0.1"));
        assert_eq!(link_dns("Link 7 (wg0): 10.64.0.1#vpn.example\n").as_deref(), Some("10.64.0.1"));
        assert_eq!(link_dns("Link 7 (wg0): fc00::1\n"), None);
        assert_eq!(link_dns("Link 7 (wg0):\n"), None);
        assert_eq!(link_dns(""), None);
    }

    #[test]
    fn the_hosts_server_is_the_first_one_that_is_not_on_this_pc() {
        let text =
            "search lan\nnameserver 127.0.0.53\nnameserver ::1\nnameserver 192.168.1.1\nnameserver 9.9.9.9\n";
        assert_eq!(resolv_conf_dns(text, |_| false).as_deref(), Some("192.168.1.1"));
        let this_pc = |address: &str| address == "192.168.1.1";
        assert_eq!(resolv_conf_dns(text, this_pc).as_deref(), Some("9.9.9.9"));
        assert_eq!(resolv_conf_dns("nameserver 127.0.0.1\n", |_| false), None);
        assert_eq!(resolv_conf_dns("", |_| false), None);
    }

    #[test]
    fn an_address_of_this_pc_is_seen_by_binding_it() {
        assert!(is_of_this_pc("127.0.0.1"));
        assert!(!is_of_this_pc("203.0.113.7"), "documentation range, so nobody's");
    }

    #[test]
    fn the_typed_server_comes_before_the_links_and_the_links_before_the_hosts() {
        let link = || Some("10.64.0.1".to_owned());
        let host = || Some("192.168.1.1".to_owned());
        assert_eq!(choose_dns(" 10.1.1.1 ", link(), host()).as_deref(), Some("10.1.1.1"));
        assert_eq!(choose_dns("", link(), host()).as_deref(), Some("10.64.0.1"));
        assert_eq!(choose_dns("", None, host()).as_deref(), Some("192.168.1.1"));
        assert_eq!(choose_dns("", None, None), None, "never a public default");
    }

    #[test]
    fn a_typed_server_needs_no_lookup_on_the_host() {
        assert_eq!(dns_now("10.1.1.1", "no-such-link").as_deref(), Some("10.1.1.1"));
    }

    #[test]
    fn interfaces_are_the_folders_names_without_loopback() {
        let scratch = Scratch::new();
        for name in ["wg0", "eth0", "lo"] {
            scratch.write(&format!("net/{name}/address"), "x");
        }
        let folder = scratch.path("net");
        assert_eq!(names_in(&folder), ["eth0", "wg0"]);
        assert!(exists_in(&folder, "wg0") && !exists_in(&folder, "tun0") && !exists_in(&folder, ""));
        assert!(names_in(&scratch.path("nothing")).is_empty());
    }

    #[test]
    fn an_empty_name_is_no_setting() {
        assert_eq!(OnlyThrough::read("  "), None);
        assert_eq!(OnlyThrough::read("no-such0").map(|found| found.interface).as_deref(), Some("no-such0"));
    }
}
