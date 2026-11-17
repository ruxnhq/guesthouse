//! Starting a macOS guest. It runs on reims-vgpu's QEMU, with no daemon and no forwarding: the
//! guest's own drivers draw through the device, and the device draws on the host's Vulkan. Its QEMU
//! opens the card itself, so it only starts jailed, as a GPU machine's does.

use super::{Task, plan_for};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::jailing::choose;
use crate::linux::amd::amd_card;
use crate::linux::card::forwarded_card;
use crate::machine::Machine;
use crate::macos_setup::NO_MACOS_PART;
use crate::start_checks::start_problems;
use crate::vm_args::{Boot, MACOS_QEMU, MACOS_ROM, MacosBoot, MacosCard, UDMABUF};
use crate::vm_host::sound_on_this_host;
use std::fs;
use std::path::{Path, PathBuf};

/// The firmware OpenCore was made for, which is kept beside its image.
const FIRMWARE_CODE: &str = "OVMF_CODE.fd";
/// The jailed QEMU's home, in the machine's folder: see [`MacosBoot::home`].
const HOME_FOLDER: &str = "gpu-cache";
const NO_UDMABUF: &str = "/dev/udmabuf can't be opened. A macOS machine's memory reaches the GPU through \
                          it. Add yourself to the group that owns it, usually kvm, and log in again.";
const NO_CARD: &str = "This PC has no GPU a macOS machine can use. It needs an NVIDIA or AMD card with \
                       its driver loaded.";
/// The PCI device a DRM node belongs to, and its number there.
const DEVICE_NUMBER: &str = "device/device";

/// A card's PCI device number, from its render node's folder in the DRM class.
fn device_number(context: &Context, render_node: &str) -> Option<u32> {
    let text = fs::read_to_string(context.drm_class.join(render_node).join(DEVICE_NUMBER)).ok()?;
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// The card the device draws on: the AMD card chosen for the machine, or else the NVIDIA card.
fn card_of(context: &Context, machine: &Machine) -> Result<MacosCard> {
    if !machine.amd_gpu.is_empty() {
        let card = amd_card(context, &machine.amd_gpu).ok_or_else(|| {
            Failure(format!(
                "The AMD card at {} isn't in this PC now. Choose the GPU again in the machine's settings.",
                machine.amd_gpu
            ))
        })?;
        return Ok(MacosCard::amd(&card));
    }
    let card = forwarded_card(context).ok_or_else(|| Failure::new(NO_CARD))?;
    let device = card.render_node.as_deref().and_then(|node| device_number(context, node));
    Ok(MacosCard::nvidia(&card, device))
}

/// What this machine boots from, or what is missing.
fn boot_of(context: &Context, machine: &Machine, card: MacosCard) -> Result<MacosBoot> {
    let opencore = PathBuf::from(&machine.opencore);
    let firmware_code = opencore.parent().unwrap_or(Path::new("")).join(FIRMWARE_CODE);
    let rom = context.paths.helper(MACOS_ROM);
    for file in [&opencore, &firmware_code] {
        if !file.exists() {
            return Err(Failure(format!("{} is missing.", file.display())));
        }
    }
    if !rom.exists() || !context.paths.helper(MACOS_QEMU).exists() {
        return Err(Failure::new(NO_MACOS_PART));
    }
    let home = machine.folder(context).join(HOME_FOLDER);
    fs::create_dir_all(&home)?;
    Ok(MacosBoot { opencore, firmware_code, installer: machine.iso.clone(), rom, card, home })
}

fn can_open(device: &str) -> bool {
    fs::OpenOptions::new().read(true).write(true).open(device).is_ok()
}

/// What on this PC would stop any macOS machine's start, whatever its files: no card, or no way for
/// its memory to reach one. Asked before a machine is made, so nothing is downloaded for nothing.
pub fn start_refusal(context: &Context, machine: &Machine) -> Vec<String> {
    let mut found: Vec<String> =
        card_of(context, machine).err().map(|failure| failure.0).into_iter().collect();
    if !can_open(UDMABUF) {
        found.push(NO_UDMABUF.to_owned());
    }
    found
}

pub fn start(task: &mut Task) -> Result<()> {
    if let Some(problem) = start_problems(task.context, &task.machine)?.into_iter().next() {
        return Err(Failure(problem));
    }
    let card = card_of(task.context, &task.machine)?;
    let boot = boot_of(task.context, &task.machine, card)?;
    if !can_open(UDMABUF) {
        return Err(Failure::new(NO_UDMABUF));
    }
    let mut plan = plan_for(task.context, &task.machine, Boot::Macos(boot));
    plan.qemu = task.context.paths.helper(MACOS_QEMU);
    // Its QEMU finds its own files beside itself.
    plan.qemu_files = None;
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

    fn mac(context: &Context) -> Machine {
        let folder = context.paths.machines.join("mac");
        fs::create_dir_all(&folder).unwrap();
        for name in ["disk.qcow2", "OVMF_VARS.fd", "opencore.qcow2", FIRMWARE_CODE] {
            fs::write(folder.join(name), "").unwrap();
        }
        let file = |name: &str| folder.join(name).to_string_lossy().into_owned();
        Machine {
            kind: Kind::Plain,
            opencore: file("opencore.qcow2"),
            ..Machine::new("mac", "Mac", &file("disk.qcow2"), &file("OVMF_VARS.fd"))
        }
    }

    fn with_macos_part(context: &Context) {
        for name in [MACOS_QEMU, MACOS_ROM] {
            let file = context.paths.helper(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "").unwrap();
        }
    }

    #[test]
    fn it_boots_from_the_files_beside_its_opencore_image() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        with_macos_part(&context);
        let machine = mac(&context);
        let boot = boot_of(&context, &machine, MacosCard::default()).unwrap();
        assert_eq!(boot.firmware_code, machine.folder(&context).join(FIRMWARE_CODE));
        assert_eq!(boot.rom, context.paths.helper(MACOS_ROM));
        assert_eq!(boot.installer, "");
        assert!(boot.home.is_dir() && boot.home.starts_with(machine.folder(&context)));
    }

    #[test]
    fn a_missing_file_or_part_is_said_before_anything_starts() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = mac(&context);
        assert_eq!(boot_of(&context, &machine, MacosCard::default()).unwrap_err().to_string(), NO_MACOS_PART);
        with_macos_part(&context);
        fs::remove_file(machine.folder(&context).join(FIRMWARE_CODE)).unwrap();
        let said = boot_of(&context, &machine, MacosCard::default()).unwrap_err().to_string();
        assert!(said.ends_with("OVMF_CODE.fd is missing."), "{said}");
    }
}
