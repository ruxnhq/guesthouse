//! Remote View on a Windows machine's page: other devices on this PC's network viewing the
//! machine's screen through remoteman, which the service runs. This is what the group shows and
//! what each of its controls asks, made from the service's last answer, with nothing of Windows in
//! it, so its tests run on any system. Drawing it is `window.rs`'s, and asking is `bridge.rs`'s.
//!
//! The words are `guesthouse_remote_view_core::wording`'s. Those every host says are used from there
//! directly; the few only this host says are there too, under keys of their own, and [`proposed`]
//! keeps the names this app has used for them.
//!
//! On this host one device shows the machine at a time, Guesthouse's own window included: a device
//! that connects takes the screen from this PC, and Open screen takes it back. There's no
//! Disconnect, as the host can't drop a device without risking its own window.

// Off Windows only the tests use these.
#![cfg_attr(not(windows), allow(dead_code))]

use std::net::IpAddr;

use guesthouse_control::Request;
use guesthouse_remote_view_core::share::{DeviceInfo, PAIRING_SECONDS, PERMISSIONS, ShareInfo, ShareState};
use guesthouse_remote_view_core::wording;

/// Words this host shows that the other hosts don't. They are the shared wording's now, under their
/// own keys there, and keep their names here.
pub mod proposed {
    use guesthouse_remote_view_core::wording;

    /// The group's heading.
    pub const TITLE: &str = wording::REMOTE_VIEW_TITLE;
    pub const ADDRESS: &str = wording::ADDRESS_LABEL;
    pub const PORT: &str = wording::PORT_LABEL;
    pub const FINGERPRINT: &str = wording::FINGERPRINT_LABEL;
    /// The heading of the row while pairing is open, as the Linux app has it.
    pub const PAIRING_OPEN: &str = wording::PAIRING_OPEN_TITLE;
    pub const CANCEL: &str = wording::CANCEL;
    /// While pairing is open, or a device waits, the host holds clipboard and files off for every
    /// device, Guesthouse's own window included.
    pub const HELD: &str = wording::HELD_WHILE_PAIRING;
    /// Under the switch while the machine is bridged, which the service allows and nobody has tried.
    pub const BRIDGED: &str = wording::BRIDGED_UNTRIED;
    /// Said once an open pairing's time is up, until the service says it closed.
    pub const CLOSING: &str = wording::CLOSING;

    /// The countdown of an open pairing, "Closes in 1:45", as the Linux app has it.
    pub fn closes_in(seconds: u64) -> String {
        wording::closes_in(seconds)
    }

    /// The time a waiting device has to be answered in, as the Linux app has it.
    pub fn left_to_answer(seconds: u64) -> String {
        wording::left_to_answer(seconds)
    }

    /// In the group while a device views the machine: it has the screen, what that did to this PC's
    /// own window, and how to take it back. Tried with remoteman 0.1.9: the viewer's window here says
    /// its session ended as another device took over, and neither side connects again by itself.
    /// The shared `only_notice`, which says the picture comes back, isn't true here and isn't shown.
    pub fn takes_screen(name: &str) -> String {
        wording::takes_screen(name)
    }

    /// The Screen row's line while a device views the machine.
    pub fn screen_line(name: &str) -> String {
        wording::screen_line(name)
    }

    /// What a device's switch is called for a screen reader, which reads it apart from its row.
    pub fn switch_name(label: &str, device: &str) -> String {
        wording::switch_name(label, device)
    }

    /// "1:45" for a time left, which is what a countdown reads best as. [`closes_in`] writes its
    /// time this way through the shared wording, so only the tests call this one now; it keeps its
    /// name.
    #[allow(dead_code)]
    pub fn clock(seconds: u64) -> String {
        wording::countdown(seconds)
    }
}

/// What the person asked of a machine's remote view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The switch.
    Switch(bool),
    /// Open pairing for the shared crate's time.
    OpenPairing,
    StopPairing,
    Accept,
    Decline,
    /// Allow or refuse a paired device one of [`PERMISSIONS`].
    Allow {
        fingerprint: String,
        what: &'static str,
        on: bool,
    },
    /// Stop trusting a device.
    Revoke(String),
}

impl Ask {
    /// The request that asks it of the service, for the machine `name`.
    pub fn request(&self, name: &str) -> Request {
        let name = name.to_owned();
        match self.clone() {
            Ask::Switch(on) => Request::Share { name, on: Some(on), pairing: None, pairing_seconds: None },
            Ask::OpenPairing => Request::Share {
                name,
                on: Some(true),
                pairing: Some(true),
                pairing_seconds: u32::try_from(PAIRING_SECONDS).ok(),
            },
            Ask::StopPairing => Request::PairClose { name },
            Ask::Accept => Request::PairAccept { name },
            Ask::Decline => Request::PairDecline { name },
            Ask::Allow { fingerprint, what, on } => {
                Request::DeviceAllow { name, fingerprint, what: what.to_owned(), on }
            }
            Ask::Revoke(fingerprint) => Request::DeviceRevoke { name, fingerprint },
        }
    }
}

/// What the page knows of the machine from the service's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub running: bool,
    pub bridged: bool,
    /// Its switch, as the list says.
    pub switch: bool,
    /// Its port on this PC's network, which it keeps once given.
    pub port: Option<u16>,
}

/// A shown machine's remote view: the service's last answer, and what was asked since.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Viewing {
    pub machine: String,
    pub info: Option<ShareInfo>,
    /// This PC's address on its network, found with the answer.
    pub address: Option<IpAddr>,
    /// Why the switch was refused, until it's turned again.
    pub refused: Option<String>,
    /// The switch as turned, until the service answers: a switch shows what it's told, so this is
    /// what keeps it where the person put it meanwhile.
    pub asked_switch: Option<bool>,
    /// The same for each device's switches.
    pub asked_allow: Vec<(String, &'static str, bool)>,
    /// When the waiting device's time to be answered ends by this PC's clock, from when it was
    /// first seen, by its fingerprint and code.
    pub waiting_until: Option<(String, u64)>,
    /// The device whose Remove is being asked about: its fingerprint and the name shown.
    pub removing: Option<(String, String)>,
}

/// What the group shows of pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingShown {
    /// How the last pairing ended, or only that it's closed, and whether it can be opened now.
    Closed { text: String, can_open: bool },
    /// Open, until this time if it has an end.
    Open { until_ms: Option<u64> },
    /// A device waits, with the code to compare, until this time.
    Waiting { name: String, code: String, until_ms: u64 },
}

/// One of a paired device's switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permission {
    pub what: &'static str,
    pub label: &'static str,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceShown {
    pub fingerprint: String,
    /// Its name, or its fingerprint's start when it gave none.
    pub name: String,
    /// "Viewing now · 9f2c 41ab".
    pub line: String,
    pub permissions: Vec<Permission>,
}

/// What the group shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub on: bool,
    /// The switch can't be turned while a turn waits for its answer.
    pub switch_enabled: bool,
    pub headline: String,
    pub refused: Option<String>,
    /// What the service says beside a state that works, as when the machine's name is announced by
    /// something else and only its address reaches it. A failure's reason is in the headline.
    pub note: Option<String>,
    /// "192.168.1.20:7792", or only the port when this PC's address isn't known.
    pub address: Option<String>,
    pub port: Option<u16>,
    pub fingerprint: Option<String>,
    /// The device viewing the machine now, which has the screen. On this host any device does, in
    /// whatever placement, so the shared `only_notice`, which says the picture here comes back when
    /// the device leaves, isn't shown: it doesn't come back by itself.
    pub viewer: Option<String>,
    pub bridged: bool,
    /// Hidden while sharing failed or the switch is off.
    pub pairing: Option<PairingShown>,
    /// Pairing is open or a device waits, so clipboard and files are held off.
    pub held: bool,
    pub devices: Vec<DeviceShown>,
}

/// The key of a waiting device, which changes when another comes.
fn waiting_key(info: &ShareInfo) -> Option<String> {
    info.pending.as_ref().map(|pending| format!("{}/{}", pending.fingerprint, pending.code))
}

/// `info` without what changes at every reading of the host while nothing else does: the seconds a
/// waiting device has left, which the page counts down itself from the first.
pub fn settled(info: &ShareInfo) -> ShareInfo {
    let mut info = info.clone();
    if let Some(pending) = &mut info.pending {
        pending.seconds = 0;
    }
    info
}

impl Viewing {
    pub fn new(machine: &str) -> Viewing {
        Viewing { machine: machine.to_owned(), ..Viewing::default() }
    }

    /// Takes an answer about the machine at `now_ms`. One about another machine is let go.
    pub fn take(&mut self, info: ShareInfo, address: Option<IpAddr>, now_ms: u64) {
        if info.machine != self.machine {
            return;
        }
        let key = waiting_key(&info);
        if self.waiting_until.as_ref().map(|(seen, _)| seen) != key.as_ref() {
            self.waiting_until =
                key.zip(info.pending.as_ref().map(|pending| now_ms + pending.seconds * 1000));
        }
        self.info = Some(info);
        if address.is_some() {
            self.address = address;
        }
    }

    /// Notes what was asked, so its control shows it until the answer.
    pub fn asked(&mut self, ask: &Ask) {
        match ask {
            Ask::Switch(on) => {
                self.asked_switch = Some(*on);
                self.refused = None;
            }
            Ask::Allow { fingerprint, what, on } => {
                self.asked_allow.retain(|(other, which, _)| !(other == fingerprint && which == what));
                self.asked_allow.push((fingerprint.clone(), what, *on));
            }
            _ => {}
        }
    }

    /// Takes the answer to `ask`. Returns the words for a refusal that's to be said apart from the
    /// switch, which shows its own.
    pub fn answered(&mut self, ask: &Ask, answer: Result<ShareInfo, String>, now_ms: u64) -> Option<String> {
        match ask {
            Ask::Switch(_) => self.asked_switch = None,
            Ask::Allow { fingerprint, what, .. } => {
                self.asked_allow.retain(|(other, which, _)| !(other == fingerprint && which == what))
            }
            _ => {}
        }
        match answer {
            Ok(info) => {
                self.take(info, None, now_ms);
                None
            }
            Err(reason) if matches!(ask, Ask::Switch(_)) => {
                self.refused = Some(reason);
                None
            }
            Err(reason) => Some(reason),
        }
    }

    /// Whether the switch is on: as asked, then as the last answer says, then as the list does.
    pub fn on(&self, facts: &Facts) -> bool {
        self.asked_switch
            .or_else(|| self.info.as_ref().map(|info| info.state != ShareState::Off))
            .unwrap_or(facts.switch)
    }

    /// Whether the open pairing, or a waiting device, should be closed when the app goes.
    pub fn pairing_open(&self) -> bool {
        self.info.as_ref().is_some_and(|info| info.pairing || info.pending.is_some())
    }

    pub fn shown(&self, facts: &Facts) -> Shown {
        let on = self.on(facts);
        let off = ShareInfo::off(&self.machine);
        let info = self.info.as_ref().unwrap_or(&off);
        let port = info.port.or(facts.port);
        let failed = info.state == ShareState::Failed;
        let address = port.filter(|_| on).map(|port| match self.address {
            Some(IpAddr::V6(address)) => format!("[{address}]:{port}"),
            Some(address) => format!("{address}:{port}"),
            None => port.to_string(),
        });
        let viewer = info.viewer.clone().filter(|_| on);
        let pairing = (on && !failed).then(|| self.pairing(info, facts));
        let devices =
            if on { info.devices.iter().map(|device| self.device(device)).collect() } else { Vec::new() };
        Shown {
            on,
            switch_enabled: self.asked_switch.is_none(),
            headline: wording::headline(on, facts.running, info),
            refused: self.refused.clone(),
            note: info.reason.clone().filter(|_| on && !failed),
            address,
            port,
            fingerprint: info.fingerprint.clone().filter(|_| on),
            viewer,
            bridged: facts.bridged,
            held: on && (info.pairing || info.pending.is_some()),
            pairing,
            devices,
        }
    }

    fn pairing(&self, info: &ShareInfo, facts: &Facts) -> PairingShown {
        if let Some(pending) = &info.pending {
            let until_ms = self.waiting_until.as_ref().map_or(0, |(_, until)| *until);
            return PairingShown::Waiting {
                name: pending.name.clone(),
                code: pending.code.clone(),
                until_ms,
            };
        }
        if info.pairing {
            return PairingShown::Open { until_ms: info.pairing_until_ms };
        }
        PairingShown::Closed {
            text: wording::closed_text(info.closed_reason, info.paired.as_deref()),
            can_open: facts.running && info.state == ShareState::Listening,
        }
    }

    fn device(&self, device: &DeviceInfo) -> DeviceShown {
        let name = device_name(device);
        let permissions = PERMISSIONS
            .iter()
            .filter_map(|what| {
                let label = wording::permission_label(what)?;
                let asked = self
                    .asked_allow
                    .iter()
                    .find(|(fingerprint, which, _)| *fingerprint == device.fingerprint && which == what)
                    .map(|(_, _, on)| *on);
                Some(Permission { what, label, on: asked.or(device.permissions.get(what))? })
            })
            .collect();
        DeviceShown {
            fingerprint: device.fingerprint.clone(),
            name,
            line: wording::device_line(device),
            permissions,
        }
    }
}

/// A device's name, or its fingerprint's start when it gave none.
pub fn device_name(device: &DeviceInfo) -> String {
    if device.name.is_empty() { wording::short_fingerprint(&device.fingerprint) } else { device.name.clone() }
}

/// An open pairing's countdown at `now_ms`.
pub fn pairing_left(until_ms: Option<u64>, now_ms: u64) -> String {
    match until_ms {
        Some(until) => match wording::seconds_until(until, now_ms) {
            Some(left) => proposed::closes_in(left),
            None => proposed::CLOSING.to_owned(),
        },
        None => String::new(),
    }
}

/// A waiting device's countdown at `now_ms`.
pub fn waiting_left(until_ms: u64, now_ms: u64) -> String {
    proposed::left_to_answer(wording::seconds_until(until_ms, now_ms).unwrap_or(0))
}

/// The six digits of a pairing code, for a screen reader to read one by one.
pub fn spoken_code(code: &str) -> String {
    let digits: Vec<String> = code.chars().filter(char::is_ascii_digit).map(String::from).collect();
    format!("Code {}", digits.join(" "))
}

/// What a preview answers to `ask`, as a service would, so its buttons do something with no service.
#[cfg(any(feature = "preview", test))]
pub fn pretend(info: &ShareInfo, ask: &Ask, now_ms: u64) -> Result<ShareInfo, String> {
    use guesthouse_remote_view_core::share::{CloseReason, Permissions};
    let mut info = info.clone();
    match ask {
        Ask::Switch(on) => {
            info.state = if *on { ShareState::Listening } else { ShareState::Off };
            if !on {
                info.pairing = false;
                info.pending = None;
                info.viewer = None;
            }
        }
        Ask::OpenPairing => {
            info.pairing = true;
            info.pairing_until_ms = Some(now_ms + PAIRING_SECONDS * 1000);
            info.closed_reason = None;
            info.paired = None;
        }
        Ask::StopPairing => {
            info.pairing = false;
            info.pending = None;
            info.pairing_until_ms = None;
            info.closed_reason = Some(CloseReason::Asked);
        }
        Ask::Accept => {
            let pending = info.pending.take().ok_or("Nobody is waiting.")?;
            info.pairing = false;
            info.pairing_until_ms = None;
            info.closed_reason = Some(CloseReason::Paired);
            info.paired = Some(pending.name.clone());
            info.devices.push(DeviceInfo {
                fingerprint: pending.fingerprint,
                name: pending.name,
                connected: false,
                permissions: Permissions { clipboard: false, files: false, ..Permissions::default() },
            });
        }
        Ask::Decline => {
            info.pending.take().ok_or("Nobody is waiting.")?;
            info.pairing = false;
            info.pairing_until_ms = None;
            info.closed_reason = Some(CloseReason::Declined);
        }
        Ask::Allow { fingerprint, what, on } => {
            let device = info
                .devices
                .iter_mut()
                .find(|device| &device.fingerprint == fingerprint)
                .ok_or("No such device.")?;
            let permissions = &mut device.permissions;
            match *what {
                "input" => permissions.input = *on,
                "clipboard" => permissions.clipboard = *on,
                "files" => permissions.files = *on,
                "sound" => permissions.sound = *on,
                "mic" => permissions.mic = *on,
                "camera" => permissions.camera = *on,
                _ => return Err(format!("{what} isn't something a device can be allowed.")),
            }
        }
        Ask::Revoke(fingerprint) => info.devices.retain(|device| &device.fingerprint != fingerprint),
    }
    Ok(info)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_words;
