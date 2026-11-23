//! The library of machines: making one, importing the first, and taking them away.

use crate::account;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::gpu_support::{NO_GPU_PART, gpu_ready};
use crate::host::{free_gib, host_ram_limit, plain_ram_limit};
use crate::kept_logs;
use crate::machine::{Kind, MAX_REFRESH_HZ, MIN_REFRESH_HZ, Machine, load_all, sorted_names};
use crate::monitor::{Command, failed};
use crate::network;
use crate::remote_access::{Access, access_problem, write_access};
use crate::settings::with_suffix;
use crate::setup::guest_driver::guest_driver;
use crate::snapshots::{list_snapshots, snapshots_folder};
use crate::tasks::{State, slot_of, status};
use crate::writable_copy::copy_writable;
use serde_json::json;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process;

/// The entry made for the VM from before the library. The user can remove it, and this name in a
/// mark file then keeps it from being imported again.
pub const EXISTING_ID: &str = "windows-11";
const REMOVED_MARK: &str = ".existing-removed";
pub const MIN_WINDOWS_DISK_GIB: u32 = 40;
pub const MIN_PLAIN_DISK_GIB: u32 = 1;
pub const MAX_DISK_GIB: u32 = 16384;
/// A finished install, and the install disc beside it.
pub const INSTALL_ROOM_GIB: u64 = 40;
pub const NO_DRIVER_FILES: &str = "Windows needs NVIDIA's driver first. Run Setup Again from the menu.";
const SHUT_DOWN_FIRST: &str = "Shut the VM down first.";
const NO_FIRMWARE: &str = "QEMU's UEFI firmware isn't installed. Install ovmf, which Fedora calls edk2-ovmf.";
/// The file a disk is called in its machine's folder.
pub const DISK_FILE: &str = "disk.qcow2";
pub const FIRMWARE_FILE: &str = "vars.fd";

/// Makes the first entry from the VM from before the library. Its files stay where they are.
/// Anything in the folder stops this, which includes the mark that the user removed this entry.
pub fn import_existing(context: &Context) -> Result<Option<Machine>> {
    if !sorted_names(&context.paths.machines).is_empty() {
        return Ok(None);
    }
    let disk = context.paths.store.join("disk/win.qcow2");
    if !disk.exists() {
        return Ok(None);
    }
    let firmware = context.paths.store.join("disk/vars.fd");
    let machine =
        Machine::new(EXISTING_ID, "Windows 11", &disk.to_string_lossy(), &firmware.to_string_lossy());
    machine.save(context)?;
    Ok(Some(machine))
}

/// A folder name from the machine's name, which can hold anything.
pub fn new_id(context: &Context, name: &str) -> String {
    let lower = name.to_lowercase();
    let words = lower.split(|letter: char| !letter.is_ascii_alphanumeric()).filter(|word| !word.is_empty());
    let base = words.collect::<Vec<_>>().join("-");
    let base = if base.is_empty() { "machine".to_owned() } else { base };
    let mut taken = sorted_names(&context.paths.machines);
    // A copy still being made has its name already.
    taken.extend(sorted_names(&crate::duplicate::staging_root(context)));
    let mut found = base.clone();
    let mut number = 1;
    while taken.contains(&found) {
        number += 1;
        found = format!("{base}-{number}");
    }
    found
}

pub fn unused_mac(context: &Context) -> String {
    let machines = load_all(context).0;
    network::new_mac(machines.iter().map(|machine| machine.mac.as_str()))
}

/// What a new machine is to be.
#[derive(Debug, Clone)]
pub struct NewMachine {
    pub name: String,
    pub iso: String,
    pub kind: Kind,
    /// Whether the disc is Windows, which a plain VM can be made from too. It gets a TPM and Secure
    /// Boot then, as Windows 11's setup asks for both.
    pub windows_disc: bool,
    pub cores: u32,
    pub ram_gib: u32,
    pub disk_gib: u32,
    /// The refresh rate of the monitor it was made on, in Hz, which its screen starts at, so the guest
    /// doesn't draw frames the monitor can't show. None, or a rate it couldn't run at, keeps the usual
    /// one, as for a machine made without a desktop.
    pub refresh_hz: Option<u32>,
    /// The guest account's name, or empty for the one the scripts have always made.
    pub user: String,
    /// Its password, or empty for a random one.
    pub password: String,
    /// The card its GPU runs on, by PCI address, or empty for the NVIDIA card setup chose. None takes
    /// this PC's usual choice.
    pub amd_gpu: Option<String>,
    /// A folder on another drive to keep the disk in, or empty for the machine's own folder. The
    /// disk gets a new folder of its own in there.
    pub disk_place: String,
    /// Whose card that is, for a Windows machine. None is what the address says.
    pub gpu_maker: Option<GpuMaker>,
    /// Remote Desktop and SSH, for a Windows machine. Other kinds have neither from here.
    pub access: Access,
}

/// The folder a new machine's disk goes in.
fn disk_home(context: &Context, new: &NewMachine, id: &str) -> PathBuf {
    if new.disk_place.is_empty() {
        context.paths.machines.join(id)
    } else {
        Path::new(&new.disk_place).join(id)
    }
}

/// Says why the disk can't go in the folder the user chose.
fn disk_place_problem(new: &NewMachine, id: &str) -> Option<String> {
    let place = Path::new(&new.disk_place);
    if !place.is_absolute() {
        return Some("Give the disk's folder as a full path, from /.".to_owned());
    }
    if !place.is_dir() {
        return Some(format!("{} is not a folder on this PC.", new.disk_place));
    }
    let taken = place.join(id);
    taken.exists().then(|| format!("{} is already there. Choose another folder or name.", taken.display()))
}

/// Says why this new machine can't be made now.
pub fn create_problems(context: &Context, new: &NewMachine, machine: &Machine) -> Result<Vec<String>> {
    problems(context, new, machine, Driver::Needed)
}

/// Refuses a new machine for anything but its card's driver not being there. Unpacking a driver
/// takes a while and takes the place of the one before, so a caller about to unpack one asks this
/// first, and a machine that would be refused anyway leaves the driver as it was.
pub fn refuse_before_its_driver(context: &Context, new: &NewMachine) -> Result<()> {
    let machine = planned(context, new);
    match problems(context, new, &machine, Driver::StillToCome)?.into_iter().next() {
        Some(problem) => Err(Failure(problem)),
        None => Ok(()),
    }
}

/// Whether a new machine's driver has to be there already.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Driver {
    Needed,
    StillToCome,
}

fn problems(context: &Context, new: &NewMachine, machine: &Machine, driver: Driver) -> Result<Vec<String>> {
    let mut found = machine.problems(crate::settings::max_vram_gib_for(context, machine));
    found.extend(account::password_problem(&new.password));
    let disk_gib = new.disk_gib;
    let least = if machine.kind == Kind::Windows { MIN_WINDOWS_DISK_GIB } else { MIN_PLAIN_DISK_GIB };
    if !(least..=MAX_DISK_GIB).contains(&disk_gib) {
        found.push(format!("The disk must be between {least} and {MAX_DISK_GIB} GiB."));
    }
    if !Path::new(&machine.iso).is_file() {
        found.push("Choose an ISO first.".to_owned());
    }
    // What counts is what it will boot with after the install, and Windows needs more then.
    let limit =
        if machine.kind == Kind::Windows { host_ram_limit(context)? } else { plain_ram_limit(context) };
    if u64::from(machine.ram_gib) > limit {
        found.push(format!(
            "This PC has room for {limit} GiB right now. Give the machine that much, or stop another VM."
        ));
    }
    let firmware = &context.paths.firmware;
    if !machine.vars.is_empty()
        && !(if machine.tpm { &firmware.vars_secure } else { &firmware.vars }).exists()
    {
        found.push(NO_FIRMWARE.to_owned());
    }
    fs::create_dir_all(&context.paths.machines)?;
    if machine.kind == Kind::Windows && !gpu_ready(context) {
        found.push(NO_GPU_PART.to_owned());
    }
    let elsewhere = !new.disk_place.is_empty();
    let place_problem = if elsewhere { disk_place_problem(new, &machine.id) } else { None };
    let place_is_fine = place_problem.is_none();
    found.extend(place_problem);
    if machine.kind == Kind::Windows {
        let place = if elsewhere { Path::new(&new.disk_place) } else { &context.paths.machines };
        if place_is_fine && let Some(short) = install_room_problem(context, place)? {
            found.push(short);
        }
        if driver == Driver::Needed
            && let Err(missing) = guest_driver(context, machine)
        {
            found.push(missing.to_owned());
        }
        found.extend(access_problem(context, &new.access).map(str::to_owned));
    }
    Ok(found)
}

/// Says so when the drive this folder is on has too little room for a Windows install.
pub fn install_room_problem(context: &Context, folder: &Path) -> Result<Option<String>> {
    let room = free_gib(folder)?;
    let needed = context.install_room_gib();
    let short = room < needed as f64;
    Ok(short.then(|| format!("An install needs {needed} GiB free here, and there is {room:.0} GiB.")))
}

pub fn make_disk(context: &Context, machine: &Machine, gib: u32) -> Result<()> {
    let result = process::Command::new("qemu-img")
        .args(["create", "-q", "-f", "qcow2", &machine.disk, &format!("{gib}G")])
        .output()?;
    if !result.status.success() {
        let said = String::from_utf8_lossy(&result.stderr);
        return Err(Failure(format!("The disk could not be made: {}", said.trim())));
    }
    let firmware = &context.paths.firmware;
    if !machine.vars.is_empty() {
        copy_writable(if machine.tpm { &firmware.vars_secure } else { &firmware.vars }, &machine.vars)?;
    }
    let _ = fs::remove_dir_all(machine.tpm_folder(context));
    Ok(())
}

/// The machine a new one would be saved as. Nothing is made yet.
fn planned(context: &Context, new: &NewMachine) -> Machine {
    let id = new_id(context, &new.name);
    let folder = context.paths.machines.join(&id);
    let disk_folder = disk_home(context, new, &id);
    let disk = disk_folder.join(DISK_FILE);
    let firmware = folder.join(FIRMWARE_FILE);
    let windows = new.kind == Kind::Windows;
    let installs = windows || new.kind == Kind::Linux;
    // Ubuntu's firmware is in QEMU, so it has no settings file.
    let firmware =
        if new.kind == Kind::Linux { String::new() } else { firmware.to_string_lossy().into_owned() };
    let amd_gpu = new.amd_gpu.clone().unwrap_or_else(|| crate::linux::amd::default_linux_gpu(context));
    Machine {
        refresh_hz: new
            .refresh_hz
            .filter(|hz| (MIN_REFRESH_HZ..=MAX_REFRESH_HZ).contains(hz))
            .unwrap_or_else(|| Machine::new("", "", "", "").refresh_hz),
        cores: new.cores,
        ram_gib: new.ram_gib,
        kind: new.kind,
        iso: new.iso.clone(),
        installing: installs,
        tpm: windows || new.windows_disc,
        // The install's own Windows has NVMe's driver. What a plain machine boots may not.
        nvme: windows,
        mac: unused_mac(context),
        gpu_maker: new.gpu_maker.and_then(|maker| maker.recorded_for(&amd_gpu)).filter(|_| windows),
        amd_gpu,
        user: if installs { new.user.trim().to_owned() } else { String::new() },
        disk_folder: if new.disk_place.is_empty() {
            String::new()
        } else {
            disk_folder.to_string_lossy().into_owned()
        },
        ..Machine::new(&id, new.name.trim(), &disk.to_string_lossy(), &firmware)
    }
}

/// Makes a machine's folder, disk and firmware file. Installing Windows is a task of its own.
pub fn create(context: &Context, new: &NewMachine) -> Result<Machine> {
    create_with(context, new, &Extras::default())
}

/// What a machine can be made with besides what a [`NewMachine`] says: what a recipe asked of it, and
/// lean Windows.
#[derive(Debug, Clone, Default)]
pub struct Extras {
    /// The most video memory it may use, in GiB. 0 is no cap.
    pub vram_gib: u32,
    /// The recipe it is made for. Its steps run after the install.
    pub recipe: Option<crate::recipe::Chosen>,
    /// Made as lean Windows, which only a Windows machine can be.
    pub lean: bool,
}

/// [`create`] for a machine that has a recipe. The cap is held to the same limit a setting is, and what
/// the recipe is goes in the machine's folder.
pub fn create_with(context: &Context, new: &NewMachine, extras: &Extras) -> Result<Machine> {
    if let Some(chosen) = &extras.recipe
        && chosen.recipe.kind() != new.kind
    {
        let name = &chosen.recipe.name;
        return Err(Failure(format!(
            "The recipe {name} is for {}, and this machine is not.",
            chosen.recipe.guest.name()
        )));
    }
    if extras.lean && new.kind != Kind::Windows {
        return Err(Failure::new("Only a Windows machine can be made lean."));
    }
    let machine = Machine { vram_gib: extras.vram_gib, ..planned(context, new) };
    let folder = context.paths.machines.join(&machine.id);
    if let Some(problem) = create_problems(context, new, &machine)?.into_iter().next() {
        return Err(Failure(problem));
    }
    crate::card_choice::learn_machines_card(context, &machine);
    machine.save(context)?;
    if let Err(failure) = make_disk_folder(&machine) {
        let _ = fs::remove_dir_all(&folder);
        return Err(failure);
    }
    let made = make_files(context, new, &machine, extras);
    if made.is_err() {
        let _ = remove_disk_folder(&machine);
        let _ = fs::remove_dir_all(&folder);
    }
    made.map(|()| machine)
}

/// The disk's own folder on another drive, if it has one. It must be new, so that nothing that was
/// there before is ever taken for the machine's.
fn make_disk_folder(machine: &Machine) -> Result<()> {
    if machine.disk_folder.is_empty() {
        return Ok(());
    }
    fs::create_dir(&machine.disk_folder).map_err(|error| {
        Failure(format!("The disk's folder {} could not be made: {error}", machine.disk_folder))
    })
}

/// The password, the choices of Remote Desktop, SSH and lean Windows, the recipe if there is one, and
/// the disk.
fn make_files(context: &Context, new: &NewMachine, machine: &Machine, extras: &Extras) -> Result<()> {
    if installs_itself(machine) && !new.password.is_empty() {
        store_password(&machine.password_file(context), &new.password)?;
    }
    if machine.kind == Kind::Windows {
        write_access(context, machine, &new.access)?;
        crate::lean::write_lean(context, machine, extras.lean)?;
    }
    if let Some(chosen) = &extras.recipe {
        crate::recipe::write_chosen(context, machine, chosen)?;
    }
    make_disk(context, machine, new.disk_gib)
}

fn installs_itself(machine: &Machine) -> bool {
    matches!(machine.kind, Kind::Windows | Kind::Linux)
}

/// Takes the disk and the folder made for it on another drive. Anything else that was put in the
/// folder since isn't for here to remove, and then the folder stays.
fn remove_disk_folder(machine: &Machine) -> Result<()> {
    if machine.disk_folder.is_empty() {
        return Ok(());
    }
    if Path::new(&machine.disk).exists() {
        fs::remove_file(&machine.disk)?;
    }
    let _ = fs::remove_dir(&machine.disk_folder);
    Ok(())
}

/// The password the user chose, where the install will look for one and keep it: a file only this
/// user can read, the same as the random one it would otherwise make.
fn store_password(file: &Path, password: &str) -> Result<()> {
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }
    let mut new = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(file)?;
    writeln!(new, "{password}")?;
    Ok(())
}

/// Says why this machine's files can't go now. A running VM or task still needs them.
fn removal_problem(context: &Context, machine: &Machine) -> Option<Failure> {
    let current = status(context, machine);
    (current.state != State::Off || current.busy).then(|| Failure::new(SHUT_DOWN_FIRST))
}

/// The folders a delete empties, as a question about it names them.
pub fn folders_text(context: &Context, machine: &Machine) -> String {
    let folder = machine.folder(context).display().to_string();
    if machine.disk_folder.is_empty() { folder } else { format!("{folder} and {}", machine.disk_folder) }
}

/// Deletes the machine's folder, with its disk, and the disk's own folder if it was made one on
/// another drive. Any other disk outside the folder is never touched, and such a machine can only
/// lose its entry.
pub fn delete(context: &Context, machine: &Machine) -> Result<()> {
    if let Some(problem) = removal_problem(context, machine) {
        return Err(problem);
    }
    if !machine.owns_files(context) {
        return Err(Failure::new(
            "This machine's disk is not in its folder, so only its entry can be removed.",
        ));
    }
    let folder = machine.folder(context);
    let linked = folder.is_symlink();
    let real_parent = fs::canonicalize(&folder).ok().and_then(|real| Some(real.parent()?.to_owned()));
    if linked || real_parent != fs::canonicalize(&context.paths.machines).ok() {
        return Err(Failure::new("This machine's folder is not where it should be, so it was left alone."));
    }
    remove_disk_folder(machine)?;
    fs::remove_dir_all(folder)?;
    kept_logs::forget(context, &machine.id);
    Ok(())
}

/// Forgets a machine whose files live elsewhere. Only its own settings file goes.
pub fn remove_entry(context: &Context, machine: &Machine) -> Result<()> {
    if let Some(problem) = removal_problem(context, machine) {
        return Err(problem);
    }
    if machine.owns_files(context) {
        return Err(Failure::new("This machine's disk is in its folder. Delete the machine instead."));
    }
    if list_snapshots(context, machine).iter().any(|snapshot| !snapshot.automatic) {
        return Err(Failure::new("Delete its snapshots first. They are inside the disk, which stays."));
    }
    // The ones Guesthouse took by itself are not the user's to delete, so they go with the entry.
    crate::auto_snapshot::delete_all(context, machine)?;
    let _ = fs::remove_dir_all(snapshots_folder(context, machine));
    kept_logs::forget(context, &machine.id);
    let thumbnail = machine.thumbnail(context);
    for path in [with_suffix(&thumbnail, ".new"), thumbnail] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    let folder = machine.folder(context);
    if !machine.disk_folder.is_empty() {
        // Its disk was on another drive that's gone or was emptied. What's left here is all the
        // app's own, and useless without the disk.
        return Ok(fs::remove_dir_all(folder)?);
    }
    fs::remove_file(folder.join("machine.json"))?;
    // Anything else in there isn't for here to remove, and then the folder stays.
    let _ = fs::remove_dir(&folder);
    if machine.id == EXISTING_ID {
        File::create(context.paths.machines.join(REMOVED_MARK))?;
    }
    Ok(())
}

/// A Mac's installer is a disk, which can't leave a running VM: it's gone from the next start. The
/// machine's own copy goes with it, as Apple's download is kept for the next Mac.
fn take_out_installer(context: &Context, machine: &mut Machine) -> Result<()> {
    let installer = std::mem::take(&mut machine.iso);
    machine.save(context)?;
    if Path::new(&installer).starts_with(machine.folder(context)) {
        let _ = fs::remove_file(installer);
    }
    Ok(())
}

/// Takes the ISO out, at once if the VM is running, so that it boots from its disk.
pub fn eject(context: &Context, machine: &mut Machine) -> Result<()> {
    if machine.macos() {
        return take_out_installer(context, machine);
    }
    if status(context, machine).state == State::Running {
        let command = Command::new("eject", json!({"device": "cd0", "force": true}));
        if failed(&context.ask_monitor(slot_of(context, machine), &command)) {
            return Err(Failure::new("QEMU would not eject the ISO."));
        }
    }
    machine.iso.clear();
    machine.save(context)
}

/// Only for this user, as it shows what was on the guest's screen.
pub fn save_thumbnail(context: &Context, machine: &Machine, png: &[u8]) -> Result<()> {
    const PRIVATE: u32 = 0o600;
    let path = machine.thumbnail(context);
    let new = with_suffix(&path, ".new");
    let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(PRIVATE).open(&new)?;
    file.set_permissions(fs::Permissions::from_mode(PRIVATE))?;
    file.write_all(png)?;
    fs::rename(new, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::load;
    use crate::monitor::stand_in::RecordingMonitor;
    use crate::processes::stand_in::fake_process;
    use crate::remote_access::read_access;
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::sync::Arc;

    fn plain(scratch: &Scratch, name: &str, iso: Option<&str>) -> NewMachine {
        let iso = iso.map_or_else(|| scratch.write("linux.iso", "disc").display().to_string(), str::to_owned);
        NewMachine {
            name: name.into(),
            iso,
            kind: Kind::Plain,
            windows_disc: false,
            cores: 1,
            ram_gib: 1,
            disk_gib: 2,
            refresh_hz: None,
            user: String::new(),
            password: String::new(),
            amd_gpu: None,
            disk_place: String::new(),
            gpu_maker: None,
            access: Access::NEW,
        }
    }

    fn import(scratch: &Scratch, context: &Context) -> Machine {
        scratch.write("home/.local/share/guesthouse/disk/win.qcow2", "user data");
        scratch.write("home/.local/share/guesthouse/disk/vars.fd", "user data");
        import_existing(context).unwrap().unwrap()
    }

    /// Makes the running VM use this machine's disk.
    fn run(context: &Context, machine: &Machine) {
        let qemu = context.paths.qemu.display().to_string();
        let socket =
            format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::first()).display());
        let drive = format!("file={},format=qcow2", machine.disk);
        fake_process(context, 60, &[&qemu, "-qmp", &socket, "-drive", &drive], &[], "");
    }

    #[test]
    fn a_chosen_account_is_kept_for_the_install_and_a_bad_one_refused() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let iso = scratch.write("ubuntu.iso", "disc").display().to_string();
        let new = NewMachine {
            name: "Ubuntu".into(),
            iso,
            kind: Kind::Linux,
            windows_disc: false,
            cores: 2,
            ram_gib: 2,
            disk_gib: 20,
            refresh_hz: None,
            user: "ana".into(),
            password: "chosen".into(),
            amd_gpu: None,
            disk_place: String::new(),
            gpu_maker: None,
            access: Access::NEW,
        };
        let made = create(&context, &new).unwrap();
        assert_eq!(made.account(), "ana");
        let file = made.password_file(&context);
        assert_eq!(fs::read_to_string(&file).unwrap(), "chosen\n");
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600, "the password is this user's alone");
        let refused = create(&context, &NewMachine { name: "Two".into(), user: "Ana".into(), ..new.clone() });
        assert!(refused.unwrap_err().0.contains("user name"));
        let refused =
            create(&context, &NewMachine { name: "Two".into(), password: "a\nb".into(), ..new.clone() });
        assert!(refused.unwrap_err().0.contains("password"));
        assert!(!context.paths.machines.join("two").exists(), "a refused machine leaves nothing");
        let random =
            create(&context, &NewMachine { name: "Three".into(), password: String::new(), ..new }).unwrap();
        assert!(!random.password_file(&context).exists(), "the install makes a random one later");
    }

    #[test]
    fn the_import_takes_the_scripts_values_and_leaves_the_files_alone() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(import_existing(&context).unwrap(), None, "no disk, so nothing to import");
        let mut made = import(&scratch, &context);
        let disk = context.paths.store.join("disk/win.qcow2");
        assert_eq!((made.cores, made.ram_gib, made.refresh_hz, made.vcache_pin), (8, 16, 144, false));
        assert_eq!(made.disk, disk.display().to_string());
        assert_eq!(fs::read(&disk).unwrap(), b"user data");
        made.cores = 12;
        made.save(&context).unwrap();
        assert_eq!(import_existing(&context).unwrap(), None, "a second run must not import again");
        assert_eq!(load(&context, EXISTING_ID).unwrap().cores, 12);
    }

    #[test]
    fn a_machine_starts_at_its_monitor_s_refresh_rate() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let made =
            create(&context, &NewMachine { refresh_hz: Some(60), ..plain(&scratch, "Sixty", None) }).unwrap();
        assert_eq!(made.refresh_hz, 60);
        let made =
            create(&context, &NewMachine { refresh_hz: Some(500), ..plain(&scratch, "Fast", None) }).unwrap();
        assert_eq!(made.refresh_hz, 144, "a rate past what the guest can run keeps the usual one");
    }

    #[test]
    fn a_broken_file_is_never_replaced_by_the_import() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        import(&scratch, &context);
        fs::write(context.paths.machines.join("windows-11/machine.json"), "{").unwrap();
        assert_eq!(import_existing(&context).unwrap(), None);
    }

    #[test]
    fn a_plain_machine_from_a_windows_disc_gets_a_tpm_and_secure_boot() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let new = NewMachine { windows_disc: true, ..plain(&scratch, "By Hand", None) };
        let made = create(&context, &new).unwrap();
        assert!(made.kind == Kind::Plain && made.tpm && !made.installing && !made.has_gpu());
        assert!(!made.nvme, "what a plain machine boots may have no NVMe driver");
        assert_eq!(fs::read_to_string(&made.vars).unwrap(), "firmware settings with keys");
    }

    #[test]
    fn a_plain_machine_gets_a_folder_disk_and_firmware() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let made = create(&context, &plain(&scratch, " Tiny, Linux! ", None)).unwrap();
        assert_eq!((made.id.as_str(), made.name.as_str()), ("tiny-linux", "Tiny, Linux!"));
        assert!(made.owns_files(&context) && !made.installing && !made.has_gpu() && !made.tpm);
        assert_eq!(fs::read_to_string(&made.vars).unwrap(), "blank firmware settings");
        assert_eq!(load(&context, "tiny-linux").unwrap(), made);
        assert_eq!(read_access(&context, &made), Access::default(), "Remote Desktop and SSH are Windows'");
        let second = create(&context, &plain(&scratch, "Tiny Linux", None)).unwrap();
        assert_eq!(second.id, "tiny-linux-2", "a second machine with this name gets its own folder");
        assert_eq!(new_id(&context, "???"), "machine");
    }

    #[test]
    fn an_install_that_wouldnt_fit_offers_a_number_that_would() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.install_room = Some(0);
        context.ram_limit = Some(6);
        fs::create_dir_all(crate::settings::driver_folder(&context)).unwrap();
        let iso = scratch.write("win.iso", "disc").display().to_string();
        let new = NewMachine {
            name: "Win".into(),
            iso,
            kind: Kind::Windows,
            windows_disc: true,
            cores: 8,
            ram_gib: 16,
            disk_gib: 80,
            refresh_hz: None,
            user: String::new(),
            password: String::new(),
            amd_gpu: None,
            disk_place: String::new(),
            gpu_maker: None,
            access: Access::NEW,
        };
        assert!(create(&context, &new).unwrap_err().0.contains("room for 6 GiB"));
    }

    #[test]
    fn with_no_firmware_it_says_what_to_install() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.firmware.vars = scratch.path("not-installed");
        let iso = scratch.write("disc.iso", "disc").display().to_string();
        let new = NewMachine {
            name: "VM".into(),
            iso,
            kind: Kind::Plain,
            windows_disc: false,
            cores: 2,
            ram_gib: 2,
            disk_gib: 4,
            refresh_hz: None,
            user: String::new(),
            password: String::new(),
            amd_gpu: None,
            disk_place: String::new(),
            gpu_maker: None,
            access: Access::NEW,
        };
        assert_eq!(create(&context, &new).unwrap_err().0, NO_FIRMWARE);
    }

    #[test]
    fn a_new_windows_machine_installs_with_a_tpm_on_nvme() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.install_room = Some(0);
        fs::create_dir_all(crate::settings::driver_folder(&context)).unwrap();
        let iso = scratch.write("win.iso", "disc").display().to_string();
        let new = NewMachine {
            name: "Win".into(),
            iso,
            kind: Kind::Windows,
            windows_disc: true,
            cores: 2,
            ram_gib: 4,
            disk_gib: 64,
            refresh_hz: None,
            user: String::new(),
            password: String::new(),
            amd_gpu: None,
            disk_place: String::new(),
            gpu_maker: None,
            access: Access::NEW,
        };
        let made = create(&context, &new).unwrap();
        assert!(made.installing && made.tpm && made.nvme && !made.has_gpu());
        assert_eq!(fs::read_to_string(&made.vars).unwrap(), "firmware settings with keys");
        assert_eq!(read_access(&context, &made), Access::NEW);
        let with_ssh = Access { ssh: Some(true), ..Access::NEW };
        for (change, text) in [
            (NewMachine { access: with_ssh, ..new.clone() }, "OpenSSH"),
            (NewMachine { cores: 1, ..new.clone() }, "Cores"),
            (NewMachine { ram_gib: 3, ..new.clone() }, "RAM"),
            (NewMachine { disk_gib: MIN_WINDOWS_DISK_GIB - 1, ..new.clone() }, "disk"),
            (NewMachine { name: " ".into(), ..new.clone() }, "name"),
            (NewMachine { iso: scratch.path("gone.iso").display().to_string(), ..new.clone() }, "ISO"),
        ] {
            let before = sorted_names(&context.paths.machines);
            assert!(create(&context, &change).unwrap_err().0.contains(text));
            assert_eq!(sorted_names(&context.paths.machines), before, "a refused machine leaves nothing");
        }
    }

    #[test]
    fn ubuntu_installs_with_no_firmware_file_and_no_windows_driver() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let iso = scratch.write("server.iso", "disc").display().to_string();
        let new = NewMachine {
            name: "Ubuntu".into(),
            iso,
            kind: Kind::Linux,
            windows_disc: false,
            cores: 8,
            ram_gib: 8,
            disk_gib: 2,
            refresh_hz: None,
            user: String::new(),
            password: String::new(),
            amd_gpu: None,
            disk_place: String::new(),
            gpu_maker: None,
            access: Access::NEW,
        };
        let made = create(&context, &new).unwrap();
        assert!(made.installing && !made.tpm && made.vars.is_empty() && !made.nvme);
        assert!(made.owns_files(&context));
    }

    #[test]
    fn windows_needs_the_drivers_files() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.install_room = Some(0);
        let iso = scratch.write("win.iso", "disc").display().to_string();
        let new = NewMachine {
            name: "Win".into(),
            iso,
            kind: Kind::Windows,
            windows_disc: true,
            cores: 2,
            ram_gib: 4,
            disk_gib: 64,
            refresh_hz: None,
            user: String::new(),
            password: String::new(),
            amd_gpu: None,
            disk_place: String::new(),
            gpu_maker: None,
            access: Access::NEW,
        };
        assert_eq!(create(&context, &new).unwrap_err().0, NO_DRIVER_FILES);
    }

    #[test]
    fn a_windows_machine_made_on_the_second_nvidia_card_keeps_it_and_its_memory() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.install_room = Some(0);
        context.card_memories = || "00000000:01:00.0, 16376\n00000000:02:00.0, 6144\n".to_owned();
        std::fs::create_dir_all(crate::settings::driver_folder(&context)).unwrap();
        let card = serde_json::json!({"card": {"device": 0x2704, "pci": "0000:01:00.0"}});
        crate::settings::write_setup(&context, card.as_object().cloned().unwrap()).unwrap();
        let iso = scratch.write("win.iso", "disc").display().to_string();
        let new = NewMachine {
            name: "Win".into(),
            iso,
            kind: Kind::Windows,
            windows_disc: true,
            cores: 2,
            ram_gib: 4,
            disk_gib: 64,
            refresh_hz: None,
            user: String::new(),
            password: String::new(),
            amd_gpu: Some("0000:02:00.0".into()),
            disk_place: String::new(),
            gpu_maker: Some(GpuMaker::Nvidia),
            access: Access::NEW,
        };
        let made = create(&context, &new).unwrap();
        let read = crate::machine::load(&context, &made.id).unwrap();
        assert_eq!((read.amd_gpu.as_str(), read.windows_maker()), ("0000:02:00.0", Some(GpuMaker::Nvidia)));
        let setup = crate::settings::read_setup(&context);
        assert_eq!(setup[crate::settings::CARD_MEMORIES_FIELD]["0000:02:00.0"], 6144, "learnt for the cap");
        assert_eq!(crate::settings::max_vram_gib_for(&context, &read), 4);
        let on_setups =
            create(&context, &NewMachine { name: "Old".into(), amd_gpu: Some(String::new()), ..new })
                .unwrap();
        assert_eq!((on_setups.amd_gpu.as_str(), on_setups.gpu_maker), ("", None), "written as before");
    }

    #[test]
    fn a_disk_that_cannot_be_made_says_why() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = Machine { kind: Kind::Plain, ..Machine::new("x", "x", "/nowhere/d", "/nowhere/v") };
        assert!(make_disk(&context, &machine, 1).unwrap_err().0.contains("could not be made"));
    }

    #[test]
    fn an_imported_disk_is_never_deleted() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let imported = import(&scratch, &context);
        let disk = fs::metadata(&imported.disk).unwrap().ino();
        assert!(!imported.owns_files(&context));
        assert!(delete(&context, &imported).unwrap_err().0.contains("only its entry"));
        remove_entry(&context, &imported).unwrap();
        assert!(!imported.folder(&context).exists());
        assert_eq!(fs::metadata(&imported.disk).unwrap().ino(), disk);
        assert_eq!(import_existing(&context).unwrap(), None, "a removed entry does not come back by itself");
    }

    #[test]
    fn a_disk_in_the_folder_only_through_a_link_is_not_the_machines() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let outside = scratch.write("outside.qcow2", "user data");
        let folder = context.paths.machines.join("linked");
        let disk = folder.join("disk.qcow2");
        let linked = Machine::new("linked", "Linked", &disk.to_string_lossy(), &outside.to_string_lossy());
        linked.save(&context).unwrap();
        symlink(&outside, &disk).unwrap();
        assert!(!linked.owns_files(&context));
        assert!(delete(&context, &linked).unwrap_err().0.contains("only its entry"));
        remove_entry(&context, &linked).unwrap();
        assert!(disk.is_symlink(), "what it does not own stays");
    }

    #[test]
    fn a_folder_that_is_a_link_is_left_alone() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fs::create_dir_all(&context.paths.machines).unwrap();
        let outside = scratch.path("outside");
        fs::create_dir_all(&outside).unwrap();
        let folder = context.paths.machines.join("moved");
        symlink(&outside, &folder).unwrap();
        let (disk, firmware) = (folder.join("disk.qcow2"), folder.join("vars.fd"));
        fs::write(&disk, "data").unwrap();
        fs::write(&firmware, "data").unwrap();
        let moved = Machine::new("moved", "Moved", &disk.to_string_lossy(), &firmware.to_string_lossy());
        assert!(delete(&context, &moved).unwrap_err().0.contains("left alone"));
        assert_eq!(sorted_names(&outside), ["disk.qcow2", "vars.fd"]);
    }

    #[test]
    fn a_running_machine_keeps_its_files() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let made = create(&context, &plain(&scratch, "Busy", None)).unwrap();
        run(&context, &made);
        assert!(delete(&context, &made).unwrap_err().0.contains("Shut the VM down"));
        assert!(remove_entry(&context, &made).unwrap_err().0.contains("Shut the VM down"));
        assert!(Path::new(&made.disk).exists(), "a refused delete takes nothing");
        fs::remove_dir_all(context.processes.join("60")).unwrap();
        fs::create_dir_all(kept_logs::kept_folder(&context, &made.id)).unwrap();
        delete(&context, &made).unwrap();
        assert!(!made.folder(&context).exists());
        assert!(!kept_logs::kept_folder(&context, &made.id).exists(), "its kept logs go with it");
    }

    #[test]
    fn a_disk_on_another_drive_gets_a_folder_there_and_goes_with_the_machine() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let other = scratch.path("other-drive");
        fs::create_dir_all(&other).unwrap();
        let elsewhere =
            NewMachine { disk_place: other.display().to_string(), ..plain(&scratch, "Far", None) };
        let made = create(&context, &elsewhere).unwrap();
        assert_eq!(Path::new(&made.disk), other.join("far/disk.qcow2"));
        assert!(Path::new(&made.disk).exists() && Path::new(&made.vars).starts_with(made.folder(&context)));
        assert!(made.owns_files(&context));
        assert_eq!(load(&context, "far").unwrap(), made);
        let again = create(&context, &NewMachine { name: "Far 2".into(), ..elsewhere.clone() }).unwrap();
        fs::write(other.join("far-2/notes.txt"), "the user's").unwrap();
        delete(&context, &made).unwrap();
        assert!(!other.join("far").exists() && !made.folder(&context).exists());
        delete(&context, &again).unwrap();
        assert!(!Path::new(&again.disk).exists());
        assert!(other.join("far-2/notes.txt").exists(), "what the user put beside the disk stays");
    }

    #[test]
    fn a_machine_whose_other_drive_is_gone_can_be_forgotten_whole() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let other = scratch.path("other-drive");
        fs::create_dir_all(&other).unwrap();
        let elsewhere =
            NewMachine { disk_place: other.display().to_string(), ..plain(&scratch, "Far", None) };
        let made = create(&context, &elsewhere).unwrap();
        fs::remove_dir_all(&other).unwrap();
        assert!(!made.owns_files(&context));
        assert!(delete(&context, &made).unwrap_err().0.contains("only its entry"));
        remove_entry(&context, &made).unwrap();
        assert!(!made.folder(&context).exists(), "nothing is left to block the name");
        assert!(load_all(&context).1.is_empty());
    }

    #[test]
    fn a_disk_place_that_is_taken_or_no_folder_is_refused() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let other = scratch.path("other-drive");
        fs::create_dir_all(other.join("far")).unwrap();
        let taken = NewMachine { disk_place: other.display().to_string(), ..plain(&scratch, "Far", None) };
        assert!(create(&context, &taken).unwrap_err().0.contains("is already there"));
        let missing = NewMachine { disk_place: scratch.path("nowhere").display().to_string(), ..taken };
        assert!(create(&context, &missing).unwrap_err().0.contains("is not a folder"));
        let relative = NewMachine { disk_place: "big".into(), ..missing };
        assert!(create(&context, &relative).unwrap_err().0.contains("full path"));
        assert!(sorted_names(&context.paths.machines).is_empty(), "a refusal makes nothing");
    }

    #[test]
    fn eject_reaches_a_running_vm_and_a_refusal_changes_nothing() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let monitor = Arc::new(RecordingMonitor::answering(Some(json!({"return": {}}))));
        context.monitor = monitor.clone();
        let iso = plain(&scratch, "Eject", None).iso.clone();
        let mut machine = create(&context, &plain(&scratch, "Eject", None)).unwrap();
        eject(&context, &mut machine).unwrap();
        assert!(monitor.asked().is_empty(), "with the VM off, this is only the setting");
        assert_eq!(load(&context, &machine.id).unwrap().iso, "");
        machine.iso = iso.clone();
        run(&context, &machine);
        eject(&context, &mut machine).unwrap();
        assert_eq!(monitor.asked()[0].name, "eject");
        machine.iso = iso.clone();
        machine.save(&context).unwrap();
        monitor.set_answer(Some(json!({"error": {"desc": "locked"}})));
        assert!(eject(&context, &mut machine).unwrap_err().0.contains("would not eject"));
        assert_eq!(load(&context, &machine.id).unwrap().iso, iso, "a failed eject changes nothing");
    }

    #[test]
    fn a_macs_installer_is_taken_out_as_a_setting_and_its_copy_goes() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let monitor = Arc::new(RecordingMonitor::answering(Some(json!({"return": {}}))));
        context.monitor = monitor.clone();
        let mut mac = create(&context, &plain(&scratch, "Mac", None)).unwrap();
        let installer = mac.folder(&context).join("installer.img");
        fs::write(&installer, "apple's").unwrap();
        mac.opencore = mac.folder(&context).join("opencore.qcow2").to_string_lossy().into_owned();
        mac.iso = installer.to_string_lossy().into_owned();
        run(&context, &mac);
        eject(&context, &mut mac).unwrap();
        assert!(monitor.asked().is_empty(), "a disk can't leave a running VM");
        assert_eq!(load(&context, &mac.id).unwrap().iso, "");
        assert!(!installer.exists());
    }

    #[test]
    fn a_thumbnail_is_for_this_user_only() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let imported = import(&scratch, &context);
        save_thumbnail(&context, &imported, b"png one").unwrap();
        save_thumbnail(&context, &imported, b"png two").unwrap();
        let path = imported.thumbnail(&context);
        assert_eq!(fs::read(&path).unwrap(), b"png two");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(!with_suffix(&path, ".new").exists());
        remove_entry(&context, &imported).unwrap();
        assert!(!imported.folder(&context).exists(), "it goes with the entry");
    }

    fn a_recipe(guest: &str) -> crate::recipe::Chosen {
        let text = format!(
            "name = \"R\"\ndescription = \"D.\"\nguest = \"{guest}\"\n[steps]\npackages = [\"git\"]\n"
        );
        crate::recipe::Chosen {
            source: crate::recipe::Source::App,
            file: String::new(),
            recipe: crate::recipe::Recipe::parse(&text).unwrap(),
        }
    }

    #[test]
    fn a_machine_made_for_a_recipe_keeps_it_and_one_made_for_none_has_no_file() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let new = NewMachine { kind: Kind::Linux, ..plain(&scratch, "Ub", None) };
        let none = create(&context, &new).unwrap();
        assert!(crate::recipe::read_chosen(&context, &none).is_none());
        assert!(!none.folder(&context).join("recipe.json").exists());
        let extras = Extras { vram_gib: 0, recipe: Some(a_recipe("ubuntu")), lean: false };
        let made = create_with(&context, &NewMachine { name: "Ub2".into(), ..new }, &extras).unwrap();
        assert_eq!(crate::recipe::read_chosen(&context, &made), extras.recipe);
    }

    #[test]
    fn a_recipe_for_another_kind_makes_nothing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let extras = Extras { vram_gib: 0, recipe: Some(a_recipe("windows")), lean: false };
        let said = create_with(&context, &plain(&scratch, "Plain", None), &extras).unwrap_err().0;
        assert!(said.contains("is for Windows"), "{said}");
        assert!(load_all(&context).0.is_empty(), "no machine was left behind");
    }

    #[test]
    fn a_cap_a_recipe_asks_is_held_to_the_cards_limit() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.install_room = Some(0);
        fs::create_dir_all(crate::settings::driver_folder(&context)).unwrap();
        let new = NewMachine {
            kind: Kind::Windows,
            windows_disc: true,
            cores: 2,
            ram_gib: 4,
            disk_gib: 64,
            ..plain(&scratch, "Win", Some(&scratch.write("win.iso", "disc").display().to_string()))
        };
        let said = create_with(&context, &new, &Extras { vram_gib: 999, ..Extras::default() }).unwrap_err().0;
        assert!(said.contains("video memory cap"), "{said}");
        let made = create_with(&context, &new, &Extras::default()).unwrap();
        assert_eq!(made.vram_gib, 0);
    }

    #[test]
    fn only_windows_is_made_lean_and_the_machine_keeps_the_choice() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.install_room = Some(0);
        fs::create_dir_all(crate::settings::driver_folder(&context)).unwrap();
        let lean = Extras { lean: true, ..Extras::default() };
        let said = create_with(&context, &plain(&scratch, "Plain", None), &lean).unwrap_err().0;
        assert!(said.contains("Only a Windows machine"), "{said}");
        let new = NewMachine {
            kind: Kind::Windows,
            windows_disc: true,
            cores: 2,
            ram_gib: 4,
            disk_gib: 64,
            ..plain(&scratch, "Win", Some(&scratch.write("win.iso", "disc").display().to_string()))
        };
        let made = create_with(&context, &new, &lean).unwrap();
        assert!(crate::lean::is_lean(&context, &made));
        let usual =
            create_with(&context, &NewMachine { name: "Win2".into(), ..new }, &Extras::default()).unwrap();
        assert!(!usual.folder(&context).join(crate::lean::LEAN_FILE).exists());
    }
}
