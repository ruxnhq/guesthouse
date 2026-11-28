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
//! Pairing itself is driven by the shared `session` driver in `guesthouse-remote-view-core`, which
//! this gives the agent, the log, that file and the clock.

use super::{Request, Settings, end, pid, start};
use crate::actions::guest_upload::put_big_file_with;
use crate::context::Context;
use crate::downloads::fetch_checked;
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::log::write_log;
use crate::machine::{Kind, Machine, load, load_all};
use crate::tasks::{State, status};
use guesthouse_basics::text::shown_short;
use guesthouse_daemon::slot::Slot;
use guesthouse_remote_view_core::ctl::{self, Ack, Words, parse_ack};
use guesthouse_remote_view_core::encoder_for_maker;
use guesthouse_remote_view_core::install::{self, Check, INSTALLED, Pin};
use guesthouse_remote_view_core::pairing::{Event, HostWide, Millis};
use guesthouse_remote_view_core::policy::Policy;
use guesthouse_remote_view_core::session::{Driver, Guest, Sessions, StepEvent, Unanswered};
use guesthouse_remote_view_core::share::{ShareInfo, ShareState, pairing_seconds};
use guesthouse_remote_view_core::status::{Reply, parse_reply};
use guesthouse_remote_view_core::switch::{SWITCH_FILE, Switch, others_ports};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::Child;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Beside the switch: what pairing has held off and owes back.
pub const HELD_FILE: &str = "remote-view-held.json";
/// One `ctl` command in the guest. The agent answers in about 40 ms.
/// How much of what a guest said goes into the log.
const GUEST_WORDS_LOGGED: usize = 300;
const CTL_SECONDS: u64 = 20;
/// The last line of every `ctl` script [`run_ctl`] sends. The agent closes the connection once a
/// script ends, and says nothing at all for a token it doesn't take, for a guest whose agent isn't
/// up behind QEMU's forward, or for a script that printed nothing, and an answer cut off by the
/// time reads as a whole one. This line tells an answer, empty or not, from none.
const CTL_END: &str = "guesthouse-ctl-end";
const CHECK_SECONDS: u64 = 30;
/// The installer took 18 s in a guest with Secure Boot on.
const INSTALL_SECONDS: u64 = 180;
/// How often `set updates off` is sent before it is given up on until the next start, and how long
/// apart: right after the installer the host may not answer yet, and it first looks for an update a
/// minute after its service starts.
const UPDATES_OFF_TRIES: u32 = 6;
const UPDATES_OFF_PAUSE: Duration = Duration::from_secs(3);
/// Where the downloaded installer is kept, in the fetched parts' folder.
const PARTS_FOLDER: &str = "remoteman";
/// Where a build that ships the installer has it, in the program's folder.
const SHIPPED: &str = "remoteman/remoteman-host-setup.exe";
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
    let taken = others_ports(&machines, |other| other.id == machine.id, |other| port(context, other));
    let switch =
        read_switch(context, machine).turned(on, &taken, guesthouse_relay::udp_port_free).map_err(Failure)?;
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
    encoder_for_maker(machine.windows_maker().map(GpuMaker::vendor))
}

/// remoteman's host installer at this host's pin, the release every host pins in
/// `shared/remote-view/remoteman.json`. 0.1.6 is the first whose `--no-pair` still lets `pair on`
/// open pairing, which every pairing here needs. 0.1.9 is the first that takes `set updates off`.
pub fn pin() -> Pin {
    install::pinned().host_pin()
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
/// it installed. The installer runs only when one of those changed, as each run blips the picture.
///
/// After an install, and whenever the host is current but still looks for updates by itself, the
/// host is told `set updates off`: this host pins remoteman and updates it itself. If that doesn't
/// take, it is logged and asked again at the next start; the remote view goes on without it.
pub fn ensure_installed(
    context: &Context,
    machine: &Machine,
    ask: &dyn Fn(&str, u64) -> String,
) -> Result<Option<String>> {
    let pin = pin();
    let encoder = encoder(machine);
    match install::read_check(&ask(&install::check_script(&pin.version, encoder), CHECK_SECONDS)) {
        Check::Current => return Ok(None),
        Check::UpdatesOn => {
            updates_off(context, machine, ask, UPDATES_OFF_PAUSE);
            return Ok(None);
        }
        Check::Needed => {}
        Check::OldTakeOver => return Err(Failure::new(TOO_OLD)),
        Check::Silent => return Err(Failure::new("Windows did not answer. Try again once it is up.")),
        Check::Other(other) => return Err(Failure(format!("Windows said: {other}"))),
    }
    let local = installer(context, &pin)?;
    if !put_big_file_with(ask, &local, install::GUEST_INSTALLER)? {
        return Err(Failure::new("remoteman's host did not reach Windows. Try again."));
    }
    let said = ask(&install::install_script(&pin.version, encoder), INSTALL_SECONDS);
    let _ = write_log(
        context,
        &format!("remote view: {}: {}", machine.id, shown_short(said.trim(), GUEST_WORDS_LOGGED)),
    );
    if !said.contains(INSTALLED) {
        return Err(Failure(format!("remoteman's host could not be installed: {}", said.trim())));
    }
    updates_off(context, machine, ask, UPDATES_OFF_PAUSE);
    Ok(install::fingerprint_after(&said))
}

/// Tells the guest's host never to look for updates by itself, a few times until it takes. Gives
/// whether it took; when it didn't, the host's last words are logged.
fn updates_off(
    context: &Context,
    machine: &Machine,
    ask: &dyn Fn(&str, u64) -> String,
    pause: Duration,
) -> bool {
    let script = install::ctl_script(&ctl::updates_off());
    let mut why = String::new();
    for attempt in 0..UPDATES_OFF_TRIES {
        if attempt > 0 {
            thread::sleep(pause);
        }
        match ctl::updates_off_took(&ask(&script, CTL_SECONDS)) {
            Ok(()) => return true,
            Err(said) => why = said,
        }
    }
    let _ = write_log(
        context,
        &format!(
            "remote view: {}: remoteman's own update check could not be switched off: {}",
            machine.id,
            shown_short(&why, GUEST_WORDS_LOGGED)
        ),
    );
    false
}

/// Runs one `ctl` command in the guest, and gives what the host printed, or that no whole answer came.
fn run_ctl(
    context: &Context,
    machine: &Machine,
    slot: Slot,
    words: &Words,
) -> std::result::Result<String, Unanswered> {
    ctl_answer(&context.ask_guest(machine, &ctl_script(words), CTL_SECONDS, Some(slot)))
}

/// The `ctl` command as the agent runs it, with [`CTL_END`] printed after it. A command the guest
/// can't run is an answer too, in the words the agent would have given for it.
fn ctl_script(words: &Words) -> String {
    let command = install::ctl_script(words);
    format!("try {{ {} }} catch {{ \"ERROR: $_\" }}\n'{CTL_END}'\n", command.trim_end())
}

/// What the guest printed before [`CTL_END`], which may be nothing. With no end line the guest gave
/// no answer, or not all of one, and the command may or may not have run.
fn ctl_answer(said: &str) -> std::result::Result<String, Unanswered> {
    let all = said.trim_end();
    match all.strip_suffix(CTL_END) {
        Some(before) if before.is_empty() || before.ends_with('\n') => Ok(before.to_string()),
        _ if all.is_empty() => Err(Unanswered("no answer came from the guest's agent".into())),
        _ => Err(Unanswered("only part of the answer came".into())),
    }
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
        settings.hold_clocks = clock_card(context, machine, true);
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

/// The NVIDIA card whose memory clock is held while a viewer streams the machine's screen, if the
/// machine's setting has it held. `say` writes one line in the log when it isn't, and why.
fn clock_card(context: &Context, machine: &Machine, say: bool) -> Option<String> {
    let reason = if machine.windows_maker() != Some(GpuMaker::Nvidia) {
        "the machine's card isn't NVIDIA's, so its clocks are left alone."
    } else if !machine.stream_clocks() {
        "turned off for this machine."
    } else {
        let card = crate::screen_gpu::machine_card(context, machine);
        if !card.is_empty() {
            return Some(card);
        }
        "the machine's NVIDIA card can't be found, so its clocks are left alone."
    };
    if say {
        let _ = write_log(context, &format!("remote view: stream clocks: {reason}"));
    }
    None
}

/// After the machine's remote view has ended. Its clock helper ended with it and put the card's
/// clock back; were both killed, this puts it back. A clock somebody locked by hand, or one another
/// machine on the card still holds, stays.
pub fn sweep_clocks(context: &Context, machine: &Machine) {
    if machine.windows_maker() != Some(GpuMaker::Nvidia) {
        return;
    }
    let card = crate::screen_gpu::machine_card(context, machine);
    if !card.is_empty() {
        // Into the log, never onto the terminal of a `guesthouse-vm stop`.
        let said = crate::log::open_for_appending(context).map_or(super::Said::Nowhere, super::Said::Into);
        super::sweep(super::clock_helper().as_deref(), &card, said);
    }
}

/// The machine's stream clocks setting changed: a running machine's helper holds or lets go at once.
pub fn stream_clocks_changed(context: &Context, machine: &Machine) {
    let Some(slot) = running_slot(context, machine) else { return };
    if pid(context, Some(slot)).is_none() {
        return;
    }
    let card = clock_card(context, machine, false);
    let _ = super::ask(&context.run_folder(Some(slot)), &Request::StreamClocks { card });
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
        let _ = write_log(
            context,
            &format!("remote view: sharing on: {}", shown_short(said.trim(), GUEST_WORDS_LOGGED)),
        );
    }
    open_helper(context, machine, slot, port, fingerprint)
}

// ---- Pairing and the state shown ----

/// The shared driver's way into a running machine: `ctl` through the agent, this host's log, and
/// `held` in its file beside the switch.
struct AgentGuest<'a> {
    context: &'a Context,
    machine: &'a Machine,
    slot: Slot,
}

impl Guest for AgentGuest<'_> {
    /// A script that ran to its end is an answer, whatever it printed. Anything else is
    /// [`Unanswered`], so the driver reads the status again before it sends anything more.
    fn ctl(&self, words: &Words) -> std::result::Result<String, Unanswered> {
        run_ctl(self.context, self.machine, self.slot, words)
    }

    fn log(&self, line: &str) {
        let _ = write_log(self.context, &format!("remote view: {}", shown_short(line, GUEST_WORDS_LOGGED)));
    }

    fn held(&self) -> Option<HostWide> {
        read_held(self.context, self.machine)
    }

    fn keep_held(&self, held: Option<HostWide>) {
        write_held(self.context, self.machine, held);
    }
}

fn sessions() -> &'static Sessions {
    static SESSIONS: OnceLock<Sessions> = OnceLock::new();
    SESSIONS.get_or_init(Sessions::default)
}

fn now_ms() -> Millis {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as Millis)
}

/// Runs `work` with the shared driver for a running machine.
fn drive<R>(context: &Context, machine: &Machine, slot: Slot, work: impl FnOnce(&Driver) -> R) -> R {
    let (guest, policy) = (AgentGuest { context, machine, slot }, Policy::linux());
    work(&Driver { sessions: sessions(), machine: &machine.id, policy: &policy, guest: &guest, now: &now_ms })
}

fn step(context: &Context, machine: &Machine, slot: Slot, event: StepEvent) -> Option<String> {
    drive(context, machine, slot, |driver| driver.step(event))
}

/// Reads the host and lets the clock go on, then keeps doing so once a second for as long as pairing
/// is open or a device waits. Runs behind [`info`], one at a time per machine.
fn refresh_behind(context: &Context, machine: &Machine, slot: Slot) {
    let already = sessions().with(&machine.id, |session| {
        let busy = session.busy;
        session.busy = true;
        busy
    });
    if already {
        return;
    }
    let (context, machine) = (context.clone(), machine.clone());
    thread::spawn(move || {
        loop {
            let poll = drive(&context, &machine, slot, |driver| {
                driver.step(StepEvent::Read);
                driver.step(StepEvent::Plain(Event::Tick));
                driver.poll_ms()
            });
            match poll {
                Some(ms) if running_slot(&context, &machine) == Some(slot) => {
                    thread::sleep(Duration::from_millis(ms))
                }
                _ => break,
            }
        }
        sessions().with(&machine.id, |session| session.busy = false);
    });
}

/// Where the machine's remote view stands, from what was last read. Never waits for the guest: a
/// reading older than two seconds is taken again behind it, and the clock runs there.
pub fn info(context: &Context, machine: &Machine) -> ShareInfo {
    reap();
    let switch = read_switch(context, machine);
    let slot = running_slot(context, machine);
    let helper = slot.and_then(|slot| helper_status(context, slot));
    let state = match (switch.allow_remote_view, slot, &helper) {
        (false, ..) => ShareState::Off,
        (true, None, _) => ShareState::Starting,
        (true, Some(_), Some(helper)) if helper.state == ShareState::Listening => ShareState::Listening,
        (true, Some(_), Some(helper)) if helper.state == ShareState::Failed => ShareState::Failed,
        (true, Some(_), _) => ShareState::Starting,
    };
    let reason = helper.and_then(|helper| helper.reason);
    let (shown, stale) = sessions().with(&machine.id, |session| {
        (
            session.shown(&machine.id, &Policy::linux(), state, switch.remote_port, reason),
            session.stale(now_ms()),
        )
    });
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
        let _ = run_ctl(context, machine, slot, &ctl::share(false));
        close_helper(context, slot);
        return Ok(info(context, machine));
    }
    if !was || pid(context, Some(slot)).is_none() || !helper_listening(context, slot) {
        let ask = |script: &str, seconds: u64| context.ask_guest(machine, script, seconds, Some(slot));
        if let Err(failure) = bring_up(context, machine, slot, &ask) {
            sessions().with(&machine.id, |session| session.reason = Some(failure.0.clone()));
            return Err(failure);
        }
    }
    if pairing {
        if let Some(why) = drive(context, machine, slot, |driver| driver.open_pairing(seconds)) {
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
    drive(context, machine, slot, |driver| driver.device(&words)).map_err(Failure)?;
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
            drive(context, machine, slot, |driver| driver.started());
        }
    }
}

/// When the app quits: pairing closes everywhere, and what it held is given back.
pub fn quitting(context: &Context) {
    for id in sessions().machines() {
        let Ok(machine) = load(context, &id) else { continue };
        if let Some(slot) = running_slot(context, &machine) {
            step(context, &machine, slot, StepEvent::Plain(Event::Shutdown));
        }
    }
}

/// The machine stopped: its helper ends, and what was read of it goes.
pub fn stopped(context: &Context, machine: &Machine, slot: Option<Slot>) {
    end(context, slot);
    sweep_clocks(context, machine);
    sessions().remove(&machine.id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::stand_in::ScriptedAgent;
    use crate::testing::{Scratch, scratch_context};
    use guesthouse_remote_view_core::encoder_for;
    use guesthouse_remote_view_core::pairing::Pairing;
    use guesthouse_remote_view_core::session::fresh_pairing;
    use guesthouse_remote_view_core::status::Status;
    use std::cell::RefCell;
    use std::sync::Arc;

    /// A pairing for a run of the app, as [`AgentGuest`] gives the driver one: from the held file.
    fn fresh(context: &Context, machine: &Machine) -> Pairing {
        fresh_pairing(Policy::linux(), read_held(context, machine))
    }

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
    fn what_the_guest_says_goes_into_the_log_cut_and_on_one_line() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = machine(&context, "words", Kind::Windows);
        let guest = AgentGuest { context: &context, machine: &machine, slot: crate::slots::number(0) };
        let said = format!("refused\n[2025] fake line\x1b]0;x\x07{}", "z".repeat(4 << 20));
        guest.log(&said);
        let log = crate::log::read_log(&context);
        assert!(log.len() < 2 * GUEST_WORDS_LOGGED, "{}", log.len());
        assert_eq!(log.trim_end().lines().count(), 1, "{log}");
        assert!(!log.contains('\x1b'));
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
    fn the_pin_is_at_least_0_1_9_which_takes_set_updates_off() {
        let pin = pin();
        let numbers: Vec<u32> = pin.version.split('.').map(|part| part.parse().unwrap()).collect();
        assert!(
            numbers.as_slice() >= [0, 1, 9].as_slice(),
            "remoteman {} doesn't take `set updates off`",
            pin.version
        );
        assert_eq!(pin.version, "0.1.10");
        assert_eq!(pin.url, "https://files.ruxn.com/p/remoteman/remoteman-host-setup-0.1.10.exe");
        assert_eq!(pin.sha256, "9e163215ed120d237bbbc30940360f273a61684a01ad1e23ca33a9872b3477f4");
    }

    /// Every field of the installer this host fetches and checks, whole, so where the pin is kept can
    /// change without the installer's name, address or SHA-256 changing by a byte.
    #[test]
    fn the_pin_is_the_installer_it_always_was() {
        let pin = Pin {
            version: "0.1.10".into(),
            url: "https://files.ruxn.com/p/remoteman/remoteman-host-setup-0.1.10.exe".into(),
            file: "remoteman-host-setup-0.1.10.exe".into(),
            sha256: "9e163215ed120d237bbbc30940360f273a61684a01ad1e23ca33a9872b3477f4".into(),
        };
        assert_eq!(super::pin(), pin);
    }

    /// What a guest is asked, and answers from a table: the first answer whose key the script holds.
    fn guest<'a>(
        answers: &'a [(&'a str, &'a str)],
        asked: &'a RefCell<Vec<String>>,
    ) -> impl Fn(&str, u64) -> String + 'a {
        move |script: &str, _| {
            asked.borrow_mut().push(script.to_string());
            answers
                .iter()
                .find(|(key, _)| script.contains(key))
                .map(|(_, said)| (*said).to_string())
                .unwrap_or_default()
        }
    }

    /// A machine installed at 0.1.8 has a different version noted, so it is installed again and told
    /// `set updates off` after. One installed at the pin whose host still looks for updates gets only
    /// `set updates off`, and no installer runs, so the picture doesn't blip.
    #[test]
    fn the_right_host_with_updates_on_gets_only_the_ctl_call() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let windows = machine(&context, "win", Kind::Windows);
        let asked = RefCell::new(Vec::new());
        let answers = [("updates.json", install::UPDATES_ON), ("'set' 'updates' 'off'", "ok\r\n")];
        assert_eq!(ensure_installed(&context, &windows, &guest(&answers, &asked)).unwrap(), None);
        let asked = asked.into_inner();
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(asked[1].ends_with("ctl --port 7791 'set' 'updates' 'off'\n"), "{}", asked[1]);
        assert!(asked.iter().all(|script| !script.contains("remoteman-host-setup.exe")), "no installer runs");
    }

    #[test]
    fn a_current_host_with_updates_off_is_asked_nothing_more() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let windows = machine(&context, "win", Kind::Windows);
        let asked = RefCell::new(Vec::new());
        let answers = [("updates.json", install::CURRENT)];
        assert_eq!(ensure_installed(&context, &windows, &guest(&answers, &asked)).unwrap(), None);
        assert_eq!(asked.into_inner().len(), 1);
    }

    /// An older host, or one that won't, is logged and asked again at the next start; the remote
    /// view goes on.
    #[test]
    fn updates_that_wont_go_off_are_logged_and_nothing_fails() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let windows = machine(&context, "win", Kind::Windows);
        let asked = RefCell::new(Vec::new());
        let answers =
            [("'set' 'updates' 'off'", "this host was started with --updates check, which decides this")];
        assert!(!updates_off(&context, &windows, &guest(&answers, &asked), Duration::ZERO));
        assert_eq!(asked.borrow().len(), UPDATES_OFF_TRIES as usize);
        let log = fs::read_to_string(context.paths.log_file()).unwrap_or_default();
        assert!(
            log.contains("could not be switched off: this host was started with --updates check"),
            "{log}"
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

    /// The shared driver keeps `held` in the file beside the switch, and its lines go in this host's
    /// log with the same words as before it was shared.
    #[test]
    fn the_driver_keeps_held_in_the_file_and_logs_as_this_host_does() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let windows = machine(&context, "win", Kind::Windows);
        let guest = AgentGuest { context: &context, machine: &windows, slot: crate::slots::number(0) };
        let both = HostWide { clipboard: true, files: true };
        guest.keep_held(Some(both));
        assert!(held_file(&context, &windows).is_file());
        assert_eq!(guest.held(), Some(both));
        guest.keep_held(None);
        assert!(!held_file(&context, &windows).exists());
        assert_eq!(guest.held(), None);
        guest.log("ctl pair on: no");
        let log = fs::read_to_string(context.paths.log_file()).unwrap_or_default();
        assert!(log.contains("remote view: ctl pair on: no"), "{log}");
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
        let first = fresh(&context, &windows).step(1, Event::Reading(&idle)).pairing;
        let opened = first.step(2, Event::Open { seconds: 120 });
        assert!(opened.commands.contains(&ctl::host_allow("clipboard", false).unwrap()));
        write_held(&context, &windows, opened.pairing.held());
        assert!(held_file(&context, &windows).is_file());

        // The next launch: what the host says now is pairing still open, clipboard and files off.
        let left_open = status(r#"{"pairing":true,"clipboard":false,"files":false,"devices":[]}"#);
        let next = fresh(&context, &windows).step(10_000, Event::Reading(&left_open));
        assert!(next.commands.contains(&ctl::pair_close()), "{:?}", next.commands);
        assert!(next.commands.contains(&ctl::host_allow("clipboard", true).unwrap()));
        assert!(next.commands.contains(&ctl::host_allow("files", true).unwrap()));
        assert_eq!(next.pairing.held(), None);
        write_held(&context, &windows, next.pairing.held());
        assert!(!held_file(&context, &windows).exists());
    }

    #[test]
    fn a_ctl_script_runs_the_command_and_then_prints_its_end_line() {
        let script = ctl_script(&ctl::status());
        assert!(script.starts_with("try { & "), "{script}");
        assert!(script.contains("ctl --port 7791 'status' } catch { \"ERROR: $_\" }\n"), "{script}");
        assert!(script.ends_with("\n'guesthouse-ctl-end'\n"), "{script}");
    }

    /// The agent's answer as it comes: Windows ends lines with CRLF. Whatever the script printed
    /// before its end line is the answer, even nothing. No end line is no answer.
    #[test]
    fn only_an_answer_with_its_end_line_is_one() {
        let answered = [
            ("ok\r\nguesthouse-ctl-end\r\n", "ok\r\n"),
            ("{\"pairing\":false}\r\nguesthouse-ctl-end\r\n", "{\"pairing\":false}\r\n"),
            ("guesthouse-ctl-end\r\n", ""),
            ("\r\nguesthouse-ctl-end\r\n", "\r\n"),
            (
                "ERROR: The term 'remoteman-host.exe' is not recognized\r\nguesthouse-ctl-end\r\n",
                "ERROR: The term 'remoteman-host.exe' is not recognized\r\n",
            ),
        ];
        for (said, answer) in answered {
            assert_eq!(ctl_answer(said), Ok(answer.to_string()), "{said:?}");
        }
        let none = Unanswered("no answer came from the guest's agent".into());
        let part = Unanswered("only part of the answer came".into());
        let unanswered = [
            ("", &none),
            ("\r\n", &none),
            ("{\"pairing\":fal", &part),
            ("ok\r\n", &part),
            ("not the guesthouse-ctl-end\r\n", &part),
        ];
        for (said, why) in unanswered {
            assert_eq!(ctl_answer(said).as_ref(), Err(why), "{said:?}");
        }
    }

    fn agent_guest(scratch: &Scratch, answers: &[&str]) -> (Context, Machine, Arc<ScriptedAgent>) {
        let mut context = scratch_context(scratch);
        let agent = Arc::new(ScriptedAgent::answering(answers));
        context.agent = agent.clone();
        let windows = machine(&context, "win", Kind::Windows);
        crate::agent::make_token(&context, &windows).unwrap();
        (context, windows, agent)
    }

    const IDLE: &str =
        "{\"pairing\":false,\"clipboard\":true,\"files\":true,\"devices\":[]}\r\nguesthouse-ctl-end\r\n";
    const DONE: &str = "ok\r\nguesthouse-ctl-end\r\n";

    /// A guest that says nothing to `allow clipboard off` is a guest that did not answer: the step
    /// stops before the rest, and the next one reads first. Before, it read as the host refusing with
    /// no words, and pairing went on to the next command.
    #[test]
    fn a_silent_guest_stops_the_step_and_the_next_one_reads_first() {
        let scratch = Scratch::new();
        let (context, windows, agent) = agent_guest(&scratch, &[IDLE, ""]);
        let (sessions, policy) = (Sessions::default(), Policy::linux());
        let guest = AgentGuest { context: &context, machine: &windows, slot: crate::slots::number(0) };
        let clock = || 1_000_000;
        let driver =
            Driver { sessions: &sessions, machine: &windows.id, policy: &policy, guest: &guest, now: &clock };
        let why = driver.open_pairing(120).expect("the step says why it stopped");
        assert_eq!(
            why,
            "The guest did not answer allow clipboard off: no answer came from the guest's agent. It is read \
             again before anything more is sent."
        );
        assert_eq!(agent.requests.lock().unwrap().len(), 2, "nothing more was sent");
        assert!(sessions.with(&windows.id, |session| session.unsure));
        let log = fs::read_to_string(context.paths.log_file()).unwrap_or_default();
        assert!(log.contains(&format!("remote view: {why}")), "{log}");
    }

    /// A guest that answers, even with nothing, goes on as before.
    #[test]
    fn a_guest_that_answers_opens_pairing_as_before() {
        let scratch = Scratch::new();
        let open =
            "{\"pairing\":true,\"clipboard\":false,\"files\":false,\"devices\":[]}\r\nguesthouse-ctl-end\r\n";
        let quiet = "guesthouse-ctl-end\r\n";
        let (context, windows, agent) = agent_guest(&scratch, &[IDLE, DONE, quiet, DONE, open]);
        let (sessions, policy) = (Sessions::default(), Policy::linux());
        let guest = AgentGuest { context: &context, machine: &windows, slot: crate::slots::number(0) };
        let clock = || 1_000_000;
        let driver =
            Driver { sessions: &sessions, machine: &windows.id, policy: &policy, guest: &guest, now: &clock };
        assert_eq!(driver.open_pairing(120), None);
        let scripts: Vec<String> =
            agent.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect();
        let words =
            ["'status'", "'allow' 'clipboard' 'off'", "'allow' 'files' 'off'", "'pair' 'on'", "'status'"];
        assert_eq!(scripts.len(), words.len(), "{scripts:?}");
        for (script, words) in scripts.iter().zip(words) {
            assert!(script.contains(&format!("ctl --port 7791 {words} }}")), "{words} in {script}");
        }
        assert!(!sessions.with(&windows.id, |session| session.unsure));
        assert!(sessions.with(&windows.id, |session| session.pairing.as_ref().unwrap().view().open));
        let log = fs::read_to_string(context.paths.log_file()).unwrap_or_default();
        assert!(
            log.contains("remote view: ctl allow files off:"),
            "an empty answer is a refusal with no words, as before: {log}"
        );
    }
}
