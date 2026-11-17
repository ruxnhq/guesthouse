//! Starting a macOS guest. It runs on reims-vgpu's QEMU, with no daemon and no forwarding: the
//! guest's own drivers draw through the device, and the device draws on the host's Vulkan.

use super::{Task, plan_for};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use crate::start_checks::start_problems;
use crate::vm_args::{Boot, MACOS_QEMU, MACOS_ROM, MacosBoot};
use std::path::{Path, PathBuf};

/// The firmware OpenCore was made for, which is kept beside its image.
const FIRMWARE_CODE: &str = "OVMF_CODE.fd";
const NO_MACOS_PART: &str = "This install has no macOS part, which a macOS machine starts with.";
const NO_UDMABUF: &str = "/dev/udmabuf can't be opened. A macOS machine's memory reaches the GPU through \
                          it. Add yourself to the group that owns it, usually kvm, and log in again.";
const UDMABUF: &str = "/dev/udmabuf";

/// What this machine boots from, or what is missing.
fn boot_of(context: &Context, machine: &Machine) -> Result<MacosBoot> {
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
    Ok(MacosBoot { opencore, firmware_code, installer: machine.iso.clone(), rom })
}

fn can_open(device: &str) -> bool {
    std::fs::OpenOptions::new().read(true).write(true).open(device).is_ok()
}

pub fn start(task: &mut Task) -> Result<()> {
    if let Some(problem) = start_problems(task.context, &task.machine)?.into_iter().next() {
        return Err(Failure(problem));
    }
    let boot = boot_of(task.context, &task.machine)?;
    if !can_open(UDMABUF) {
        return Err(Failure::new(NO_UDMABUF));
    }
    let mut plan = plan_for(task.context, &task.machine, Boot::Macos(boot));
    plan.qemu = task.context.paths.helper(MACOS_QEMU);
    // Its QEMU finds its own files beside itself.
    plan.qemu_files = None;
    task.step("Starting the VM");
    task.host.start(&plan, &task.machine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Kind;
    use crate::testing::{Scratch, scratch_context};
    use std::fs;

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
        let boot = boot_of(&context, &machine).unwrap();
        assert_eq!(boot.firmware_code, machine.folder(&context).join(FIRMWARE_CODE));
        assert_eq!(boot.rom, context.paths.helper(MACOS_ROM));
        assert_eq!(boot.installer, "");
    }

    #[test]
    fn a_missing_file_or_part_is_said_before_anything_starts() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = mac(&context);
        assert_eq!(boot_of(&context, &machine).unwrap_err().to_string(), NO_MACOS_PART);
        with_macos_part(&context);
        fs::remove_file(machine.folder(&context).join(FIRMWARE_CODE)).unwrap();
        let said = boot_of(&context, &machine).unwrap_err().to_string();
        assert!(said.ends_with("OVMF_CODE.fd is missing."), "{said}");
    }
}
