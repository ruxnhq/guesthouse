//! Starting an Android guest. It runs on our QEMU with the virtio GPU on the host's card, with no daemon
//! and no forwarding: the guest's Mesa draws through virgl and Venus, which run on the card's own
//! drivers here. Its QEMU opens the card itself, so it starts jailed when it can, as a GPU machine does.

use super::{Task, plan_for};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::jailing::choose;
use crate::linux::card::forwarded_card;
use crate::machine::Machine;
use crate::render_cards::render_cards;
use crate::start_checks::start_problems;
use crate::vm_args::{AndroidBoot, AndroidCard, Boot};
use crate::vm_host::sound_on_this_host;
use std::fs;
use std::path::PathBuf;

/// The jailed QEMU's cache of compiled shaders, in the machine's folder.
const CACHE_FOLDER: &str = "gpu-cache";
const NO_CARD: &str = "This PC has no GPU an Android machine can use. It needs an NVIDIA, AMD or Intel card \
                       with its driver loaded.";

/// The NVIDIA card, if it has a render node, which the virtio GPU draws through.
fn nvidia_card(context: &Context, pci: &str) -> Option<AndroidCard> {
    forwarded_card(context)
        .filter(|card| pci.is_empty() || card.pci == pci)
        .and_then(|card| AndroidCard::nvidia(&card))
}

/// The card the machine draws on: the one chosen for it, or else the first NVIDIA card, then AMD, then
/// Intel.
pub fn card_of(context: &Context, machine: &Machine) -> Result<AndroidCard> {
    let pci = machine.android_gpu.as_str();
    if let Some(card) = nvidia_card(context, pci) {
        return Ok(card);
    }
    for maker in [GpuMaker::Amd, GpuMaker::Intel] {
        let found = render_cards(context, maker)
            .into_iter()
            .find(|card| (pci.is_empty() || card.pci == pci) && !card.render_node.as_os_str().is_empty());
        if let Some(card) = found {
            return Ok(AndroidCard::render(maker, &card));
        }
    }
    if pci.is_empty() {
        return Err(Failure::new(NO_CARD));
    }
    Err(Failure(format!(
        "The card at {pci} isn't in this PC now. Choose the GPU again in the machine's settings."
    )))
}

fn boot_of(context: &Context, machine: &Machine, card: AndroidCard) -> Result<AndroidBoot> {
    let data = PathBuf::from(&machine.android_data);
    for file in [PathBuf::from(&machine.disk), data.clone()] {
        if !file.exists() {
            return Err(Failure(format!("{} is missing.", file.display())));
        }
    }
    let cache = machine.folder(context).join(CACHE_FOLDER);
    fs::create_dir_all(&cache)?;
    Ok(AndroidBoot { data, card, cache, play: machine.android_play })
}

pub fn start(task: &mut Task) -> Result<()> {
    if let Some(problem) = start_problems(task.context, &task.machine)?.into_iter().next() {
        return Err(Failure(problem));
    }
    let card = card_of(task.context, &task.machine)?;
    let boot = boot_of(task.context, &task.machine, card)?;
    let mut plan = plan_for(task.context, &task.machine, Boot::Android(boot));
    // Before the jail is chosen, which lets a QEMU with sound reach PipeWire's socket.
    plan.sound = if task.machine.sound { sound_on_this_host() } else { None };
    let choice = choose(task.context, &task.machine, &plan, &(task.context.jail_offer)(&plan))?;
    plan.jail = choice.qemu.map(|jail| plan.with_backing_files(jail)).transpose()?;
    plan.jail_note = choice.note;
    task.step("Starting the VM");
    task.host.start(&plan, &task.machine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Kind;
    use crate::testing::{Scratch, scratch_context};

    fn droid(context: &Context) -> Machine {
        let folder = context.paths.machines.join("droid");
        fs::create_dir_all(&folder).unwrap();
        for name in ["disk.qcow2", "data.qcow2", "OVMF_VARS.fd"] {
            fs::write(folder.join(name), "").unwrap();
        }
        let file = |name: &str| folder.join(name).to_string_lossy().into_owned();
        Machine {
            kind: Kind::Plain,
            android_data: file("data.qcow2"),
            ..Machine::new("droid", "Droid", &file("disk.qcow2"), &file("OVMF_VARS.fd"))
        }
    }

    fn card() -> AndroidCard {
        AndroidCard { maker: GpuMaker::Amd, render_node: "/dev/dri/renderD129".into(), nodes: Vec::new() }
    }

    #[test]
    fn it_boots_its_two_disks_with_a_cache_in_its_folder() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = droid(&context);
        assert!(machine.android());
        let boot = boot_of(&context, &machine, card()).unwrap();
        assert_eq!(boot.data, PathBuf::from(&machine.android_data));
        assert!(boot.cache.is_dir() && boot.cache.starts_with(machine.folder(&context)));
    }

    #[test]
    fn a_missing_data_disk_is_said_before_anything_starts() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = droid(&context);
        fs::remove_file(&machine.android_data).unwrap();
        let said = boot_of(&context, &machine, card()).unwrap_err().to_string();
        assert!(said.ends_with("data.qcow2 is missing."), "{said}");
    }
}
