//! What a machine's network needs of the host beyond HCN's network itself: port forwards, which are
//! `netsh interface portproxy` entries with a Windows Firewall rule beside each, and the adapters a
//! machine can be bridged onto.
//!
//! The service does it all as the system, through [`Network`]. This module is every part of it that
//! isn't a call into Windows, so it's tested anywhere: the arguments `netsh` is run with, which are
//! fixed words and numbers and never go through a shell; reading what `netsh` prints, in any of
//! Windows' languages; and which adapters are offered. [`windows`](crate::windows) runs the real
//! `netsh.exe`.

use guesthouse_control::{AdapterInfo, Bridging};
use guesthouse_hcs::adapters::{self, Interface};
use std::net::Ipv4Addr;

/// What every firewall rule of Guesthouse's port forwards starts with, which is how a rule left
/// behind is known for Guesthouse's own.
pub const RULE_PREFIX: &str = "Guesthouse ";

/// The service that is Hyper-V's virtual machine management, which is installed with the Hyper-V
/// feature of Windows Pro, Education and Enterprise.
pub const HYPER_V_SERVICE: &str = "vmms";

/// The name of the firewall rule for `machine`'s forward from this PC's `port`.
pub fn rule_name(machine: &str, port: u16) -> String {
    format!("{RULE_PREFIX}{machine} {port}")
}

/// The machine and port in a rule's name as [`rule_name`] makes it. A machine's name has no spaces,
/// so the port is whatever follows the last one.
pub fn parse_rule_name(name: &str) -> Option<(&str, u16)> {
    let (machine, port) = name.strip_prefix(RULE_PREFIX)?.rsplit_once(' ')?;
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    (!machine.is_empty() && !machine.contains(' ')).then_some((machine, port))
}

/// One `netsh interface portproxy` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proxy {
    pub listen_address: Ipv4Addr,
    pub listen_port: u16,
    pub connect_address: Ipv4Addr,
    pub connect_port: u16,
}

/// The guest address a forward may lead to: one the guest could really have. The guest says its
/// address itself and it ends up in a command run as the system, so it's formatted from the parsed
/// address and never passed on as it was written.
pub fn usable_guest_address(text: &str) -> Option<Ipv4Addr> {
    let address: Ipv4Addr = text.trim().parse().ok()?;
    let unusable = address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || address.is_broadcast()
        || address.is_link_local();
    (!unusable).then_some(address)
}

/// The entries in the answer to `portproxy show v4tov4`, read by their shape, which is a listen
/// address and port then a connect address and port, as the headings and the blank lines are in the
/// language of the system.
pub fn parse_proxies(text: &str) -> Vec<Proxy> {
    let address = |word: &str| if word == "*" { Some(Ipv4Addr::UNSPECIFIED) } else { word.parse().ok() };
    text.lines()
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let [listen_address, listen_port, connect_address, connect_port] = words[..] else { return None };
            Some(Proxy {
                listen_address: address(listen_address)?,
                listen_port: listen_port.parse().ok()?,
                connect_address: address(connect_address)?,
                connect_port: connect_port.parse().ok()?,
            })
        })
        .collect()
}

/// The names of Guesthouse's rules in the answer to `firewall show rule`. Each rule's block starts
/// with its name's line and a line of dashes, and the word before the colon is in the language of
/// the system, so the name is what follows the colon on the line before dashes.
pub fn parse_rule_names(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut names = Vec::new();
    for pair in lines.windows(2) {
        let dashes = pair[1].len() >= 10 && pair[1].bytes().all(|byte| byte == b'-');
        if dashes
            && let Some((_, name)) = pair[0].split_once(':')
            && name.trim().starts_with(RULE_PREFIX)
        {
            names.push(name.trim().to_owned());
        }
    }
    names
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|word| (*word).to_owned()).collect()
}

pub fn show_proxies_args() -> Vec<String> {
    words(&["interface", "portproxy", "show", "v4tov4"])
}

/// Passes this PC's `listen` port, on every address, on to `connect` on the guest.
pub fn add_proxy_args(listen: u16, address: Ipv4Addr, connect: u16) -> Vec<String> {
    let mut args = words(&["interface", "portproxy", "add", "v4tov4"]);
    args.extend([
        format!("listenport={listen}"),
        "listenaddress=0.0.0.0".to_owned(),
        format!("connectport={connect}"),
        format!("connectaddress={address}"),
    ]);
    args
}

pub fn delete_proxy_args(listen: u16) -> Vec<String> {
    let mut args = words(&["interface", "portproxy", "delete", "v4tov4"]);
    args.extend([format!("listenport={listen}"), "listenaddress=0.0.0.0".to_owned()]);
    args
}

pub fn show_rules_args() -> Vec<String> {
    words(&["advfirewall", "firewall", "show", "rule", "name=all", "dir=in"])
}

/// Lets TCP `port` in from this PC's own network, on any profile, as a connection from the other
/// devices on the network is what a forward is for. The internet's own addresses are left out: a
/// forward is never for them.
pub fn add_rule_args(name: &str, port: u16) -> Vec<String> {
    let mut args = words(&["advfirewall", "firewall", "add", "rule"]);
    args.extend([
        format!("name={name}"),
        "dir=in".to_owned(),
        "action=allow".to_owned(),
        "protocol=TCP".to_owned(),
        format!("localport={port}"),
        "remoteip=localsubnet".to_owned(),
        "profile=any".to_owned(),
    ]);
    args
}

pub fn delete_rule_args(name: &str) -> Vec<String> {
    let mut args = words(&["advfirewall", "firewall", "delete", "rule"]);
    args.extend([format!("name={name}"), "dir=in".to_owned()]);
    args
}

/// Runs `netsh` with arguments, and answers what it printed, or why it didn't work.
pub trait Run: Send + Sync {
    fn run(&self, args: &[String]) -> Result<String, String>;
}

/// The `netsh` calls, over a way to run it.
pub struct Netsh<R: Run>(pub R);

impl<R: Run> Netsh<R> {
    pub fn proxies(&self) -> Result<Vec<Proxy>, String> {
        self.0.run(&show_proxies_args()).map(|text| parse_proxies(&text))
    }

    pub fn add_proxy(&self, listen: u16, address: Ipv4Addr, connect: u16) -> Result<(), String> {
        self.0.run(&add_proxy_args(listen, address, connect)).map(drop)
    }

    pub fn delete_proxy(&self, listen: u16) -> Result<(), String> {
        self.0.run(&delete_proxy_args(listen)).map(drop)
    }

    /// Guesthouse's rules' names. Windows has hundreds of rules of its own, so a failure to list
    /// them is a real one.
    pub fn rules(&self) -> Result<Vec<String>, String> {
        self.0.run(&show_rules_args()).map(|text| parse_rule_names(&text))
    }

    pub fn add_rule(&self, name: &str, port: u16) -> Result<(), String> {
        guard_rule_name(name)?;
        self.0.run(&add_rule_args(name, port)).map(drop)
    }

    pub fn delete_rule(&self, name: &str) -> Result<(), String> {
        guard_rule_name(name)?;
        self.0.run(&delete_rule_args(name)).map(drop)
    }
}

/// Only Guesthouse's own rules are made and deleted: one of its name's shape, which needs no quoting.
fn guard_rule_name(name: &str) -> Result<(), String> {
    match parse_rule_name(name) {
        Some(_) if name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')) => Ok(()),
        _ => Err(format!("{name:?} isn't the name of one of Guesthouse's firewall rules.")),
    }
}

/// The host's side of a machine's network, which only Windows can do.
pub trait Network: Send + Sync {
    /// Every portproxy entry, whoever made it.
    fn proxies(&self) -> Result<Vec<Proxy>, String>;
    fn add_proxy(&self, listen: u16, address: Ipv4Addr, connect: u16) -> Result<(), String>;
    /// Deletes the entry listening on every address at `listen`.
    fn delete_proxy(&self, listen: u16) -> Result<(), String>;
    /// The names of Guesthouse's inbound rules.
    fn rules(&self) -> Result<Vec<String>, String>;
    fn add_rule(&self, name: &str, port: u16) -> Result<(), String>;
    fn delete_rule(&self, name: &str) -> Result<(), String>;
    /// Whether nothing listens on TCP `port` of this PC, by trying to.
    fn port_free(&self, port: u16) -> bool;
    /// Whether IP Helper (iphlpsvc) runs, which listens for every portproxy entry.
    fn ip_helper_running(&self) -> bool;
    /// Whether a machine can be bridged, and onto which adapters.
    fn bridging(&self) -> Bridging;
}

/// What bridging offers when Hyper-V's service is there or not, and these interfaces are: the
/// physical adapters that are up. Hyper-V's switch is what a bridged network is made on, and its
/// virtual machine management service is there only with the feature turned on.
pub fn bridging_from(hyper_v: bool, interfaces: Result<Vec<Interface>, String>) -> Bridging {
    if !hyper_v {
        return Bridging {
            available: false,
            reason: Some(
                "Bridging needs Windows' Hyper-V feature, which isn't turned on, or isn't in this edition: \
                 Windows Home has none. On Pro, Education and Enterprise, turn on Hyper-V in Windows Features \
                 and restart."
                    .into(),
            ),
            adapters: Vec::new(),
        };
    }
    match interfaces {
        Ok(interfaces) => Bridging {
            available: true,
            reason: None,
            adapters: adapters::physical(&interfaces)
                .into_iter()
                .map(|adapter| AdapterInfo {
                    name: adapter.name.clone(),
                    description: adapter.description.clone(),
                    wifi: adapter.kind == adapters::WIFI,
                })
                .collect(),
        },
        Err(reason) => Bridging { available: false, reason: Some(reason), adapters: Vec::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        asked: Mutex<Vec<Vec<String>>>,
        answer: Mutex<Result<String, String>>,
    }

    impl Run for Fake {
        fn run(&self, args: &[String]) -> Result<String, String> {
            self.asked.lock().unwrap().push(args.to_vec());
            self.answer.lock().unwrap().clone()
        }
    }

    fn netsh(answer: Result<&str, &str>) -> Netsh<Fake> {
        Netsh(Fake {
            asked: Mutex::default(),
            answer: Mutex::new(answer.map(str::to_owned).map_err(str::to_owned)),
        })
    }

    fn asked(netsh: &Netsh<Fake>) -> Vec<Vec<String>> {
        netsh.0.asked.lock().unwrap().clone()
    }

    #[test]
    fn a_rule_is_named_for_its_machine_and_port_and_the_name_reads_back() {
        assert_eq!(rule_name("win11", 8080), "Guesthouse win11 8080");
        assert_eq!(parse_rule_name("Guesthouse win11 8080"), Some(("win11", 8080)));
        assert_eq!(parse_rule_name("Guesthouse a_b-c 1"), Some(("a_b-c", 1)));
        for other in [
            "Guesthouse",
            "Guesthouse win11",
            "Guesthouse win11 0",
            "Guesthouse win11 65536",
            "Guesthouse two words 80",
            "Guesthouse  80",
            "Remote Desktop 3389",
        ] {
            assert_eq!(parse_rule_name(other), None, "{other}");
        }
    }

    #[test]
    fn a_forward_is_made_with_these_exact_arguments() {
        let netsh = netsh(Ok(""));
        netsh.add_rule("Guesthouse win 8080", 8080).unwrap();
        netsh.add_proxy(8080, Ipv4Addr::new(172, 29, 1, 2), 80).unwrap();
        assert_eq!(
            asked(&netsh),
            [
                [
                    "advfirewall",
                    "firewall",
                    "add",
                    "rule",
                    "name=Guesthouse win 8080",
                    "dir=in",
                    "action=allow",
                    "protocol=TCP",
                    "localport=8080",
                    "remoteip=localsubnet",
                    "profile=any"
                ]
                .to_vec(),
                [
                    "interface",
                    "portproxy",
                    "add",
                    "v4tov4",
                    "listenport=8080",
                    "listenaddress=0.0.0.0",
                    "connectport=80",
                    "connectaddress=172.29.1.2"
                ]
                .to_vec()
            ]
        );
    }

    #[test]
    fn a_forward_is_removed_with_these_exact_arguments() {
        let netsh = netsh(Ok(""));
        netsh.delete_proxy(8080).unwrap();
        netsh.delete_rule("Guesthouse win 8080").unwrap();
        assert_eq!(
            asked(&netsh),
            [
                ["interface", "portproxy", "delete", "v4tov4", "listenport=8080", "listenaddress=0.0.0.0"]
                    .to_vec(),
                ["advfirewall", "firewall", "delete", "rule", "name=Guesthouse win 8080", "dir=in"].to_vec()
            ]
        );
    }

    #[test]
    fn only_guesthouses_own_rules_are_made_or_deleted() {
        let netsh = netsh(Ok(""));
        for name in
            ["Remote Desktop", "Guesthouse win 80\" & calc", "Guesthouse win 99999", "Guesthouse a.b 80"]
        {
            assert!(netsh.add_rule(name, 80).is_err(), "{name}");
            assert!(netsh.delete_rule(name).is_err(), "{name}");
        }
        assert!(asked(&netsh).is_empty(), "nothing was run");
    }

    const PROXIES_ENGLISH: &str = "\r\nListen on ipv4:             Connect to ipv4:\r\n\r\n\
        Address         Port        Address         Port\r\n\
        --------------- ----------  --------------- ----------\r\n\
        0.0.0.0         8080        172.29.1.2      80\r\n\
        192.168.1.5     2222        10.0.0.9        22\r\n\r\n";

    #[test]
    fn the_proxies_are_read_by_their_shape_in_any_language() {
        let german = "\nAuf IPv4 überwachen:        Verbindung mit IPv4 herstellen:\n\n\
            Adresse         Port        Adresse         Port\n\
            --------------- ----------  --------------- ----------\n\
            *               81          172.29.1.2      81\n";
        let netsh = netsh(Ok(PROXIES_ENGLISH));
        assert_eq!(
            netsh.proxies().unwrap(),
            [
                Proxy {
                    listen_address: Ipv4Addr::UNSPECIFIED,
                    listen_port: 8080,
                    connect_address: Ipv4Addr::new(172, 29, 1, 2),
                    connect_port: 80
                },
                Proxy {
                    listen_address: Ipv4Addr::new(192, 168, 1, 5),
                    listen_port: 2222,
                    connect_address: Ipv4Addr::new(10, 0, 0, 9),
                    connect_port: 22
                }
            ]
        );
        assert_eq!(parse_proxies(german).len(), 1);
        assert_eq!(parse_proxies(german)[0].listen_address, Ipv4Addr::UNSPECIFIED);
        assert!(parse_proxies("").is_empty());
        assert_eq!(asked(&netsh), [["interface", "portproxy", "show", "v4tov4"].to_vec()]);
    }

    #[test]
    fn the_rules_are_found_by_their_names_after_the_colon_whatever_the_language() {
        let shown = "\r\nRule Name:                            Guesthouse win 8080\r\n\
            ----------------------------------------------------------------------\r\n\
            Enabled:                              Yes\r\n\
            Direction:                            In\r\n\r\n\
            Rule Name:                            Core Networking - DNS (UDP-Out)\r\n\
            ----------------------------------------------------------------------\r\n\
            Enabled:                              Yes\r\n\r\n\
            Regelname:                            Guesthouse ubuntu 2222\r\n\
            ----------------------------------------------------------------------\r\n\
            Aktiviert:                            Ja\r\n";
        assert_eq!(parse_rule_names(shown), ["Guesthouse win 8080", "Guesthouse ubuntu 2222"]);
        assert!(parse_rule_names("No rules match the specified criteria.\r\n").is_empty());
        assert!(netsh(Err("The Windows Firewall service isn't running.")).rules().is_err());
    }

    #[test]
    fn the_guest_says_its_address_but_only_a_real_one_is_forwarded_to() {
        assert_eq!(usable_guest_address("172.29.1.2"), Some(Ipv4Addr::new(172, 29, 1, 2)));
        assert_eq!(usable_guest_address(" 192.168.1.40 "), Some(Ipv4Addr::new(192, 168, 1, 40)));
        for address in [
            "0.0.0.0",
            "127.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
            "169.254.3.4",
            "172.29.1.2 & calc",
            "::1",
            "",
            "1.2.3",
        ] {
            assert_eq!(usable_guest_address(address), None, "{address:?}");
        }
    }

    fn interface(name: &str, kind: u32, hardware: bool) -> Interface {
        Interface {
            name: name.into(),
            description: format!("{name} device"),
            kind,
            up: true,
            hardware,
            filter: false,
        }
    }

    #[test]
    fn bridging_is_offered_with_hyper_v_and_lists_the_physical_adapters() {
        let found = vec![
            interface("Ethernet", adapters::ETHERNET, true),
            interface("Wi-Fi", adapters::WIFI, true),
            interface("vEthernet (WSL)", adapters::ETHERNET, false),
        ];
        let bridging = bridging_from(true, Ok(found.clone()));
        assert!(bridging.available && bridging.reason.is_none());
        assert_eq!(
            bridging.adapters,
            [
                AdapterInfo { name: "Ethernet".into(), description: "Ethernet device".into(), wifi: false },
                AdapterInfo { name: "Wi-Fi".into(), description: "Wi-Fi device".into(), wifi: true }
            ]
        );
        let without = bridging_from(false, Ok(found));
        assert!(!without.available && without.adapters.is_empty());
        assert!(without.reason.unwrap().contains("Hyper-V"));
        let broken = bridging_from(true, Err("couldn't list".into()));
        assert!(!broken.available && broken.reason.as_deref() == Some("couldn't list"));
    }
}
