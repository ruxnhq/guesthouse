use super::super::desktop::DESKTOP_SCRIPT;
use super::super::tests::{FakeClock, machine, plain_task_context, with_vm_running};
use super::super::{Stopped, Task, VmHost, load};
use super::*;
use crate::agent::stand_in::ScriptedAgent;
use crate::gpu_identity::{IdentityState, Shown, maker_identity, read_state, write_state};
use crate::machine::Kind;
use crate::monitor::stand_in::RecordingMonitor;
use crate::testing::{Scratch, scratch_context};
use crate::vm_args::VmPlan;
use crate::vm_host::stand_in::RecordingHost;
use std::sync::{Arc, Mutex};

const IDENTITY_SCRIPT: &str = "make the package";

#[test]
fn install_steps_follow_the_serial_log() {
    let line = "\"Windows Boot Manager\"\n";
    let cases = [
        (String::new(), 0),
        (
            "loading Boot0004 \"Windows Boot Manager\"\nstarting Boot0004 \"Windows Boot Manager\"\n"
                .to_owned(),
            1,
        ),
        (line.repeat(4), 2),
        (line.repeat(20), 3),
    ];
    for (text, step) in cases {
        assert_eq!(install_step(text.as_bytes()), INSTALL_STEPS[step]);
    }
}

fn with_finish_script(scratch: &Scratch) {
    scratch.write("program/guest/finish.ps1", "is it finished");
}

#[test]
fn a_disk_that_stops_growing_ends_the_wait_and_keeps_the_screen() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    with_finish_script(&scratch);
    with_vm_running(&context);
    context.clock = Arc::new(FakeClock::default());
    let monitor = Arc::new(RecordingMonitor::default());
    context.monitor = monitor.clone();
    let vm = machine(&context);
    fs::write(&vm.disk, vec![1u8; 1 << 16]).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    let error = wait_for_setup(&mut task).unwrap_err();
    assert!(error.0.contains("Show Screen"), "{error}");
    assert_eq!(monitor.asked()[0].name, "screendump");
}

/// Grows the disk a little each time it's asked, as a slow install does.
struct GrowingGuest {
    disk: String,
    left: Mutex<u32>,
}

impl crate::agent::GuestAgent for GrowingGuest {
    fn ask(&self, _port: u16, _token: &str, _script: &str, _seconds: u64) -> String {
        let mut left = self.left.lock().unwrap();
        *left = left.saturating_sub(1);
        let size = fs::metadata(&self.disk).unwrap().len();
        fs::write(&self.disk, vec![1u8; size as usize + (1 << 16)]).unwrap();
        if *left == 0 { "finished".to_owned() } else { String::new() }
    }
}

#[test]
fn growing_slowly_is_not_stuck() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    with_finish_script(&scratch);
    with_vm_running(&context);
    context.clock = Arc::new(FakeClock::default());
    let vm = machine(&context);
    context.agent = Arc::new(GrowingGuest { disk: vm.disk.clone(), left: Mutex::new(301) });
    crate::agent::make_token(&context, &vm).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    wait_for_setup(&mut task).unwrap();
}

#[test]
fn a_vm_that_goes_away_during_the_install_says_so() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_finish_script(&scratch);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", machine(&context));
    assert!(wait_for_setup(&mut task).unwrap_err().0.contains("stopped during the install"));
}

#[test]
fn the_answer_file_gets_the_hosts_zone() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/guest/autounattend.xml", "<a><TimeZone>UTC</TimeZone></a>");
    assert_eq!(
        answer_file(&context, "Asia/Tokyo", "gpupv", "secret", None).unwrap(),
        "<a><TimeZone>Tokyo Standard Time</TimeZone></a>"
    );
    assert_eq!(
        answer_file(&context, "Mars/Olympus_Mons", "gpupv", "secret", None).unwrap(),
        "<a><TimeZone>UTC</TimeZone></a>"
    );
}

#[test]
fn the_answer_file_gets_the_machines_password() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/guest/autounattend.xml", "<Value>@PASSWORD@</Value><Value>@PASSWORD@</Value>");
    assert_eq!(
        answer_file(&context, "Asia/Tokyo", "gpupv", "secret", None).unwrap(),
        "<Value>secret</Value><Value>secret</Value>"
    );
}

#[test]
fn the_answer_file_gets_the_account_as_xml() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/guest/autounattend.xml", "<Name>@USER@</Name><Value>@PASSWORD@</Value>");
    assert_eq!(
        answer_file(&context, "UTC", "Ana B", "a<b>&c", None).unwrap(),
        "<Name>Ana B</Name><Value>a&lt;b&gt;&amp;c</Value>"
    );
}

#[test]
fn the_answer_file_picks_the_edition_and_its_key() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write(
        "program/guest/autounattend.xml",
        "<Value>@EDITION@</Value>@PRODUCT_KEY@<AcceptEula>true</AcceptEula>",
    );
    let pro = answer_file(&context, "UTC", "gpupv", "secret", Some("Windows 11 Pro")).unwrap();
    assert!(pro.contains("<Value>Windows 11 Pro</Value>"));
    assert!(pro.contains("W269N-WFGWX-YVC9B-4J6C9-T83GX"));
    let iot =
        answer_file(&context, "UTC", "gpupv", "secret", Some("Windows 11 IoT Enterprise LTSC")).unwrap();
    assert!(iot.contains("<Value>Windows 11 IoT Enterprise LTSC</Value>"));
    assert!(iot.contains("KBN8V-HFGQ4-MGXVD-347P6-PDQGT"));
    let year =
        answer_file(&context, "UTC", "gpupv", "secret", Some("Windows 11 IoT Enterprise LTSC 2024")).unwrap();
    assert!(year.contains("<Value>Windows 11 IoT Enterprise LTSC 2024</Value>"), "the image's own name");
    assert!(year.contains("KBN8V-HFGQ4-MGXVD-347P6-PDQGT"), "a year on the end is the same edition");
    let defaulted = answer_file(&context, "UTC", "gpupv", "secret", None).unwrap();
    assert!(defaulted.contains("<Value>Windows 11 Pro</Value>"), "an unread disc still gets Pro");
}

#[test]
fn an_answer_file_with_no_password_is_refused() {
    let unfilled = "<Value>@PASSWORD@</Value>".to_owned();
    assert!(password_is_filled(unfilled).unwrap_err().0.contains("no password"));
    assert_eq!(password_is_filled("<Value>secret</Value>".to_owned()).unwrap(), "<Value>secret</Value>");
}

fn with_install_files(scratch: &Scratch, context: &Context) {
    for name in INSTALL_FILES {
        scratch.write(&format!("program/guest/{name}"), "<TimeZone>UTC</TimeZone>");
    }
    scratch.write("program/guest/setmode.ps1", "mode");
    scratch.write("program/guest/remote-desktop.ps1", "turn it");
    scratch.write("program/guest/ssh.ps1", "set it up");
    let driver = crate::settings::driver_folder(context);
    fs::create_dir_all(driver.join("sub")).unwrap();
    fs::write(driver.join("nv_dispi.inf"), "driver").unwrap();
    fs::write(driver.join("sub/a.dll"), "driver").unwrap();
}

fn on_disc(path: &Path, name: &str) -> Vec<u8> {
    let result = Command::new("isoinfo").args(["-J", "-x", name, "-i"]).arg(path).output().unwrap();
    result.stdout
}

#[test]
fn the_answer_disc_holds_this_machines_token_and_is_private() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_install_files(&scratch, &context);
    let vm = machine(&context);
    make_answer_disc(&context, &vm).unwrap();
    let path = answer_disc_path(&context, &vm);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600, "the disc holds the token");
    let listing = Command::new("isoinfo").args(["-J", "-f", "-i"]).arg(&path).output().unwrap().stdout;
    let listing = String::from_utf8_lossy(&listing);
    for name in [
        "/autounattend.xml",
        "/SetupComplete.cmd",
        "/setup.ps1",
        "/agent.ps1",
        "/setmode.ps1",
        "/token.txt",
        "/nvidia/nv_dispi.inf",
        "/nvidia/sub/a.dll",
    ] {
        assert!(listing.lines().any(|line| line == name), "{name} is not on the disc");
    }
    let token = fs::read_to_string(vm.token_file(&context)).unwrap();
    assert_eq!(String::from_utf8_lossy(&on_disc(&path, "/token.txt")).trim(), token.trim());
    assert!(!vm.folder(&context).join("answer-disc.new").exists(), "the files for the disc are gone");
}

/// As in NixOS's store, where the program folder's files can't be written.
fn make_read_only(folder: &Path) {
    for entry in fs::read_dir(folder).unwrap().flatten() {
        fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o444)).unwrap();
    }
}

#[test]
fn a_read_only_program_folder_still_makes_the_disc() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_install_files(&scratch, &context);
    scratch.write("program/openssh/OpenSSH-Win64.msi", "installer");
    make_read_only(&scratch.path("program/guest"));
    make_read_only(&scratch.path("program/openssh"));
    let vm = machine(&context);
    let access = crate::remote_access::Access { ssh: Some(true), ..crate::remote_access::Access::NEW };
    crate::remote_access::write_access(&context, &vm, &access).unwrap();
    make_answer_disc(&context, &vm).unwrap();
    let answers = on_disc(&answer_disc_path(&context, &vm), "/autounattend.xml");
    assert!(
        String::from_utf8_lossy(&answers).contains("<TimeZone>"),
        "the filled answer file is on the disc"
    );
}

#[test]
fn a_machine_on_an_amd_card_gets_amds_driver_on_its_disc() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_install_files(&scratch, &context);
    let amd_driver = scratch.path("amd-driver");
    fs::create_dir_all(&amd_driver).unwrap();
    fs::write(amd_driver.join("amdxc64.dll"), "driver").unwrap();
    fs::create_dir_all(context.paths.setup_folder()).unwrap();
    fs::write(
        context.paths.setup_folder().join("amd.json"),
        json!({"driver_folder": amd_driver}).to_string(),
    )
    .unwrap();
    let vm = Machine { amd_gpu: "0000:7c:00.0".into(), ..machine(&context) };
    make_answer_disc(&context, &vm).unwrap();
    let path = answer_disc_path(&context, &vm);
    let listing = Command::new("isoinfo").args(["-J", "-f", "-i"]).arg(&path).output().unwrap().stdout;
    let listing = String::from_utf8_lossy(&listing);
    assert!(listing.lines().any(|line| line == "/amd/amdxc64.dll"), "{listing}");
    assert!(!listing.contains("/nvidia"), "NVIDIA's driver is not this machine's");
}

#[test]
fn the_disc_carries_remote_desktop_and_ssh_as_the_machine_was_made() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_install_files(&scratch, &context);
    let vm = machine(&context);
    let access = crate::remote_access::Access { ssh: Some(true), ..crate::remote_access::Access::NEW };
    crate::remote_access::write_access(&context, &vm, &access).unwrap();
    let refused = make_answer_disc(&context, &vm).unwrap_err();
    assert!(refused.0.contains("no OpenSSH"), "{}", refused.0);
    scratch.write("program/openssh/OpenSSH-Win64.msi", "installer");
    make_answer_disc(&context, &vm).unwrap();
    let path = answer_disc_path(&context, &vm);
    assert!(String::from_utf8_lossy(&on_disc(&path, "/remote-desktop.ps1")).starts_with("$on = $true\n"));
    let ssh = String::from_utf8_lossy(&on_disc(&path, "/ssh.ps1")).into_owned();
    assert!(ssh.contains("$package = Join-Path $PSScriptRoot 'OpenSSH-Win64.msi'\n"), "{ssh}");
    assert_eq!(on_disc(&path, "/OpenSSH-Win64.msi"), b"installer");
}

fn disc_files(path: &Path) -> Vec<(String, Vec<u8>)> {
    let listing = Command::new("isoinfo").args(["-J", "-f", "-i"]).arg(path).output().unwrap().stdout;
    let mut names: Vec<String> = String::from_utf8_lossy(&listing).lines().map(str::to_owned).collect();
    names.sort();
    names.into_iter().map(|name| (name.clone(), on_disc(path, &name))).collect()
}

/// Lean Windows adds two files and one line, and only to a lean machine's disc: off, the disc is the
/// same to the byte as one made before the choice existed.
#[test]
fn only_a_lean_machines_disc_has_lean_windows_on_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_install_files(&scratch, &context);
    let real = |name: &str| {
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../guest").join(name)).unwrap()
    };
    scratch.write("program/guest/autounattend.xml", real("autounattend.xml"));
    scratch.write("program/guest/lean.ps1", real("lean.ps1"));
    let vm = machine(&context);
    make_answer_disc(&context, &vm).unwrap();
    let path = answer_disc_path(&context, &vm);
    let before = disc_files(&path);
    assert!(
        before
            .iter()
            .all(|(name, bytes)| !name.contains("lean") && !String::from_utf8_lossy(bytes).contains("lean"))
    );
    lean::write_lean(&context, &vm, true).unwrap();
    make_answer_disc(&context, &vm).unwrap();
    let lean_disc = disc_files(&path);
    let names: Vec<&str> = lean_disc.iter().map(|(name, _)| name.as_str()).collect();
    assert!(names.contains(&"/lean.ps1") && names.contains(&"/lean.json"), "{names:?}");
    let answers = |files: &[(String, Vec<u8>)]| {
        String::from_utf8(files.iter().find(|(name, _)| name == "/autounattend.xml").unwrap().1.clone())
            .unwrap()
    };
    let with_step = answers(&lean_disc);
    assert_eq!(lean::with_specialize_step(&answers(&before)).unwrap(), with_step);
    let data: serde_json::Value =
        serde_json::from_slice(&lean_disc.iter().find(|(name, _)| name == "/lean.json").unwrap().1).unwrap();
    assert_eq!(data["apps"].as_array().unwrap().len(), lean::APPS.len());
    lean::write_lean(&context, &vm, false).unwrap();
    make_answer_disc(&context, &vm).unwrap();
    assert_eq!(disc_files(&path), before, "off again is the disc as it was");
}

/// Stands in for a guest that installs and shuts down.
#[derive(Default)]
struct InstalledGuest;

impl crate::agent::GuestAgent for InstalledGuest {
    fn ask(&self, _port: u16, _token: &str, script: &str, _seconds: u64) -> String {
        match script {
            "is it finished" => "finished",
            DESKTOP_SCRIPT => "3",
            _ if script.contains("check it") => "all there",
            _ if script.contains(IDENTITY_SCRIPT) => "identity staged",
            _ => "",
        }
        .to_owned()
    }
}

fn install_setup(scratch: &Scratch) -> (Context, Machine) {
    let mut context = plain_task_context(scratch);
    context.install_room = Some(0);
    context.agent = Arc::new(InstalledGuest);
    with_install_files(scratch, &context);
    with_finish_script(scratch);
    scratch.write("program/guest/check.ps1", "check it");
    let iso = scratch.write("win.iso", "disc").display().to_string();
    let new = crate::library::NewMachine {
        name: "Steps".into(),
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
        access: crate::remote_access::Access::NEW,
    };
    let vm = crate::library::create(&context, &new).unwrap();
    (context, vm)
}

/// A host whose VM shows up when started, and goes when the guest would shut it down.
struct InstallHost<'a> {
    inner: RecordingHost,
    context: &'a Context,
}

impl VmHost for InstallHost<'_> {
    fn start(&self, plan: &VmPlan, machine: &Machine) -> Result<()> {
        self.inner.start(plan, machine)?;
        with_vm_running(self.context);
        Ok(())
    }

    fn stop(&self, machine: &Machine, seconds: u64) -> Result<Stopped> {
        self.inner.stop(machine, seconds)
    }

    fn release(&self) -> Result<()> {
        self.inner.release()
    }
}

#[test]
fn a_failed_install_stays_one_so_it_can_start_over() {
    let scratch = Scratch::new();
    let (mut context, vm) = install_setup(&scratch);
    // The guest never finishes, and goes away.
    context.agent = Arc::new(ScriptedAgent::default());
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    let error = install(&mut task).unwrap_err();
    assert!(error.0.contains("stopped during the install"), "{error}");
    assert!(load(&context, &vm.id).unwrap().installing);
}

#[test]
fn the_install_runs_its_steps_in_order_up_to_the_shutdown() {
    let scratch = Scratch::new();
    let (context, vm) = install_setup(&scratch);
    let host = InstallHost { inner: RecordingHost::default(), context: &context };
    let mut task = Task::new(&context, &host, "install", vm.clone());
    let error = install(&mut task).unwrap_err();
    assert!(error.0.contains("would not shut down"), "the stand-in VM never goes: {error}");
    assert_eq!(host.inner.said(), ["start plain"], "an install has no GPU yet");
    let plans = host.inner.plans.lock().unwrap();
    let answer = answer_disc_path(&context, &vm);
    assert_eq!(plans[0].boot, Boot::Install { iso: vm.iso.clone(), answer: answer.clone() });
    assert!(answer.exists(), "the VM is still up, so the disc stays for now");
}

#[test]
fn an_install_has_windows_take_nvidias_package_before_its_shutdown() {
    let scratch = Scratch::new();
    let (mut context, vm) = install_setup(&scratch);
    scratch.write("program/guest/gpu-identity.ps1", IDENTITY_SCRIPT);
    let qemu = scratch.write("qemu", "#!/bin/sh\necho 'channel=<str>'; echo 'vendor-id=<uint16>'\n");
    fs::set_permissions(&qemu, fs::Permissions::from_mode(0o700)).unwrap();
    context.paths.qemu = qemu;
    let host = InstallHost { inner: RecordingHost::default(), context: &context };
    let mut task = Task::new(&context, &host, "install", vm.clone());
    assert!(install(&mut task).is_err(), "the stand-in VM never shuts down");
    let state = read_state(&context, &vm);
    assert_eq!(state.shown, Shown::Maker, "so the first start shows NVIDIA's number in its first VM");
    assert!(state.restart_due, "and restarts Windows once");
}

/// An install over a machine that had Windows before. What was written down about the old Windows
/// would otherwise be believed of the new one: no package, and a first start with no GPU.
#[test]
fn an_install_forgets_what_the_old_windows_had_taken_in() {
    let scratch = Scratch::new();
    let (mut context, vm) = install_setup(&scratch);
    scratch.write("program/guest/gpu-identity.ps1", IDENTITY_SCRIPT);
    let qemu = scratch.write("qemu", "#!/bin/sh\necho 'channel=<str>'; echo 'vendor-id=<uint16>'\n");
    fs::set_permissions(&qemu, fs::Permissions::from_mode(0o700)).unwrap();
    context.paths.qemu = qemu;
    let old = IdentityState {
        shown: Shown::Maker,
        package_vendor: maker_identity(&vm).vendor,
        ..IdentityState::default()
    };
    write_state(&context, &vm, &old).unwrap();
    let display = vm.folder(&context).join("display.json");
    fs::write(&display, r#"{"driver":"installed"}"#).unwrap();
    let failed_start = crate::auto_snapshot::record::Record {
        tag: "guesthouse-0a1b2c3d".into(),
        name: "Automatic: before the vendor ID".into(),
        pending: true,
        covered: vec!["files:old".into()],
        back_to: None,
    };
    crate::auto_snapshot::record::write(&context, &vm, &failed_start).unwrap();
    let host = InstallHost { inner: RecordingHost::default(), context: &context };
    let mut task = Task::new(&context, &host, "install", vm.clone());
    assert!(install(&mut task).is_err(), "the stand-in VM never shuts down");
    let record = crate::auto_snapshot::record::read(&context, &vm);
    assert_eq!(record, Default::default(), "no way back to a snapshot that went with the old disk");
    assert!(read_state(&context, &vm).restart_due, "the new Windows was given the package again");
    assert!(!display.exists(), "and its display driver is looked at afresh");
}

#[test]
fn a_machine_outside_its_folder_is_never_installed_over() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let elsewhere = Machine {
        installing: true,
        ..Machine::new("elsewhere", "Elsewhere", "/nowhere/disk", "/nowhere/vars")
    };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", elsewhere);
    assert!(install(&mut task).unwrap_err().0.contains("not in its folder"));
}

/// A guest that installs, and shuts down when told to. Every ask is kept, in order.
struct ShutdownGuest {
    process: std::path::PathBuf,
    asked: Mutex<Vec<String>>,
}

impl crate::agent::GuestAgent for ShutdownGuest {
    fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String {
        self.asked.lock().unwrap().push(script.to_owned());
        if script == "Stop-Computer -Force" {
            let _ = fs::remove_dir_all(&self.process);
            return String::new();
        }
        InstalledGuest.ask(port, token, script, seconds)
    }
}

const RECIPE: &str =
    "name = \"Box\"\ndescription = \"A box.\"\nguest = \"windows\"\n[steps]\npackages = [\"Git.Git\"]\n";

/// Installs to the end, as a guest that shuts down does, with the recipe if there is one. There is no
/// room to start the machine after it, so it ends off, which is the one end that needs no GPU.
fn install_to_the_end(scratch: &Scratch, recipe: Option<&str>) -> (Context, Machine, Vec<String>, String) {
    let (mut context, vm) = install_setup(scratch);
    let guest = Arc::new(ShutdownGuest { process: context.processes.join("40"), asked: Mutex::default() });
    context.agent = guest.clone();
    context.ram_limit = Some(1);
    if let Some(text) = recipe {
        let chosen = crate::recipe::Chosen {
            source: crate::recipe::Source::App,
            file: String::new(),
            recipe: crate::recipe::Recipe::parse(text).unwrap(),
        };
        crate::recipe::write_chosen(&context, &vm, &chosen).unwrap();
    }
    let host = InstallHost { inner: RecordingHost::default(), context: &context };
    let mut task = Task::new(&context, &host, "install", vm.clone());
    install(&mut task).unwrap();
    let asked = guest.asked.lock().unwrap().clone();
    let (made, error) = (load(&context, &vm.id).unwrap(), task.error);
    (context, made, asked, error)
}

#[test]
fn an_install_with_no_recipe_ends_as_it_did_and_asks_a_guest_nothing_after_its_shutdown() {
    let scratch = Scratch::new();
    let (_, made, asked, error) = install_to_the_end(&scratch, None);
    assert!(!made.installing);
    assert_eq!(error, "Windows is installed. There isn't room to start it right now.");
    assert_eq!(asked.last().map(String::as_str), Some("Stop-Computer -Force"), "{asked:?}");
}

#[test]
fn a_recipes_steps_wait_when_the_machine_ends_off_and_say_so() {
    let scratch = Scratch::new();
    let (context, made, asked, error) = install_to_the_end(&scratch, Some(RECIPE));
    assert!(!made.installing, "the machine is installed");
    assert!(error.contains("isn't room") && error.contains("recipe's steps wait"), "{error}");
    assert_eq!(
        asked.last().map(String::as_str),
        Some("Stop-Computer -Force"),
        "the guest was asked nothing more"
    );
    assert_eq!(crate::recipe::read_outcome(&context, &made), crate::recipe::Outcome::Pending);
}
