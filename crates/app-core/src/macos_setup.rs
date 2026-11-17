//! Making a macOS machine. There is no disc to choose: its installer comes from Apple, and its boot
//! image is made for it, both by reims-util from the program folder's macOS part. The machine is made
//! here with its disk, and the rest is its install, a task of its own: see `actions::macos_install`.

use crate::context::Context;
use crate::defaults::Sizes;
use crate::failure::{Failure, Result};
use crate::host::plain_ram_limit;
use crate::library::{MAX_DISK_GIB, new_id, unused_mac};
use crate::machine::{Kind, Machine};
use crate::vm_args::{MACOS_QEMU, MACOS_ROM};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// The one macOS offered: the device's drivers load on macOS 26.4 and later.
pub const MACOS_TITLE: &str = "macOS Tahoe";
/// What reims-util calls it.
pub const MACOS_VERSION: &str = "tahoe";
pub const MACOS_SIZES: Sizes = Sizes { cores: 8, ram: 16, disk: 128 };
/// macOS and its installer's download take about 40 GiB between them.
pub const MIN_MACOS_DISK_GIB: u32 = 64;
/// The tool that makes the boot image and fetches the installer, and the guest's drivers it adds.
pub const REIMS_UTIL: &str = "macos/reims-util";
pub const KEXTS_FOLDER: &str = "macos/kexts";
pub const KEXT_SET: &str = "set.toml";
pub const NO_MACOS_PART: &str =
    "This install has no macOS part, which a macOS machine is made and started with.";
pub const HOW_IT_INSTALLS: &str = "Downloads Apple's installer, about 1 GiB, and starts it. In its window, \
                                   erase the disk in Disk Utility as APFS, then choose Reinstall macOS, \
                                   which downloads the rest.";

pub const DISK_FILE: &str = "disk.qcow2";
pub const OPENCORE_FILE: &str = "opencore.qcow2";
pub const INSTALLER_FILE: &str = "installer.img";
pub const FIRMWARE_CODE_FILE: &str = "OVMF_CODE.fd";
pub const FIRMWARE_VARS_FILE: &str = "OVMF_VARS.fd";

/// What a new macOS machine is to be.
#[derive(Debug, Clone)]
pub struct NewMac {
    pub name: String,
    pub cores: u32,
    pub ram_gib: u32,
    pub disk_gib: u32,
}

/// Whether this install can make and start a macOS machine.
pub fn has_macos_part(context: &Context) -> bool {
    let paths = &context.paths;
    let set = paths.helper(KEXTS_FOLDER).join(KEXT_SET);
    [paths.helper(REIMS_UTIL), paths.helper(MACOS_QEMU), paths.helper(MACOS_ROM), set]
        .iter()
        .all(|file| file.exists())
}

fn file_in(folder: &std::path::Path, name: &str) -> String {
    folder.join(name).to_string_lossy().into_owned()
}

/// The machine as it will be once installed, with nothing made yet but its name.
fn planned(context: &Context, new: &NewMac) -> Machine {
    let id = new_id(context, &new.name);
    let folder: PathBuf = context.paths.machines.join(&id);
    Machine {
        kind: Kind::Plain,
        cores: new.cores,
        ram_gib: new.ram_gib,
        installing: true,
        opencore: file_in(&folder, OPENCORE_FILE),
        iso: file_in(&folder, INSTALLER_FILE),
        mac: unused_mac(context),
        ..Machine::new(
            &id,
            new.name.trim(),
            &file_in(&folder, DISK_FILE),
            &file_in(&folder, FIRMWARE_VARS_FILE),
        )
    }
}

/// Says why this macOS machine can't be made now. What would stop its start is said here, before
/// anything is downloaded.
pub fn create_problems(context: &Context, new: &NewMac, machine: &Machine) -> Vec<String> {
    let mut found = machine.problems(crate::settings::max_vram_gib(context));
    if new.name.trim().is_empty() {
        found.push("The machine needs a name.".to_owned());
    }
    if !(MIN_MACOS_DISK_GIB..=MAX_DISK_GIB).contains(&new.disk_gib) {
        found.push(format!("The disk must be between {MIN_MACOS_DISK_GIB} and {MAX_DISK_GIB} GiB."));
    }
    let limit = plain_ram_limit(context);
    if u64::from(machine.ram_gib) > limit {
        found.push(format!(
            "This PC has room for {limit} GiB right now. Give the machine that much, or stop another VM."
        ));
    }
    if !has_macos_part(context) {
        found.push(NO_MACOS_PART.to_owned());
    }
    found.extend(crate::actions::macos_start_refusal(context, machine));
    found
}

/// Makes a macOS machine's folder and disk. Its boot image and installer are its install's work.
pub fn create_macos(context: &Context, new: &NewMac) -> Result<Machine> {
    let machine = planned(context, new);
    if let Some(problem) = create_problems(context, new, &machine).into_iter().next() {
        return Err(Failure(problem));
    }
    fs::create_dir_all(&context.paths.machines)?;
    machine.save(context)?;
    let made = Command::new("qemu-img")
        .args(["create", "-q", "-f", "qcow2", &machine.disk, &format!("{}G", new.disk_gib)])
        .output();
    let failure = match made {
        Ok(result) if result.status.success() => return Ok(machine),
        Ok(result) => {
            format!("The disk could not be made: {}", String::from_utf8_lossy(&result.stderr).trim())
        }
        Err(error) => format!("The disk could not be made: {error}"),
    };
    let _ = fs::remove_dir_all(machine.folder(context));
    Err(Failure(failure))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    fn new_mac() -> NewMac {
        NewMac { name: "My Mac".into(), cores: 2, ram_gib: 2, disk_gib: 128 }
    }

    #[test]
    fn a_planned_mac_is_a_plain_machine_that_boots_through_opencore_and_still_installs() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = planned(&context, &new_mac());
        let folder = context.paths.machines.join("my-mac");
        assert!(machine.macos() && machine.installing && machine.kind == Kind::Plain);
        assert_eq!(machine.opencore, file_in(&folder, OPENCORE_FILE));
        assert_eq!(machine.iso, file_in(&folder, INSTALLER_FILE));
        assert_eq!(machine.vars, file_in(&folder, FIRMWARE_VARS_FILE));
        assert!(!machine.mac.is_empty(), "its boot image's ROM is its card's address");
    }

    #[test]
    fn what_would_stop_its_start_stops_it_being_made() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let small = NewMac { disk_gib: 8, ..new_mac() };
        let problems = create_problems(&context, &small, &planned(&context, &small));
        assert!(problems.iter().any(|problem| problem.starts_with("The disk must be between 64")));
        assert!(problems.contains(&NO_MACOS_PART.to_owned()), "{problems:?}");
        assert!(create_macos(&context, &new_mac()).is_err());
        assert!(!context.paths.machines.join("my-mac").exists(), "nothing is left of one that wasn't made");
    }
}
