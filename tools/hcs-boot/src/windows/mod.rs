//! The commands themselves, which call into Windows.

mod boot;
mod linux;
mod prepare;

use crate::machine::{Guest, Machine};
use crate::record::{Record, files_disk};
use crate::{Command, Options};
use guesthouse_hcs::disk::{Kind, VirtualDisk, Volume};
use guesthouse_hcs::drivers::copy_driver;
use guesthouse_hcs::gpu::{self, Card};
use guesthouse_hcs::host;
use guesthouse_hcs::schema::{Attachment, AttachmentType, Gpu, ModifySettingRequest};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// How long Windows may take to show the volumes of a disk it has just attached.
const VOLUMES_SECONDS: u64 = 15;

pub fn run(options: &Options) -> Result<(), String> {
    match options.command {
        Command::Gpus => list_cards(),
        Command::Document => print_document(options),
        Command::Prepare => prepare::prepare(options),
        Command::Boot => boot::boot(options),
    }
}

fn list_cards() -> Result<(), String> {
    let cards = gpu::cards().map_err(|error| error.to_string())?;
    if cards.is_empty() {
        println!("No card on this host can be shared with a guest.");
    }
    for (number, card) in cards.iter().enumerate() {
        println!("{number}: {}", card.name);
        println!("   device:    {}", card.instance_id);
        println!("   interface: {}", card.interface_path);
        match &card.driver_folder {
            Some(folder) => println!("   driver:    {}", folder.display()),
            None => println!("   driver:    (its folder could not be found)"),
        }
    }
    Ok(())
}

fn print_document(options: &Options) -> Result<(), String> {
    let disk = absolute(options.disk.as_deref().expect("parse asks for a disk"))?;
    let record = Record::load(&disk)?;
    let machine = machine(options, disk, &record, None);
    println!("{}", serde_json::to_string_pretty(&machine.document()).expect("a document always serialises"));
    if options.network {
        eprintln!("(boot adds a network card here, plugged into an endpoint it makes then.)");
    }
    if let Some(card) = chosen_card(options)? {
        println!("\nThen, once the machine runs:");
        let change = ModifySettingRequest::gpu(&Gpu::list([card.interface_path.as_str()]));
        println!("{}", serde_json::to_string_pretty(&change).expect("a change always serialises"));
    }
    Ok(())
}

fn machine(
    options: &Options,
    disk: PathBuf,
    record: &Record,
    network: Option<guesthouse_hcs::schema::NetworkAdapter>,
) -> Machine {
    Machine {
        name: options.name.clone(),
        more_disks: more_disks(&disk, record),
        disk,
        memory_mb: options.memory_mb,
        processors: options.processors,
        tpm: options.tpm,
        network,
        guest: record.guest,
    }
}

/// A Linux guest's files disk, and its installer until it has installed.
fn more_disks(disk: &Path, record: &Record) -> Vec<Attachment> {
    if record.guest != Guest::Linux {
        return Vec::new();
    }
    let attach = |kind, path: &Path| Attachment {
        r#type: kind,
        path: path.to_string_lossy().into_owned(),
        read_only: None,
    };
    let mut disks = vec![attach(AttachmentType::VirtualDisk, &files_disk(disk))];
    if let Some(installer) = record.installer.as_deref().filter(|_| !record.installed) {
        disks.push(attach(AttachmentType::Iso, installer));
    }
    disks
}

/// The card asked for, or the first NVIDIA card, or the first card; none with `--no-gpu`.
fn chosen_card(options: &Options) -> Result<Option<Card>, String> {
    if options.no_gpu {
        return Ok(None);
    }
    let cards = gpu::cards().map_err(|error| error.to_string())?;
    let chosen = match &options.gpu {
        Some(text) => {
            let text = text.to_lowercase();
            cards.iter().find(|card| {
                card.name.to_lowercase().contains(&text) || card.instance_id.to_lowercase().contains(&text)
            })
        }
        None => cards.iter().find(|card| card.is_nvidia()).or(cards.first()),
    };
    match chosen {
        Some(card) => Ok(Some(card.clone())),
        None if cards.is_empty() => {
            Err("No card on this host can be shared with a guest. Use --no-gpu to boot without one.".into())
        }
        None => Err(format!(
            "No card matches --gpu. These can be shared: {}",
            cards.iter().map(|card| card.name.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn need_elevation() -> Result<(), String> {
    if host::is_elevated() {
        Ok(())
    } else {
        Err("This needs an elevated prompt, as HCS, its network and attaching a disk all do. Open Terminal \
             with Run as administrator and run it again."
            .into())
    }
}

/// HCS and DISM resolve no relative paths, so every path they're given is made whole first.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The volumes of an attached disk, once Windows shows any.
fn volumes(disk: &VirtualDisk) -> Result<Vec<Volume>, String> {
    let started = Instant::now();
    loop {
        let found = disk.volumes().map_err(|error| error.to_string())?;
        if !found.is_empty() || started.elapsed() > Duration::from_secs(VOLUMES_SECONDS) {
            return Ok(found);
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Copies the card's driver onto the Windows volume of the disk, which no machine may have open.
fn copy_driver_to_disk(disk: &Path, card: &Card) -> Result<(), String> {
    let folder = card.driver_folder.as_ref().ok_or_else(|| {
        format!("The folder of {}'s driver could not be found, so the guest can't be given it.", card.name)
    })?;
    let mut attached = VirtualDisk::open(disk, Kind::Vhdx).map_err(|error| error.to_string())?;
    attached.attach(false).map_err(|error| error.to_string())?;
    let windows = volumes(&attached)?
        .into_iter()
        .find(|volume| volume.root.join(r"Windows\System32").is_dir())
        .ok_or_else(|| format!("{} has no volume with Windows on it.", disk.display()))?;
    copy_into(folder, &windows.root)
}

fn copy_into(folder: &Path, root: &Path) -> Result<(), String> {
    let copied = copy_driver(folder, root).map_err(|error| error.to_string())?;
    let name = folder.file_name().unwrap_or_default().to_string_lossy();
    println!("Driver {name}: {} files copied, {} already there.", copied.copied, copied.kept);
    Ok(())
}

/// A program of Windows' own, by its full path, so nothing else on PATH can stand in for it.
fn system_program(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}
