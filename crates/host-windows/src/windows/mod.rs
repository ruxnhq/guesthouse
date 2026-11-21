//! The work that calls into Windows: making a machine's disks, and running the machine.

pub mod boot;
pub mod linux;
pub mod prepare;

use crate::events::{Event, Sink};
use crate::machine::{Guest, Machine};
use crate::record::{Record, files_disk};
use crate::screen;
use crate::settings::{BootSettings, GpuChoice};
use guesthouse_hcs::disk::{Kind, VirtualDisk, Volume};
use guesthouse_hcs::drivers::copy_driver;
use guesthouse_hcs::gpu::{self, Card};
use guesthouse_hcs::host;
use guesthouse_hcs::schema::{Attachment, AttachmentType};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// How long Windows may take to show the volumes of a disk it has just attached.
const VOLUMES_SECONDS: u64 = 15;

/// The machine `settings` asks for, on this disk, with this record and network card.
pub fn machine(
    settings: &BootSettings,
    disk: PathBuf,
    record: &Record,
    network: Option<guesthouse_hcs::schema::NetworkAdapter>,
) -> Machine {
    Machine {
        name: settings.name.clone(),
        more_disks: more_disks(&disk, record),
        disk,
        memory_mb: settings.memory_mb,
        processors: settings.processors,
        tpm: settings.tpm,
        network,
        guest: record.guest,
        console_access: settings.console_access.clone(),
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

/// The card asked for, or the one with the most memory of its own, or none.
pub fn chosen_card(choice: &GpuChoice) -> Result<Option<Card>, String> {
    let cards = gpu::cards().map_err(|error| error.to_string())?;
    let chosen = match choice {
        GpuChoice::None => return Ok(None),
        GpuChoice::Matching(text) => {
            let text = text.to_lowercase();
            cards.iter().find(|card| {
                card.name.to_lowercase().contains(&text) || card.instance_id.to_lowercase().contains(&text)
            })
        }
        GpuChoice::Default => gpu::default_card(&cards),
    };
    match chosen {
        Some(card) => Ok(Some(card.clone())),
        None if cards.is_empty() => {
            Err("No card on this host can be shared with a guest, so it can only boot with none.".into())
        }
        None => Err(format!(
            "No card matches the one asked for. These can be shared: {}",
            cards.iter().map(|card| card.name.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

pub(crate) fn need_elevation() -> Result<(), String> {
    if host::is_elevated() {
        Ok(())
    } else {
        Err("This needs an elevated prompt, as HCS, its network and attaching a disk all do. Open Terminal \
             with Run as administrator and run it again."
            .into())
    }
}

/// HCS and DISM resolve no relative paths, so every path they're given is made whole first.
pub fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The volumes of an attached disk, once Windows shows any.
pub(crate) fn volumes(disk: &VirtualDisk) -> Result<Vec<Volume>, String> {
    let started = Instant::now();
    loop {
        let found = disk.volumes().map_err(|error| error.to_string())?;
        if !found.is_empty() || started.elapsed() > Duration::from_secs(VOLUMES_SECONDS) {
            return Ok(found);
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Brings a Windows guest's disk up to date before it boots, which no machine may have open then:
/// the card's driver, and the guest's screen when there's a remoteman installer to give it.
fn refresh_windows_disk(disk: &Path, card: Option<&Card>, sink: &dyn Sink) -> Result<(), String> {
    let setup = screen_setup();
    if card.is_none() && setup.is_none() {
        return Ok(());
    }
    let mut attached = VirtualDisk::open(disk, Kind::Vhdx).map_err(|error| error.to_string())?;
    attached.attach(false).map_err(|error| error.to_string())?;
    let windows = volumes(&attached)?
        .into_iter()
        .find(|volume| volume.root.join(r"Windows\System32").is_dir())
        .ok_or_else(|| format!("{} has no volume with Windows on it.", disk.display()))?;
    if let Some(card) = card {
        let folder = card.driver_folder.as_ref().ok_or_else(|| {
            format!(
                "The folder of {}'s driver could not be found, so the guest can't be given it.",
                card.name
            )
        })?;
        copy_into(folder, &windows.root, sink)?;
    }
    match setup {
        Some(setup) => give_screen(disk, &windows.root, &setup, card, sink),
        None => Ok(()),
    }
}

/// remoteman's host's installer, beside the program that boots the machine, if it's there.
fn screen_setup() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok()?.parent()?.join(screen::SETUP);
    beside.is_file().then_some(beside)
}

/// Copies a file unless the copy there has its size and time already. Gives whether it copied.
fn copy_if_changed(from: &Path, to: &Path) -> std::io::Result<bool> {
    let given = std::fs::metadata(from)?;
    let same = std::fs::metadata(to)
        .is_ok_and(|kept| kept.len() == given.len() && kept.modified().ok() == given.modified().ok());
    if !same {
        std::fs::copy(from, to)?;
    }
    Ok(!same)
}

/// Puts what the guest needs for its screen in its screen folder: remoteman's installer, the script
/// that runs it, the viewer as the one the guest trusts, the encoder for its card, and its account,
/// so it signs in by itself. The report, which runs the script, is written again too, as guests
/// made before there was a screen have one that doesn't.
fn give_screen(
    disk: &Path,
    root: &Path,
    setup: &Path,
    card: Option<&Card>,
    sink: &dyn Sink,
) -> Result<(), String> {
    let folder = root.join(screen::GUEST_FOLDER);
    let write = |name: &str, text: &str| {
        let path = folder.join(name);
        std::fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))
    };
    std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let state = screen::state_beside(disk);
    let viewer = screen::viewer_identity(&state)?;
    write("trusted", &screen::trust_list(&viewer))?;
    write("encoder.txt", card.map_or("auto", |card| screen::encoder_for(&card.instance_id)))?;
    write("screen.ps1", screen::SCRIPT)?;
    let account = disk.with_extension("account.txt");
    if let Ok(text) = std::fs::read_to_string(&account) {
        write("logon.txt", &text)?;
    }
    let guest_setup = folder.join(screen::GUEST_SETUP);
    let copied = copy_if_changed(setup, &guest_setup)
        .map_err(|error| format!("{}: {error}", guest_setup.display()))?;
    if copied {
        sink.note("remoteman's host's installer given to the guest.");
    }
    // A guest given remoteman's files before its installer could run unattended has them still.
    let _ = std::fs::remove_dir_all(folder.join("remoteman-host"));
    let report = root.join(crate::guest::REPORT_PATH);
    std::fs::write(&report, crate::guest::REPORT)
        .map_err(|error| format!("{}: {error}", report.display()))?;
    sink.note(format!("The guest has remoteman for its screen. The viewer's folder is {}.", state.display()));
    Ok(())
}

fn copy_into(folder: &Path, root: &Path, sink: &dyn Sink) -> Result<(), String> {
    let copied = copy_driver(folder, root).map_err(|error| error.to_string())?;
    let name = folder.file_name().unwrap_or_default().to_string_lossy();
    sink.note(format!("Driver {name}: {} files copied, {} already there.", copied.copied, copied.kept));
    Ok(())
}

/// How long Windows may take to let go of a machine's disk once the machine has stopped.
const RELEASE_SECONDS: u64 = 30;

/// Prints the report a Windows guest wrote on its own disk, which says what it saw of its card
/// even when its serial port said nothing.
fn print_guest_report(disk: &Path, sink: &dyn Sink) {
    let started = Instant::now();
    let attached = loop {
        let opened =
            VirtualDisk::open(disk, Kind::Vhdx).and_then(|mut opened| opened.attach(false).map(|()| opened));
        match opened {
            Ok(opened) => break opened,
            Err(_) if started.elapsed() < Duration::from_secs(RELEASE_SECONDS) => {
                thread::sleep(Duration::from_secs(1))
            }
            Err(error) => return sink.problem(format!("The guest's report couldn't be read: {error}")),
        }
    };
    let report = volumes(&attached)
        .unwrap_or_default()
        .into_iter()
        .map(|volume| volume.root.join(crate::guest::REPORT_FILE))
        .find(|path| path.is_file());
    match report.map(std::fs::read) {
        Some(Ok(bytes)) => {
            sink.note("The guest's report, from its disk:");
            for line in bytes.split(|&byte| byte == b'\n') {
                sink.event(Event::Report(crate::printable::printable(line)));
            }
        }
        Some(Err(error)) => sink.problem(format!("The guest's report couldn't be read: {error}")),
        None => sink.note(
            "The guest wrote no report on its disk, so it never got as far as its first sign-in screen.",
        ),
    }
}

/// A program of Windows' own, by its full path, so nothing else on PATH can stand in for it.
fn system_program(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}
