//! Makes a guest's disk from a Windows image with no installer and no screen: a new VHDX is
//! attached, given the partitions UEFI boots from, the edition is applied to it with DISM, and its
//! boot files are written with bcdboot, all programs every Windows has. Then the answer file, the
//! report and the card's driver go on it, and the first start finishes the rest.

use super::{absolute, chosen_card, copy_into, need_elevation, system_program, volumes};
use crate::Options;
use crate::guest;
use crate::wim_info;
use guesthouse_hcs::disk::{self, Kind, VirtualDisk};
use guesthouse_hcs::host;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// UEFI's system partition, in MB, as Windows' own installer makes it.
const SYSTEM_PARTITION_MB: u32 = 260;
/// The partition Windows reserves for its own use on a GPT disk, in MB.
const RESERVED_PARTITION_MB: u32 = 16;
const BYTES_PER_GB: u64 = 1 << 30;

pub fn prepare(options: &Options) -> Result<(), String> {
    need_elevation()?;
    let disk = absolute(options.disk.as_deref().expect("parse asks for a disk"))?;
    let image = absolute(options.image.as_deref().expect("parse asks for an image"))?;
    if disk.exists() {
        return Err(format!(
            "{} is there already. prepare makes a new disk, and never writes over one.",
            disk.display()
        ));
    }
    let card = chosen_card(options)?;

    // An ISO stays attached while its image is applied.
    let (_iso, wim) = image_file(&image)?;
    let listing = run_captured(
        Command::new(system_program("dism.exe"))
            .arg("/English")
            .arg("/Get-WimInfo")
            .raw_arg(format!("/WimFile:\"{}\"", wim.display())),
    )?;
    let index = wim_info::index_of(&listing, &options.edition).ok_or_else(|| {
        let names: Vec<String> = wim_info::editions(&listing).into_iter().map(|(_, name)| name).collect();
        format!(
            "The image has no {}. It has: {}. Pick one with --edition.",
            options.edition,
            names.join(", ")
        )
    })?;

    println!("Making {} ({} GB at most).", disk.display(), options.size_gb);
    disk::create_vhdx(&disk, options.size_gb * BYTES_PER_GB).map_err(|error| error.to_string())?;
    let made =
        fill(&disk, &wim, index, options, card.as_ref().and_then(|card| card.driver_folder.as_deref()));
    if made.is_err() {
        // A half-made disk would only be refused by the next run, so it goes.
        let _ = fs::remove_file(&disk);
    }
    made
}

/// The Windows image to apply, and the ISO it's on, attached until dropped, if it's on one.
fn image_file(image: &Path) -> Result<(Option<VirtualDisk>, PathBuf), String> {
    let is_iso = image.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("iso"));
    if !is_iso {
        return Ok((None, image.to_owned()));
    }
    let mut iso = VirtualDisk::open(image, Kind::Iso).map_err(|error| error.to_string())?;
    iso.attach(true).map_err(|error| error.to_string())?;
    // DISM is given the drive letter Windows gave the ISO, and the volume's own path only without one.
    let root = volumes(&iso)?
        .into_iter()
        .next()
        .map(|volume| volume.mount_points.into_iter().next().unwrap_or(volume.root))
        .ok_or_else(|| format!("Windows showed no volume for {}.", image.display()))?;
    let wim = ["install.wim", "install.esd"]
        .iter()
        .map(|name| root.join("sources").join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| format!("{} has no sources\\install.wim or install.esd.", image.display()))?;
    Ok((Some(iso), wim))
}

fn fill(
    disk_path: &Path,
    wim: &Path,
    index: u32,
    options: &Options,
    driver: Option<&Path>,
) -> Result<(), String> {
    let mut attached = VirtualDisk::open(disk_path, Kind::Vhdx).map_err(|error| error.to_string())?;
    attached.attach(false).map_err(|error| error.to_string())?;
    let number = attached.device_number().map_err(|error| error.to_string())?;
    // diskpart wipes the disk it's given, so it's given this one only when it's certain to be the
    // file just made: a disk backed by a file, with nothing on it yet.
    let file_backed = disk::is_file_backed(number).map_err(|error| error.to_string())?;
    if !file_backed || !attached.volumes().map_err(|error| error.to_string())?.is_empty() {
        return Err(format!("Disk {number} is not the new, empty file. Nothing was written to it."));
    }

    let [system, windows] = free_letters()?;
    partition(number, system, windows)?;
    let windows_root = PathBuf::from(format!("{windows}:\\"));

    println!("Applying {} from {}. This takes a while.", options.edition, wim.display());
    run(Command::new(system_program("dism.exe"))
        .arg("/Apply-Image")
        .raw_arg(format!("/ImageFile:\"{}\"", wim.display()))
        .arg(format!("/Index:{index}"))
        .raw_arg(format!("/ApplyDir:{windows}:\\")))?;
    run(Command::new(system_program("bcdboot.exe")).arg(windows_root.join("Windows")).args([
        "/s",
        &format!("{system}:"),
        "/f",
        "UEFI",
    ]))?;

    let password = host::guid_string(&host::new_guid().map_err(|error| error.to_string())?).replace('-', "");
    write(&windows_root, guest::ANSWER_FILE_PATH, &guest::answer_file(&options.user, &password))?;
    write(&windows_root, guest::SETUP_COMPLETE_PATH, guest::SETUP_COMPLETE)?;
    write(&windows_root, guest::REPORT_PATH, guest::REPORT)?;
    if let Some(folder) = driver {
        copy_into(folder, &windows_root)?;
    }

    let account = account_file(disk_path);
    fs::write(&account, format!("user: {}\npassword: {password}\n", options.user))
        .map_err(|error| format!("{}: {error}", account.display()))?;
    println!("Done. The guest's administrator and password are in {}.", account.display());
    println!("Boot it with: guesthouse-hcs-boot boot --disk \"{}\"", disk_path.display());
    Ok(())
}

/// Where `prepare` keeps the guest's account, beside its disk.
fn account_file(disk: &Path) -> PathBuf {
    disk.with_extension("account.txt")
}

/// Two drive letters nothing uses, from the end of the alphabet, for the new disk's partitions
/// while they're written.
fn free_letters() -> Result<[char; 2], String> {
    let mut free = ('D'..='Z').rev().filter(|letter| !Path::new(&format!("{letter}:\\")).exists());
    match (free.next(), free.next()) {
        (Some(first), Some(second)) => Ok([first, second]),
        _ => Err("Two free drive letters are needed while the disk is written, and there aren't two.".into()),
    }
}

/// UEFI's partitions on the disk: the system partition on `system` and Windows' on `windows`.
fn partition(number: u32, system: char, windows: char) -> Result<(), String> {
    let script = format!(
        "select disk {number}\n\
         online disk noerr\n\
         attributes disk clear readonly noerr\n\
         clean\n\
         convert gpt\n\
         create partition efi size={SYSTEM_PARTITION_MB}\n\
         format quick fs=fat32 label=\"System\"\n\
         assign letter={system}\n\
         create partition msr size={RESERVED_PARTITION_MB}\n\
         create partition primary\n\
         format quick fs=ntfs label=\"Windows\"\n\
         assign letter={windows}\n\
         exit\n"
    );
    let path = std::env::temp_dir().join(format!("guesthouse-diskpart-{}.txt", std::process::id()));
    fs::write(&path, script).map_err(|error| format!("{}: {error}", path.display()))?;
    let result = run_captured(Command::new(system_program("diskpart.exe")).arg("/s").arg(&path));
    let _ = fs::remove_file(&path);
    result?;
    for letter in [system, windows] {
        if !Path::new(&format!("{letter}:\\")).exists() {
            return Err(format!("diskpart made no {letter}: drive."));
        }
    }
    Ok(())
}

fn write(root: &Path, relative: &str, contents: &str) -> Result<(), String> {
    let path = root.join(relative);
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    }
    fs::write(&path, contents).map_err(|error| format!("{}: {error}", path.display()))
}

fn run(command: &mut Command) -> Result<(), String> {
    let status = command.status().map_err(|error| format!("{}: {error}", program(command)))?;
    if status.success() { Ok(()) } else { Err(format!("{} failed ({status}).", program(command))) }
}

fn run_captured(command: &mut Command) -> Result<String, String> {
    let output = command.output().map_err(|error| format!("{}: {error}", program(command)))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "{} failed ({}):\n{text}{}",
            program(command),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn program(command: &Command) -> String {
    Path::new(command.get_program()).file_name().unwrap_or_default().to_string_lossy().into_owned()
}
