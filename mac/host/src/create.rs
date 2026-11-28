// SPDX-License-Identifier: GPL-2.0-or-later
//! Making a machine from files the person already has, and renaming one.
//!
//! A new machine is a folder in the machines folder holding clones (or, off APFS, copies) of
//! the disk image and firmware files, and a `machine.json` in the format `store.rs` reads.
//! Everything is checked first, and every problem comes back as a plain sentence, so the sheet
//! can show them all at once instead of one at a time.

use crate::launch::socket_problem;
use crate::store::{self, MACHINE_FILE};
use guesthouse_basics::json_file::write_json;
use serde::Deserialize;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Windows 11 asks for 4 GB; below 2 GB it does not get through setup or start reliably.
pub const MIN_MEMORY_MB: u64 = 2048;

/// What macOS itself keeps, so a guest is never given all of the Mac's memory.
pub const HOST_RESERVE_MB: u64 = 2048;

/// Long enough to be told apart in Finder, short enough that sockets under it fit.
const MAX_ID_CHARS: usize = 16;

const MAX_NAME_CHARS: usize = 60;

/// The file names inside a machine's folder. Fixed, so two machines never depend on the names
/// of the files they were made from.
const DISK_FILE: &str = "disk.img";
const CODE_FILE: &str = "code.fd";
const VARS_FILE: &str = "vars.fd";
const ROM_FILE: &str = "option.rom";

/// The first bytes of a qcow2 image. The machine's command line says the disk is raw, so a
/// qcow2 would show Windows a disk of garbage.
const QCOW2_MAGIC: [u8; 4] = *b"QFI\xfb";

/// What the sheet sends: the person's choices, and the Mac they are made on.
#[derive(Deserialize, Debug, Clone)]
pub struct NewMachine {
    pub name: String,
    pub cores: u32,
    pub memory_mb: u64,
    pub disk: PathBuf,
    pub firmware_code: PathBuf,
    pub firmware_vars: PathBuf,
    #[serde(default)]
    pub option_rom: Option<PathBuf>,
    /// The Mac's memory and processors, for the limits.
    pub host_memory_mb: u64,
    pub host_cores: u32,
}

/// What is wrong with a processor count and a memory size on a Mac of this size, in plain words.
pub fn hardware_problems(cores: u32, memory_mb: u64, host_cores: u32, host_memory_mb: u64) -> Vec<String> {
    let mut found = Vec::new();
    if cores == 0 || cores > host_cores {
        found.push(format!("Processors: choose from 1 to {}, the number this Mac has.", host_cores));
    }
    let most = host_memory_mb.saturating_sub(HOST_RESERVE_MB);
    if memory_mb < MIN_MEMORY_MB {
        found.push(format!("Memory: at least {} GB, or Windows will not run well.", MIN_MEMORY_MB / 1024));
    } else if memory_mb > most {
        found.push(format!(
            "Memory: this Mac has {} GB, and {} GB stay with macOS, so at most {} GB.",
            host_memory_mb / 1024,
            HOST_RESERVE_MB / 1024,
            most / 1024
        ));
    }
    found
}

/// Every reason `request` cannot be made into a machine in `machines`, in plain words. Empty
/// means it can.
pub fn problems(machines: &Path, request: &NewMachine) -> Vec<String> {
    let mut found = Vec::new();
    found.extend(name_problem(machines, None, &request.name));
    found.extend(hardware_problems(
        request.cores,
        request.memory_mb,
        request.host_cores,
        request.host_memory_mb,
    ));
    found.extend(file_problem("The disk image", &request.disk));
    if request.disk.is_file() && is_qcow2(&request.disk) {
        found.push(
            "The disk image is in qcow2 format. Guesthouse starts raw images (usually .img); convert it with qemu-img first."
                .into(),
        );
    }
    found.extend(file_problem("The firmware code file", &request.firmware_code));
    found.extend(file_problem("The firmware variables file", &request.firmware_vars));
    if let Some(rom) = &request.option_rom {
        found.extend(file_problem("The option ROM", rom));
    }
    if let Err(problem) = pick_id(machines, &request.name) {
        found.push(problem);
    }
    found
}

/// Why `name` cannot be a machine's name: empty, too long, or another machine has it.
/// `own_id` is the machine being renamed, which may keep its own name.
pub fn name_problem(machines: &Path, own_id: Option<&str>, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Give the machine a name.".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Some(format!("The name is longer than {MAX_NAME_CHARS} characters."));
    }
    let taken = store::list(machines)
        .map(|listing| listing.machines.iter().any(|m| m.name == name && Some(m.id.as_str()) != own_id))
        .unwrap_or(false);
    taken.then(|| format!("A machine called {name} already exists."))
}

fn file_problem(what: &str, path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return Some(format!("{what} is not chosen."));
    }
    if !path.is_file() {
        return Some(format!("{what} is not there: {}", path.display()));
    }
    None
}

fn is_qcow2(path: &Path) -> bool {
    let mut start = [0u8; 4];
    fs::File::open(path).and_then(|mut file| file.read_exact(&mut start)).is_ok_and(|()| start == QCOW2_MAGIC)
}

/// A folder name for the machine: its name in lowercase letters, digits and dashes, cut short
/// enough that the sockets in its run folder fit, and not taken. Fails only when the Guesthouse
/// folder is in a place whose own path leaves no room.
pub(crate) fn pick_id(machines: &Path, name: &str) -> Result<String, String> {
    let mut slug = String::new();
    for c in name.trim().chars() {
        let c = if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' };
        if c != '-' || !(slug.is_empty() || slug.ends_with('-')) {
            slug.push(c);
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "machine" } else { slug };
    let base: String = slug.chars().take(MAX_ID_CHARS).collect();

    for number in 1.. {
        let suffix = if number == 1 { String::new() } else { format!("-{number}") };
        let mut id = format!("{base}{suffix}");
        // Shorten the part before the number until the sockets fit.
        while !sockets_fit(machines, &id) && id.len() > suffix.len() + 1 {
            id = format!("{}{suffix}", &base[..id.len() - suffix.len() - 1]);
        }
        if !sockets_fit(machines, &id) {
            return Err(
                "The Guesthouse folder is in a place whose path is too long for the machine's sockets, so no machine can be made there."
                    .into(),
            );
        }
        if !machines.join(&id).exists() {
            return Ok(id);
        }
    }
    unreachable!("an unused number always turns up")
}

fn sockets_fit(machines: &Path, id: &str) -> bool {
    let run = store::run_folder(machines, id);
    // The longest name of the sockets a machine may be given, guarded or not.
    socket_problem(&[&run.join("qmp.sock"), &run.join("input.sock"), &run.join("control.sock")]).is_none()
}

/// Makes the machine and returns its id, or every problem found. Nothing is left behind when it
/// fails: a folder this call made is removed again.
pub fn create(machines: &Path, request: &NewMachine) -> Result<String, Vec<String>> {
    let found = problems(machines, request);
    if !found.is_empty() {
        return Err(found);
    }
    let id = pick_id(machines, &request.name).map_err(|problem| vec![problem])?;
    let folder = machines.join(&id);
    fs::create_dir_all(machines).map_err(|e| vec![format!("Cannot make {}: {e}", machines.display())])?;
    // `create_dir` fails if the folder appeared since the check, so two sheets never share one.
    fs::create_dir(&folder).map_err(|e| vec![format!("Cannot make {}: {e}", folder.display())])?;
    if let Err(problem) = fill(&folder, request) {
        let _ = fs::remove_dir_all(&folder);
        return Err(vec![problem]);
    }
    Ok(id)
}

fn fill(folder: &Path, request: &NewMachine) -> Result<(), String> {
    let mut files = vec![
        (&request.disk, DISK_FILE),
        (&request.firmware_code, CODE_FILE),
        (&request.firmware_vars, VARS_FILE),
    ];
    if let Some(rom) = &request.option_rom {
        files.push((rom, ROM_FILE));
    }
    for (from, name) in files {
        clone_or_copy(from, &folder.join(name))
            .map_err(|e| format!("Cannot copy {} into the machine's folder: {e}", from.display()))?;
    }
    let mut settings = serde_json::json!({
        "name": request.name.trim(),
        "kind": "windows",
        "cores": request.cores,
        "memory_mb": request.memory_mb,
        "disk": DISK_FILE,
        "firmware_code": CODE_FILE,
        "firmware_vars": VARS_FILE,
        // New machines use the protected graphics process, as every machine is meant to.
        "graphics": "guarded",
    });
    if request.option_rom.is_some() {
        settings["option_rom"] = ROM_FILE.into();
    }
    write_settings(folder, &settings).map_err(|e| format!("Cannot write {MACHINE_FILE}: {e}"))
}

/// An APFS clone: instant, and no space used until one side changes. A copy where the folder
/// is on another volume or a file system without clones.
fn clone_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        if let (Ok(src), Ok(dst)) =
            (CString::new(from.as_os_str().as_bytes()), CString::new(to.as_os_str().as_bytes()))
        {
            // SAFETY: both are nul-terminated paths that outlive the call.
            if unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) } == 0 {
                return Ok(());
            }
        }
    }
    fs::copy(from, to).map(|_| ())
}

/// Writes `machine.json` in one step, so a crash never leaves half of one.
fn write_settings(folder: &Path, settings: &serde_json::Value) -> io::Result<()> {
    write_json(&folder.join(MACHINE_FILE), settings)
}

/// Gives machine `id` another name, keeping every other setting as it is.
pub fn rename(machines: &Path, id: &str, name: &str) -> Result<(), String> {
    if let Some(problem) = name_problem(machines, Some(id), name) {
        return Err(problem);
    }
    let folder = machines.join(store::find(machines, id)?.id);
    let path = folder.join(MACHINE_FILE);
    let text = fs::read_to_string(&path).map_err(|e| format!("Cannot read {MACHINE_FILE}: {e}"))?;
    let mut settings: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{MACHINE_FILE} is not valid: {e}"))?;
    settings["name"] = name.trim().into();
    write_settings(&folder, &settings).map_err(|e| format!("Cannot write {MACHINE_FILE}: {e}"))
}

/// Moves a machine's graphics device: `guarded` or `qemu`. Every other setting stays.
pub fn set_graphics(machines: &Path, id: &str, graphics: &str) -> Result<(), String> {
    let graphics = crate::launch::Graphics::from_name(graphics)
        .ok_or_else(|| format!("{graphics} is not a graphics choice."))?;
    let folder = machines.join(store::find(machines, id)?.id);
    let path = folder.join(MACHINE_FILE);
    let text = fs::read_to_string(&path).map_err(|e| format!("Cannot read {MACHINE_FILE}: {e}"))?;
    let mut settings: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{MACHINE_FILE} is not valid: {e}"))?;
    settings["graphics"] = graphics.name().into();
    write_settings(&folder, &settings).map_err(|e| format!("Cannot write {MACHINE_FILE}: {e}"))
}

/// Gives a machine another processor count and memory size, checked as a new machine's are.
/// The machine must not be running: its QEMU has them already, and the app only asks of a
/// stopped one. Every other setting in `machine.json` stays as it is.
pub fn set_hardware(
    machines: &Path,
    id: &str,
    cores: u32,
    memory_mb: u64,
    host_cores: u32,
    host_memory_mb: u64,
) -> Result<(), Vec<String>> {
    let problems = hardware_problems(cores, memory_mb, host_cores, host_memory_mb);
    if !problems.is_empty() {
        return Err(problems);
    }
    let folder = machines.join(store::find(machines, id).map_err(|e| vec![e])?.id);
    let path = folder.join(MACHINE_FILE);
    let text = fs::read_to_string(&path).map_err(|e| vec![format!("Cannot read {MACHINE_FILE}: {e}")])?;
    let mut settings: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| vec![format!("{MACHINE_FILE} is not valid: {e}")])?;
    settings["cores"] = cores.into();
    settings["memory_mb"] = memory_mb.into();
    write_settings(&folder, &settings).map_err(|e| vec![format!("Cannot write {MACHINE_FILE}: {e}")])
}

#[cfg(test)]
mod tests;
