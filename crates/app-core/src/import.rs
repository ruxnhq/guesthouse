//! Making a machine of a file [`crate::export`] wrote, which came from somewhere else. Nothing in it
//! is taken on trust:
//!
//! - Each file's name must be a plain one inside the machine's folder, and each is made new, for this
//!   user alone. A file that's damaged or cut short stops the import, and nothing is left of it.
//! - The settings are read into a machine's own type, and only what isn't this PC's is taken from
//!   them. Paths, the shared folder, host drives, port forwards, the bridge and the jail's setting
//!   are this PC's, and come from here or not at all. The MAC and the card are chosen here.
//! - The disk must be a qcow2 that stands alone: no backing file and no data file, which could name
//!   any file of this PC's for QEMU to read. Its snapshots must have the names the app gives them,
//!   as each name becomes the name of a file. qemu-img reads it under limits of time and memory.
//! - The agent's token and the account's password must look like what the app makes, and the guest
//!   is moved to a new token at its first start here.

use crate::carry::{Member, Reader};
use crate::context::Context;
use crate::disk_image::header::{check_stands_alone, looks_like_qcow2};
use crate::duplicate::{Staging, sweep_abandoned};
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::host::{GIB, free_gib};
use crate::library::{DISK_FILE, FIRMWARE_FILE, new_id, unused_mac};
use crate::log::write_log;
use crate::machine::{Kind, MACHINE_FILE, Machine};
use crate::network::Mode;
use crate::render_cards::render_cards;
use crate::settings::{max_vram_gib_for, write_json};
use crate::snapshots::SNAPSHOT_TAG;
use crate::token_change::plan_token_change_at;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What the settings can't be bigger than, far over any machine's.
const MOST_SETTINGS_BYTES: u64 = 256 << 10;
/// Far more files than a machine's folder holds.
const MOST_FILES: usize = 10_000;
/// Left free beside the machine, so the drive isn't filled to its last byte.
const SPARE_GIB: f64 = 2.0;
/// What qemu-img may take reading a disk from elsewhere, as a disk made to trip it up could
/// otherwise take all of this PC's memory or time.
const QEMU_IMG_MEMORY: &str = "--as=2147483648";
const QEMU_IMG_SECONDS: &str = "--cpu=60";
/// The same for time spent waiting, which a limit on the CPU doesn't count.
const QEMU_IMG_WAIT: &str = "120";
/// The hex digits after the tag in a snapshot's name, as [`crate::snapshots`] makes them.
const SNAPSHOT_HEX_DIGITS: usize = 8;
const TOKEN_FILE: &str = "guest.token";
const PRIVATE: u32 = 0o600;

/// A machine made from an export, and what was changed or left out on the way, for the user.
#[derive(Debug)]
pub struct Imported {
    pub machine: Machine,
    pub notes: Vec<String>,
}

/// Whether a value in the settings says something of the PC that wrote them, which isn't taken.
fn names_its_pc(value: &str) -> bool {
    !value.is_empty()
}

/// The machine to make: the settings that aren't this PC's from the export, and the rest from here.
fn machine_from(
    context: &Context,
    carried: &Machine,
    name: &str,
    notes: &mut Vec<String>,
) -> Result<Machine> {
    if carried.installing {
        return Err(Failure::new("The machine in this export hadn't finished installing."));
    }
    let id = new_id(context, name);
    let folder = context.paths.machines.join(&id);
    let in_folder = |file: &str| folder.join(file).to_string_lossy().into_owned();
    let host_specific =
        [&carried.share, &carried.disk_folder, &carried.only_through, &carried.dns, &carried.iso];
    if host_specific.iter().any(|value| names_its_pc(value))
        || !carried.drives.is_empty()
        || !carried.forwards.is_empty()
    {
        notes.push(
            "The shared folder, host drives and port forwards are the other PC's, so they were left out."
                .to_owned(),
        );
    }
    let vars = if carried.vars.is_empty() || carried.kind == Kind::Linux {
        String::new()
    } else {
        in_folder(FIRMWARE_FILE)
    };
    if carried.clipboard || carried.linux_gpu {
        notes.push(
            "The shared clipboard and the Linux GPU preview are off until you turn them on here.".to_owned(),
        );
    }
    let opencore = match carried.opencore.as_str() {
        "" => String::new(),
        named => crate::carry::safe_path(named).map(|path| in_folder(&path.to_string_lossy())).ok_or_else(
            || Failure::new("The export names its OpenCore image in a way that can't be made safely."),
        )?,
    };
    // Only what is the guest's own or a choice about how it shows is taken, field by field, so a
    // setting added later is this PC's until it's added here.
    let mut machine = Machine {
        cores: carried.cores,
        ram_gib: carried.ram_gib,
        screen: carried.screen.clone(),
        refresh_hz: carried.refresh_hz,
        hdr: carried.hdr,
        kind: carried.kind,
        vram_gib: carried.vram_gib,
        tpm: carried.tpm,
        overlay: carried.overlay,
        direct_frames_off: carried.direct_frames_off,
        bar_stays: carried.bar_stays,
        opens_fullscreen: carried.opens_fullscreen,
        window_size_off: carried.window_size_off,
        network_mode: if carried.network_mode == Mode::Bridged { Mode::Nat } else { carried.network_mode },
        passt: carried.passt,
        mac: unused_mac(context),
        gpu_preview: carried.gpu_preview,
        sound: carried.sound,
        user: carried.user.clone(),
        window_speed_off: carried.window_speed_off,
        file_drop_off: carried.file_drop_off,
        nvapi_wrapper_off: carried.nvapi_wrapper_off,
        nvme: carried.nvme,
        opencore,
        throwaway: carried.throwaway,
        ..Machine::new(&id, name.trim(), &in_folder(DISK_FILE), &vars)
    };
    if carried.network_mode == Mode::Bridged {
        notes.push("It was on a bridge there, and is on NAT here. Its network settings can put it on a bridge of this PC's.".to_owned());
    }
    choose_card(context, &mut machine, carried, notes)?;
    fit_to_this_pc(context, &mut machine, notes);
    let cap = max_vram_gib_for(context, &machine);
    if machine.vram_gib > cap {
        machine.vram_gib = 0;
        notes.push("Its video memory cap was more than this PC's card has, so it has none.".to_owned());
    }
    Ok(machine)
}

/// Cores and RAM no more than this PC has, as the settings would take any number.
fn fit_to_this_pc(context: &Context, machine: &mut Machine, notes: &mut Vec<String>) {
    let cores = crate::host::physical_cores(&context.cpus).max(1);
    let ram_gib = u32::try_from(crate::host::total_ram(context) / GIB).unwrap_or(u32::MAX).max(1);
    if machine.cores > cores || machine.ram_gib > ram_gib {
        machine.cores = machine.cores.min(cores);
        machine.ram_gib = machine.ram_gib.min(ram_gib);
        notes.push(format!(
            "It had more cores or RAM than this PC has, so it has {} cores and {} GiB here.",
            machine.cores, machine.ram_gib
        ));
    }
}

/// A card here of the maker the guest's driver is for.
fn choose_card(
    context: &Context,
    machine: &mut Machine,
    carried: &Machine,
    notes: &mut Vec<String>,
) -> Result<()> {
    if carried.kind == Kind::Linux || carried.own_linux_gpu() {
        machine.amd_gpu = crate::linux::amd::default_linux_gpu(context);
        return Ok(());
    }
    let Some(maker) = carried.windows_maker() else {
        machine.amd_gpu = String::new();
        return Ok(());
    };
    if maker == GpuMaker::Nvidia {
        machine.use_windows_card(maker, "");
        return Ok(());
    }
    let card = render_cards(context, maker).into_iter().next().ok_or_else(|| {
        Failure(format!(
            "Windows in this export was installed for an {} card, and this PC has none.",
            maker.name()
        ))
    })?;
    machine.use_windows_card(maker, &card.pci);
    notes.push(format!("It runs on the {} at {}. Its settings can choose another.", card.name, card.pci));
    Ok(())
}

/// Reads the settings, the first file in every export.
fn read_settings(reader: &mut Reader<BufReader<File>>) -> Result<Machine> {
    let first = reader.next_file()?.ok_or_else(|| Failure::new("The export holds no machine."))?;
    if first.path != Path::new(MACHINE_FILE) || first.size > MOST_SETTINGS_BYTES {
        return Err(Failure::new("The export doesn't start with a machine's settings."));
    }
    let mut text = Vec::new();
    reader.copy_to(&first, &mut text)?;
    serde_json::from_slice(&text).map_err(|error| {
        Failure(format!(
            "The export's settings can't be read here, maybe as a newer Guesthouse wrote them: {error}"
        ))
    })
}

/// Makes one file of the export in `folder`, new, for this user alone.
fn write_member(reader: &mut Reader<BufReader<File>>, member: &Member, folder: &Path) -> Result<()> {
    let path = folder.join(&member.path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE)
        .open(&path)
        .map_err(|error| Failure(format!("{} could not be made: {error}", member.path.display())))?;
    let mut out = BufWriter::new(file);
    reader.copy_to(member, &mut out)?;
    out.flush()?;
    Ok(())
}

/// What qemu-img says of a disk from elsewhere, read as a qcow2 whatever it looks like.
fn disk_from_elsewhere(disk: &Path) -> Result<Value> {
    let looked = Command::new("timeout")
        .args([QEMU_IMG_WAIT, "prlimit", QEMU_IMG_MEMORY, QEMU_IMG_SECONDS, "--"])
        .args(["qemu-img", "info", "-f", "qcow2", "--output=json"])
        .arg(disk)
        .output()?;
    if !looked.status.success() {
        let said = String::from_utf8_lossy(&looked.stderr);
        return Err(Failure(format!(
            "The disk in the export isn't a disk this app can use: {}",
            said.trim()
        )));
    }
    serde_json::from_slice(&looked.stdout).map_err(|error| Failure(error.to_string()))
}

fn is_snapshot_tag(name: &str) -> bool {
    name.strip_prefix(SNAPSHOT_TAG).is_some_and(|rest| {
        rest.len() == SNAPSHOT_HEX_DIGITS
            && rest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Refuses a disk that would reach past itself, or whose snapshots would name files of their own
/// choosing. Its header is read here first, so qemu-img never opens one that names another file.
fn check_disk(disk: &Path) -> Result<()> {
    check_stands_alone(disk)?;
    let info = disk_from_elsewhere(disk)?;
    if info.get("backing-filename").is_some() || info.get("full-backing-filename").is_some() {
        return Err(Failure::new(
            "The disk in the export is built on another file, which an import can't take.",
        ));
    }
    if info.pointer("/format-specific/data/data-file").is_some() {
        return Err(Failure::new(
            "The disk in the export keeps its data in another file, which an import can't take.",
        ));
    }
    let snapshots = info["snapshots"].as_array().cloned().unwrap_or_default();
    if snapshots.iter().any(|snapshot| !snapshot["name"].as_str().is_some_and(is_snapshot_tag)) {
        return Err(Failure::new(
            "The disk in the export has a snapshot this app didn't make, which an import can't take.",
        ));
    }
    Ok(())
}

/// Makes the machine in the export at `file`, called `name` or what it was called there. `step` hears
/// what it's doing. Nothing shows in the library until the machine is whole.
pub fn import(
    context: &Context,
    file: &Path,
    name: Option<&str>,
    step: &mut dyn FnMut(&str),
) -> Result<Imported> {
    let opened = File::open(file)
        .map_err(|error| Failure(format!("{} could not be read: {error}", file.display())))?;
    let size = opened.metadata()?.len();
    let mut reader = Reader::new(BufReader::new(opened))?;
    let carried = read_settings(&mut reader)?;
    let name = name.map_or_else(|| carried.name.clone(), str::to_owned);
    if name.trim().is_empty() {
        return Err(Failure::new("The machine needs a name."));
    }
    let mut notes = Vec::new();
    let machine = machine_from(context, &carried, &name, &mut notes)?;
    fs::create_dir_all(&context.paths.machines)?;
    let (gib, free) = (size as f64 / GIB as f64, free_gib(&context.paths.machines)?);
    if free < gib + SPARE_GIB {
        return Err(Failure(format!(
            "The machine needs {:.0} GiB free in {}, and there is {free:.0} GiB.",
            (gib + SPARE_GIB).ceil(),
            context.paths.machines.display()
        )));
    }
    sweep_abandoned(context);
    let staging = Staging::new(context, &machine)?;
    step(&format!("Reading {gib:.1} GiB"));
    let made = make(context, &mut reader, &carried, &machine, &staging, &mut notes, step);
    if made.is_err() {
        let _ = fs::remove_dir_all(&staging.root);
        let _ = fs::remove_dir(crate::duplicate::staging_root(context));
        made?;
    }
    let _ = fs::remove_dir_all(&staging.root);
    let _ = fs::remove_dir(crate::duplicate::staging_root(context));
    let _ = write_log(context, &format!("{} was imported from {}", machine.id, file.display()));
    Ok(Imported { machine, notes })
}

fn make(
    context: &Context,
    reader: &mut Reader<BufReader<File>>,
    carried: &Machine,
    machine: &Machine,
    staging: &Staging,
    notes: &mut Vec<String>,
    step: &mut dyn FnMut(&str),
) -> Result<()> {
    let folder = staging.folder();
    let mut seen = BTreeSet::new();
    let mut left_out = 0;
    while let Some(member) = reader.next_file()? {
        if member.path == Path::new(MACHINE_FILE)
            || !seen.insert(member.path.clone())
            || seen.len() > MOST_FILES
        {
            return Err(Failure(format!(
                "The export has {} more than once, or too many files.",
                member.path.display()
            )));
        }
        let Some(handling) = accepted::handling(&member.path, carried) else {
            reader.copy_to(&member, &mut std::io::sink())?;
            left_out += 1;
            continue;
        };
        if handling == accepted::Handling::Small {
            accepted::small_enough(&member.path, member.size)?;
        }
        write_member(reader, &member, &folder)?;
        // Anything that is a qcow2 may be opened as one, as an OpenCore image is.
        let written = folder.join(&member.path);
        if looks_like_qcow2(&written) {
            check_stands_alone(&written)?;
        }
    }
    let wanted: Vec<PathBuf> = [machine.disk.as_str(), machine.vars.as_str(), machine.opencore.as_str()]
        .iter()
        .filter(|path| !path.is_empty())
        .filter_map(|path| Path::new(path).strip_prefix(machine.folder(context)).ok().map(Path::to_owned))
        .collect();
    if let Some(missing) = wanted.iter().find(|path| !seen.contains(*path)) {
        return Err(Failure(format!("The export has no {}, which the machine needs.", missing.display())));
    }
    step("Checking the disk");
    check_disk(&folder.join(DISK_FILE))?;
    accepted::finish(&folder, notes)?;
    if left_out > 0 {
        notes.push(format!(
            "{left_out} files of the other PC's that this app doesn't take here were left out."
        ));
    }
    if machine.throwaway {
        crate::throwaway::keep_notes(&folder)?;
    }
    plan_token_change_at(&folder.join(TOKEN_FILE))?;
    crate::actions::copy_identity::plan_at(&folder, machine.kind)?;
    if let Some(problem) = machine.problems(max_vram_gib_for(context, machine)).into_iter().next() {
        return Err(Failure(problem));
    }
    write_json(&folder.join(MACHINE_FILE), machine)?;
    fs::rename(&folder, machine.folder(context))?;
    Ok(())
}

mod accepted;

#[cfg(test)]
mod tests;
