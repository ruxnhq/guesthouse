//! A machine's port forwards: while it runs, and its guest has said its address, each of the
//! machine's forwards is a portproxy entry and a firewall rule on this PC, and both are taken away
//! when the machine stops. A forward that can't be made is a line in the machine's log, and never a
//! boot that fails. Everything that touches Windows is [`Network`], so all of it is tested with a
//! double.

use super::{Service, Work};
use guesthouse_control::{Event, Forward, NetworkMode};
use guesthouse_host_windows::network::{parse_rule_name, rule_name, usable_guest_address};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// How often a running machine is looked at for an address to forward to, and for having stopped.
const POLL: Duration = Duration::from_millis(100);

/// Where a machine's forwards were pointed, and which of them were made.
struct Made {
    /// What they were made for, so a change of either is known.
    wanted: (Ipv4Addr, Vec<Forward>),
    /// The ports of this PC with a rule and an entry now.
    ports: Vec<u16>,
}

impl<W: Work> Service<W> {
    /// Keeps `name`'s forwards as its settings and its guest's address say, until `over` is set, and
    /// takes them away then. It runs on a thread of its own for as long as the machine does, so a
    /// slow `netsh` holds up nothing.
    pub(super) fn keep_forwards(&self, name: &str, over: &AtomicBool) {
        let mut made: Option<Made> = None;
        let mut refused_address: Option<String> = None;
        loop {
            let finished = over.load(Ordering::SeqCst);
            let wanted = if finished { None } else { self.wanted_forwards(name, &mut refused_address) };
            if made.as_ref().map(|made| &made.wanted) != wanted.as_ref() {
                if let Some(made) = made.take() {
                    self.unmake_forwards(name, &made);
                }
                if let Some(wanted) = wanted {
                    made = Some(self.make_forwards(name, wanted));
                }
            }
            if finished {
                return;
            }
            thread::sleep(POLL);
        }
    }

    /// The address and forwards `name` should have made now, if any: it runs on the shared network
    /// and has forwards, and its guest has said an address this run that a forward can lead to.
    fn wanted_forwards(
        &self,
        name: &str,
        refused_address: &mut Option<String>,
    ) -> Option<(Ipv4Addr, Vec<Forward>)> {
        let (forwards, said) = {
            let entries = self.machines.lock();
            let entry = entries.get(name)?;
            let settings = &entry.settings;
            if !settings.network
                || settings.network_mode != NetworkMode::Shared
                || settings.forwards.is_empty()
            {
                return None;
            }
            (settings.forwards.clone(), entry.run_address.clone()?)
        };
        match usable_guest_address(&said) {
            Some(address) => Some((address, forwards)),
            None => {
                if refused_address.as_deref() != Some(said.as_str()) {
                    self.machines.tell(
                        name,
                        Event::Problem(format!(
                            "The guest's address {said:?} isn't one a port can be forwarded to, so no forwards are made."
                        )),
                    );
                    *refused_address = Some(said);
                }
                None
            }
        }
    }

    /// Makes each of `wanted`'s forwards that can be made, saying in the machine's log why the others
    /// can't. A rule is made before its entry, so a rule is there whenever an entry is, which is how
    /// a crash's leftovers are known as Guesthouse's own.
    fn make_forwards(&self, name: &str, wanted: (Ipv4Addr, Vec<Forward>)) -> Made {
        let _one_at_a_time = self.forwarding.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let network = self.work.network();
        let (address, forwards) = (wanted.0, &wanted.1);
        let mut made = Made { wanted: wanted.clone(), ports: Vec::new() };
        if !network.ip_helper_running() {
            self.machines.tell(
                name,
                Event::Problem(
                    "IP Helper (the iphlpsvc service) isn't running, and port forwards need it: nothing \
                     listens for them until it's started."
                        .into(),
                ),
            );
        }
        let present = match network.proxies() {
            Ok(present) => present,
            Err(error) => {
                self.machines.tell(
                    name,
                    Event::Problem(format!(
                        "The port forwards couldn't be made, as the existing ones can't be listed: {error}"
                    )),
                );
                return made;
            }
        };
        for forward in forwards {
            let port = forward.host_port;
            let why_not = |reason: String| {
                self.machines.tell(
                    name,
                    Event::Problem(format!("Port {port} of this PC isn't forwarded to the guest: {reason}")),
                );
            };
            if present.iter().any(|proxy| proxy.listen_port == port) {
                // An entry of Guesthouse's own that an earlier run couldn't take away is cleared. One
                // that no rule of Guesthouse's marks is someone else's.
                let ours = network.rules().is_ok_and(|rules| rules.contains(&rule_name(name, port)));
                if !ours {
                    why_not("something else forwards this port already.".into());
                    continue;
                }
                if let Err(error) = network.delete_proxy(port) {
                    why_not(format!("an old forward of this machine's couldn't be cleared: {error}"));
                    continue;
                }
            }
            if !network.port_free(port) {
                why_not("another program is using it.".into());
                continue;
            }
            let rule = rule_name(name, port);
            if let Err(error) = network.add_rule(&rule, port) {
                why_not(format!("the firewall rule couldn't be made: {error}"));
                continue;
            }
            if let Err(error) = network.add_proxy(port, address, forward.guest_port) {
                // Without its entry the rule marks nothing, so it goes.
                let _ = network.delete_rule(&rule);
                why_not(format!("the forward couldn't be made: {error}"));
                continue;
            }
            made.ports.push(port);
            self.machines.tell(
                name,
                Event::Note(format!(
                    "Port {port} of this PC is forwarded to port {} of the guest, {address}.",
                    forward.guest_port
                )),
            );
        }
        made
    }

    /// Takes `made`'s forwards away: each entry, then its rule.
    fn unmake_forwards(&self, name: &str, made: &Made) {
        if made.ports.is_empty() {
            return;
        }
        let _one_at_a_time = self.forwarding.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let network = self.work.network();
        for &port in &made.ports {
            let mut left = false;
            if let Err(error) = network.delete_proxy(port) {
                left = true;
                self.machines.tell(
                    name,
                    Event::Problem(format!("The forward of port {port} couldn't be taken away: {error}")),
                );
            }
            // A rule is kept while its entry is, as it's what says the entry is Guesthouse's.
            if !left && let Err(error) = network.delete_rule(&rule_name(name, port)) {
                self.machines.tell(
                    name,
                    Event::Problem(format!("The firewall rule for port {port} couldn't be deleted: {error}")),
                );
            }
        }
        self.machines.tell(name, Event::Note("Its port forwards are taken away.".into()));
    }

    /// Takes away what a service that stopped without its machines' stopping left: each firewall rule
    /// of Guesthouse's, and the entry beside it. Entries are only Guesthouse's when a rule of its
    /// name is there, or when a stored machine has a forward to the very address the entry leads to,
    /// as other programs, WSL's setups among them, use portproxy too.
    pub(super) fn clear_leftover_forwards(&self) {
        let network = self.work.network();
        let stored: Vec<(String, Forward, Option<Ipv4Addr>)> = self
            .machines
            .lock()
            .iter()
            .flat_map(|(name, entry)| {
                let address = entry.settings.address.as_deref().and_then(usable_guest_address);
                entry.settings.forwards.iter().map(move |forward| (name.clone(), *forward, address))
            })
            .collect();
        let rules = match network.rules() {
            Ok(rules) => rules,
            Err(error) => {
                eprintln!(
                    "The firewall's rules couldn't be listed, so no forwards left behind were looked for: {error}"
                );
                return;
            }
        };
        let proxies = match network.proxies() {
            Ok(proxies) => proxies,
            Err(error) => {
                eprintln!(
                    "The port forwards couldn't be listed, so none left behind were looked for: {error}"
                );
                return;
            }
        };
        let told = |name: &str, text: String| {
            eprintln!("{text}");
            if self.machines.find(name).is_some() {
                self.machines.tell(name, Event::Note(text));
            }
        };
        let named: Vec<(String, u16)> = rules
            .iter()
            .filter_map(|rule| parse_rule_name(rule).map(|(machine, port)| (machine.to_owned(), port)))
            .collect();
        for (name, port) in &named {
            if proxies.iter().any(|proxy| proxy.listen_port == *port && proxy.listen_address.is_unspecified())
            {
                let _ = network.delete_proxy(*port);
            }
            if network.delete_rule(&rule_name(name, *port)).is_ok() {
                told(name, format!("A port forward left behind, from this PC's port {port}, is taken away."));
            }
        }
        // What the stored machines' forwards say is left is looked for by its own names too, in case
        // the firewall's answer was in a shape this didn't read. Only an entry that leads where the
        // machine's guest last was is taken, and only when no rule was found at all, as every rule
        // found was dealt with above.
        if !named.is_empty() {
            return;
        }
        for (name, forward, address) in &stored {
            let port = forward.host_port;
            let ours = proxies.iter().any(|proxy| {
                proxy.listen_port == port
                    && proxy.listen_address.is_unspecified()
                    && Some(proxy.connect_address) == *address
            });
            if ours {
                let _ = network.delete_proxy(port);
            }
            if network.delete_rule(&rule_name(name, port)).is_ok() {
                told(name, format!("A firewall rule left behind, for this PC's port {port}, is deleted."));
            }
        }
    }
}
