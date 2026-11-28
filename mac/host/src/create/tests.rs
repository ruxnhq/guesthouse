// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh folder for one test, with a machines folder and some source files in it.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Scratch {
    fn new() -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "gh-create-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("Machines")).unwrap();
        Scratch(path)
    }

    fn machines(&self) -> PathBuf {
        self.0.join("Machines")
    }

    fn source(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    fn request(&self, name: &str) -> NewMachine {
        NewMachine {
            name: name.into(),
            cores: 4,
            memory_mb: 6144,
            disk: self.source("in.img", b"disk"),
            firmware_code: self.source("in-code.fd", b"code"),
            firmware_vars: self.source("in-vars.fd", b"vars"),
            option_rom: None,
            host_memory_mb: 16384,
            host_cores: 10,
        }
    }
}

#[test]
fn a_good_request_makes_a_machine_the_store_reads() {
    let s = Scratch::new();
    let mut request = s.request("Windows 11");
    request.option_rom = Some(s.source("in.rom", b"rom"));
    let id = create(&s.machines(), &request).unwrap();
    assert_eq!(id, "windows-11");
    let folder = s.machines().join(&id);
    assert_eq!(fs::read(folder.join("disk.img")).unwrap(), b"disk");
    assert_eq!(fs::read(folder.join("option.rom")).unwrap(), b"rom");
    let listing = store::list(&s.machines()).unwrap();
    assert!(listing.skipped.is_empty());
    let machine = &listing.machines[0];
    assert_eq!(machine.name, "Windows 11");
    assert_eq!((machine.cores, machine.memory_mb), (4, 6144));
    assert!(machine.startable);
    // The source files are left alone.
    assert!(request.disk.is_file());
}

#[test]
fn the_files_are_copies_not_the_originals() {
    let s = Scratch::new();
    let request = s.request("A");
    let id = create(&s.machines(), &request).unwrap();
    fs::write(s.machines().join(id).join("vars.fd"), b"changed").unwrap();
    assert_eq!(fs::read(&request.firmware_vars).unwrap(), b"vars");
}

#[test]
fn every_problem_is_reported_together() {
    let s = Scratch::new();
    let mut request = s.request("  ");
    request.cores = 64;
    request.memory_mb = 1024;
    request.disk = s.0.join("nope.img");
    let found = problems(&s.machines(), &request);
    assert_eq!(found.len(), 4, "{found:?}");
    assert!(found.iter().any(|p| p.contains("name")));
    assert!(found.iter().any(|p| p.contains("Processors")));
    assert!(found.iter().any(|p| p.contains("Memory")));
    assert!(found.iter().any(|p| p.contains("disk image is not there")));
    // Nothing was made.
    assert_eq!(fs::read_dir(s.machines()).unwrap().count(), 0);
}

#[test]
fn memory_stays_within_the_mac() {
    let s = Scratch::new();
    let mut request = s.request("Big");
    request.memory_mb = 16384;
    let found = problems(&s.machines(), &request);
    assert!(found[0].contains("at most 14 GB"), "{found:?}");
    request.memory_mb = 14336;
    assert!(problems(&s.machines(), &request).is_empty());
}

#[test]
fn a_qcow2_disk_is_refused() {
    let s = Scratch::new();
    let mut request = s.request("Q");
    request.disk = s.source("q.qcow2", b"QFI\xfb\0\0\0\x03");
    let found = problems(&s.machines(), &request);
    assert!(found.iter().any(|p| p.contains("qcow2")));
}

#[test]
fn a_second_machine_with_the_same_name_is_refused_and_ids_never_collide() {
    let s = Scratch::new();
    create(&s.machines(), &s.request("Win")).unwrap();
    let found = problems(&s.machines(), &s.request("Win"));
    assert_eq!(found, vec!["A machine called Win already exists.".to_string()]);
    // Different names that make the same id get different folders.
    let first = create(&s.machines(), &s.request("a b")).unwrap();
    let second = create(&s.machines(), &s.request("a-b")).unwrap();
    assert_ne!(first, second);
}

#[test]
fn ids_are_short_and_plain() {
    let s = Scratch::new();
    let id = create(&s.machines(), &s.request("Ünïcode Name With A Very Long Title Indeed")).unwrap();
    assert!(id.len() <= MAX_ID_CHARS, "{id}");
    assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
}

#[test]
fn a_support_folder_with_no_room_for_sockets_is_said_plainly() {
    let s = Scratch::new();
    let deep = s.0.join("x".repeat(100)).join("Machines");
    fs::create_dir_all(&deep).unwrap();
    let found = problems(&deep, &s.request("W"));
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("too long for the machine's sockets"));
}

#[test]
fn a_failed_copy_leaves_no_folder() {
    let s = Scratch::new();
    let mut request = s.request("F");
    // Passes the checks (it is a file) but cannot be read as one while copying.
    request.option_rom = Some(s.0.clone());
    assert!(!problems(&s.machines(), &request).is_empty());
    assert!(create(&s.machines(), &request).is_err());
    assert_eq!(fs::read_dir(s.machines()).unwrap().count(), 0);
}

#[test]
fn rename_keeps_every_other_setting() {
    let s = Scratch::new();
    let id = create(&s.machines(), &s.request("Old")).unwrap();
    let file = s.machines().join(&id).join(MACHINE_FILE);
    let mut settings: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    settings["modes"] = serde_json::json!(["1024x768@60"]);
    fs::write(&file, settings.to_string()).unwrap();

    rename(&s.machines(), &id, " New ").unwrap();
    let after: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(after["name"], "New");
    assert_eq!(after["modes"], settings["modes"]);
    assert_eq!(after["cores"], 4);
}

#[test]
fn rename_refuses_empty_taken_and_unknown() {
    let s = Scratch::new();
    let a = create(&s.machines(), &s.request("A")).unwrap();
    create(&s.machines(), &s.request("B")).unwrap();
    assert!(rename(&s.machines(), &a, "").is_err());
    assert!(rename(&s.machines(), &a, "B").unwrap_err().contains("already exists"));
    assert!(rename(&s.machines(), "nope", "C").is_err());
    // Keeping its own name is fine.
    rename(&s.machines(), &a, "A").unwrap();
}

#[test]
fn hardware_is_changed_within_the_macs_size_and_nothing_else_moves() {
    let s = Scratch::new();
    let id = create(&s.machines(), &s.request("H")).unwrap();
    set_hardware(&s.machines(), &id, 2, 8192, 10, 16384).unwrap();
    let machine = store::find(&s.machines(), &id).unwrap();
    assert_eq!((machine.cores, machine.memory_mb), (2, 8192));
    assert!(machine.startable);
    let found = set_hardware(&s.machines(), &id, 0, 1024, 10, 16384).unwrap_err();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(set_hardware(&s.machines(), &id, 4, 16384, 10, 16384).is_err());
    // A refused change leaves the file as it was.
    assert_eq!(store::find(&s.machines(), &id).unwrap().cores, 2);
    assert!(set_hardware(&s.machines(), "nope", 2, 4096, 10, 16384).is_err());
}

#[test]
fn a_new_machine_is_guarded_and_graphics_can_be_moved_and_a_missing_key_reads_as_guarded() {
    let s = Scratch::new();
    let id = create(&s.machines(), &s.request("G")).unwrap();
    assert_eq!(store::find(&s.machines(), &id).unwrap().graphics, "guarded");
    set_graphics(&s.machines(), &id, "qemu").unwrap();
    let machine = store::find(&s.machines(), &id).unwrap();
    assert_eq!(machine.graphics, "qemu");
    assert!(machine.startable);
    assert!(set_graphics(&s.machines(), &id, "magic").is_err());
    // A file from before the setting existed.
    let file = s.machines().join(&id).join(MACHINE_FILE);
    let mut settings: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    settings.as_object_mut().unwrap().remove("graphics");
    fs::write(&file, settings.to_string()).unwrap();
    assert_eq!(store::find(&s.machines(), &id).unwrap().graphics, "guarded");
}
