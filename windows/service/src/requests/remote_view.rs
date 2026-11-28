//! A machine's remote view: other devices on this PC's network viewing its screen through
//! remoteman, which is in every Windows guest already for Guesthouse's own window.
//!
//! The switch and the port are kept in the machine's own folder, beside its settings and not in
//! them, so they change while the machine runs. While a machine runs, a keeper on a thread of its
//! own holds the network side as the switch, the machine and its guest's address say: a firewall
//! rule that lets UDP in on the port from this PC's own network, then a relay from that port to
//! remoteman's port at the guest's address. Both go when the switch goes off or the machine stops,
//! the relay first, and before the machine counts as stopped. Turning it off never says `sharing
//! off` or `drop` to the guest, which would cut Guesthouse's own window too.
//!
//! The relay takes whatever address the guest says it has, on the shared network or bridged onto
//! this PC's, so one address and one port reach the machine either way.
//!
//! What remoteman's host says is read through the guest's serial port, only when the app asks, and
//! goes to the app and nowhere else: never into the machine's events, a report, or the service's
//! log, as it names a person's devices and the codes they pair with.

use super::{Service, Work, no_machine};
use crate::machines::{ViewStanding, write_switch};
use crate::store::Held;
use guesthouse_control::Event as MachineEvent;
use guesthouse_control::{Answer, NetworkMode};
use guesthouse_host_windows::guest_ctl::GuestCtl;
use guesthouse_host_windows::machine::Guest;
use guesthouse_host_windows::network::{
    Protocol, parse_view_rule_name, usable_guest_address, view_rule_name,
};
use guesthouse_host_windows::record::Record;
use guesthouse_host_windows::screen::{self, state_beside};
use guesthouse_relay::{Lan, LanSettings, ShareState as RelayState};
use guesthouse_remote_view_core::announce::{Announcement, Announcer};
use guesthouse_remote_view_core::pairing::{Event, Millis};
use guesthouse_remote_view_core::policy::Policy;
use guesthouse_remote_view_core::session::{Driver, StepEvent};
use guesthouse_remote_view_core::share::{ShareState, pairing_seconds};
use guesthouse_remote_view_core::switch::others_ports;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often a running machine is looked at for a change of its switch or its guest's address, and
/// for having stopped.
const POLL: Duration = Duration::from_millis(100);
/// How long an answer to `remote_view` waits for a reading of remoteman's host it has asked for.
/// One over the serial port takes well under this; a guest that doesn't answer is answered for
/// with the last reading, and the reading goes on behind.
const READ_WAIT: Duration = Duration::from_secs(3);

pub const NOT_WINDOWS: &str = "Only a Windows machine can be viewed from other devices.";

/// What a machine's remote view is opened with on this PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewSettings {
    pub machine: String,
    /// This PC's UDP port, on every network.
    pub port: u16,
    /// remoteman's host in the guest.
    pub upstream: SocketAddrV4,
    /// The guest's host's fingerprint, to announce by, once known.
    pub fingerprint: Option<String>,
}

/// An announcer that says nothing, for tests and wherever nothing may go on the network: the
/// machine is reached by the address and port the app shows. The service on Windows announces
/// through Windows' own responder behind the same trait.
pub struct Unannounced;

impl Announcer for Unannounced {
    fn announce(&mut self, _: &Announcement) -> Result<(), String> {
        Ok(())
    }

    fn withdraw(&mut self) {}
}

/// The relay open for a machine, which closes, and frees its port, when dropped.
struct Open(Lan);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// A machine's remote view opened on this PC's network.
pub struct Opened {
    /// The relay, which closes, taking its announcement back first, when dropped.
    pub relay: Held,
    /// Why the machine isn't listed on the network by its name, when it isn't. Its address and
    /// port still work.
    pub unlisted: Option<String>,
}

/// Opens `view` as the shared relay does it: listening on its port on every network of this PC,
/// passing each device on to the guest, and announced by `announcer` while it listens.
pub fn open_relay(view: &ViewSettings, announcer: Box<dyn Announcer + Send>) -> Result<Opened, String> {
    let settings = LanSettings {
        listen: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), view.port)),
        name: Some(view.machine.clone()),
        fingerprint: view.fingerprint.clone(),
        ..LanSettings::new(view.machine.clone(), SocketAddr::V4(view.upstream))
    };
    let lan = Lan::with_announcer(&settings, announcer);
    let status = lan.status();
    match status.state {
        // Listening with a reason is listening unannounced.
        RelayState::Listening => Ok(Opened { relay: Box::new(Open(lan)), unlisted: status.reason }),
        _ => Err(status.reason.unwrap_or_else(|| format!("port {} couldn't be listened on.", view.port))),
    }
}

/// What a keeper has opened, and for what.
struct Made {
    wanted: Wanted,
    rule: Option<String>,
    relay: Option<Held>,
    standing: ViewStanding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wanted {
    port: u16,
    guest: Ipv4Addr,
}

fn standing(state: ShareState, reason: Option<String>) -> ViewStanding {
    ViewStanding { state, reason }
}

pub(super) fn now_ms() -> Millis {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as Millis)
}

/// Why `record`'s machine can't be viewed from other devices, if it can't: what turning it on
/// refuses, and what a running machine's view says when it was turned on before.
fn refusal(name: &str, record: &Record, network: bool) -> Option<String> {
    if record.guest != Guest::Windows {
        return Some(NOT_WINDOWS.into());
    }
    if !record.installed {
        return Some(format!(
            "{name} isn't set up yet. Its first start sets it up; turn this on once it has."
        ));
    }
    (!network).then(|| {
        format!("{name} has no network, so no other device can reach it. Turn its network on first.")
    })
}

/// The pairing policy for the machine whose disk is `disk`: Guesthouse's own viewer, by its
/// fingerprint beside the disk, is kept out of sight. A machine that never had a viewer made has
/// none to hide.
/// Why a bridged guest can't be reached now.
fn moved(from: Ipv4Addr) -> String {
    format!(
        "This PC's address on the guest's network is {from} now, which the guest didn't have as this PC's when \
         it started, so it lets nothing in from it. Restart the machine to share its screen again."
    )
}

pub(super) fn policy(disk: &Path) -> Policy {
    let certificate = state_beside(disk).join("client").join("cert.der");
    fs::read(certificate).map_or_else(|_| Policy::linux(), |der| Policy::windows(&screen::fingerprint(&der)))
}

impl<W: Work> Service<W> {
    /// Turns the machine's remote view on or off, opens or closes pairing, and answers where it
    /// stands.
    pub(super) fn share(
        self: &Arc<Self>,
        asked: &str,
        on: Option<bool>,
        pairing: Option<bool>,
        seconds: Option<u32>,
    ) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        let seconds = match pairing_seconds(seconds.map(u64::from)) {
            Ok(seconds) => seconds,
            Err(reason) => return Answer::Refused { reason },
        };
        if pairing == Some(true) && on == Some(false) {
            return Answer::Refused { reason: "Pairing needs viewing from other devices on.".into() };
        }
        if let Some(on) = on
            && let Err(reason) = self.turn(&name, on)
        {
            return Answer::Refused { reason };
        }
        if on == Some(false) {
            self.end_pairing(&name);
        }
        match pairing {
            Some(true) => {
                if let Err(reason) = self.open_pairing(&name, seconds) {
                    return Answer::Refused { reason };
                }
            }
            // Closing a pairing that isn't open is no failure here.
            Some(false) => {
                if let Some(ctl) = self.running_ctl(&name) {
                    self.drive(&name, &ctl, |driver| driver.step(StepEvent::Plain(Event::Stop)));
                }
            }
            None => {}
        }
        self.remote_view(&name)
    }

    /// The way into the machine's guest, while it runs.
    pub(super) fn running_ctl(&self, name: &str) -> Option<GuestCtl> {
        self.machines.lock().get(name).and_then(|entry| entry.ctl.clone())
    }

    /// Runs `work` with the shared driver for the running machine `name`, one at a time per
    /// machine, as each step reads and writes the machine's pairing whole.
    pub(super) fn drive<R>(&self, name: &str, ctl: &GuestCtl, work: impl FnOnce(&Driver) -> R) -> R {
        let one = {
            let mut driving = self.driving.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            Arc::clone(driving.entry(name.to_owned()).or_default())
        };
        let _one_at_a_time = one.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let policy = policy(&self.machines.store.disk(name));
        let began = Instant::now();
        let done = work(&Driver {
            sessions: &self.sessions,
            machine: name,
            policy: &policy,
            guest: ctl,
            now: &now_ms,
        });
        let answered = self.sessions.with(name, |session| {
            let reading = session.reading.as_ref().filter(|_| session.reason.is_none())?;
            let hidden = reading.devices.iter().filter(|device| policy.hides(&device.fingerprint)).count();
            Some((reading.devices.len(), hidden))
        });
        self.told_answered(name, answered, began.elapsed());
        done
    }

    /// A pairing that is open, a device that waits, or the host-wide clipboard and files held off
    /// for either, end with the switch: pairing closes, the device is turned away, and what was held
    /// is given back. Nothing that would cut Guesthouse's own window is sent.
    fn end_pairing(&self, name: &str) {
        let Some(ctl) = self.running_ctl(name) else { return };
        let busy = self.sessions.with(name, |session| {
            session.pairing.as_ref().is_some_and(|pairing| {
                let view = pairing.view();
                view.open || view.pending.is_some() || pairing.held().is_some()
            })
        });
        if busy {
            self.drive(name, &ctl, |driver| driver.step(StepEvent::Plain(Event::SwitchOff)));
        }
    }

    /// Turns the switch, giving it a port the first time: one no other machine has, which a relay
    /// could listen on now. Only turning it on can be refused.
    fn turn(&self, name: &str, on: bool) -> Result<(), String> {
        let _one_at_a_time = self.switching.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if on {
            let record = Record::load(&self.machines.store.disk(name)).unwrap_or_default();
            let network = self.machines.lock()[name].settings.network;
            if let Some(reason) = refusal(name, &record, network) {
                return Err(reason);
            }
        }
        let (now, taken) = {
            let entries = self.machines.lock();
            let taken = others_ports(
                entries.iter(),
                |(other, _)| *other == name,
                |(_, entry)| entry.remote_view.remote_port,
            );
            (entries[name].remote_view, taken)
        };
        let turned = now.turned(on, &taken, |port| self.work.udp_port_free(port))?;
        if turned == now {
            return Ok(());
        }
        write_switch(&self.machines.store, name, &turned)?;
        if let Some(entry) = self.machines.lock().get_mut(name) {
            entry.remote_view = turned;
        }
        if turned.allow_remote_view != now.allow_remote_view {
            let said = if on {
                "Viewing from other devices is turned on."
            } else {
                "Viewing from other devices is turned off."
            };
            self.machines.tell(name, MachineEvent::Note(said.into()));
        }
        Ok(())
    }

    /// Where the machine's remote view stands. While it runs with the switch on, remoteman's host
    /// in the guest is read first if the last reading is old, for as long as [`READ_WAIT`].
    pub(super) fn remote_view(self: &Arc<Self>, asked: &str) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        let (switch, ctl, view) = {
            let entries = self.machines.lock();
            let entry = &entries[&name];
            (entry.remote_view, entry.ctl.clone(), entry.view.clone())
        };
        let disk = self.machines.store.disk(&name);
        let policy = policy(&disk);
        let (state, reason) = match (switch.allow_remote_view, &ctl) {
            (false, _) => (ShareState::Off, None),
            // On, and shared once the machine runs.
            (true, None) => (ShareState::Starting, None),
            // Turned on a moment ago, before its keeper has looked.
            (true, Some(_)) if view.state == ShareState::Off => (ShareState::Starting, None),
            (true, Some(_)) => (view.state, view.reason),
        };
        if let (true, Some(ctl)) = (switch.allow_remote_view, ctl) {
            self.read(&name, ctl);
        }
        let mut info = self
            .sessions
            .with(&name, |session| session.shown(&name, &policy, state, switch.remote_port, reason));
        if info.fingerprint.is_none() {
            info.fingerprint = Record::load(&disk).ok().and_then(|record| record.screen);
        }
        Answer::Share { info }
    }

    /// Reads remoteman's host through the shared driver if the last reading is old and none is
    /// being taken, and waits for it up to [`READ_WAIT`]. One that takes longer goes on behind.
    fn read(self: &Arc<Self>, name: &str, ctl: GuestCtl) {
        let begun = self.sessions.with(name, |session| {
            let begin = !session.busy && session.stale(now_ms());
            session.busy |= begin;
            begin
        });
        if !begun {
            return;
        }
        let (done, finished) = mpsc::channel();
        let (service, name) = (Arc::clone(self), name.to_owned());
        thread::spawn(move || {
            service.read_now(&name, &ctl, false);
            service.sessions.with(&name, |session| session.busy = false);
            let _ = done.send(());
        });
        let _ = finished.recv_timeout(READ_WAIT);
    }

    /// Reads remoteman's host through the shared driver, and lets pairing's clock go on after it
    /// when `tick`.
    pub(super) fn read_now(&self, name: &str, ctl: &GuestCtl, tick: bool) {
        self.drive(name, ctl, |driver| {
            driver.step(StepEvent::Read);
            if tick {
                driver.step(StepEvent::Plain(Event::Tick));
            }
        });
    }

    /// Writes in the service's log when the guest's remoteman first answers this run, and whenever
    /// whether it answers or how many devices it trusts changes: how long a reading took, and how
    /// many devices, of which how many are Guesthouse's own viewer. Nothing it said: no name, no
    /// fingerprint, no code.
    fn told_answered(&self, name: &str, answered: Option<(usize, usize)>, took: Duration) {
        let changed = {
            let mut entries = self.machines.lock();
            let Some(entry) = entries.get_mut(name).filter(|entry| entry.ctl.is_some()) else { return };
            let changed = entry.view_answered != Some(answered);
            entry.view_answered = Some(answered);
            changed
        };
        if changed {
            let line = if let Some((devices, hidden)) = answered {
                format!(
                    "{name}: remoteman in the guest answered over the serial port, in {} ms: {devices} devices trusted, {hidden} of them Guesthouse's own viewer.",
                    took.as_millis()
                )
            } else {
                format!("{name}: remoteman in the guest didn't answer over the serial port.")
            };
            self.work.log(&line);
        }
    }

    /// Keeps `name`'s remote view open on this PC's network as its switch, its guest and its
    /// guest's address say, until `over` is set, and closes it then. It runs on a thread of its own
    /// for as long as the machine does, as `netsh` and closing a relay take a moment.
    pub(super) fn keep_remote_view(&self, name: &str, over: &AtomicBool) {
        // Whether it's a Windows guest that has been set up doesn't change while it runs, unless this
        // is the first start, which sets it up and is no time to open it.
        let record = Record::load(&self.machines.store.disk(name)).unwrap_or_default();
        let mut made: Option<Made> = None;
        loop {
            let finished = over.load(Ordering::SeqCst);
            let wanted =
                if finished { Err(standing(ShareState::Off, None)) } else { self.wanted_view(name, &record) };
            if made.as_ref().map(|made| made.wanted) != wanted.as_ref().ok().copied() {
                if let Some(made) = made.take() {
                    self.close_view(name, made);
                }
                if let Ok(wanted) = wanted {
                    // The fingerprint the guest said last, which a first start after setting up
                    // may have said only now.
                    let disk = self.machines.store.disk(name);
                    let fingerprint =
                        Record::load(&disk).ok().and_then(|now| now.screen).or(record.screen.clone());
                    made = Some(self.open_view(name, wanted, fingerprint));
                }
            }
            let now = match (&wanted, &made) {
                (Ok(_), Some(made)) => made.standing.clone(),
                (Err(standing), _) => standing.clone(),
                (Ok(_), None) => standing(ShareState::Starting, None),
            };
            if let Some(entry) = self.machines.lock().get_mut(name) {
                entry.view = now;
            }
            if finished {
                return;
            }
            thread::sleep(POLL);
        }
    }

    /// The port and guest address `name`'s view should be open on now, or what it shows instead.
    fn wanted_view(&self, name: &str, record: &Record) -> Result<Wanted, ViewStanding> {
        let (switch, network, said, bridged, given) = {
            let entries = self.machines.lock();
            let entry = entries.get(name).ok_or_else(|| standing(ShareState::Off, None))?;
            (
                entry.remote_view,
                entry.settings.network,
                entry.run_address.clone(),
                matches!(entry.settings.network_mode, NetworkMode::Bridged { .. }),
                entry.boot_addresses.clone(),
            )
        };
        let port = match switch.values() {
            (true, Some(port)) => port,
            _ => return Err(standing(ShareState::Off, None)),
        };
        if let Some(reason) = refusal(name, record, network) {
            return Err(standing(ShareState::Failed, Some(reason)));
        }
        // Rules a stopped service left are swept first, or this one would go with them.
        if !self.cleaned.load(Ordering::SeqCst) {
            return Err(standing(ShareState::Starting, None));
        }
        let said = said.ok_or_else(|| standing(ShareState::Starting, None))?;
        let guest = usable_guest_address(&said).ok_or_else(|| {
            standing(
                ShareState::Failed,
                Some(format!("The guest's address {said:?} isn't one other devices can be passed on to.")),
            )
        })?;
        // A bridged guest lets in only the addresses this PC had as it started. One this PC has
        // taken since, as a new lease or another network gives, is let in from its next start.
        if bridged
            && let Some(from) = self.work.source_towards(guest)
            && !given.contains(&from)
        {
            return Err(standing(ShareState::Failed, Some(moved(from))));
        }
        Ok(Wanted { port, guest })
    }

    /// Makes the firewall rule, then the relay. A failure is said once, and kept, so nothing is
    /// tried again until the switch, the port or the guest's address changes.
    fn open_view(&self, name: &str, wanted: Wanted, fingerprint: Option<String>) -> Made {
        let port = wanted.port;
        let failed = |reason: String| {
            self.machines.tell(
                name,
                MachineEvent::Problem(format!(
                    "Other devices can't view it at this PC's port {port}: {reason}"
                )),
            );
            Made { wanted, rule: None, relay: None, standing: standing(ShareState::Failed, Some(reason)) }
        };
        let rule = view_rule_name(name, port);
        {
            let _one_at_a_time = self.forwarding.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Err(error) = self.work.network().add_rule(&rule, port, Protocol::Udp) {
                return failed(format!("the firewall rule couldn't be made: {error}"));
            }
        }
        let view = ViewSettings {
            machine: name.to_owned(),
            port,
            upstream: SocketAddrV4::new(wanted.guest, screen::PORT),
            fingerprint,
        };
        match self.work.open_view(&view) {
            Ok(Opened { relay, unlisted }) => {
                self.machines.tell(
                    name,
                    MachineEvent::Note(format!(
                        "Other devices on this PC's network can view it at port {port}, passed on to the guest, {}.",
                        wanted.guest
                    )),
                );
                if let Some(why) = &unlisted {
                    let said = format!("{why} Devices still reach it by this PC's address and port {port}.");
                    self.machines.tell(name, MachineEvent::Note(said.clone()));
                    self.work.log(&format!("{name}: {said}"));
                }
                let reason = unlisted
                    .map(|why| format!("{why} Devices still reach it by this PC's address and port."));
                Made {
                    wanted,
                    rule: Some(rule),
                    relay: Some(relay),
                    standing: standing(ShareState::Listening, reason),
                }
            }
            Err(error) => {
                // Without its relay the rule lets nothing through, so it goes.
                let _one_at_a_time = self.forwarding.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let _ = self.work.network().delete_rule(&rule);
                failed(format!("the relay couldn't listen: {error}"))
            }
        }
    }

    /// Closes the relay, so nothing more reaches the guest, then takes its rule away.
    fn close_view(&self, name: &str, made: Made) {
        let Made { relay, rule, .. } = made;
        let was_open = relay.is_some();
        drop(relay);
        if let Some(rule) = rule {
            let _one_at_a_time = self.forwarding.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Err(error) = self.work.network().delete_rule(&rule) {
                self.machines.tell(
                    name,
                    MachineEvent::Problem(format!(
                        "The firewall rule for its remote view couldn't be deleted: {error}"
                    )),
                );
            }
        }
        if was_open {
            self.machines.tell(name, MachineEvent::Note("Other devices can't view it now.".into()));
        }
    }

    /// Takes away the remote views' firewall rules a service that stopped without its machines'
    /// stopping left. No machine runs yet, so every one is a leftover.
    pub(super) fn clear_leftover_views(&self) {
        let network = self.work.network();
        let rules = match network.rules() {
            Ok(rules) => rules,
            Err(error) => {
                eprintln!(
                    "The firewall's rules couldn't be listed, so no remote view's left behind was looked for: {error}"
                );
                return;
            }
        };
        for rule in rules {
            let Some((name, port)) = parse_view_rule_name(&rule) else { continue };
            if network.delete_rule(&rule).is_ok() {
                let text = format!(
                    "A remote view's firewall rule left behind, for this PC's port {port}, is taken away."
                );
                eprintln!("{text}");
                if let Some(name) = self.machines.find(name) {
                    self.machines.tell(&name, MachineEvent::Note(text));
                }
            }
        }
    }

    /// The machine has stopped: what was read of its remoteman goes with it.
    pub(super) fn view_stopped(&self, name: &str) {
        self.sessions.remove(name);
        if let Some(entry) = self.machines.lock().get_mut(name) {
            entry.ctl = None;
            entry.view = ViewStanding::default();
            entry.view_answered = None;
        }
    }
}
