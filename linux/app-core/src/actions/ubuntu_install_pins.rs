//! What an Ubuntu install's steps do today, held before their plain half moves: the steps it says,
//! the two hours it is given, how often it looks, and every sentence. Time is a fake clock's.

use super::*;
use crate::actions::tests::{FakeClock, with_vm_running};
use crate::testing::{Scratch, scratch_context};
use crate::vm_host::stand_in::RecordingHost;
use guesthouse_basics::clock::Clock;
use std::sync::Arc;

fn ubuntu_to_install(context: &crate::context::Context, iso: &str) -> crate::machine::Machine {
    let folder = context.paths.machines.join("ubuntu");
    fs::create_dir_all(&folder).unwrap();
    let disk = folder.join("disk.qcow2");
    fs::write(&disk, "").unwrap();
    let machine = crate::machine::Machine {
        kind: crate::machine::Kind::Linux,
        cores: 2,
        ram_gib: 2,
        installing: true,
        iso: iso.to_owned(),
        ..crate::machine::Machine::new("ubuntu", "Ubuntu", &disk.to_string_lossy(), "")
    };
    machine.save(context).unwrap();
    machine
}

/// The installer is given two hours to power the VM off, looked at every 500 ms.
#[test]
fn the_installer_is_given_two_hours_to_power_off() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let clock = Arc::new(FakeClock::default());
    context.clock = clock.clone();
    let host = RecordingHost::default();
    let task = Task::new(&context, &host, "install", ubuntu_to_install(&context, ""));
    wait_for_poweroff(&task).unwrap();
    assert_eq!(clock.now(), Duration::ZERO, "a VM that is off is waited for no longer");
    with_vm_running(&context);
    let error = wait_for_poweroff(&task).unwrap_err();
    assert_eq!(error.0, "The install did not finish in 2 hours. Show Screen says how far it got.");
    assert_eq!(clock.now(), Duration::from_secs(2 * 3600));
}

#[test]
fn every_refusal_before_an_ubuntu_install_and_its_first_step() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let host = RecordingHost::default();
    let elsewhere = crate::machine::Machine {
        kind: crate::machine::Kind::Linux,
        installing: true,
        ..crate::machine::Machine::new("elsewhere", "Elsewhere", "/nowhere/disk", "")
    };
    let mut task = Task::new(&context, &host, "install", elsewhere);
    assert_eq!(
        install(&mut task).unwrap_err().0,
        "This machine's disk is not in its folder, so nothing will install over it."
    );
    let gone = scratch.path("gone.iso").display().to_string();
    let mut task = Task::new(&context, &host, "install", ubuntu_to_install(&context, &gone));
    assert_eq!(install(&mut task).unwrap_err().0, format!("{gone} is missing."));
    assert!(task.said.lock().unwrap().is_empty());
    let not_ubuntu = scratch.write("not-ubuntu.iso", "not a disc").display().to_string();
    let mut task = Task::new(&context, &host, "install", ubuntu_to_install(&context, &not_ubuntu));
    assert_eq!(
        install(&mut task).unwrap_err().0,
        format!("{not_ubuntu} holds no generic kernel in its pool, so it is not an Ubuntu server ISO.")
    );
    assert_eq!(task.said.lock().unwrap().as_slice(), ["Preparing the install"]);
}

/// A file the install takes from the disc that isn't on it.
#[test]
fn a_disc_without_the_installers_kernel_is_not_a_server_disc() {
    let scratch = Scratch::new();
    let not_ubuntu = scratch.write("not-ubuntu.iso", "not a disc").display().to_string();
    let error = extract(&not_ubuntu, "/casper/vmlinuz", &scratch.path("vmlinuz")).unwrap_err();
    assert_eq!(error.0, format!("{not_ubuntu} has no /casper/vmlinuz, so it is not an Ubuntu server ISO."));
    assert!(!scratch.path("vmlinuz").exists());
}
