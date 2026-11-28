// SPDX-License-Identifier: GPL-2.0-or-later
#![allow(clippy::field_reassign_with_default)]
use super::*;
use crate::install::fake::{FakeGuest as Control, FakeNetwork as Network};
use crate::install::plan::Plan;
use crate::install::runner::Runner;
use crate::install::state::{Choice, Phase, StepStatus};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::install::fake::UBUNTU_BYTES;

const PASSWORD: &str = "k7mn-p3xq-ab9d-w4ht";

struct World {
    root: PathBuf,
    machines: PathBuf,
    /// What the install is given as the account's password; none to try the missing case.
    password: Option<String>,
}

impl World {
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!("gh-steps-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let machines = root.join("Machines");
        fs::create_dir_all(machines.join("m")).unwrap();
        fs::write(
            machines.join("m").join(MACHINE_FILE),
            r#"{"name":"m","kind":"linux","cores":4,"memory_mb":6144,"disk":"disk.img","firmware_code":"code.fd","firmware_vars":"vars.fd","graphics":"guarded"}"#,
        )
        .unwrap();
        // QEMU's firmware, one file compressed as the lab keeps it.
        let firmware = root.join("firmware");
        fs::create_dir_all(&firmware).unwrap();
        fs::write(firmware.join("edk2-aarch64-code.fd"), b"code").unwrap();
        fs::write(firmware.join("edk2-arm-vars.fd"), b"vars").unwrap();
        World { root, machines, password: Some(PASSWORD.into()) }
    }

    fn setup(&self, guest: Guest, driver: bool) -> Setup {
        let driver_dir = self.root.join("driver");
        if driver {
            fs::create_dir_all(&driver_dir).unwrap();
            for name in driver::REQUIRED {
                fs::write(driver_dir.join(name), "x").unwrap();
            }
        }
        Setup {
            machines: self.machines.clone(),
            id: "m".into(),
            guest,
            disk_gb: 1,
            mac_user: "nick".into(),
            downloads: self.root.join("Downloads"),
            password: self.password.clone(),
            driver_dir: driver.then_some(driver_dir),
            // The room on this Mac is not what these tests are about (one test sets it on, with a
            // disk no Mac has), so they pass on a full disk too.
            check_space: false,
            firmware_dirs: vec![self.root.join("firmware")],
            desktop: Default::default(),
        }
    }

    fn big_iso(&self) -> String {
        let path = self.root.join("Win11_arm64.iso");
        File::create(&path).unwrap().set_len(2_000_000_001).unwrap();
        path.display().to_string()
    }
}

fn run(
    world: &World,
    guest: Guest,
    source: Source,
    control: &mut Control,
    driver: bool,
    info: Option<InstallInfo>,
) -> InstallInfo {
    run_on(world, guest, source, control, driver, info, &mut Network)
}

fn run_on(
    world: &World,
    guest: Guest,
    source: Source,
    control: &mut Control,
    driver: bool,
    info: Option<InstallInfo>,
    network: &mut dyn Transport,
) -> InstallInfo {
    let setup = world.setup(guest, driver);
    let mut steps = RealSteps { setup, transport: network, control };
    let mut changed = |_: &InstallInfo| {};
    let clock = || 0;
    let mut runner = Runner {
        folder: world.machines.join("m"),
        info: info.unwrap_or_else(|| InstallInfo::new("m", &Plan::for_guest(guest), source)),
        cancel: Arc::new(AtomicBool::new(false)),
        clock: &clock,
        changed: &mut changed,
    };
    runner.run(&mut steps);
    runner.info
}

fn machine_file(world: &World) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(world.machines.join("m").join(MACHINE_FILE)).unwrap()).unwrap()
}

#[test]
fn ubuntu_is_downloaded_checked_installed_and_stops_at_the_driver_when_it_could_not_be_downloaded() {
    let world = World::new("ubuntu");
    let mut control = Control::default();
    control.says = vec![(60_000, ubuntu::FINISHED.into())];
    control.stops_at = Some(90_000);
    let info = run(&world, Guest::Ubuntu, Source::Download, &mut control, false, None);

    // The download was kept, checked, and named in the machine's file for the installer to boot.
    let file = machine_file(&world);
    let installer = file["installer"].as_str().unwrap();
    assert!(installer.ends_with("ubuntu/ubuntu-26.04-live-server-arm64.iso"));
    assert_eq!(fs::read(installer).unwrap(), UBUNTU_BYTES);
    // The disk is sparse at its size, with QEMU's firmware beside it.
    let folder = world.machines.join("m");
    assert_eq!(fs::metadata(folder.join("disk.img")).unwrap().len(), 1024 * 1024 * 1024);
    assert_eq!(fs::read(folder.join("code.fd")).unwrap(), b"code");
    // The answers went on a CIDATA disc with the account named after the Mac's user.
    let (label, text) = &control.discs[0];
    assert_eq!(label, "CIDATA");
    assert!(text.contains("username: nick") && text.contains("instance-id: guesthouse-m"));
    assert_eq!(file["answer_disc"], "answers.iso");

    // It stopped at the driver step, saying why the package was not got, with its choices.
    assert_eq!(info.state, Phase::Failed);
    let failure = info.failure.clone().unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::InstallDriver, Some("driver_download")));
    assert!(failure.reason.contains("could not be downloaded"), "{}", failure.reason);
    assert!(
        failure.reason.contains("is not somewhere the stand-in knows"),
        "the reason is kept: {}",
        failure.reason
    );
    assert!(!failure.reason.contains("not packaged"));
    assert!(failure.choices.contains(&Choice::OpenWithoutDriver));
    assert!(failure.choices.contains(&Choice::TryAgain));

    // Open without it: the first start boots the disk alone and the install is done.
    let mut after = info;
    assert!(after.open_without_driver());
    let mut control = Control { now: 200_000, ..Control::default() };
    let done = run(&world, Guest::Ubuntu, Source::Download, &mut control, false, Some(after));
    assert_eq!(done.state, Phase::Done);
    assert_eq!(done.status_of(Step::InstallDriver), StepStatus::Skipped);
    let file = machine_file(&world);
    assert!(file.get("installer").is_none() && file.get("answer_disc").is_none());
    assert_eq!(control.started, ["m"]);
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn windows_is_installed_from_a_chosen_file_with_our_answers_and_the_driver_on_a_stick() {
    let world = World::new("windows");
    let iso = world.big_iso();
    let mut control = Control::default();
    control.says = vec![
        (120_000, windows::marker::SYSTEM_INSTALLED.into()),
        (180_000, windows::marker::DRIVER_INSTALLED.into()),
        (400_000, windows::marker::FIRST_START.into()),
    ];
    let info = run(&world, Guest::Windows, Source::File { path: iso.clone() }, &mut control, true, None);
    assert_eq!(info.state, Phase::Done, "{:?}", info.failure);
    let (label, text) = &control.discs[0];
    assert_eq!(label, "GPUPV_SETUP");
    assert!(text.contains("<Name>nick</Name>") && text.contains("bcdedit /set testsigning on"));
    assert!(text.contains(PASSWORD), "the answers have the password");
    let mut files = control.stick_files.clone();
    files.sort();
    let mut wanted: Vec<&str> = driver::REQUIRED.to_vec();
    wanted.sort();
    assert_eq!(files, wanted);
    // Nothing of the answers, and so no password, stays on the Mac once the disc is out.
    assert!(!world.machines.join("m").join("answers.iso").exists());
    assert!(!world.machines.join("m").join("answers").exists());
    // The stick is attached while the installer runs and gone from the finished machine.
    let file = machine_file(&world);
    assert!(file.get("stick").is_none() && file.get("installer").is_none());
    assert_eq!(control.started, ["m"], "Windows installs and restarts itself in one run");
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn windows_needs_a_chosen_installer_and_says_so_and_a_chosen_one_goes_on() {
    let world = World::new("choose");
    let mut control = Control::default();
    let mut info = run(&world, Guest::Windows, Source::Download, &mut control, true, None);
    let failure = info.failure.clone().unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::Fetch, Some("choose_installer")));
    assert_eq!(failure.choices, [Choice::TryAgain, Choice::ChooseInstaller]);
    assert!(failure.reason.contains("Microsoft"));
    assert!(control.started.is_empty());

    assert!(info.choose_installer(&world.big_iso()));
    control.says = vec![
        (1_000, windows::marker::SYSTEM_INSTALLED.into()),
        (2_000, windows::marker::DRIVER_INSTALLED.into()),
        (3_000, windows::marker::FIRST_START.into()),
    ];
    let done = run(&world, Guest::Windows, Source::Download, &mut control, true, Some(info));
    assert_eq!(done.state, Phase::Done, "{:?}", done.failure);
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn a_chosen_file_that_is_not_an_installer_is_refused_before_anything_starts() {
    let world = World::new("bad");
    let mut control = Control::default();
    let small = world.root.join("tiny.iso");
    fs::write(&small, "x").unwrap();
    let info = run(
        &world,
        Guest::Windows,
        Source::File { path: small.display().to_string() },
        &mut control,
        true,
        None,
    );
    let failure = info.failure.unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::Fetch, Some("bad_installer")));
    assert!(failure.reason.contains("too small"));
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn a_driver_windows_turns_away_gives_its_code_and_the_choice_to_go_on_without() {
    let world = World::new("driver");
    let mut control = Control::default();
    control.says = vec![
        (1_000, windows::marker::SYSTEM_INSTALLED.into()),
        (2_000, format!("{} 0x800b0109", windows::marker::DRIVER_FAILED)),
    ];
    let info = run(&world, Guest::Windows, Source::File { path: world.big_iso() }, &mut control, true, None);
    let failure = info.failure.unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::InstallDriver, Some("0x800b0109")));
    assert!(failure.reason.contains("0x800b0109"));
    assert!(failure.choices.contains(&Choice::OpenWithoutDriver));
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn too_little_room_says_how_much_to_free() {
    let error = room_for(20, 12_400_000_000).unwrap_err();
    assert_eq!(error.code.as_deref(), Some("no_space"));
    assert!(
        error.message.contains("needs 20 GB") && error.message.contains("free 8 GB"),
        "{}",
        error.message
    );
    assert!(room_for(20, 24_000_000_000).is_ok());
}

#[test]
fn a_guest_that_never_finishes_times_out_and_one_that_fails_says_why() {
    let world = World::new("timeout");
    let mut control = Control::default();
    let info = run(&world, Guest::Windows, Source::File { path: world.big_iso() }, &mut control, true, None);
    let failure = info.failure.unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::InstallSystem, Some("timeout")));

    let world = World::new("failed");
    let mut control =
        Control { fails: Some("QEMU stopped: the disk image is not there".into()), ..Control::default() };
    let info = run(&world, Guest::Windows, Source::File { path: world.big_iso() }, &mut control, true, None);
    assert!(info.failure.unwrap().reason.contains("the disk image is not there"));
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn without_a_lab_folder_windows_fetches_the_published_package_and_puts_it_on_the_stick() {
    let world = World::new("nodriver");
    let mut control = Control::default();
    control.says = vec![
        (120_000, windows::marker::SYSTEM_INSTALLED.into()),
        (180_000, windows::marker::DRIVER_INSTALLED.into()),
        (400_000, windows::marker::FIRST_START.into()),
    ];
    let info = run(&world, Guest::Windows, Source::File { path: world.big_iso() }, &mut control, false, None);
    assert_eq!(info.state, Phase::Done, "{:?}", info.failure);
    let mut files = control.stick_files.clone();
    files.sort();
    let mut wanted: Vec<&str> = driver::REQUIRED.to_vec();
    wanted.sort();
    assert_eq!(files, wanted);
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn an_install_without_its_password_stops_before_the_machine_is_started() {
    for guest in [Guest::Windows, Guest::Ubuntu] {
        let mut world = World::new("nopassword");
        world.password = None;
        let mut control = Control::default();
        let info = run(
            &world,
            guest,
            Source::File {
                path: if guest == Guest::Windows {
                    world.big_iso()
                } else {
                    let path = world.root.join("u.iso");
                    fs::write(&path, "x").unwrap();
                    path.display().to_string()
                },
            },
            &mut control,
            true,
            None,
        );
        let failure = info.failure.unwrap();
        assert_eq!((failure.step, failure.code.as_deref()), (Step::InstallSystem, Some("password_missing")));
        assert!(failure.choices.contains(&Choice::TryAgain));
        assert!(control.started.is_empty());
        let _ = fs::remove_dir_all(world.root);
    }
}

#[test]
fn the_machines_file_is_edited_by_key_and_nothing_else_moves() {
    let world = World::new("keys");
    let folder = world.machines.join("m");
    set_keys(&folder, &[("installer", Some("/a.iso")), ("nothing", None)]).unwrap();
    assert_eq!(machine_file(&world)["installer"], "/a.iso");
    set_keys(&folder, &[("installer", None)]).unwrap();
    let file = machine_file(&world);
    assert!(file.get("installer").is_none());
    assert_eq!(file["cores"], 4);
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn firmware_is_found_in_the_first_folder_that_has_both_files_and_its_absence_is_said() {
    let world = World::new("firmware");
    let (code, vars) = find_firmware(&[world.root.join("empty"), world.root.join("firmware")]).unwrap();
    assert_eq!((code, vars), (b"code".to_vec(), b"vars".to_vec()));
    assert_eq!(find_firmware(&[world.root.join("empty")]).unwrap_err().code.as_deref(), Some("no_firmware"));
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn firmware_is_looked_for_in_the_prefix_and_beside_a_qemu_run_from_its_build_folder() {
    let world = World::new("firmware-beside");
    let installed = world.root.join("prefix/bin/qemu-system-aarch64");
    assert_eq!(
        firmware_dirs_of(&installed),
        vec![
            world.root.join("prefix/share/qemu"),
            world.root.join("prefix/bin/pc-bios"),
            world.root.join("prefix/Resources/qemu")
        ]
    );
    // A QEMU in its build folder, as ours is: the firmware is in pc-bios beside it.
    let build = world.root.join("qemu-build");
    fs::create_dir_all(build.join("pc-bios")).unwrap();
    fs::write(build.join("pc-bios/edk2-aarch64-code.fd"), b"code").unwrap();
    fs::write(build.join("pc-bios/edk2-aarch64-vars.fd"), b"vars").unwrap();
    let found = find_firmware(&firmware_dirs_of(&build.join("qemu-system-aarch64"))).unwrap();
    assert_eq!(found, (b"code".to_vec(), b"vars".to_vec()));
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn a_qemu_in_an_app_bundle_finds_its_firmware_in_the_bundles_resources() {
    let world = World::new("firmware-bundle");
    let qemu = world.root.join("Guesthouse.app/Contents/Helpers/qemu-system-aarch64");
    let resources = world.root.join("Guesthouse.app/Contents/Resources/qemu");
    fs::create_dir_all(&resources).unwrap();
    fs::write(resources.join("edk2-aarch64-code.fd"), b"code").unwrap();
    fs::write(resources.join("edk2-aarch64-vars.fd"), b"vars").unwrap();
    let found = find_firmware(&firmware_dirs_of(&qemu)).unwrap();
    assert_eq!(found, (b"code".to_vec(), b"vars".to_vec()));
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn try_again_at_the_driver_step_fetches_the_package_again() {
    use crate::install::fake::FakeSite;
    let world = World::new("again");
    let mut control = Control::default();
    control.says = vec![(60_000, ubuntu::FINISHED.into())];
    control.stops_at = Some(90_000);
    // The first time the file site has no Linux package: the install stops at the driver step.
    let first = run(&world, Guest::Ubuntu, Source::Download, &mut control, false, None);
    assert_eq!(first.failure.as_ref().and_then(|f| f.code.as_deref()), Some("driver_download"));
    // The site has it now, and gives no length to a HEAD. Try Again fetches it and goes on.
    let mut again = first;
    assert!(again.retry());
    let mut control = Control { now: 200_000, ..Control::default() };
    let mut site = FakeSite { no_length: true };
    let second = run_on(&world, Guest::Ubuntu, Source::Download, &mut control, false, Some(again), &mut site);
    assert!(
        second.failure.as_ref().is_none_or(|f| f.code.as_deref() != Some("driver_download")),
        "{:?}",
        second.failure
    );
    assert!(control.stick_files.iter().any(|name| name == driver::LINUX_SCRIPT), "{:?}", control.stick_files);
    let _ = fs::remove_dir_all(world.root);
}

/// The firmware's lines for a Windows disc, as the serial port of the contributor's machine had them.
const FIRMWARE_STARTS: &str = "BdsDxe: starting Boot0002 \"UEFI QEMU QEMU USB HARDDRIVE 1-0000:00:01.0-3\"";
const PROMPT: &str = "Press any key to boot from CD or DVD......";

/// A Windows install's steps on the stand-in guest, for what happens before the installer runs.
fn windows_steps<'a>(world: &World, network: &'a mut Network, control: &'a mut Control) -> RealSteps<'a> {
    RealSteps { setup: world.setup(Guest::Windows, true), transport: network, control }
}

#[test]
fn the_key_is_pressed_while_the_boot_prompt_stands_and_stops_after() {
    let world = World::new("prompt");
    let mut control = Control::default();
    control.says = vec![(2_000, FIRMWARE_STARTS.into()), (3_000, PROMPT.into())];
    let mut network = Network;
    windows_steps(&world, &mut network, &mut control).pass_boot_prompt().unwrap();
    let times: Vec<u64> = control.pressed.iter().map(|(at, _)| *at).collect();
    assert!(control.pressed.iter().all(|(_, key)| key == "spc"));
    // Nothing before the prompt showed, a key every half second after, and none once it had
    // stood for its countdown.
    assert_eq!(times.first(), Some(&3_000));
    assert!(times.windows(2).all(|pair| pair[1] - pair[0] == 500));
    assert!(*times.last().unwrap() < 3_000 + 8_000);
    assert_eq!(times.len(), 16);
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn a_second_prompt_after_a_time_out_is_answered_too_and_the_installer_ends_it() {
    let world = World::new("second");
    let mut control = Control::default();
    control.says = vec![
        (1_000, PROMPT.into()),
        (2_000, "Error: Image at 00045CDF000 start failed: Time out".into()),
        (4_000, PROMPT.into()),
        (9_000, "guesthouse: system installed".into()),
    ];
    let mut network = Network;
    windows_steps(&world, &mut network, &mut control).pass_boot_prompt().unwrap();
    let times: Vec<u64> = control.pressed.iter().map(|(at, _)| *at).collect();
    // The first prompt is answered until it times out, the second from when it shows, and the
    // guest's own words end the pressing.
    assert!(times.iter().any(|at| *at < 2_000) && times.iter().any(|at| *at >= 4_000));
    assert!(times.iter().all(|at| !(2_000..4_000).contains(at)), "none while it is timed out: {times:?}");
    assert!(times.iter().all(|at| *at < 9_000));
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn no_key_is_pressed_when_no_prompt_shows_and_the_wait_is_bounded() {
    let world = World::new("noprompt");
    let mut control = Control::default();
    control.says = vec![(1_000, FIRMWARE_STARTS.into())];
    let mut network = Network;
    windows_steps(&world, &mut network, &mut control).pass_boot_prompt().unwrap();
    assert!(control.pressed.is_empty());
    assert!(control.now <= 91_000, "it gave up when the firmware gave no prompt: {}", control.now);
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn a_firmware_with_nothing_to_boot_is_said_at_once_and_not_waited_out() {
    let world = World::new("nothing");
    let mut control = Control::default();
    control.says =
        vec![(1_000, PROMPT.into()), (9_000, "BdsDxe: No bootable option or device was found.".into())];
    let mut network = Network;
    let error = windows_steps(&world, &mut network, &mut control).pass_boot_prompt().unwrap_err();
    assert_eq!(error.code.as_deref(), Some("no_boot"));
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn a_disk_that_wrote_nothing_for_minutes_is_said_and_one_that_writes_is_not() {
    let world = World::new("quiet");
    let mut control = Control::default();
    control.written = vec![(0, 1_000), (60_000, 5_000)];
    let mut network = Network;
    let mut steps = windows_steps(&world, &mut network, &mut control);
    let mut last = None;
    // Written at the start and a minute in: not quiet yet, however the bytes read.
    assert_eq!(steps.quiet_minutes(&mut last), None);
    steps.control.sleep_ms(120_000);
    assert_eq!(steps.quiet_minutes(&mut last), None, "written a minute in, one minute ago");
    steps.control.sleep_ms(6 * 60_000);
    assert_eq!(
        steps.quiet_minutes(&mut last),
        Some(6),
        "the same bytes for six minutes since it was last seen to change"
    );
    // It writes again: the hint is gone.
    steps.control.sleep_ms(1_000);
    drop(steps);
    control.written.push((control.now, 9_000));
    let mut steps = windows_steps(&world, &mut network, &mut control);
    assert_eq!(steps.quiet_minutes(&mut last), None);
    // A machine that cannot say says nothing.
    drop(steps);
    control.written.clear();
    let mut steps = windows_steps(&world, &mut network, &mut control);
    assert_eq!(steps.quiet_minutes(&mut None), None);
    let _ = fs::remove_dir_all(world.root);
}

#[test]
fn both_guests_get_the_option_rom_found_in_the_bundle_layout() {
    use crate::install::steps::firmware_dirs_of;
    for guest in [Guest::Windows, Guest::Ubuntu] {
        let world = World::new(if guest == Guest::Windows { "rom-win" } else { "rom-linux" });
        // The layout of the app: the QEMU in Contents/Helpers, its data in Contents/Resources/qemu.
        let app = world.root.join("Guesthouse.app/Contents");
        fs::create_dir_all(app.join("Helpers")).unwrap();
        fs::create_dir_all(app.join("Resources/qemu")).unwrap();
        for (name, text) in
            [("edk2-aarch64-code.fd", "code"), ("edk2-aarch64-vars.fd", "vars"), ("pvgfb.rom", "rom")]
        {
            fs::write(app.join("Resources/qemu").join(name), text).unwrap();
        }
        let mut setup = world.setup(guest, true);
        setup.firmware_dirs = firmware_dirs_of(&app.join("Helpers/qemu-system-aarch64"));
        let mut control = Control::default();
        let mut network = Network;
        let mut steps = RealSteps { setup, transport: &mut network, control: &mut control };
        steps.make_disk().unwrap();
        let folder = world.machines.join("m");
        assert_eq!(fs::read(folder.join("pvgfb.rom")).unwrap(), b"rom");
        assert_eq!(machine_file(&world)["option_rom"], "pvgfb.rom");
        assert_eq!(fs::read(folder.join("code.fd")).unwrap(), b"code");
        let _ = fs::remove_dir_all(world.root);
    }
}

#[test]
fn a_driver_folder_counts_only_for_the_guest_whose_files_it_holds() {
    let world = World::new("folders");
    let mut network = Network;
    let mut control = Control::default();
    // The folder of a Windows package, as the setting names it.
    let windows = world.setup(Guest::Windows, true);
    let steps = RealSteps { setup: windows, transport: &mut network, control: &mut control };
    assert!(steps.lab_folder(driver::SCRIPT).is_some());
    assert!(steps.lab_folder(driver::LINUX_SCRIPT).is_none(), "Windows' files are not a Linux package");
    // A Linux package in it: the Windows install does not take it, and falls to the published one.
    let linux_folder = world.root.join("linux-package");
    fs::create_dir_all(&linux_folder).unwrap();
    fs::write(linux_folder.join(driver::LINUX_SCRIPT), "#!/bin/sh\n").unwrap();
    let mut setup = world.setup(Guest::Windows, false);
    setup.driver_dir = Some(linux_folder);
    let mut control = Control::default();
    let steps = RealSteps { setup, transport: &mut network, control: &mut control };
    assert!(steps.lab_folder(driver::SCRIPT).is_none(), "a Linux package is not a Windows install's");
    assert!(steps.lab_folder(driver::LINUX_SCRIPT).is_some());
    let _ = fs::remove_dir_all(world.root);
}
