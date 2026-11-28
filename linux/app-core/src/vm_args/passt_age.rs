//! Which of the app's passt words this PC's passt takes. Ubuntu 24.04's (2024_02_20) knows neither
//! `--map-host-loopback` nor `--dns-host`, and passt from 2024_10_30 to 2025_04_15 has `--dns-host`
//! but not `none` for the other. A packaged passt doesn't say which it is: Ubuntu's says "unknown
//! version" and Debian's prints none into a pipe. So passt is given the words and `--version`, and
//! it stops at the first word it doesn't take with a status that isn't success.

use std::fs;
use std::net::Ipv4Addr;
use std::process::{Command, Stdio};

/// systemd-resolved's list of the servers it asks, for a PC whose `/etc/resolv.conf` names only its
/// own stub on 127.0.0.53.
const RESOLVED_UPLINKS: &str = "/run/systemd/resolve/resolv.conf";
const PASST: &str = "passt";
const VERSION_WORD: &str = "--version";
/// Any address does for asking: passt only checks that it is one.
pub(super) const PROBE_DNS_HOST: &str = "192.0.2.1";

/// How old this PC's passt is, by the words it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasstAge {
    /// It takes all the app's words.
    #[default]
    Current,
    /// From 2024_10_30 to 2025_04_15: `--no-map-gw` in place of `--map-host-loopback none`, which
    /// does the same.
    NoLoopbackNone,
    /// Older than 2024_10_30, so no `--dns-host` either. Told no resolver, passt forwards DNS to none
    /// at all, and it only learns the host's from `/etc/resolv.conf` when it isn't given `--dns`. So
    /// the guest is told the host's resolver and asks it straight, as the PC's other programs do.
    NoDnsHost,
}

impl PasstAge {
    pub fn takes_loopback_none(self) -> bool {
        self == Self::Current
    }

    pub fn takes_dns_host(self) -> bool {
        self != Self::NoDnsHost
    }
}

/// [`PasstAge`] from what passt did when asked: whether it took the words of each age, newest first.
/// `None` if it took none of them, or couldn't be run.
pub fn age_from(takes: impl Fn(PasstAge) -> Option<bool>) -> Option<PasstAge> {
    [PasstAge::Current, PasstAge::NoLoopbackNone, PasstAge::NoDnsHost]
        .into_iter()
        .find(|age| takes(*age) == Some(true))
}

/// This PC's passt's age, asked with `words`, the app's own words for each age.
pub fn age_now(words: impl Fn(PasstAge) -> Vec<String>) -> Option<PasstAge> {
    let program = super::program_on_path(PASST)?;
    age_from(|age| {
        let mut command = Command::new(&program);
        command.args(words(age)).arg(VERSION_WORD);
        let status = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        status.ok().map(|status| status.success())
    })
}

/// The first IPv4 nameserver in a resolv.conf that isn't on the PC itself, which a guest can reach
/// through a passt that doesn't map the host's loopback.
pub fn first_outside_resolver(text: &str) -> Option<Ipv4Addr> {
    text.lines()
        .filter_map(|line| line.strip_prefix("nameserver ")?.trim().parse::<Ipv4Addr>().ok())
        .find(|address| !address.is_loopback() && !address.is_unspecified())
}

/// The resolver for a guest that asks it straight ([`PasstAge::NoDnsHost`]): the host's own if it
/// isn't on the PC, or else the ones systemd-resolved asks for it.
pub fn outside_resolver(resolv_conf: &str, uplinks: Option<&str>) -> Option<Ipv4Addr> {
    first_outside_resolver(resolv_conf).or_else(|| first_outside_resolver(uplinks?))
}

/// [`outside_resolver`] for this PC now.
pub fn outside_resolver_now() -> Option<String> {
    let resolv_conf = fs::read_to_string(super::RESOLVER_FILE).unwrap_or_default();
    let uplinks = fs::read_to_string(RESOLVED_UPLINKS).ok();
    outside_resolver(&resolv_conf, uplinks.as_deref()).map(|address| address.to_string())
}

/// What to say when a passt machine can't start on this PC's passt, if it can't.
pub fn start_problem(age: Option<PasstAge>, outside_resolver: Option<&str>) -> Option<String> {
    match age {
        None => Some(
            "This PC's passt doesn't take the app's network settings. Update passt, or turn passt off \
             for this machine."
                .to_owned(),
        ),
        Some(PasstAge::NoDnsHost) if outside_resolver.is_none() => Some(
            "This PC's passt is too old to pass DNS on to a resolver on the PC itself, and no other \
             resolver was found. It needs passt 2024_10_30 or newer, or turn passt off for this machine."
                .to_owned(),
        ),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_words_passt_takes_give_its_age() {
        assert_eq!(age_from(|_| Some(true)), Some(PasstAge::Current));
        let middle = |age| Some(age != PasstAge::Current);
        assert_eq!(age_from(middle), Some(PasstAge::NoLoopbackNone));
        let oldest = |age| Some(age == PasstAge::NoDnsHost);
        assert_eq!(age_from(oldest), Some(PasstAge::NoDnsHost));
        assert_eq!(age_from(|_| Some(false)), None);
        assert_eq!(age_from(|_| None), None, "passt that can't be run");
    }

    #[test]
    fn a_guest_asks_the_hosts_resolver_unless_it_is_on_the_pc() {
        assert_eq!(outside_resolver("nameserver 192.168.1.1\n", None), Some(Ipv4Addr::new(192, 168, 1, 1)));
        let ubuntu = "# stub\nnameserver 127.0.0.53\noptions edns0 trust-ad\n";
        let uplinks = "nameserver fe80::1%eth0\nnameserver 10.0.0.1\nnameserver 10.0.0.2\n";
        assert_eq!(outside_resolver(ubuntu, Some(uplinks)), Some(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(outside_resolver(ubuntu, None), None);
        assert_eq!(outside_resolver("nameserver 127.0.0.1\n", Some("nameserver 127.0.0.1\n")), None);
        let both = "nameserver 127.0.0.53\nnameserver 9.9.9.9\n";
        assert_eq!(outside_resolver(both, None), Some(Ipv4Addr::new(9, 9, 9, 9)));
    }

    #[test]
    fn only_passt_with_no_way_to_dns_or_no_words_at_all_refuses() {
        assert_eq!(start_problem(Some(PasstAge::Current), None), None);
        assert_eq!(start_problem(Some(PasstAge::NoLoopbackNone), None), None);
        assert_eq!(start_problem(Some(PasstAge::NoDnsHost), Some("10.0.0.1")), None);
        assert!(start_problem(Some(PasstAge::NoDnsHost), None).unwrap().contains("2024_10_30"));
        assert!(start_problem(None, Some("10.0.0.1")).unwrap().contains("turn passt off"));
    }
}
