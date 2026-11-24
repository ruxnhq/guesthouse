//! Making an Android machine from an image of Android for VMs (LineageOS's virtio target, as its
//! `vm-utm-zip` makes it). The image holds two disks: the system one, with GRUB, the kernel and Android's
//! partitions, and an empty one Android formats for its data at its first boot. The machine gets its own
//! copy of the first and a new second of the size asked for. Nothing installs: the first start is its
//! setup screen.

use crate::context::Context;
use crate::defaults::Sizes;
use crate::failure::{Failure, Result};
use crate::host::plain_ram_limit;
use crate::library::{MAX_DISK_GIB, new_id, unused_mac};
use crate::machine::{Kind, Machine};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const ANDROID_TITLE: &str = "Android";
pub const ANDROID_SIZES: Sizes = Sizes { cores: 4, ram: 8, disk: 32 };
/// Android's data partition, for its apps and what they keep. Games can want a lot of it.
pub const MIN_ANDROID_DATA_GIB: u32 = 8;
pub const DISK_FILE: &str = "disk.qcow2";
pub const DATA_FILE: &str = "data.qcow2";
pub const FIRMWARE_VARS_FILE: &str = "OVMF_VARS.fd";
/// The image's system disk, and the folder of a UTM bundle it's in.
const IMAGE_SYSTEM_DISK: &str = "vda.qcow2";
const BUNDLE_DATA_FOLDER: &str = "Data";

/// What a new Android machine is to be.
#[derive(Debug, Clone)]
pub struct NewAndroid {
    pub name: String,
    pub cores: u32,
    pub ram_gib: u32,
    pub data_gib: u32,
    /// Play services through microG.
    pub play: bool,
    /// The image: its system disk, the folder that holds it, or the UTM bundle that does.
    pub image: PathBuf,
}

fn file_in(folder: &Path, name: &str) -> String {
    folder.join(name).to_string_lossy().into_owned()
}

/// The image's system disk, wherever in it it is.
pub fn system_disk_of(image: &Path) -> Option<PathBuf> {
    if image.is_file() {
        return Some(image.to_owned());
    }
    let bundles = fs::read_dir(image).ok()?.flatten().map(|entry| entry.path().join(BUNDLE_DATA_FOLDER));
    [image.to_owned(), image.join(BUNDLE_DATA_FOLDER)]
        .into_iter()
        .chain(bundles)
        .map(|folder| folder.join(IMAGE_SYSTEM_DISK))
        .find(|disk| disk.is_file())
}

/// The machine as it will be, with nothing made yet but its name.
fn planned(context: &Context, new: &NewAndroid) -> Machine {
    let id = new_id(context, &new.name);
    let folder: PathBuf = context.paths.machines.join(&id);
    Machine {
        kind: Kind::Plain,
        cores: new.cores,
        ram_gib: new.ram_gib,
        android_data: file_in(&folder, DATA_FILE),
        android_play: new.play,
        mac: unused_mac(context),
        ..Machine::new(
            &id,
            new.name.trim(),
            &file_in(&folder, DISK_FILE),
            &file_in(&folder, FIRMWARE_VARS_FILE),
        )
    }
}

/// Says why this Android machine can't be made now, before anything is copied.
pub fn create_problems(context: &Context, new: &NewAndroid, machine: &Machine) -> Vec<String> {
    let mut found = machine.problems(crate::settings::max_vram_gib(context));
    if new.name.trim().is_empty() {
        found.push("The machine needs a name.".to_owned());
    }
    if !(MIN_ANDROID_DATA_GIB..=MAX_DISK_GIB).contains(&new.data_gib) {
        found.push(format!("The disk must be between {MIN_ANDROID_DATA_GIB} and {MAX_DISK_GIB} GiB."));
    }
    if system_disk_of(&new.image).is_none() {
        found.push(format!(
            "{} isn't an Android image. Choose the folder or UTM bundle that holds {IMAGE_SYSTEM_DISK}.",
            new.image.display()
        ));
    }
    let limit = plain_ram_limit(context);
    if u64::from(machine.ram_gib) > limit {
        found.push(format!(
            "This PC has room for {limit} GiB right now. Give the machine that much, or stop another VM."
        ));
    }
    if let Err(failure) = crate::actions::android_card_of(context, machine) {
        found.push(failure.0);
    }
    found
}

fn run(what: &str, command: &mut Command) -> Result<()> {
    match command.output() {
        Ok(result) if result.status.success() => Ok(()),
        Ok(result) => Err(Failure(format!("{what}: {}", String::from_utf8_lossy(&result.stderr).trim()))),
        Err(error) => Err(Failure(format!("{what}: {error}"))),
    }
}

fn make_files(context: &Context, new: &NewAndroid, machine: &Machine) -> Result<()> {
    let system = system_disk_of(&new.image).ok_or_else(|| Failure::new("The image has no system disk."))?;
    // A copy that shares the image's blocks where the file system can, as the disk is only written over.
    run(
        "The system disk could not be copied",
        Command::new("cp").args(["--reflink=auto", "--"]).arg(&system).arg(&machine.disk),
    )?;
    run(
        "The data disk could not be made",
        Command::new("qemu-img").args([
            "create",
            "-q",
            "-f",
            "qcow2",
            &machine.android_data,
            &format!("{}G", new.data_gib),
        ]),
    )?;
    fs::copy(&context.paths.firmware.vars, &machine.vars)?;
    Ok(())
}

/// Makes an Android machine's folder and disks, ready to start.
pub fn create_android(context: &Context, new: &NewAndroid) -> Result<Machine> {
    let machine = planned(context, new);
    if let Some(problem) = create_problems(context, new, &machine).into_iter().next() {
        return Err(Failure(problem));
    }
    fs::create_dir_all(machine.folder(context))?;
    if let Err(failure) = make_files(context, new, &machine).and_then(|()| machine.save(context)) {
        let _ = fs::remove_dir_all(machine.folder(context));
        return Err(failure);
    }
    Ok(machine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn the_system_disk_is_found_in_a_folder_a_bundle_or_a_bundle_s_folder() {
        let scratch = Scratch::new();
        let bundle = scratch.path("dl/LineageOS_on_x86_64.utm");
        fs::create_dir_all(bundle.join(BUNDLE_DATA_FOLDER)).unwrap();
        let disk = bundle.join(BUNDLE_DATA_FOLDER).join(IMAGE_SYSTEM_DISK);
        fs::write(&disk, "").unwrap();
        for image in [disk.clone(), bundle.join(BUNDLE_DATA_FOLDER), bundle.clone(), scratch.path("dl")] {
            assert_eq!(system_disk_of(&image), Some(disk.clone()), "{}", image.display());
        }
        assert_eq!(system_disk_of(&scratch.path("nothing")), None);
    }

    #[test]
    fn a_planned_android_is_a_plain_machine_with_a_data_disk_beside_its_own() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let new = NewAndroid {
            name: "Phone".into(),
            cores: 2,
            ram_gib: 2,
            data_gib: 32,
            play: false,
            image: "/i".into(),
        };
        let machine = planned(&context, &new);
        let folder = context.paths.machines.join("phone");
        assert!(machine.android() && !machine.installing && machine.kind == Kind::Plain);
        assert_eq!(machine.disk, file_in(&folder, DISK_FILE));
        assert_eq!(machine.android_data, file_in(&folder, DATA_FILE));
    }

    #[test]
    fn something_that_isnt_an_image_is_refused_and_nothing_is_left() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let new = NewAndroid {
            name: "Phone".into(),
            cores: 2,
            ram_gib: 2,
            data_gib: 32,
            play: false,
            image: scratch.path("not-an-image"),
        };
        let problems = create_problems(&context, &new, &planned(&context, &new));
        assert!(problems.iter().any(|problem| problem.contains("isn't an Android image")), "{problems:?}");
        assert!(create_android(&context, &new).is_err());
        assert!(!context.paths.machines.join("phone").exists());
    }
}
