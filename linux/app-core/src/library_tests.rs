//! More of what removing a machine does.

use crate::library::delete;
use crate::machine::Machine;
use crate::testing::{Scratch, scratch_context};
use std::fs;
use std::process;

#[test]
fn a_disk_that_a_vm_outside_the_app_has_open_is_not_deleted() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let folder = context.paths.machines.join("held");
    fs::create_dir_all(&folder).unwrap();
    let disk = folder.join("disk.qcow2");
    let made = std::process::Command::new("qemu-img")
        .args(["create", "-q", "-f", "qcow2"])
        .arg(&disk)
        .arg("1M")
        .status()
        .unwrap();
    assert!(made.success());
    fs::write(folder.join("vars.fd"), "v").unwrap();
    let held =
        Machine::new("held", "Held", &disk.to_string_lossy(), &folder.join("vars.fd").to_string_lossy());
    held.save(&context).unwrap();
    let mut holder = std::process::Command::new("qemu-io")
        .args(["-f", "qcow2", "-c", "sleep 20000"])
        .arg(&disk)
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::inherit())
        .spawn()
        .unwrap();
    // Looking at the disk meanwhile would take the lock from it, so it is only given time.
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(holder.try_wait().unwrap().is_none(), "the holder took the disk");
    let refused = delete(&context, &held);
    let _ = holder.kill();
    let _ = holder.wait();
    let said = refused.unwrap_err().0;
    assert!(said.contains("in use by a VM"), "{said}");
    assert!(disk.exists());
    delete(&context, &held).unwrap();
    assert!(!folder.exists());
}
