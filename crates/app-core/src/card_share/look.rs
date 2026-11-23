//! One look at how each card with a running machine is shared. It runs on the window's refresh
//! thread each second. Engine shares need the look before, which the caller keeps and hands back.
//!
//! Each machine's figures come from its own processes. Its daemon's books give the video memory it
//! holds against its cap, on any maker's card, and the card clients its daemon and QEMU hold give the
//! time each engine spent on it, where the driver counts that per client, as amdgpu and xe do.
//! NVIDIA's driver keeps its own per-process figures, which aren't read here yet.

use super::engine_time::{Look, shares};
use super::fdinfo::{Client, clients_of};
use super::whole_card::{self, WholeCard};
use crate::context::Context;
use crate::daemon::is_own_daemon;
use crate::gpu_maker::GpuMaker;
use crate::host_sensors::sensors_file;
use crate::machine::Machine;
use crate::render_cards::render_card;
use crate::settings::read_setup;
use crate::setup::cards::normal_address;
use crate::slots;
use guesthouse_sensors::guest_use::{GuestUse, SharedGuestUse};
use guesthouse_sensors::{SharedSnapshot, SnapshotReader};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// The card a machine's GPU is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnCard {
    pub maker: GpuMaker,
    /// Its PCI address, where it's known. An NVIDIA card is setup's when the machine names none.
    pub pci: Option<String>,
}

/// A machine that is running, and the processes it runs in.
#[derive(Debug, Clone)]
pub struct Running {
    pub machine: Machine,
    pub slot: u8,
    pub qemu: Option<u32>,
}

/// What each machine's clients were at the last look, by machine.
pub type Looks = BTreeMap<String, Look>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineShare {
    pub name: String,
    /// Each engine's share, by the driver's name for it. `None` is no reading.
    pub engines: Option<BTreeMap<String, u32>>,
    /// The video memory by its daemon's books. `None` is no reading.
    pub video: Option<GuestUse>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardShare {
    pub card: OnCard,
    pub name: String,
    pub whole: WholeCard,
    pub machines: Vec<MachineShare>,
}

fn setup_card(context: &Context) -> Option<String> {
    let setup = read_setup(context);
    let pci = setup.get("card").and_then(|card| card.get("pci")).and_then(Value::as_str)?;
    Some(pci).filter(|pci| !pci.trim().is_empty()).map(normal_address)
}

/// The card a machine's GPU is on, if it has one.
pub fn card_of(context: &Context, machine: &Machine) -> Option<OnCard> {
    if let Some(maker) = machine.windows_maker().filter(|_| machine.has_gpu()) {
        let pci = machine.windows_card_address().map(normal_address);
        let pci = pci.or_else(|| (maker == GpuMaker::Nvidia).then(|| setup_card(context)).flatten());
        return Some(OnCard { maker, pci });
    }
    if !machine.wants_forwarded_gpu() && !machine.own_linux_gpu() {
        return None;
    }
    Some(match machine.amd_card() {
        Some(pci) => OnCard { maker: GpuMaker::Amd, pci: Some(normal_address(pci)) },
        None => OnCard { maker: GpuMaker::Nvidia, pci: setup_card(context) },
    })
}

/// The record the slot's daemon keeps, if the daemon running in that slot is ours and wrote it.
fn guest_use(context: &Context, slot: u8) -> Option<GuestUse> {
    let slot = slots::number(slot);
    let file = SharedGuestUse::open_to_read(&context.paths.run_folder(slot)).ok()?;
    file.read().filter(|used| is_own_daemon(context, &used.daemon.to_string(), slot))
}

/// The clients of `card` its processes hold. A client that names no card is the card's, as a
/// process here only opens the one it was given.
fn clients(context: &Context, pids: &[u32], card: &OnCard) -> Vec<Client> {
    let on_card = |client: &Client| match (&client.card, &card.pci) {
        (Some(theirs), Some(ours)) => theirs == ours,
        _ => true,
    };
    pids.iter().flat_map(|pid| clients_of(&context.processes, *pid)).filter(on_card).collect()
}

fn whole(context: &Context, card: &OnCard, machine: &Machine) -> WholeCard {
    match (card.maker, &card.pci) {
        (GpuMaker::Amd, Some(pci)) => whole_card::amd(&context.pci_devices.join(pci)),
        (GpuMaker::Nvidia, _) => {
            let snapshot = SharedSnapshot::open_to_read(&sensors_file(context, machine)).ok();
            whole_card::nvidia(snapshot.and_then(|snapshot| snapshot.latest()).as_ref())
        }
        // xe has no busy figure for the whole card in sysfs, and its memory query isn't read here yet.
        _ => WholeCard::default(),
    }
}

fn card_name(context: &Context, card: &OnCard) -> String {
    let named = card.pci.as_deref().and_then(|pci| render_card(context, card.maker, pci));
    named.map_or_else(|| format!("{} card", card.maker.name()), |found| found.name)
}

/// Looks at each running machine with a GPU, grouped by card. `at` is when, on a clock that only goes
/// forward, and `before` the looks the last call gave back.
pub fn look(context: &Context, running: &[Running], before: &Looks, at: Duration) -> (Vec<CardShare>, Looks) {
    let mut cards: Vec<CardShare> = Vec::new();
    let mut looks = Looks::new();
    for one in running {
        let Some(card) = card_of(context, &one.machine) else { continue };
        let video = guest_use(context, one.slot);
        let pids: Vec<u32> = video.map(|used| used.daemon).into_iter().chain(one.qemu).collect();
        let now = Look { at, clients: clients(context, &pids, &card) };
        let engines = before.get(&one.machine.id).and_then(|earlier| shares(earlier, &now));
        looks.insert(one.machine.id.clone(), now);
        let share = MachineShare { name: one.machine.name.clone(), engines, video };
        match cards.iter_mut().find(|found| found.card == card) {
            Some(found) => found.machines.push(share),
            None => cards.push(CardShare {
                name: card_name(context, &card),
                whole: whole(context, &card, &one.machine),
                card,
                machines: vec![share],
            }),
        }
    }
    (cards, looks)
}

#[cfg(test)]
mod tests {
    use super::super::fdinfo::tests::AMDGPU;
    use super::*;
    use crate::machine::Kind;
    use crate::processes::stand_in::fake_process;
    use crate::testing::{Scratch, scratch_context};
    use std::fs;

    const CARD: &str = "0000:14:00.0";
    const DAEMON: u32 = 700;
    const QEMU: u32 = 600;

    fn amd_machine() -> Machine {
        let mut machine = Machine::new("w", "Work", "/d", "/v");
        machine.kind = Kind::Windows;
        machine.use_windows_card(GpuMaker::Amd, CARD);
        machine
    }

    /// A daemon of ours in slot 0 with one client, and its record.
    fn running_daemon(context: &Context, client: &str, video: u64) {
        let program = context.paths.binary("guesthouse-daemon").display().to_string();
        let base = context.paths.runtime.display().to_string();
        fake_process(
            context,
            DAEMON,
            &[&program, &base],
            &["GUESTHOUSE_SLOT=0"],
            "/memfd:gpupv-ram (deleted)",
        );
        let descriptors = context.processes.join(DAEMON.to_string()).join("fdinfo");
        fs::create_dir_all(&descriptors).unwrap();
        fs::write(descriptors.join("12"), client).unwrap();
        let run = context.paths.run_folder(slots::first());
        fs::create_dir_all(&run).unwrap();
        let used =
            GuestUse { daemon: DAEMON, video_bytes: video, spilled_bytes: 0, cap_bytes: Some(4 << 30) };
        SharedGuestUse::open(&run).unwrap().publish(&used);
    }

    fn running(machine: Machine) -> Vec<Running> {
        vec![Running { machine, slot: 0, qemu: Some(QEMU) }]
    }

    #[test]
    fn a_machine_on_an_amd_card_has_its_engines_and_its_books() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        scratch.write(&format!("pci/{CARD}/gpu_busy_percent"), "99\n");
        running_daemon(&context, AMDGPU, 1 << 30);
        let at = Duration::from_secs(10);
        let (first, looks) = look(&context, &running(amd_machine()), &Looks::new(), at);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].whole.load_percent, Some(99));
        assert_eq!(first[0].machines[0].engines, None, "one look is no share yet");
        assert_eq!(first[0].machines[0].video.unwrap().video_bytes, 1 << 30);
        let busier = AMDGPU.replace("4671345800 ns", "5171345800 ns");
        running_daemon(&context, &busier, 1 << 30);
        let (second, _) = look(&context, &running(amd_machine()), &looks, at + Duration::from_secs(1));
        let engines = second[0].machines[0].engines.clone().unwrap();
        assert_eq!((engines["gfx"], engines["dma"]), (50, 0));
    }

    #[test]
    fn a_record_left_by_a_daemon_that_has_gone_is_no_reading() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        running_daemon(&context, AMDGPU, 1 << 30);
        fs::remove_dir_all(context.processes.join(DAEMON.to_string())).unwrap();
        let (cards, _) = look(&context, &running(amd_machine()), &Looks::new(), Duration::ZERO);
        assert_eq!(cards[0].machines[0].video, None);
    }

    #[test]
    fn a_client_of_another_card_is_not_this_ones() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        running_daemon(&context, &AMDGPU.replace(CARD, "0000:03:00.0"), 0);
        let card = card_of(&context, &amd_machine()).unwrap();
        assert!(clients(&context, &[DAEMON], &card).is_empty());
    }

    #[test]
    fn a_machine_without_a_gpu_is_not_on_a_card() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut plain = Machine::new("p", "Plain", "/d", "/v");
        plain.kind = Kind::Plain;
        assert_eq!(card_of(&context, &plain), None);
        let (cards, looks) = look(&context, &running(plain), &Looks::new(), Duration::ZERO);
        assert!(cards.is_empty() && looks.is_empty());
    }

    #[test]
    fn an_nvidia_machine_with_no_address_is_on_setups_card() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut machine = Machine::new("n", "Games", "/d", "/v");
        machine.kind = Kind::Windows;
        let card = serde_json::json!({"card": {"pci": "0000:01:00.0"}});
        crate::settings::write_setup(&context, card.as_object().cloned().unwrap()).unwrap();
        assert_eq!(
            card_of(&context, &machine),
            Some(OnCard { maker: GpuMaker::Nvidia, pci: Some("0000:01:00.0".to_owned()) })
        );
    }
}
