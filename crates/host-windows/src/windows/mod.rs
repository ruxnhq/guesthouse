//! The work that calls into Windows: making a machine's disks, and running the machine.

pub mod boot;
pub mod linux;
pub mod network;
pub mod prepare;
pub mod ssh;

use crate::events::{Event, Sink};
use crate::machine::{Guest, Machine};
use crate::record::{Record, files_disk};
use crate::screen;
use crate::settings::{BootSettings, GpuChoice, Share};
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
/// The NVAPI wrapper a Windows guest on an NVIDIA card gets, from the program's folder.
const NVAPI_WRAPPER: &str = r"guest\nvapi64.dll";

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
        nested_virtualization: settings.nested_virtualization,
        network,
        guest: record.guest,
        console_access: settings.console_access.clone(),
        share: settings.share.clone(),
    }
}

/// Where a Windows guest finds what the host shares: `report.ps1` reads this file, from the root of
/// its Windows volume, and links the share onto the desktop. With no share it isn't there.
pub const SHARE_FILE: &str = r"Guesthouse\share.txt";

/// Lets a stopped machine's disk grow to `size_gb`. The guest's own last partition is grown into
/// the room at its next start: `report.ps1` does it in Windows, and `gpu.sh` in Ubuntu.
pub fn grow_disk(disk: &Path, size_gb: u64) -> Result<(), String> {
    need_elevation()?;
    if crate::snapshots::made_on_another(disk)? {
        return Err("A disk with snapshots can't grow, as it's made on them. Delete them first.".into());
    }
    let bytes = size_gb * prepare::BYTES_PER_GB;
    let opened = VirtualDisk::open(disk, Kind::Vhdx).map_err(|error| error.to_string())?;
    let now = opened.size().map_err(|error| error.to_string())?;
    if bytes < now {
        return Err(format!(
            "The disk is {} GB already, and a disk only grows: a smaller one would cut into the guest's partitions.",
            now / prepare::BYTES_PER_GB
        ));
    }
    if bytes == now {
        return Ok(());
    }
    opened.grow(bytes).map_err(|error| error.to_string())
}

/// Snapshots' disks, through virtdisk.dll.
pub struct RealDisks;

impl crate::snapshots::Disks for RealDisks {
    fn differencing(&self, child: &Path, parent: &Path) -> Result<(), String> {
        guesthouse_hcs::disk::create_differencing_vhdx(child, parent).map_err(|error| error.to_string())
    }

    fn merge_into_parent(&self, child: &Path) -> Result<(), String> {
        let opened = VirtualDisk::open(child, Kind::Vhdx).map_err(|error| error.to_string())?;
        opened.merge_into_parent().map_err(|error| error.to_string())
    }
}

/// The largest a machine's disk may grow to now, in whole GB.
pub fn disk_size_gb(disk: &Path) -> Result<u64, String> {
    let opened = VirtualDisk::open(disk, Kind::Vhdx).map_err(|error| error.to_string())?;
    Ok(opened.size().map_err(|error| error.to_string())? / prepare::BYTES_PER_GB)
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
/// the card's driver, the guest's report, which also listens for the host asking it to shut down,
/// and the guest's screen when there's a remoteman installer to give it. `nvapi_wrapper` is whether
/// the guest has the NVAPI wrapper or NVIDIA's own NVAPI.
fn refresh_windows_disk(
    disk: &Path,
    card: Option<&Card>,
    nvapi_wrapper: bool,
    share: Option<&Share>,
    sink: &dyn Sink,
) -> Result<(), String> {
    let setup = screen_setup();
    // Attached at every boot, even with nothing else to give, as the share's file may have to go.
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
        copy_into(folder, &windows.root, nvapi_wrapper, sink)?;
    }
    // Written again at every boot, as guests made before it changed have an older one.
    let report = windows.root.join(crate::guest::REPORT_PATH);
    std::fs::write(&report, crate::guest::REPORT)
        .map_err(|error| format!("{}: {error}", report.display()))?;
    let share_file = windows.root.join(SHARE_FILE);
    match share {
        Some(_) => {
            let path =
                format!(r"{}\{}", guesthouse_hcs::schema::VIRTUAL_SMB_ROOT, crate::settings::SHARE_NAME);
            std::fs::write(&share_file, path + "\r\n")
                .map_err(|error| format!("{}: {error}", share_file.display()))?;
        }
        None => {
            let _ = std::fs::remove_file(&share_file);
        }
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
/// so it signs in by itself. The report, written before this, runs the script.
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
    sink.note(format!("The guest has remoteman for its screen. The viewer's folder is {}.", state.display()));
    Ok(())
}

/// The NVAPI wrapper for NVIDIA guests, in the program's guest folder. Not beside the program, where
/// it would be what the program itself loads for NVAPI.
fn nvapi_wrapper() -> Option<PathBuf> {
    let file = std::env::current_exe().ok()?.parent()?.join(NVAPI_WRAPPER);
    file.is_file().then_some(file)
}

/// With `nvapi_wrapper` off, NVIDIA's NVAPI goes where the wrapper was, as the placer replaces a
/// file of its own that isn't the driver's.
fn copy_into(folder: &Path, root: &Path, nvapi_wrapper: bool, sink: &dyn Sink) -> Result<(), String> {
    let copied = copy_driver(folder, root).map_err(|error| error.to_string())?;
    let name = folder.file_name().unwrap_or_default().to_string_lossy();
    sink.note(format!("Driver {name}: {} files copied, {} already there.", copied.copied, copied.kept));
    // The files the driver installs outside the driver store, such as its Vulkan and OpenCL loaders
    // and its maker's media libraries. Not having them leaves the card working for Direct3D but
    // not for programs that load those, so a failure is told and the boot goes on.
    let wrapper = if nvapi_wrapper { self::nvapi_wrapper() } else { None };
    match guesthouse_hcs::driver_files::place(folder, root, wrapper.as_deref()) {
        Ok(placed) => {
            sink.note(format!(
                "Driver {name}'s files outside the driver store: {} copied, {} already there.",
                placed.copied, placed.kept
            ));
            if !placed.left.is_empty() {
                let left: Vec<String> = placed.left.iter().map(|path| path.display().to_string()).collect();
                sink.note(format!("The guest's own copies were kept: {}.", left.join(", ")));
            }
        }
        Err(error) => {
            sink.problem(format!("Driver {name}'s files outside the driver store weren't placed: {error}"))
        }
    }
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
