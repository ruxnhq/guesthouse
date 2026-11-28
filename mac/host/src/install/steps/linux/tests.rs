// SPDX-License-Identifier: GPL-2.0-or-later
use super::super::{RealSteps, Setup};
use super::*;
use crate::install::fake::{FakeGuest, FakeNetwork};
use crate::install::plan::{Guest, Plan, Step};
use crate::install::runner::Runner;
use crate::install::state::{InstallInfo, Source};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Ubuntu installed by the stand-ins as far as the guest's own installer, from a machine folder
/// and QEMU's folder with the firmware and the device's option ROM in it.
fn install_system(name: &str) -> (PathBuf, FakeGuest, serde_json::Value) {
    let root = std::env::temp_dir().join(format!("gh-linux-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let machines = root.join("Machines");
    fs::create_dir_all(machines.join("m")).unwrap();
    fs::write(
        machines.join("m/machine.json"),
        r#"{"name":"m","kind":"linux","cores":4,"memory_mb":3072,"disk":"disk.img","firmware_code":"code.fd","firmware_vars":"vars.fd","graphics":"guarded"}"#,
    )
    .unwrap();
    let qemu = root.join("qemu/pc-bios");
    fs::create_dir_all(&qemu).unwrap();
    for (file, text) in
        [("edk2-aarch64-code.fd", "code"), ("edk2-aarch64-vars.fd", "vars"), (OPTION_ROM, "rom")]
    {
        fs::write(qemu.join(file), text).unwrap();
    }
    let mut control = FakeGuest::quick(Guest::Ubuntu);
    control.real_pause_ms = 0;
    let mut network = FakeNetwork;
    let mut steps = RealSteps {
        setup: Setup {
            machines: machines.clone(),
            id: "m".into(),
            guest: Guest::Ubuntu,
            disk_gb: 1,
            mac_user: "nick".into(),
            downloads: root.join("Downloads"),
            driver_dir: None,
            password: Some("k7mn-p3xq-ab9d-w4ht".into()),
            check_space: false,
            firmware_dirs: vec![qemu],
            desktop: Default::default(),
        },
        transport: &mut network,
        control: &mut control,
    };
    let mut info = InstallInfo::new("m", &Plan::for_guest(Guest::Ubuntu), Source::Download);
    let mut changed = |_: &InstallInfo| {};
    let clock = || 0;
    // Up to the driver step, which has no package here.
    let mut runner = Runner {
        folder: machines.join("m"),
        info: info.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        clock: &clock,
        changed: &mut changed,
    };
    runner.run(&mut steps);
    info = runner.info;
    assert_eq!(info.failure.unwrap().step, Step::InstallDriver);
    let file = serde_json::from_str(&fs::read_to_string(machines.join("m/machine.json")).unwrap()).unwrap();
    (root, control, file)
}

#[test]
fn the_installer_starts_by_its_kernel_with_the_unattended_line_and_the_rom_gives_a_picture() {
    let (root, control, file) = install_system("boot");
    let folder = root.join("Machines/m");
    assert_eq!(control.extracted, ["casper/vmlinuz", "casper/initrd"]);
    assert_eq!(file["installer_kernel"], "installer-boot/casper/vmlinuz");
    assert_eq!(file["installer_initrd"], "installer-boot/casper/initrd");
    assert!(folder.join("installer-boot/casper/vmlinuz").is_file());
    let line = file["installer_command_line"].as_str().unwrap();
    assert!(line.starts_with("autoinstall ds=nocloud "));
    assert_eq!(file["option_rom"], OPTION_ROM);
    assert_eq!(fs::read(folder.join(OPTION_ROM)).unwrap(), b"rom");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_answer_disc_carries_the_drivers_service_and_the_answers_put_it_in() {
    let (root, control, _) = install_system("service");
    let (label, text) = &control.discs[0];
    assert_eq!(label, "CIDATA");
    assert!(text.contains(ubuntu::DRIVER_SERVICE) && text.contains(ubuntu::DRIVER_RUNNER));
    assert!(text.contains(&format!("systemctl enable {}", ubuntu::DRIVER_SERVICE_FILE)));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_option_rom_is_found_beside_the_firmware_or_not_at_all() {
    let root = std::env::temp_dir().join(format!("gh-linux-rom-{}", std::process::id()));
    fs::create_dir_all(root.join("b")).unwrap();
    fs::write(root.join("b").join(OPTION_ROM), "rom").unwrap();
    assert_eq!(find_option_rom(&[root.join("a"), root.join("b")]), Some(root.join("b").join(OPTION_ROM)));
    assert_eq!(find_option_rom(&[root.join("a")]), None);
    let _ = fs::remove_dir_all(root);
}
