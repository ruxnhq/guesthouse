//! A machine's remote view as the app runs it: the switch in its side file, remoteman's host put in
//! the guest when it is needed, the helper started, opened, closed and found again, remoteman's
//! `ctl` words run in the guest, and pairing's clock.
//!
//! The settings page calls the functions at the end ([`info`], [`share`], [`pair_close`],
//! [`pair_accept`], [`pair_decline`], [`device_allow`], [`device_revoke`], [`device_drop`]), each on a
//! thread of its own: they may ask the guest, which takes tens of milliseconds, and an install takes
//! half a minute. [`info`] never asks the guest itself: it gives what was last read, and has it read
//! again behind it.
//!
//! What the guest's host cannot keep is kept here: how long pairing stays open, and the host-wide
//! clipboard and files held off while a device is let in (`held`, in a file beside the switch, so an
//! app that died with pairing open closes it and gives them back at its next launch: [`launched`]).

use super::{Request, Settings, end, pid, start};
use crate::actions::guest_upload::put_big_file_with;
use crate::context::Context;
use crate::downloads::fetch_checked;
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::log::write_log;
use crate::machine::{Kind, Machine, load, load_all};
use crate::tasks::{State, status};
use guesthouse_daemon::slot::Slot;
use guesthouse_remote_view_core::ctl::{self, Ack, Words, parse_ack};
use guesthouse_remote_view_core::encoder_for;
use guesthouse_remote_view_core::install::{self, CURRENT, INSTALLED, NEEDED, OLD_TAKE_OVER, Pin};
use guesthouse_remote_view_core::pairing::{Event, HostWide, Millis, Pairing};
use guesthouse_remote_view_core::policy::Policy;
use guesthouse_remote_view_core::share::{ShareInfo, ShareState, pairing_seconds};
use guesthouse_remote_view_core::status::{Reply, Status, parse_reply};
use guesthouse_remote_view_core::switch::{SWITCH_FILE, Switch};
use guesthouse_remote_view_core::view::{Facts, share_info};
use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::process::Child;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Beside the switch: what pairing has held off and owes back.
pub const HELD_FILE: &str = "remote-view-held.json";
/// One `ctl` command in the guest. The agent answers in about 40 ms.
const CTL_SECONDS: u64 = 20;
const CHECK_SECONDS: u64 = 30;
/// The installer took 18 s in a guest with Secure Boot on.
const INSTALL_SECONDS: u64 = 180;
/// How old a reading may be before [`info`] has another taken behind it.
const STALE: Duration = Duration::from_secs(2);
/// Where the downloaded installer is kept, in the fetched parts' folder.
const PARTS_FOLDER: &str = "remoteman";
/// Where a build that ships the installer has it, in the program's folder.
const SHIPPED: &str = "remoteman/remoteman-host-setup.exe";
/// The release of remoteman's host this host puts in a guest. 0.1.6 is the first whose `--no-pair`
/// still lets `pair on` open pairing, which every pairing here needs. The Windows host carries its own
/// release, pinned in `windows/remoteman.json`.
const REMOTEMAN_VERSION: &str = "0.1.7";
const REMOTEMAN_BASE: &str = "https://files.ruxn.com/p/remoteman/";
const REMOTEMAN_SHA256: &str = "17c9094524a4540b52ac7ae53c1e1bd0612e0b93eb8f9861b2cb9a2017ea9e57";
pub const TOO_OLD: &str = "This machine's display driver is older than viewing from other devices needs: its \
                           take-over script would take remoteman's screen for its own. Update the machine's \
                           display driver, then turn this on again.";
const NOT_WINDOWS: &str = "Only a Windows machine can be viewed from other devices.";

// ---- The switch and what is kept beside it ----

fn read_switch(context: &Context, machine: &Machine) -> Switch {
    Switch::read(fs::read_to_string(machine.folder(context).join(SWITCH_FILE)).ok().as_deref())
}

fn write_switch(context: &Context, machine: &Machine, switch: &Switch) -> Result<()> {
    let file = machine.folder(context).join(SWITCH_FILE);
    match switch.text() {
        Some(text) => fs::write(file, text)?,
        None => match fs::remove_file(file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        },
    }
    Ok(())
}

/// Whether the machine's remote view is switched on.
pub fn allowed(context: &Context, machine: &Machine) -> bool {
    machine.kind == Kind::Windows && read_switch(context, machine).allow_remote_view
}

/// The machine's port on the network, once it has been given one.
pub fn port(context: &Context, machine: &Machine) -> Option<u16> {
    read_switch(context, machine).remote_port
}

/// Turns the switch on or off, giving it a port the first time: one no other machine has and that
/// is free on this PC.
pub fn set_allowed(context: &Context, machine: &Machine, on: bool) -> Result<Switch> {
    if machine.kind != Kind::Windows {
        return Err(Failure::new(NOT_WINDOWS));
    }
    let (machines, _) = load_all(context);
    let taken: Vec<u16> = machines
        .iter()
        .filter(|other| other.id != machine.id)
        .filter_map(|other| port(context, other))
        .collect();
    let switch = read_switch(context, machine)
        .turned(on, &taken, |port| UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok())
        .map_err(Failure)?;
    write_switch(context, machine, &switch)?;
    Ok(switch)
}

fn held_file(context: &Context, machine: &Machine) -> std::path::PathBuf {
    machine.folder(context).join(HELD_FILE)
}

fn read_held(context: &Context, machine: &Machine) -> Option<HostWide> {
    serde_json::from_str(&fs::read_to_string(held_file(context, machine)).ok()?).ok()
}

fn write_held(context: &Context, machine: &Machine, held: Option<HostWide>) {
    let file = held_file(context, machine);
    let _ = match held {
        Some(held) => fs::write(file, serde_json::to_string(&held).unwrap_or_default()),
        None => fs::remove_file(file),
    };
}

// ---- The guest ----

fn running_slot(context: &Context, machine: &Machine) -> Option<Slot> {
    let found = status(context, machine);
    (found.state == State::Running).then_some(found.slot).flatten().map(crate::slots::number)
}

/// The encoder for the machine's card, as remoteman's host takes it.
pub fn encoder(machine: &Machine) -> &'static str {
    let vendor = match machine.windows_maker() {
        Some(GpuMaker::Nvidia) => "10DE",
        Some(GpuMaker::Amd) => "1002",
        Some(GpuMaker::Intel) => "8086",
        None => "",
    };
    encoder_for(&format!("PCI\\VEN_{vendor}"))
}

/// remoteman's host installer at this host's pin.
pub fn pin() -> Pin {
    let file = format!("remoteman-host-setup-{REMOTEMAN_VERSION}.exe");
    Pin {
        version: REMOTEMAN_VERSION.to_owned(),
        url: format!("{REMOTEMAN_BASE}{file}"),
        file,
        sha256: REMOTEMAN_SHA256.to_owned(),
    }
}

/// The pinned installer on this PC: the copy a build shipped, or one downloaded once into the fetched
/// parts' folder, each only if its SHA-256 is the pin's.
pub fn installer(context: &Context, pin: &Pin) -> Result<std::path::PathBuf> {
    let shipped = context.paths.program.join(SHIPPED);
    if shipped.is_file() && crate::downloads::sha256(&shipped).is_ok_and(|sum| sum == pin.sha256) {
        return Ok(shipped);
    }
    let kept = context.paths.parts.join(PARTS_FOLDER).join(&pin.file);
    fetch_checked(&pin.url, &pin.sha256, &kept, "Downloading remoteman's host", &|_| {})?;
    Ok(kept)
}

/// Puts remoteman's host in the guest when the pin, the encoder or the host itself changed, and lets
/// its port in from this PC only. `ask` runs a script in the guest. Gives the guest's fingerprint when
/// it installed. Nothing is run when all is current, as each installer run blips the picture.
pub fn ensure_installed(
    context: &Context,
    machine: &Machine,
    ask: &dyn Fn(&str, u64) -> String,
) -> Result<Option<String>> {
    let pin = pin();
    let encoder = encoder(machine);
    let said = ask(&install::check_script(&pin.version, encoder), CHECK_SECONDS);
    match said.trim() {
        CURRENT => return Ok(None),
        NEEDED => {}
        OLD_TAKE_OVER => return Err(Failure::new(TOO_OLD)),
        "" => return Err(Failure::new("Windows did not answer. Try again once it is up.")),
        other => return Err(Failure(format!("Windows said: {other}"))),
    }
    let local = installer(context, &pin)?;
    if !put_big_file_with(ask, &local, install::GUEST_INSTALLER)? {
        return Err(Failure::new("remoteman's host did not reach Windows. Try again."));
    }
    let said = ask(&install::install_script(&pin.version, encoder), INSTALL_SECONDS);
    let _ = write_log(context, &format!("remote view: {}: {}", machine.id, said.trim().replace('\n', " | ")));
    if !said.contains(INSTALLED) {
        return Err(Failure(format!("remoteman's host could not be installed: {}", said.trim())));
    }
    Ok(install::fingerprint_after(&said))
}

/// Runs one `ctl` command in the guest, and gives what the host printed.
fn run_ctl(context: &Context, machine: &Machine, slot: Slot, words: &Words) -> String {
    context.ask_guest(machine, &install::ctl_script(words), CTL_SECONDS, Some(slot))
}

// ---- The helper ----

/// Children the app started and has not yet reaped.
fn children() -> MutexGuard<'static, Vec<Child>> {
    static CHILDREN: OnceLock<Mutex<Vec<Child>>> = OnceLock::new();
    CHILDREN.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reaps the helpers this process started that have ended.
pub fn reap() {
    children().retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
}

/// The helper of a running machine listening on its port, started if it is not running (found again
/// by its pid file after an app restart), with the guest's fingerprint for its announcement.
pub fn open_helper(
    context: &Context,
    machine: &Machine,
    slot: Slot,
    port: u16,
    fingerprint: Option<String>,
) -> Result<()> {
    reap();
    let listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    if pid(context, Some(slot)).is_none() {
        let mut settings = Settings::for_machine(context, &machine.id, slot, Some(listen))?;
        settings.name = Some(machine.name.clone());
        settings.fingerprint = fingerprint;
        children().push(start(context, slot, &settings)?);
        return Ok(());
    }
    let request = Request::Open {
        address: Some(listen.ip()),
        port: Some(port),
        name: Some(machine.name.clone()),
        fingerprint,
    };
    match super::ask(&context.run_folder(Some(slot)), &request) {
        Ok(super::RelayReply::Status(_)) => Ok(()),
        Ok(super::RelayReply::Error(reason)) => Err(Failure(reason)),
        Err(error) => Err(Failure(format!("The remote view helper did not answer: {error}"))),
    }
}

/// Closes the helper's network side, leaving it ready to open again while the machine runs.
pub fn close_helper(context: &Context, slot: Slot) {
    let _ = super::ask(&context.run_folder(Some(slot)), &Request::Close);
}

/// What the helper says of itself, if it runs.
fn helper_status(context: &Context, slot: Slot) -> Option<super::RelayStatus> {
    match super::ask(&context.run_folder(Some(slot)), &Request::Status).ok()? {
        super::RelayReply::Status(status) => Some(status),
        super::RelayReply::Error(_) => None,
    }
}

/// Everything for a machine that has just come up, or whose switch was just turned on: remoteman's
/// host in the guest, then the helper open. A machine with the switch off gets nothing.
pub fn bring_up(
    context: &Context,
    machine: &Machine,
    slot: Slot,
    ask: &dyn Fn(&str, u64) -> String,
) -> Result<()> {
    let switch = read_switch(context, machine);
    if !switch.allow_remote_view || machine.kind != Kind::Windows {
        return Ok(());
    }
    let port = switch.remote_port.ok_or_else(|| Failure::new("The machine has no port yet."))?;
    let installed = ensure_installed(context, machine, ask)?;
    let said = ask(&install::ctl_script(&ctl::share(true)), CTL_SECONDS);
    let fingerprint =
        installed.or_else(|| match parse_reply(&ask(&install::ctl_script(&ctl::status()), CTL_SECONDS)) {
            Reply::Status(status) => status.fingerprint,
            _ => None,
        });
    if !matches!(parse_ack(&said), Ack::Done(_) | Ack::NotKept(_)) {
        let _ = write_log(context, &format!("remote view: sharing on: {}", said.trim()));
    }
    open_helper(context, machine, slot, port, fingerprint)
}

/// This PC's address on its network, which a device on it types in: the one the system would send
/// from to the internet, if that is on a real network. Nothing is sent to find it out.
pub fn lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

// ---- Pairing and the state shown ----

#[derive(Default)]
struct Session {
    pairing: Option<Pairing>,
    reading: Option<Status>,
    read_at: Option<SystemTime>,
    reason: Option<String>,
    /// A reading or a clock is running behind [`info`].
    busy: bool,
    fingerprint: Option<String>,
}

fn sessions() -> MutexGuard<'static, HashMap<String, Session>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> Millis {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as Millis)
}

/// Feeds one event to the machine's pairing, sends the commands it gives in order, keeps `held`, and
/// reads the host again when asked to. Gives why the event was refused.
fn step(context: &Context, machine: &Machine, slot: Slot, event: StepEvent) -> Option<String> {
    let mut pairing = {
        let mut all = sessions();
        let session = all.entry(machine.id.clone()).or_default();
        session.pairing.get_or_insert_with(|| fresh_pairing(context, machine)).clone()
    };
    let mut events = vec![event];
    let mut refused = None;
    while let Some(event) = events.pop() {
        let reading;
        let step = match &event {
            StepEvent::Plain(event) => pairing.step(now_ms(), *event),
            StepEvent::Read => {
                reading = read(context, machine, slot);
                let Some(status) = &reading else { continue };
                pairing.step(now_ms(), Event::Reading(status))
            }
        };
        pairing = step.pairing;
        refused = refused.or(step.refused);
        for words in &step.commands {
            let said = run_ctl(context, machine, slot, words);
            if let Ack::Refused(why) = parse_ack(&said) {
                let _ = write_log(context, &format!("remote view: ctl {}: {why}", words.join(" ")));
            }
        }
        write_held(context, machine, pairing.held());
        if step.read_now || !step.commands.is_empty() {
            events.push(StepEvent::Read);
        }
    }
    sessions().entry(machine.id.clone()).or_default().pairing = Some(pairing);
    refused
}

enum StepEvent {
    Plain(Event<'static>),
    Read,
}

/// A pairing for this run of the app: whatever an earlier run held is owed back.
fn fresh_pairing(context: &Context, machine: &Machine) -> Pairing {
    Pairing::new(Policy::linux()).step(0, Event::Started { held: read_held(context, machine) }).pairing
}

/// Reads the host's status, and keeps it.
fn read(context: &Context, machine: &Machine, slot: Slot) -> Option<Status> {
    let said = run_ctl(context, machine, slot, &ctl::status());
    let mut all = sessions();
    let session = all.entry(machine.id.clone()).or_default();
    session.read_at = Some(SystemTime::now());
    match parse_reply(&said) {
        Reply::Status(status) => {
            session.reason = None;
            session.fingerprint = status.fingerprint.clone().or(session.fingerprint.take());
            session.reading = Some(*status.clone());
            Some(*status)
        }
        Reply::Stopping => None,
        Reply::Error(why) => {
            session.reason = Some(why);
            None
        }
    }
}

/// Reads the host and lets the clock go on, then keeps doing so once a second for as long as pairing
/// is open or a device waits. Runs behind [`info`], one at a time per machine.
fn refresh_behind(context: &Context, machine: &Machine, slot: Slot) {
    {
        let mut all = sessions();
        let session = all.entry(machine.id.clone()).or_default();
        if session.busy {
            return;
        }
        session.busy = true;
    }
    let (context, machine) = (context.clone(), machine.clone());
    thread::spawn(move || {
        loop {
            step(&context, &machine, slot, StepEvent::Read);
            step(&context, &machine, slot, StepEvent::Plain(Event::Tick));
            let poll =
                sessions().get(&machine.id).and_then(|s| s.pairing.as_ref()).and_then(Pairing::poll_ms);
            match poll {
                Some(ms) if running_slot(&context, &machine) == Some(slot) => {
                    thread::sleep(Duration::from_millis(ms))
                }
                _ => break,
            }
        }
        sessions().entry(machine.id.clone()).or_default().busy = false;
    });
}

/// Where the machine's remote view stands, from what was last read. Never waits for the guest: a
/// reading older than two seconds is taken again behind it, and the clock runs there.
pub fn info(context: &Context, machine: &Machine) -> ShareInfo {
    reap();
    let switch = read_switch(context, machine);
    let slot = running_slot(context, machine);
    let mut all = sessions();
    let session = all.entry(machine.id.clone()).or_default();
    let helper = slot.and_then(|slot| helper_status(context, slot));
    let state = match (switch.allow_remote_view, slot, &helper) {
        (false, ..) => ShareState::Off,
        (true, None, _) => ShareState::Starting,
        (true, Some(_), Some(helper)) if helper.state == ShareState::Listening => ShareState::Listening,
        (true, Some(_), Some(helper)) if helper.state == ShareState::Failed => ShareState::Failed,
        (true, Some(_), _) => ShareState::Starting,
    };
    let reason = helper.as_ref().and_then(|helper| helper.reason.clone()).or(session.reason.clone());
    let pairing = session.pairing.clone().unwrap_or_else(|| Pairing::new(Policy::linux()));
    let facts = Facts {
        machine: &machine.id,
        state,
        port: switch.remote_port,
        fingerprint: session.fingerprint.as_deref(),
        reason: reason.as_deref(),
    };
    let shown = share_info(&facts, session.reading.as_ref(), &pairing);
    let stale = session.read_at.is_none_or(|at| at.elapsed().unwrap_or(STALE) >= STALE);
    drop(all);
    if let (true, Some(slot)) = (switch.allow_remote_view && stale, slot) {
        refresh_behind(context, machine, slot);
    }
    shown
}

/// Switches the machine's remote view on or off. On, a running machine gets remoteman's host and the
/// helper opens; with `pairing`, pairing opens for `pairing_seconds` (two minutes unless asked).
/// Off, pairing closes, the guest's host turns every device away, and the helper stops listening.
pub fn share(
    context: &Context,
    machine: &Machine,
    on: bool,
    pairing: bool,
    seconds: Option<u64>,
) -> Result<ShareInfo> {
    let seconds = pairing_seconds(seconds).map_err(Failure)?;
    let was = allowed(context, machine);
    set_allowed(context, machine, on)?;
    let Some(slot) = running_slot(context, machine) else { return Ok(info(context, machine)) };
    if !on {
        step(context, machine, slot, StepEvent::Plain(Event::SwitchOff));
        run_ctl(context, machine, slot, &ctl::share(false));
        close_helper(context, slot);
        return Ok(info(context, machine));
    }
    if !was || pid(context, Some(slot)).is_none() || !helper_listening(context, slot) {
        let ask = |script: &str, seconds: u64| context.ask_guest(machine, script, seconds, Some(slot));
        if let Err(failure) = bring_up(context, machine, slot, &ask) {
            sessions().entry(machine.id.clone()).or_default().reason = Some(failure.0.clone());
            return Err(failure);
        }
    }
    if pairing {
        // The host has to have been read before pairing can open.
        step(context, machine, slot, StepEvent::Read);
        if let Some(why) = step(context, machine, slot, StepEvent::Plain(Event::Open { seconds })) {
            return Err(Failure(why));
        }
        refresh_behind(context, machine, slot);
    }
    Ok(info(context, machine))
}

fn helper_listening(context: &Context, slot: Slot) -> bool {
    helper_status(context, slot).is_some_and(|status| status.state == ShareState::Listening)
}

fn act(context: &Context, machine: &Machine, event: Event<'static>) -> Result<ShareInfo> {
    let slot = running_slot(context, machine).ok_or_else(|| Failure::new("The machine is not running."))?;
    if let Some(why) = step(context, machine, slot, StepEvent::Plain(event)) {
        return Err(Failure(why));
    }
    refresh_behind(context, machine, slot);
    Ok(info(context, machine))
}

/// Stops pairing. A device that waits is turned away.
pub fn pair_close(context: &Context, machine: &Machine) -> Result<ShareInfo> {
    act(context, machine, Event::Stop)
}

/// Lets in the device that waits. Its clipboard and files go off as soon as it is seen.
pub fn pair_accept(context: &Context, machine: &Machine) -> Result<ShareInfo> {
    act(context, machine, Event::Accept)
}

/// Turns away the device that waits.
pub fn pair_decline(context: &Context, machine: &Machine) -> Result<ShareInfo> {
    act(context, machine, Event::Decline)
}

fn device_command(
    context: &Context,
    machine: &Machine,
    words: std::result::Result<Words, String>,
) -> Result<ShareInfo> {
    let words = words.map_err(Failure)?;
    let slot = running_slot(context, machine).ok_or_else(|| Failure::new("The machine is not running."))?;
    match parse_ack(&run_ctl(context, machine, slot, &words)) {
        Ack::Done(_) | Ack::NotKept(_) => {}
        Ack::Refused(why) => return Err(Failure(why)),
        Ack::Stopping => return Err(Failure::new("remoteman's host is stopping.")),
    }
    read(context, machine, slot);
    Ok(info(context, machine))
}

/// Allows or refuses a device one thing: `input` (control; off is view-only), `clipboard`, `files`,
/// `sound`, `mic` or `camera`.
pub fn device_allow(
    context: &Context,
    machine: &Machine,
    fingerprint: &str,
    what: &str,
    on: bool,
) -> Result<ShareInfo> {
    device_command(context, machine, Policy::linux().device_allow(fingerprint, what, on))
}

/// Stops trusting a device: it is disconnected and has to pair again.
pub fn device_revoke(context: &Context, machine: &Machine, fingerprint: &str) -> Result<ShareInfo> {
    device_command(context, machine, Policy::linux().device_revoke(fingerprint))
}

/// Disconnects the device viewing now. It stays paired.
pub fn device_drop(context: &Context, machine: &Machine) -> Result<ShareInfo> {
    device_command(context, machine, Ok(ctl::device_drop()))
}

/// At the app's launch, for each running machine with its remote view on: whatever pairing an earlier
/// run left open is closed and what it held is given back.
pub fn launched(context: &Context) {
    let (machines, _) = load_all(context);
    for machine in machines.iter().filter(|machine| allowed(context, machine)) {
        if let Some(slot) = running_slot(context, machine) {
            sessions().entry(machine.id.clone()).or_default().pairing = Some(fresh_pairing(context, machine));
            step(context, machine, slot, StepEvent::Read);
        }
    }
}

/// When the app quits: pairing closes everywhere, and what it held is given back.
pub fn quitting(context: &Context) {
    let ids: Vec<String> = sessions().keys().cloned().collect();
    for id in ids {
        let Ok(machine) = load(context, &id) else { continue };
        if let Some(slot) = running_slot(context, &machine) {
            step(context, &machine, slot, StepEvent::Plain(Event::Shutdown));
        }
    }
}

/// The machine stopped: its helper ends, and what was read of it goes.
pub fn stopped(context: &Context, machine: &Machine, slot: Option<Slot>) {
    end(context, slot);
    sessions().remove(&machine.id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    fn machine(context: &Context, id: &str, kind: Kind) -> Machine {
        let machine = Machine { kind, ..Machine::new(id, id, "/d", "/v") };
        fs::create_dir_all(machine.folder(context)).unwrap();
        machine.save(context).unwrap();
        machine
    }

    fn status(json: &str) -> Status {
        match parse_reply(json) {
            Reply::Status(status) => *status,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_pin_is_a_release_whose_no_pair_still_lets_pairing_open() {
        let pin = pin();
        let numbers: Vec<u32> = pin.version.split('.').map(|part| part.parse().unwrap()).collect();
        assert!(numbers.as_slice() >= [0, 1, 6].as_slice(), "remoteman {} refuses `pair on`", pin.version);
        assert!(pin.file.contains(&pin.version) && pin.url.ends_with(&pin.file));
        assert!(pin.url.starts_with("https://"));
        assert!(
            pin.sha256.len() == 64
                && pin.sha256.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        );
    }

    #[test]
    fn the_switch_is_off_with_no_file_and_keeps_its_port_once_given() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let first = machine(&context, "one", Kind::Windows);
        let second = machine(&context, "two", Kind::Windows);
        assert!(!allowed(&context, &first));
        assert!(!first.folder(&context).join(SWITCH_FILE).exists());
        let on = set_allowed(&context, &first, true).unwrap();
        let port = on.remote_port.unwrap();
        assert!((7792..=7899).contains(&port));
        assert!(allowed(&context, &first));
        let other = set_allowed(&context, &second, true).unwrap().remote_port.unwrap();
        assert_ne!(other, port, "each machine gets a port of its own");
        set_allowed(&context, &first, false).unwrap();
        assert!(!allowed(&context, &first));
        assert_eq!(set_allowed(&context, &first, true).unwrap().remote_port, Some(port), "the port stays");
    }

    #[test]
    fn only_a_windows_machine_has_it() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let linux = machine(&context, "lin", Kind::Linux);
        assert!(set_allowed(&context, &linux, true).is_err());
        assert!(!allowed(&context, &linux));
    }

    #[test]
    fn the_encoder_is_the_cards_makers() {
        let windows = Machine { kind: Kind::Windows, ..Machine::new("w", "w", "/d", "/v") };
        // A machine with no card named is NVIDIA's (the default maker) or none: either way one of the two.
        assert!(["auto", "d3d12"].contains(&encoder(&windows)));
        assert_eq!(encoder_for(r"PCI\VEN_10DE"), "auto");
        assert_eq!(encoder_for(r"PCI\VEN_1002"), "d3d12");
        assert_eq!(encoder_for(r"PCI\VEN_8086"), "d3d12");
    }

    /// The app died with pairing open and the host-wide clipboard and files held off. At the next
    /// launch the pairing that was found open is closed, and both are given back.
    #[test]
    fn after_an_app_death_with_pairing_open_the_next_launch_closes_it_and_gives_back_what_was_held() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let windows = machine(&context, "win", Kind::Windows);
        let idle = status(r#"{"pairing":false,"clipboard":true,"files":true,"devices":[]}"#);

        // The first run: it reads, opens pairing (holding clipboard and files), writes down `held`,
        // and dies there.
        let first = fresh_pairing(&context, &windows).step(1, Event::Reading(&idle)).pairing;
        let opened = first.step(2, Event::Open { seconds: 120 });
        assert!(opened.commands.contains(&ctl::host_allow("clipboard", false).unwrap()));
        write_held(&context, &windows, opened.pairing.held());
        assert!(held_file(&context, &windows).is_file());

        // The next launch: what the host says now is pairing still open, clipboard and files off.
        let left_open = status(r#"{"pairing":true,"clipboard":false,"files":false,"devices":[]}"#);
        let next = fresh_pairing(&context, &windows).step(10_000, Event::Reading(&left_open));
        assert!(next.commands.contains(&ctl::pair_close()), "{:?}", next.commands);
        assert!(next.commands.contains(&ctl::host_allow("clipboard", true).unwrap()));
        assert!(next.commands.contains(&ctl::host_allow("files", true).unwrap()));
        assert_eq!(next.pairing.held(), None);
        write_held(&context, &windows, next.pairing.held());
        assert!(!held_file(&context, &windows).exists());
    }
}
