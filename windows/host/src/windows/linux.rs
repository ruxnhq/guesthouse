//! A Linux guest: Ubuntu, installed by Ubuntu's own installer on the machine's first boot, with no
//! questions and no screen. Beside the guest's disk goes a small FAT disk, its files disk, which does
//! three jobs. Its UEFI fallback is the installer's own Shim and GRUB, which start the installer from
//! the ISO with a menu of ours. Its label, CIDATA, makes it the installer's answer file. And once the
//! guest runs, it holds the host's GPU files, which the guest mounts at /usr/lib/wsl as WSL has them:
//! the libraries from WSL and from the GPU's driver, and the driver's folder from the driver store.

use super::prepare::{BYTES_PER_GB, attach_new, diskpart, free_letters, new_password, write, write_account};
use super::volumes;
use crate::events::Sink;
use crate::guest::linux::{
    ACCOUNT, BOOT_FILES, FILES, INSTALLER_INITRD, LABEL, WSL_DRIVERS, WSL_LIB, account, unix_text,
};
use crate::machine::Guest;
use crate::record::{Record, files_disk};
use crate::settings::{PrepareSettings, Share};
use guesthouse_account_core::common::linux_name_problem;
use guesthouse_hcs::disk::{self, Kind, VirtualDisk};
use guesthouse_hcs::drivers::copy_folder;
use guesthouse_hcs::gpu::Card;
use std::fs;
use std::path::{Path, PathBuf};

/// The file on the files disk that names the shares for gpu.sh, a line each, as `NAME PORT ro` or
/// `NAME PORT rw`.
const SHARE: &str = r"guesthouse\share";

fn share_lines(shares: &[Share]) -> String {
    shares
        .iter()
        .map(|share| {
            let mode = if share.read_only { "ro" } else { "rw" };
            format!("{} {} {mode}\n", share.name(), guesthouse_hcs::schema::PLAN9_PORT)
        })
        .collect()
}

/// The folder on the files disk that holds the guest's own scripts.
const GUEST_SCRIPTS: &str = "guesthouse\\";
/// Room on the files disk beyond the GPU files, for a driver update that's larger.
const HEADROOM_BYTES: u64 = BYTES_PER_GB;
const SMALLEST_FILES_DISK_GB: u64 = 2;
/// diskpart formats FAT32 up to this size.
const LARGEST_FILES_DISK_GB: u64 = 32;
/// The library nvidia-smi and CUDA load beside NVIDIA's. It comes with WSL, not with the driver.
const DXCORE: &str = "libdxcore.so";

pub fn prepare(
    settings: &PrepareSettings,
    disk_path: &Path,
    iso: &Path,
    iso_root: &Path,
    card: Option<&Card>,
    sink: &dyn Sink,
) -> Result<(), String> {
    if let Some(problem) = linux_name_problem(&settings.user) {
        return Err(problem);
    }
    for needed in [INSTALLER_INITRD, BOOT_FILES[0].0, BOOT_FILES[1].0] {
        if !iso_root.join(needed).is_file() {
            return Err(format!("{} has no {needed}, which the install starts from.", iso.display()));
        }
    }
    let files = files_disk(disk_path);
    if files.exists() {
        return Err(format!("{} is there already, and prepare never writes over a disk.", files.display()));
    }
    let made = make(settings, disk_path, &files, iso, iso_root, card, sink);
    if made.is_err() {
        // Half-made disks would only be refused by the next run, so they go.
        let _ = fs::remove_file(disk_path);
        let _ = fs::remove_file(&files);
    }
    made
}

fn make(
    settings: &PrepareSettings,
    disk_path: &Path,
    files: &Path,
    iso: &Path,
    iso_root: &Path,
    card: Option<&Card>,
    sink: &dyn Sink,
) -> Result<(), String> {
    sink.note(format!(
        "Making {} ({} GB at most), for Ubuntu's installer to fill.",
        disk_path.display(),
        settings.size_gb
    ));
    disk::create_vhdx(disk_path, settings.size_gb * BYTES_PER_GB).map_err(|error| error.to_string())?;

    let size = files_disk_bytes(card);
    sink.note(format!("Making its files disk, {} ({} GB).", files.display(), size / BYTES_PER_GB));
    disk::create_vhdx(files, size).map_err(|error| error.to_string())?;
    let (_attached, number) = attach_new(files)?;
    let [letter] = free_letters()?;
    diskpart(
        number,
        &format!("create partition efi\nformat quick fs=fat32 label=\"{LABEL}\"\nassign letter={letter}\n"),
        &[letter],
    )?;
    let root = PathBuf::from(format!("{letter}:\\"));

    for (from, to) in BOOT_FILES {
        let source = iso_root.join(from);
        // MokManager is only needed to enrol keys, which this never does, so an ISO without it is fine.
        if source.is_file() {
            let target = root.join(to);
            fs::create_dir_all(target.parent().expect("a boot file is in a folder"))
                .and_then(|()| fs::copy(&source, &target))
                .map_err(|error| format!("{}: {error}", target.display()))?;
        }
    }
    for (path, text) in FILES {
        write(&root, path, &unix_text(text))?;
    }
    let password = new_password()?;
    write(&root, ACCOUNT, &account(&settings.user, &password))?;
    refresh(&root, card, &[], sink)?;

    Record { guest: Guest::Linux, installer: Some(iso.to_owned()), ..Record::default() }.save(disk_path)?;
    write_account(disk_path, &settings.user, &password, sink)?;
    sink.note(
        "It's ready. The first boot installs Ubuntu, which takes a while, and then the machine turns \
         itself off. The boot after that starts it.",
    );
    Ok(())
}

/// Brings the host's GPU files on the files disk up to date with the host's, while no machine has
/// the disk open.
pub fn refresh_disk(
    files: &Path,
    card: Option<&Card>,
    shares: &[Share],
    sink: &dyn Sink,
) -> Result<(), String> {
    let mut attached = VirtualDisk::open(files, Kind::Vhdx).map_err(|error| error.to_string())?;
    attached.attach(false).map_err(|error| error.to_string())?;
    let volume = volumes(&attached)?
        .into_iter()
        .next()
        .ok_or_else(|| format!("{} has no volume on it.", files.display()))?;
    refresh(&volume.root, card, shares, sink)
}

/// The GPU files on the files disk at `root`. WSL's own libraries go first and the driver's on top,
/// as WSL lays one over the other. Only the card's driver folder is kept, so an old driver's goes.
fn refresh(root: &Path, card: Option<&Card>, shares: &[Share], sink: &dyn Sink) -> Result<(), String> {
    // The guest runs its scripts from here at every start, so these are always this build's.
    for (path, text) in FILES.iter().filter(|(path, _)| path.starts_with(GUEST_SCRIPTS)) {
        write(root, path, &unix_text(text))?;
    }
    // gpu.sh mounts the shares it names, and unmounts those it doesn't.
    if shares.is_empty() {
        let _ = fs::remove_file(root.join(SHARE));
    } else {
        write(root, SHARE, &share_lines(shares))?;
    }
    let lib = root.join(WSL_LIB);
    for source in library_folders().into_iter().filter(|folder| folder.is_dir()) {
        let copied = copy_folder(&source, &lib).map_err(|error| error.to_string())?;
        sink.note(format!(
            "{}: {} files copied, {} already there.",
            source.display(),
            copied.copied,
            copied.kept
        ));
    }
    if !lib.join(DXCORE).is_file() {
        sink.problem(format!(
            "This host has no {DXCORE}, which comes with WSL, so nvidia-smi and CUDA won't run in the \
             guest. Installing WSL gives it."
        ));
    }

    let drivers = root.join(WSL_DRIVERS);
    fs::create_dir_all(&drivers).map_err(|error| format!("{}: {error}", drivers.display()))?;
    let Some(folder) = card.and_then(|card| card.driver_folder.as_deref()) else { return Ok(()) };
    let name = folder.file_name().expect("a driver folder has a name");
    let copied = copy_folder(folder, &drivers.join(name)).map_err(|error| error.to_string())?;
    sink.note(format!(
        "Driver {}: {} files copied, {} already there.",
        name.to_string_lossy(),
        copied.copied,
        copied.kept
    ));
    for entry in fs::read_dir(&drivers).map_err(|error| format!("{}: {error}", drivers.display()))?.flatten()
    {
        if entry.file_name() != name && entry.path().is_dir() {
            fs::remove_dir_all(entry.path())
                .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Where the host keeps the libraries a guest's /usr/lib/wsl/lib needs: WSL's own (DXCore and
/// D3D12), then those the GPU's driver puts there for WSL.
fn library_folders() -> [PathBuf; 2] {
    let programs = std::env::var_os("ProgramW6432")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .unwrap_or_else(|| r"C:\Program Files".into());
    let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    [PathBuf::from(programs).join(r"WSL\lib"), PathBuf::from(windows).join(r"System32\lxss\lib")]
}

/// Room for the GPU files with some to spare, in whole GB, within what diskpart can format as FAT32.
fn files_disk_bytes(card: Option<&Card>) -> u64 {
    let driver = card.and_then(|card| card.driver_folder.as_deref());
    let needed: u64 = library_folders()
        .iter()
        .chain(driver.map(Path::to_path_buf).iter())
        .map(|folder| size_of_folder(folder))
        .sum();
    let gb = (needed + HEADROOM_BYTES).div_ceil(BYTES_PER_GB);
    gb.clamp(SMALLEST_FILES_DISK_GB, LARGEST_FILES_DISK_GB) * BYTES_PER_GB
}

fn size_of_folder(folder: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(folder) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size_of_folder(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |metadata| metadata.len()),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_disk_is_never_smaller_or_larger_than_fat32_allows() {
        let size = files_disk_bytes(None);
        assert!(
            (SMALLEST_FILES_DISK_GB * BYTES_PER_GB..=LARGEST_FILES_DISK_GB * BYTES_PER_GB).contains(&size)
        );
        assert_eq!(size % BYTES_PER_GB, 0);
    }
}

#[cfg(test)]
mod host_tests {
    use super::*;
    use crate::events::Kept;

    /// Reads this host's own WSL and driver libraries, so it's run by hand only.
    #[test]
    #[ignore = "copies this host's WSL libraries"]
    fn the_files_disk_gets_wsl_and_the_drivers_libraries_with_dxcore() {
        let root = std::env::temp_dir().join(format!("gh-files-{}", std::process::id()));
        refresh(&root, None, &[], &Kept::default()).unwrap();
        let lib = root.join(WSL_LIB);
        assert!(lib.join(DXCORE).is_file(), "WSL's DXCore");
        assert!(lib.join("libcuda.so.1").is_file(), "the driver's CUDA");
        assert!(lib.join("nvidia-smi").is_file(), "the driver's nvidia-smi");
        assert!(root.join(WSL_DRIVERS).is_dir());
        refresh(&root, None, &[], &Kept::default()).unwrap();
        let _ = fs::remove_dir_all(root);
    }
}
