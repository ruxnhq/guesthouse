//! Installing Windows: the answer disc, the wait for setup, and the check of what it made.

use super::complete_host_driver;
use super::desktop::{BOOT_LINE, SERIAL_FILE, count, wait_for_desktop};
use super::identity::set_up_identity;
use super::recipe;
use super::{KEY_POLL, Task, plan_for, start_if_there_is_room};
use crate::account::xml_text;
use crate::agent::{QUICK_SECONDS, guest_script, make_password, make_token};
use crate::context::Context;
use crate::disk_image::disk_size_gib;
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::iso::{self, iso_maker};
use crate::lean;
use crate::library::make_disk;
use crate::machine::{Kind, Machine};
use crate::monitor::Command as Qmp;
use crate::remote_access::{INSTALLER_ON_DISC, Service, disc_scripts, read_access};
use crate::setup::guest_driver::guest_driver;
use crate::start_checks::start_problems;
use crate::vm_args::Boot;
use crate::vm_host::{last_words, send_key};
use crate::writable_copy::copy_writable;
use crate::zones;
use serde_json::json;
use std::fs;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub(super) const INSTALL_FILES: [&str; 4] =
    ["autounattend.xml", "SetupComplete.cmd", "setup.ps1", "agent.ps1"];
const SYSTEM_FILES_SCRIPT: &str = "nv-files.ps1";
const INSTALL_HOURS: u64 = 2;
/// Setup writes all the time until the desktop, so a disk that stops growing means it's waiting for
/// someone, or stuck. The longest quiet stretch in a normal install is well under this.
const INSTALL_QUIET_MINUTES: u64 = 10;
const BOOT_PROMPT_SECONDS: u64 = 6;
pub const INSTALL_STEPS: [&str; 4] = [
    "Copying Windows to the disk",
    "Windows is setting up its devices",
    "Windows is getting ready",
    "Windows is finishing its setup",
];
/// The firmware names it twice a boot.
const WINDOWS_BOOT_LINE: &[u8] = b"\"Windows Boot Manager\"";
const FIRMWARE_WAIT: Duration = Duration::from_secs(30);
const SETUP_SCRIPT_SECONDS: u64 = 30;
const CHECK_SCRIPT_SECONDS: u64 = 60;
const INSTALL_STOP_SECONDS: u64 = 120;
const SETUP_POLL: Duration = Duration::from_secs(5);
const SHORT_POLL: Duration = Duration::from_millis(200);
/// The agent's file and the time zone both go through these.
const ANSWER_DISC: &str = "answer.iso";
const ANSWER_LABEL: &str = "GPUPV_SETUP";
const SETMODE_FILE: &str = "setmode.ps1";
const TOKEN_ON_DISC: &str = "token.txt";
const PASSWORD_MARK: &str = "@PASSWORD@";
const USER_MARK: &str = "@USER@";

/// The answer file for a host in this IANA zone, making this account. The template says UTC. The
/// edition is the disc's own when it was read, and Pro when it wasn't, as it always was. Its key
/// tells Setup the edition so it doesn't stop to ask.
pub fn answer_file(
    context: &Context,
    zone: &str,
    user: &str,
    password: &str,
    edition: Option<&str>,
) -> Result<String> {
    let name = zones::windows_zone(zone);
    let template = guest_script(context, "autounattend.xml")?;
    let edition = edition.unwrap_or(iso::WINDOWS_EDITION);
    let key = iso::edition_key(edition).map_or(String::new(), |key| {
        format!("<ProductKey><Key>{key}</Key><WillShowUI>OnError</WillShowUI></ProductKey>")
    });
    let filled = template
        .replace("<TimeZone>UTC<", &format!("<TimeZone>{name}<"))
        .replace("@EDITION@", &xml_text(edition))
        .replace("@PRODUCT_KEY@", &key);
    Ok(filled.replace(USER_MARK, &xml_text(user)).replace(PASSWORD_MARK, &xml_text(password)))
}

/// The edition of the disc this machine installs from, when the disc can be read and has one a guest
/// runs. A Windows disc with another edition is a plain VM, and its edition is empty.
fn disc_edition(machine: &Machine) -> Option<String> {
    let iso = iso::read_iso(&machine.iso).ok()?;
    (iso.kind == Kind::Windows).then_some(iso.edition)
}

/// A guest script that names the account, with this machine's name in it.
pub(super) fn account_script(task: &Task, name: &str) -> Result<String> {
    Ok(guest_script(task.context, name)?.replace(USER_MARK, task.machine.account()))
}

/// An account with the template's own text for a password is one nobody chose, so no disc gets made.
fn password_is_filled(answers: String) -> Result<String> {
    if answers.contains(PASSWORD_MARK) {
        return Err(Failure::new("The answer file has no password in it, so nothing was installed."));
    }
    Ok(answers)
}

pub fn answer_disc_path(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(ANSWER_DISC)
}

/// Everything Windows setup needs from here: the answer file, what it runs, and the driver's files.
/// It holds the agent's token, so it's for this user only, and it goes once the install is done.
pub fn make_answer_disc(context: &Context, machine: &Machine) -> Result<()> {
    let files = machine.folder(context).join("answer-disc.new");
    let _ = fs::remove_dir_all(&files);
    fs::create_dir_all(&files)?;
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700))?;
    let built = fill_answer_disc(context, machine, &files);
    let _ = fs::remove_dir_all(&files);
    built
}

/// The copies are made writable: the program folder can be read-only, as in NixOS's store, and the
/// answer file is written over once it's filled in.
fn fill_answer_disc(context: &Context, machine: &Machine, files: &Path) -> Result<()> {
    for name in INSTALL_FILES {
        copy_writable(context.paths.guest_files().join(name), files.join(name))?;
    }
    copy_writable(context.paths.guest_files().join(SETMODE_FILE), files.join(SETMODE_FILE))?;
    if machine.windows_maker() == Some(GpuMaker::Intel) {
        // Setup places Intel's loaders in the system folders with the script that does it at a start.
        copy_writable(
            context.paths.guest_files().join(SYSTEM_FILES_SCRIPT),
            files.join(SYSTEM_FILES_SCRIPT),
        )?;
    }
    // Setup runs these at its end, for Remote Desktop and SSH as the machine was made.
    let access = read_access(context, machine);
    for (name, script) in disc_scripts(context, &access)? {
        fs::write(files.join(name), script)?;
    }
    if access.is_on(Service::Ssh) {
        copy_writable(context.paths.openssh_installer(), files.join(INSTALLER_ON_DISC))?;
    }
    let password = make_password(context, machine)?;
    let zone = zones::host_zone(zones::HOST_ZONE_LINK);
    let mut answers =
        answer_file(context, &zone, machine.account(), &password, disc_edition(machine).as_deref())?;
    // Only a lean machine's disc has these, and only its answer file the line that runs them.
    if lean::is_lean(context, machine) {
        copy_writable(context.paths.guest_files().join(lean::SCRIPT), files.join(lean::SCRIPT))?;
        fs::write(files.join(lean::DATA_ON_DISC), lean::guest_data())?;
        answers = lean::with_specialize_step(&answers)?;
    }
    fs::write(files.join("autounattend.xml"), password_is_filled(answers)?)?;
    fs::write(files.join(TOKEN_ON_DISC), make_token(context, machine)? + "\n")?;
    let driver = guest_driver(context, machine).map_err(Failure::new)?;
    let path = answer_disc_path(context, machine);
    // Made private first: the disc holds the token, and genisoimage keeps a file's mode.
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    let result = Command::new(iso_maker())
        .args(["-quiet", "-J", "-joliet-long", "-r", "-V", ANSWER_LABEL, "-graft-points", "-o"])
        .arg(&path)
        .arg(format!("/={}", files.display()))
        .arg(format!("{}/={}", driver.disc_folder, driver.folder.display()))
        .output()?;
    if !result.status.success() {
        let said = String::from_utf8_lossy(&result.stderr);
        let said = said.trim();
        let tail = said.char_indices().rev().nth(199).map_or(0, |found| found.0);
        return Err(Failure(format!("The install disc could not be made: {}", &said[tail..])));
    }
    Ok(())
}

fn serial_log(task: &Task) -> Vec<u8> {
    fs::read(task.run_file(SERIAL_FILE)).unwrap_or_default()
}

/// A Windows disc only boots if a key is pressed in its first seconds, and starting the same way
/// later would begin the install again. Setup's own screens must not get these keys.
fn pass_boot_prompt(task: &Task) {
    let end = task.now() + FIRMWARE_WAIT;
    while count(&serial_log(task), BOOT_LINE) == 0 && task.now() < end {
        task.sleep(SHORT_POLL);
    }
    let end = task.now() + Duration::from_secs(BOOT_PROMPT_SECONDS);
    while task.now() < end {
        send_key(task.context, "spc");
        task.sleep(KEY_POLL);
    }
}

/// What Windows setup is doing, from how often it has started from the disk.
pub fn install_step(serial: &[u8]) -> &'static str {
    let started = count(serial, WINDOWS_BOOT_LINE);
    INSTALL_STEPS[(started / 2).min(INSTALL_STEPS.len() - 1)]
}

fn disk_written(machine: &Machine) -> u64 {
    fs::metadata(&machine.disk).map_or(0, |found| found.blocks())
}

/// Waits for the agent, and then for the user account, which Windows makes late.
pub fn wait_for_setup(task: &mut Task) -> Result<()> {
    let finish = account_script(task, "finish.ps1")?;
    let quiet = Duration::from_secs(INSTALL_QUIET_MINUTES * 60);
    let end = task.now() + Duration::from_secs(INSTALL_HOURS * 3600);
    let mut step = "";
    let (mut grew_at, mut written) = (task.now(), 0);
    while task.now() < end {
        if task.pid().is_none() {
            let words = last_words(task.context);
            return Err(Failure(format!("The VM stopped during the install. {words}").trim().to_owned()));
        }
        let now_written = disk_written(&task.machine);
        if now_written > written {
            (grew_at, written) = (task.now(), now_written);
        }
        if task.now().saturating_sub(grew_at) > quiet {
            let file = task.run_file("stuck-install.ppm");
            let command = Qmp::new("screendump", json!({"filename": file}));
            task.context.ask_monitor(None, &command);
            return Err(Failure(format!(
                "Windows Setup has not written to the disk for {INSTALL_QUIET_MINUTES} minutes, so it is \
                 stuck or waiting for an answer. The VM is still up: Show Screen has what it says."
            )));
        }
        let current = install_step(&serial_log(task));
        if current != step {
            step = current;
            task.step(step);
        }
        if task.ask_guest(&finish, SETUP_SCRIPT_SECONDS).contains("finished") {
            return Ok(());
        }
        task.sleep(SETUP_POLL);
    }
    Err(Failure(format!(
        "Windows setup did not finish in {INSTALL_HOURS} hours. Show Screen says how far it got."
    )))
}

fn count_files(folder: &Path) -> usize {
    let Ok(entries) = fs::read_dir(folder) else { return 0 };
    entries.flatten().map(|entry| if entry.path().is_dir() { count_files(&entry.path()) } else { 1 }).sum()
}

/// Looks at the finished guest, so that anything missing is found here and not at first boot.
fn check_guest(task: &Task) -> Result<()> {
    task.step("Checking Windows");
    let driver = guest_driver(task.context, &task.machine).map_err(Failure::new)?;
    let expected = count_files(&driver.folder);
    let check = account_script(task, "check.ps1")?;
    let script = format!("$expected = {expected}\n$store_folder = '{}'\n{check}", driver.store_folder);
    let answer = task.ask_guest(&script, CHECK_SCRIPT_SECONDS);
    if !answer.contains("all there") {
        let said = answer.trim();
        let tail = said.char_indices().rev().nth(299).map_or(0, |found| found.0);
        let said = if said.is_empty() { "the agent went quiet" } else { &said[tail..] };
        return Err(Failure(format!("Windows is missing something: {said}")));
    }
    Ok(())
}

/// Takes a new machine from its ISO to the first boot with its GPU. Each run starts again from
/// nothing, as a half-finished Windows isn't worth keeping.
pub fn install(task: &mut Task) -> Result<()> {
    let context = task.context;
    if !task.machine.owns_files(context) {
        return Err(Failure::new(
            "This machine's disk is not in its folder, so nothing will install over it.",
        ));
    }
    let mut found: Vec<String> = start_problems(context, &task.machine)?;
    found.retain(|problem| !problem.contains("is missing"));
    if !Path::new(&task.machine.iso).exists() {
        found.push(format!("{} is missing.", task.machine.iso));
    }
    if let Some(problem) = found.into_iter().next() {
        return Err(Failure(problem));
    }
    task.step("Preparing the install disc");
    recipe::reset(task);
    // The answer file counts on it, and all that was here goes anyway.
    task.machine.tpm = true;
    task.machine.save(context)?;
    let size = disk_size_gib(&task.machine.disk)? as u32;
    complete_host_driver(task)?;
    make_answer_disc(context, &task.machine)?;
    fs::remove_file(&task.machine.disk)?;
    make_disk(context, &task.machine, size)?;
    // What's written down about the Windows that was on the disk would be believed of the new one,
    // which then gets no package for its vendor number and no display driver.
    crate::gpu_identity::forget_state(context, &task.machine);
    crate::display::forget_state(context, &task.machine);
    crate::auto_snapshot::record::forget(context, &task.machine);
    task.step("Starting the installer");
    let answer = answer_disc_path(context, &task.machine);
    let boot = Boot::Install { iso: task.machine.iso.clone(), answer: answer.clone() };
    let plan = plan_for(context, &task.machine, boot);
    task.host.start(&plan, &task.machine)?;
    pass_boot_prompt(task);
    wait_for_setup(task)?;
    task.step("Waiting for the desktop");
    if !wait_for_desktop(task) {
        return Err(Failure::new("Windows installed, but its desktop never came up."));
    }
    check_guest(task)?;
    // While the install's own session is up, as SYSTEM, and before its shutdown makes the next start
    // the first that counts. It only ever adds a note.
    super::lean::after_install(task);
    // Here, so the first start is already on the card maker's vendor number and needs no second VM.
    set_up_identity(task);
    task.step("Shutting down Windows");
    task.ask_guest("Stop-Computer -Force", QUICK_SECONDS);
    let end = task.now() + Duration::from_secs(INSTALL_STOP_SECONDS);
    while task.pid().is_some() && task.now() < end {
        task.sleep(KEY_POLL);
    }
    if task.pid().is_some() {
        return Err(Failure::new("Windows is installed but would not shut down. Try Force Off, then Start."));
    }
    fs::remove_file(&answer)?;
    task.machine.installing = false;
    task.machine.iso.clear();
    task.machine.save(context)?;
    crate::throwaway::installed(task.context, &task.machine)?;
    if !context.second_instance() {
        start_if_there_is_room(task, "Windows")?;
    }
    // Last, and after all of it has succeeded: what a recipe's steps do can't undo the install.
    recipe::after_install(task);
    Ok(())
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
