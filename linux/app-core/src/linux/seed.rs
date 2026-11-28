//! The seed disc an unattended install reads: the answers, and the scripts for first boot. It also
//! makes root's key, and a password for logging in at a console if the desktop won't start. Both stay
//! as they are if they're already there.

use super::guest_sources;
use super::layout::{FIRST_BOOT_FILES, GUEST_SETUP, META_DATA, USER_DATA_TEMPLATE};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::iso::iso_maker;
use crate::machine::Machine;
use crate::snapshots::copy_tree;
use crate::writable_copy::copy_writable;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A random password's size, before base64.
const PASSWORD_BYTES: &str = "12";
const PRIVATE: u32 = 0o600;
/// The modes the guest's scripts get, as `/usr/lib/nvfwd` and what's in it are read by the desktop's
/// user. A copy keeps the host's modes, and a host whose umask is 077 unpacked these as its own only.
const OPEN_FOLDER: u32 = 0o755;
const OPEN_PROGRAM: u32 = 0o755;
const OPEN_FILE: u32 = 0o644;
const ANY_EXECUTE: u32 = 0o111;

pub fn seed_folder(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join("seed")
}

pub fn seed_disc(context: &Context, machine: &Machine) -> PathBuf {
    seed_folder(context, machine).join("cidata.iso")
}

fn output(command: &mut Command) -> Result<String> {
    let result = command.output()?;
    if !result.status.success() {
        let said = String::from_utf8_lossy(&result.stderr);
        return Err(Failure(format!("{:?} failed: {}", command.get_program(), said.trim())));
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn ssh_key(folder: &Path) -> Result<String> {
    fs::create_dir_all(folder)?;
    let key = folder.join("id");
    if !key.is_file() {
        output(
            Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "nvfwd", "-f"]).arg(&key),
        )?;
    }
    Ok(fs::read_to_string(folder.join("id.pub"))?.trim().to_owned())
}

fn password_hash(file: &Path) -> Result<String> {
    if !file.is_file() {
        let password = output(Command::new("openssl").args(["rand", "-base64", PASSWORD_BYTES]))?;
        let mut new = fs::OpenOptions::new().write(true).create_new(true).mode(PRIVATE).open(file)?;
        writeln!(new, "{password}")?;
    }
    let mut child = Command::new("openssl")
        .args(["passwd", "-6", "-stdin"])
        .stdin(Stdio::from(fs::File::open(file)?))
        .stdout(Stdio::piped())
        .spawn()?;
    let mut hash = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        std::io::Read::read_to_string(&mut stdout, &mut hash)?;
    }
    child.wait()?;
    Ok(hash.trim().to_owned())
}

/// The answers, with this machine's account and secrets and the kernel on its ISO.
pub use guesthouse_answers_core::linux_host::ubuntu::fill_template;

fn make_payload(sources: &Path, folder: &Path) -> Result<PathBuf> {
    let payload = folder.join("payload");
    let _ = fs::remove_dir_all(&payload);
    copy_tree(&sources.join(GUEST_SETUP), &payload)?;
    // These two are needed before any driver image has been seen.
    for name in FIRST_BOOT_FILES {
        let file = Path::new(name).file_name().unwrap_or_default();
        fs::copy(sources.join(name), payload.join(file))?;
    }
    open_up(&payload)?;
    let archive = folder.join("nvfwd-guest.tar");
    output(
        Command::new("tar")
            .args(["--owner=root", "--group=root", "-cf"])
            .arg(&archive)
            .arg("-C")
            .arg(&payload)
            .arg("."),
    )?;
    let _ = fs::remove_dir_all(&payload);
    Ok(archive)
}

fn open_up(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)?;
    let mode = if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            open_up(&entry?.path())?;
        }
        OPEN_FOLDER
    } else if metadata.permissions().mode() & ANY_EXECUTE != 0 {
        OPEN_PROGRAM
    } else {
        OPEN_FILE
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Writes the seed disc, and returns where it is.
pub fn make_seed(context: &Context, machine: &Machine, kernel_package: &str) -> Result<PathBuf> {
    let sources = guest_sources(context);
    let folder = seed_folder(context, machine);
    fs::create_dir_all(&folder)?;
    let key = ssh_key(&machine.folder(context).join("ssh"))?;
    let hash = password_hash(&machine.password_file(context))?;
    let template = fs::read_to_string(sources.join(USER_DATA_TEMPLATE))?;
    let user_data = folder.join("user-data");
    let answers = fill_template(&template, machine.account(), &hash, &key, kernel_package);
    fs::write(&user_data, answers + "\n")?;
    let meta_data = folder.join("meta-data");
    copy_writable(sources.join(META_DATA), &meta_data)?;
    let archive = make_payload(&sources, &folder)?;
    let disc = seed_disc(context, machine);
    output(
        Command::new(iso_maker())
            .args(["-quiet", "-r", "-V", "cidata", "-o"])
            .arg(&disc)
            .arg("-graft-points")
            .arg(format!("user-data={}", user_data.display()))
            .arg(format!("meta-data={}", meta_data.display()))
            .arg(format!("nvfwd-guest.tar={}", archive.display())),
    )?;
    let _ = fs::remove_file(archive);
    Ok(disc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Kind;
    use crate::testing::{Scratch, scratch_context};
    use std::os::unix::fs::MetadataExt;

    fn with_sources(scratch: &Scratch) {
        let template = "username: \"@USER@\"\npassword: \"@PASSWORD_HASH@\"\nkey: \"@SSH_KEY@\"\nkernel: \"@KERNEL_PACKAGE@\"\n";
        scratch.write("program/linux/setup/seed/user-data.template", template);
        scratch.write("program/linux/setup/seed/meta-data", "instance-id: nvfwd-1\n");
        scratch.write("program/linux/setup/guest/first-boot.sh", "#!/bin/sh\n");
        scratch.write("program/linux/cast/Xwayland", "#!/bin/sh\n");
        scratch.write("program/linux/cast/firefox.desktop", "[Desktop Entry]\n");
    }

    fn machine(context: &Context) -> Machine {
        let machine = Machine { kind: Kind::Linux, ..Machine::new("ubuntu", "Ubuntu", "/d", "") };
        fs::create_dir_all(machine.folder(context)).unwrap();
        machine
    }

    fn on_disc(disc: &Path, name: &str) -> Vec<u8> {
        Command::new("isoinfo").args(["-R", "-x", name, "-i"]).arg(disc).output().unwrap().stdout
    }

    #[test]
    fn every_mark_in_the_template_is_filled() {
        let filled =
            fill_template("@USER@ @PASSWORD_HASH@ @SSH_KEY@ @KERNEL_PACKAGE@ @SSH_KEY@", "u", "h", "k", "p");
        assert_eq!(filled, "u h k p k");
    }

    #[test]
    fn the_disc_holds_the_answers_and_the_first_boot_scripts() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        with_sources(&scratch);
        let vm = machine(&context);
        let disc = make_seed(&context, &vm, "linux-image-7.0.0-38-generic").unwrap();
        let answers = String::from_utf8(on_disc(&disc, "/user-data")).unwrap();
        assert!(answers.contains("linux-image-7.0.0-38-generic") && answers.contains("ssh-ed25519"));
        assert!(answers.contains("password: \"$6$"), "only the hash goes in");
        assert!(answers.contains("username: \"desk\""), "the account the scripts have always made");
        let tar = scratch.path("guest.tar");
        fs::write(&tar, on_disc(&disc, "/nvfwd-guest.tar")).unwrap();
        let listing = Command::new("tar").args(["-tf"]).arg(&tar).output().unwrap().stdout;
        let listing = String::from_utf8_lossy(&listing);
        assert!(listing.contains("./first-boot.sh") && listing.contains("./Xwayland"));
    }

    #[test]
    fn the_scripts_can_be_read_by_everyone_whatever_the_host_unpacked_them_as() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        with_sources(&scratch);
        let session = scratch.write("program/linux/setup/guest/desktop-session.sh", "#!/bin/sh\n");
        fs::set_permissions(&session, fs::Permissions::from_mode(0o700)).unwrap();
        let note = scratch.write("program/linux/setup/guest/units/nvfwd-desktop.service", "[Unit]\n");
        fs::set_permissions(&note, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(note.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        let disc = make_seed(&context, &machine(&context), "linux-image-7.0.0-38-generic").unwrap();
        let tar = scratch.path("guest.tar");
        fs::write(&tar, on_disc(&disc, "/nvfwd-guest.tar")).unwrap();
        let listing = Command::new("tar").args(["-tvf"]).arg(&tar).output().unwrap().stdout;
        let listing = String::from_utf8_lossy(&listing);
        let mode_of = |name: &str| {
            let line =
                listing.lines().find(|line| line.ends_with(name)).unwrap_or_else(|| panic!("{listing}"));
            line.split_whitespace().next().unwrap().to_owned()
        };
        assert_eq!(mode_of("./desktop-session.sh"), "-rwxr-xr-x");
        assert_eq!(mode_of("./units/nvfwd-desktop.service"), "-rw-r--r--");
        assert_eq!(mode_of("./units/"), "drwxr-xr-x");
        assert_eq!(mode_of("./"), "drwxr-xr-x");
    }

    #[test]
    fn a_chosen_account_and_password_reach_the_answers() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        with_sources(&scratch);
        let vm = Machine { user: "ana".into(), ..machine(&context) };
        fs::create_dir_all(vm.password_file(&context).parent().unwrap()).unwrap();
        fs::write(vm.password_file(&context), "chosen\n").unwrap();
        let disc = make_seed(&context, &vm, "linux-image-7.0.0-38-generic").unwrap();
        let answers = String::from_utf8(on_disc(&disc, "/user-data")).unwrap();
        assert!(answers.contains("username: \"ana\""), "{answers}");
        let hash = answers.split("password: \"").nth(1).unwrap().split('"').next().unwrap().to_owned();
        let check = Command::new("openssl")
            .args(["passwd", "-6", "-salt"])
            .arg(hash.split('$').nth(2).unwrap())
            .arg("chosen")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&check.stdout).trim(), hash, "the hash is of the chosen password");
    }

    #[test]
    fn secrets_are_private_and_stay_as_they_were() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        with_sources(&scratch);
        let vm = machine(&context);
        make_seed(&context, &vm, "linux-image-7.0.0-38-generic").unwrap();
        let file = vm.password_file(&context);
        let password = fs::read_to_string(&file).unwrap();
        let key = fs::read_to_string(vm.folder(&context).join("ssh/id.pub")).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, PRIVATE);
        make_seed(&context, &vm, "linux-image-7.0.0-38-generic").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), password);
        assert_eq!(fs::read_to_string(vm.folder(&context).join("ssh/id.pub")).unwrap(), key);
    }
}
