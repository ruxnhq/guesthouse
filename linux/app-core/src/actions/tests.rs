use super::desktop::{BOOTS_FILE, SERIAL_FILE};
use super::gpu_check::gpu_found;
use super::guest_files::{
    name_display, place_amd_runtime, place_driver_files, place_nvapi_wrapper, place_profile_store,
    register_media_transforms,
};
use super::guest_upload::stand_in::FilesGuest;
use super::window_speed::apply_window_speed;
use super::*;
use crate::agent::stand_in::ScriptedAgent;
use crate::clock::Clock;
use crate::gpu_identity::{IdentityState, Shown, read_state, write_state};
use crate::machine::Kind;
use crate::processes::stand_in::fake_process;
use crate::slots;
use crate::tasks::{read_task, status};
use crate::testing::{Scratch, scratch_context};
use crate::vm_host::stand_in::RecordingHost;
use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Time that only moves when something sleeps.
#[derive(Default)]
pub(super) struct FakeClock(AtomicU64);

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::Relaxed))
    }

    fn sleep(&self, time: Duration) {
        self.0.fetch_add(time.as_millis() as u64, Ordering::Relaxed);
    }
}

pub(super) fn machine(context: &Context) -> Machine {
    let mut machine = Machine::new("x", "x", "", "");
    machine.save_files(context);
    machine
}

impl Machine {
    /// Gives it a real disk and firmware file, so a test can get past the checks.
    fn save_files(&mut self, context: &Context) {
        let folder = self.folder(context);
        fs::create_dir_all(&folder).unwrap();
        self.disk = folder.join("disk.qcow2").display().to_string();
        self.vars = folder.join("vars.fd").display().to_string();
        fs::write(&self.disk, "").unwrap();
        fs::write(&self.vars, "").unwrap();
        self.save(context).unwrap();
    }
}

#[test]
fn a_shared_folder_is_reported_only_if_it_did_not_map() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    scratch.write("program/guest/share-start.ps1", "map it");
    let agent = Arc::new(ScriptedAgent::answering(&["mapped\r\n", "not mapped\r\n", ""]));
    context.agent = agent.clone();
    let shared = Machine { share: "/folder".into(), ..machine(&context) };
    crate::agent::make_token(&context, &shared).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", shared.clone());
    apply_share(&mut task).unwrap();
    assert_eq!(task.error, "");
    apply_share(&mut task).unwrap();
    assert!(task.error.contains("did not reach"));
    task.error.clear();
    apply_share(&mut task).unwrap();
    assert!(task.error.contains("did not reach"));
    let asked = agent.requests.lock().unwrap().len();
    let mut alone = Task::new(&context, &host, "start", Machine { share: String::new(), ..shared });
    apply_share(&mut alone).unwrap();
    assert_eq!(agent.requests.lock().unwrap().len(), asked, "with no folder, the guest is not asked");
}

pub(super) fn with_vm_running(context: &Context) {
    let qemu = context.paths.qemu.display().to_string();
    let socket = format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::first()).display());
    fake_process(context, 40, &[&qemu, "-qmp", &socket], &[], "");
}

pub(super) fn plain_task_context(scratch: &Scratch) -> Context {
    let mut context = scratch_context(scratch);
    context.cpus = crate::host::stand_in::fake_cpus(scratch, "cpus", &[32768; 16]);
    context.meminfo =
        scratch.write("meminfo", "MemTotal:       100000000 kB\nMemAvailable:   100000000 kB\n");
    context.clock = Arc::new(FakeClock::default());
    context
}

#[test]
fn a_plain_machine_starts_through_the_host_with_its_iso() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let iso = scratch.write("disc.iso", "").display().to_string();
    let vm = Machine { kind: Kind::Plain, iso: iso.clone(), ..machine(&context) };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    start(&mut task).unwrap();
    assert_eq!(host.said(), ["start plain"]);
    assert_eq!(host.plans.lock().unwrap()[0].boot, Boot::Plain { iso });
}

#[test]
fn a_gpu_machine_with_no_setup_is_refused_before_anything_starts() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let vm = machine(&context);
    crate::agent::make_token(&context, &vm).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    let refused = start(&mut task).unwrap_err();
    assert_eq!(refused.0, crate::start_checks::NO_SETUP);
    assert!(host.said().is_empty(), "nothing was started");
}

#[test]
fn a_gpu_machine_with_no_token_is_refused_before_anything_starts() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    context.pci_names = scratch.write("pci.ids", crate::setup::cards::stand_in::NAMES);
    crate::setup::cards::stand_in::fake_pci(&scratch, "0000:01:00.0", 0x2704, "0x10de", "0x030000");
    fs::create_dir_all(crate::settings::driver_folder(&context)).unwrap();
    let card = serde_json::json!({"card": {"device": 1}});
    crate::settings::write_setup(&context, card.as_object().cloned().unwrap()).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", machine(&context));
    let refused = start(&mut task).unwrap_err();
    assert_eq!(refused.0, crate::start_checks::NO_TOKEN);
    assert!(host.said().is_empty(), "nothing was started");
}

#[test]
fn a_gpu_machine_starts_through_the_same_host_and_sets_up_its_desktop() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/display.ps1", "what the session sees");
    let agent = Arc::new(ScriptedAgent::answering(&[
        "3
",
        "gpu working 1
",
        "device missing",
        "",
        "screen basic 1920x1080 64 hdr 0",
        "",
        "screen 1920x1080
",
        "",
        "screen basic 1920x1080 64 hdr 0",
    ]));
    context.agent = agent.clone();
    let vm = machine(&context);
    crate::agent::make_token(&context, &vm).unwrap();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    with_vm_running(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    boot(&mut task).unwrap();
    assert_eq!(host.said(), ["start gpu"]);
    assert_eq!(task.error, "");
    let asked = agent.requests.lock().unwrap();
    assert!(asked[1].2.contains("Win32_VideoController"), "the GPU is checked before anything else");
    assert!(asked[2].2.contains("DEVPKEY"), "then the display driver");
    assert!(asked[3].2.contains("WriteAllBytes"), "a script goes to the guest first");
    assert!(asked[4].2.contains("New-ScheduledTaskPrincipal"), "and runs in the user's session");
}

/// A guest made before its folder had rights of its own gets them at its next start, as soon as the
/// agent answers, and a script for the user's session writes only to the file SYSTEM made for it.
#[test]
fn a_start_puts_the_guest_folders_rights_right_first() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/display.ps1", "what the session sees");
    scratch.write("program/guest/guest-rights.ps1", "$account = '@USER@'\nkeep the folder");
    let agent = Arc::new(ScriptedAgent::answering(&[
        "C:\\gpupv: SYSTEM and administrators only\r\n",
        "3\n",
        "gpu working 1\n",
        "device missing",
        "",
        "screen basic 1920x1080 64 hdr 0",
        "",
        "screen 1920x1080\n",
        "",
        "screen basic 1920x1080 64 hdr 0",
    ]));
    context.agent = agent.clone();
    let vm = machine(&context);
    let account = vm.account().to_owned();
    crate::agent::make_token(&context, &vm).unwrap();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    with_vm_running(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    boot(&mut task).unwrap();
    assert_eq!(task.error, "");
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked[0].2, format!("$account = '{account}'\nkeep the folder"));
    assert!(asked[2].2.contains("Win32_VideoController"), "{}", asked[2].2);
    let session = &asked[5].2;
    assert!(session.contains("New-ScheduledTaskPrincipal"), "{session}");
    let result = "C:\\gpupv\\cap\\apply-display.txt";
    let made = session
        .find(&format!("[IO.File]::WriteAllText('{result}', ''); icacls '{result}' /grant \"${{u}}:(R,W)\""));
    let run = session.find("Start-ScheduledTask");
    assert!(made.is_some() && made < run, "{session}");
    assert!(session.contains(&format!("while((Get-Item {result}).Length -eq 0")), "{session}");
    let log = fs::read_to_string(context.paths.log_file()).unwrap_or_default();
    assert!(log.contains("guest rights: C:\\gpupv: SYSTEM and administrators only"), "{log}");
}

#[test]
fn a_start_with_less_ram_gives_the_vm_less_and_the_machine_keeps_its_own() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/display.ps1", "what the session sees");
    let agent = Arc::new(ScriptedAgent::answering(&[
        "3\n",
        "gpu working 1\n",
        "device missing",
        "",
        "screen basic 1920x1080 64 hdr 0",
        "",
        "screen 1920x1080\n",
        "",
        "screen basic 1920x1080 64 hdr 0",
    ]));
    context.agent = agent;
    let vm = Machine { ram_gib: 6, ..machine(&context) };
    crate::agent::make_token(&context, &vm).unwrap();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    with_vm_running(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    task.ram_this_run = Some(5);
    boot(&mut task).unwrap();
    assert_eq!(host.plans.lock().unwrap()[0].ram, "5G");
    assert_eq!(task.machine.ram_gib, 6, "the next start asks for all of it again");
    let note = smaller_start_note(5, 6);
    assert!(note.starts_with("Only room for 5 GiB of RAM, so Windows has 5 GiB this time, not 6."), "{note}");
}

#[test]
fn a_desktop_at_the_wrong_size_is_set_and_checked_again() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/display.ps1", "what the session sees");
    scratch.write("program/guest/setmode.ps1", "set the mode");
    let answers = [
        "3
",
        "gpu working 1
",
        "device missing",
        "",
        "screen basic 1024x768 64 hdr 0",
        "",
        "screen 1024x768
",
        "",
        "",
        "",
        "screen 800x600
",
        "",
        "screen basic 800x600 64 hdr 0",
    ];
    context.agent = Arc::new(ScriptedAgent::answering(&answers));
    let vm = machine(&context);
    crate::agent::make_token(&context, &vm).unwrap();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    with_vm_running(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    boot(&mut task).unwrap();
    assert_eq!(task.error, "The desktop is 800x600, not 1920x1080.");
}

#[test]
fn a_failed_gpu_start_gives_back_what_it_took() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let vm = machine(&context);
    let host = RecordingHost::default();
    *host.start_failure.lock().unwrap() = Some(Failure::new("no daemon"));
    let mut task = Task::new(&context, &host, "start", vm);
    // A GPU machine needs real shared memory facts, so this goes straight to the part that matters.
    let booted = boot(&mut task);
    assert!(booted.is_err());
    assert_eq!(host.said(), ["start gpu", "release"]);
}

/// A guest whose desktop comes up, and which then says this about its GPU every time it's asked.
fn guest_saying(context: &mut Context, gpu: &'static str) -> Machine {
    let mut answers = vec!["3\n"];
    answers.resize(20, gpu);
    context.agent = Arc::new(ScriptedAgent::answering(&answers));
    context.clock = Arc::new(FakeClock::default());
    let vm = machine(context);
    crate::agent::make_token(context, &vm).unwrap();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    vm
}

#[test]
fn a_guest_with_no_gpu_fails_the_start_says_so_and_turns_off() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let vm = guest_saying(&mut context, "gpu working 0\n");
    let log = "INFO  Vgpu: Reset\nERROR /dev/shm/window: No such file\nWARN  it failed\n";
    fs::write(context.run_folder(None).join(crate::daemon::OUTPUT_FILE), log).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    let error = boot(&mut task).unwrap_err();
    assert!(error.0.contains("without its GPU"), "{error}");
    assert!(error.0.contains("/dev/shm/window: No such file"), "{error}");
    assert_eq!(host.said(), ["start gpu", "stop 30"]);
}

/// The software desktop for this build, as after a fallback: the hardware desktop is the default.
fn on_the_software_desktop(context: &Context, vm: &Machine) {
    fs::create_dir_all(vm.folder(context)).unwrap();
    crate::intel_desktop::fall_back(context, vm, "a test of the software desktop").unwrap();
}

#[test]
fn an_intel_guest_with_no_gpu_is_left_running_and_says_why() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let mut vm = guest_saying(&mut context, "gpu working 0\n");
    vm.use_windows_card(GpuMaker::Intel, "0000:03:00.0");
    on_the_software_desktop(&context, &vm);
    fs::write(context.run_folder(None).join(crate::daemon::OUTPUT_FILE), "ERROR no xe engine\n").unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    assert_eq!(boot(&mut task), Ok(()));
    assert_eq!(host.said(), ["start gpu"], "nothing turned it off");
    for said in [&task.error, &fs::read_to_string(context.paths.log_file()).unwrap()] {
        assert!(said.contains("without its GPU") && said.contains("left running"), "{said}");
        assert!(said.contains("no xe engine"), "{said}");
    }
}

#[test]
fn an_intel_guest_shown_intels_number_keeps_it_after_a_failed_check() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let mut vm = guest_saying(&mut context, "gpu working 0\n");
    vm.use_windows_card(GpuMaker::Intel, "0000:03:00.0");
    on_the_software_desktop(&context, &vm);
    let package_vendor = crate::gpu_identity::INTEL_RENDER.vendor;
    let state = IdentityState { shown: Shown::Maker, package_vendor, ..IdentityState::default() };
    write_state(&context, &vm, &state).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    assert_eq!(boot(&mut task), Ok(()));
    assert!(task.error.contains("keeps Intel's vendor ID"), "{}", task.error);
    assert_eq!(read_state(&context, &vm).shown, Shown::Maker);
    assert_eq!(host.said(), ["start gpu"]);
}

#[test]
fn only_intel_runs_without_its_gpu() {
    use super::gpu_check::runs_without_gpu;
    assert!(runs_without_gpu(Some(GpuMaker::Intel)));
    assert!(!runs_without_gpu(Some(GpuMaker::Nvidia)) && !runs_without_gpu(Some(GpuMaker::Amd)));
    assert!(!runs_without_gpu(None));
}

/// What a start says when the daemon's output file holds `log` and the guest has no GPU.
fn failure_with_daemon_saying(log: &str) -> String {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let vm = guest_saying(&mut context, "gpu working 0\n");
    fs::write(context.run_folder(None).join(crate::daemon::OUTPUT_FILE), log).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    boot(&mut task).unwrap_err().0
}

#[test]
fn a_verbose_daemon_error_gives_the_same_reason_as_a_plain_one() {
    let plain = failure_with_daemon_saying("ERROR /dev/shm/window: No such file\n");
    let verbose = failure_with_daemon_saying(
        "DEBUG [    4.999] Vgpu: Reset\nERROR [    5.010] /dev/shm/window: No such file\n",
    );
    assert_eq!(verbose, plain);
}

#[test]
fn a_daemon_error_that_starts_with_a_bracket_but_no_time_is_kept() {
    let said = failure_with_daemon_saying("ERROR [card 0] no memory\nERROR [12] no time\n");
    assert!(said.contains("[12] no time"), "{said}");
    let said = failure_with_daemon_saying("ERROR [card 0] no memory\n");
    assert!(said.contains("[card 0] no memory"), "{said}");
}

#[test]
fn a_guest_that_will_not_say_counts_as_having_no_gpu() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let vm = guest_saying(&mut context, "");
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    assert!(boot(&mut task).unwrap_err().0.contains("without its GPU"));
}

#[test]
fn a_gpu_that_is_a_moment_late_is_waited_for() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let vm = guest_saying(&mut context, "");
    context.agent = Arc::new(ScriptedAgent::answering(&["gpu working 0\n", "", "gpu working 1\n"]));
    let host = RecordingHost::default();
    let task = Task::new(&context, &host, "start", vm);
    assert!(gpu_found(&task));
}

#[test]
fn a_gpu_start_that_fails_after_the_vm_is_up_turns_it_off() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    context.clock = Arc::new(FakeClock::default());
    let vm = machine(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    let booted = boot(&mut task);
    assert!(booted.unwrap_err().0.contains("desktop never came up"));
    assert_eq!(host.said(), ["start gpu", "stop 30"]);
}

#[test]
fn a_vm_that_will_not_stop_after_a_failed_start_is_forced_and_keeps_the_first_reason() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    context.clock = Arc::new(FakeClock::default());
    let vm = machine(&context);
    let host = RecordingHost::default();
    *host.stop_answer.lock().unwrap() = Some(Err(Failure::new("it would not go")));
    let mut task = Task::new(&context, &host, "start", vm);
    let booted = boot(&mut task);
    assert!(booted.unwrap_err().0.contains("desktop never came up"));
    assert_eq!(host.said(), ["start gpu", "stop 30", "release"]);
}

#[test]
fn stopping_says_when_the_guest_ran_out_of_time() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let host = RecordingHost::default();
    *host.stop_answer.lock().unwrap() = Some(Ok(Stopped::TurnedOff));
    let mut task = Task::new(&context, &host, "stop", machine(&context));
    stop(&mut task).unwrap();
    assert!(task.error.contains("Windows did not shut down in time"));
    task.machine.kind = Kind::Plain;
    stop(&mut task).unwrap();
    assert!(task.error.contains("The guest did not shut down in time"));
}

#[test]
fn a_disk_grows_and_only_grows() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    let done =
        Command::new("qemu-img").args(["create", "-q", "-f", "qcow2", &vm.disk, "2G"]).status().unwrap();
    assert!(done.success());
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "grow", vm.clone());
    grow(&mut task, 3).unwrap();
    assert_eq!(disk_size_gib(&vm.disk).unwrap(), 3.0);
    assert!(grow(&mut task, 3).unwrap_err().0.contains("only grow"));
    assert!(grow(&mut task, 1).unwrap_err().0.contains("only grow"));
    with_vm_running(&context);
    assert!(grow(&mut task, 4).unwrap_err().0.contains("while the VM is off"));
}

#[test]
fn a_task_that_cannot_load_its_machine_says_so() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    fs::create_dir_all(context.run_folder(None)).unwrap();
    let host = RecordingHost::default();
    let result = run_task(&context, &host, "start", "no-such-machine", &[]);
    assert!(result.is_err_and(|error| error.0.contains("Could not read")));
    let found = status(&context, &Machine::new("no-such-machine", "x", "/d", "/v"));
    assert!(found.error.contains("Could not read") && !found.busy, "{found:?}");
}

#[test]
fn a_task_leaves_its_error_and_clears_its_pid() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    fs::create_dir_all(context.run_folder(None)).unwrap();
    let vm = machine(&context);
    let host = RecordingHost::default();
    let result = run_task(&context, &host, "no-such-action", &vm.id, &[]);
    assert!(result.is_err_and(|error| error.0.contains("no action")));
    let record = read_task(&context, None);
    assert_eq!(record.pid, 0);
    assert!(record.error.contains("no action"));
}

#[test]
fn a_new_install_gets_its_run_folder_from_the_first_task() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let _ = fs::remove_dir_all(context.run_folder(None));
    assert!(take_lock(&context).is_ok());
    assert!(context.run_folder(None).is_dir());
}

#[test]
fn only_one_task_holds_the_lock() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    fs::create_dir_all(context.run_folder(None)).unwrap();
    let held = take_lock(&context).unwrap();
    assert!(take_lock(&context).unwrap_err().0.contains("still in progress"));
    // Not a drop: a child another test is forking can hold a copy of the file for a moment, and with
    // that the lock. Unlocking goes for every copy.
    flock(&held, FlockOperation::Unlock).unwrap();
    take_lock(&context).unwrap();
}

#[test]
fn a_reboot_is_seen_from_the_serial_log() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let folder = context.run_folder(None);
    fs::create_dir_all(&folder).unwrap();
    assert!(!rebooted(&context, None), "a VM the app never set up is left as it is");
    fs::write(folder.join(SERIAL_FILE), "BdsDxe: starting\n").unwrap();
    fs::write(folder.join(BOOTS_FILE), "1").unwrap();
    assert!(!rebooted(&context, None));
    fs::write(folder.join(SERIAL_FILE), "BdsDxe: starting\nBdsDxe: starting\n").unwrap();
    assert!(rebooted(&context, None));
}

fn with_driver_files(scratch: &Scratch, context: &Context) {
    use crate::settings::{driver_folder, write_setup};
    fs::create_dir_all(driver_folder(context)).unwrap();
    let inf = crate::setup::guest_files::stand_in::FILES_INF;
    fs::write(driver_folder(context).join("nv_dispi.inf"), inf).unwrap();
    let card = serde_json::json!({"card": {"device": 0x2B85}});
    write_setup(context, card.as_object().cloned().unwrap()).unwrap();
    scratch.write("program/guest/nv-files.ps1", "copies files");
}

fn placing_task<'a>(context: &'a Context, host: &'a RecordingHost) -> Task<'a> {
    let vm = machine(context);
    crate::agent::make_token(context, &vm).unwrap();
    Task::new(context, host, "start", vm)
}

#[test]
fn a_guest_with_all_the_drivers_files_is_left_alone() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    with_driver_files(&scratch, &context);
    let agent = Arc::new(ScriptedAgent::answering(&["", "to place 0\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    place_driver_files(&mut placing_task(&context, &host)).unwrap();
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked.len(), 2, "only the list and the count");
    let check = &asked[1].2;
    assert!(check.contains("copies files"), "the script's own comparison counts: {check}");
    assert!(check.ends_with(r"C:\gpupv\nv-files.txt -Check"), "{check}");
}

/// A guest whose files are all there but older than the driver's, as after a driver it had before.
#[test]
fn files_that_are_there_but_older_are_placed_too() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    with_driver_files(&scratch, &context);
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../guest/windows/nv-files.ps1");
    scratch.write("program/guest/nv-files.ps1", fs::read_to_string(script).unwrap());
    let agent =
        Arc::new(ScriptedAgent::answering(&["", "to place 5\r\n", "", "copied 5, kept 0, failed 0\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    place_driver_files(&mut placing_task(&context, &host)).unwrap();
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked.len(), 4, "the list, the check, the script and its run");
    assert!(asked[1].2.contains("Test-Newer"), "the check compares versions, not only what is there");
    assert!(!asked[1].2.contains("Test-Path -LiteralPath (Join-Path"), "it isn't the count of missing files");
}

#[test]
fn missing_files_are_placed_with_the_guests_script() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    with_driver_files(&scratch, &context);
    let agent =
        Arc::new(ScriptedAgent::answering(&["", "to place 4\r\n", "", "copied System32\\one.dll\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    place_driver_files(&mut task).unwrap();
    assert_eq!(task.error, "");
    let asked = agent.requests.lock().unwrap();
    assert!(asked[2].2.contains("nv-files.ps1"), "the script goes to the guest");
    assert!(asked[3].2.contains("nv-files.txt"), "and is given the list");
}

#[test]
fn a_file_that_fails_is_said_and_the_start_goes_on() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    with_driver_files(&scratch, &context);
    let agent =
        Arc::new(ScriptedAgent::answering(&["", "to place 1\r\n", "", "FAILED System32\\one.dll\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    place_driver_files(&mut task).unwrap();
    assert!(task.error.contains("did not reach Windows"));
}

/// The list a request sent the guest.
fn sent_list(script: &str) -> String {
    let data = script.split("FromBase64String('").nth(1).unwrap().split('\'').next().unwrap();
    String::from_utf8(crate::text::base64_decode(data).unwrap()).unwrap()
}

#[test]
fn with_the_wrapper_nvidias_nvapi_is_left_out_of_the_list() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    with_driver_files(&scratch, &context);
    let inf = crate::setup::guest_files::stand_in::FILES_INF.replace("one.dll", "nvapi64.dll");
    fs::write(crate::settings::driver_folder(&context).join("nv_dispi.inf"), inf).unwrap();
    let agent = Arc::new(ScriptedAgent::answering(&["", "to place 0\r\n", "", "to place 0\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    place_driver_files(&mut placing_task(&context, &host)).unwrap();
    let nvidias = "System32\tnvapi64.dll\tnvapi64.dll\n";
    assert!(sent_list(&agent.requests.lock().unwrap()[0].2).contains(nvidias), "no wrapper here yet");
    scratch.write("program/guest/nvapi64.dll", "wrapper");
    place_driver_files(&mut placing_task(&context, &host)).unwrap();
    let list = sent_list(&agent.requests.lock().unwrap()[2].2);
    assert!(!list.contains(nvidias) && list.contains("two.dll"));
}

#[test]
fn the_wrapper_goes_to_system32_when_the_guest_lacks_this_copy() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/nvapi64.dll", "wrapper");
    let guest = Arc::new(FilesGuest::default());
    *guest.other_answer.lock().unwrap() = "nvapi needed\r\nnvapi placed\r\n".into();
    context.agent = guest.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    place_nvapi_wrapper(&mut task).unwrap();
    assert_eq!(task.error, "");
    assert_eq!(guest.files.lock().unwrap().get(r"C:\gpupv\nvapi64.dll").unwrap(), b"wrapper");
    let requests = guest.requests.lock().unwrap();
    assert!(requests[0].contains("Get-FileHash"), "it asks first");
    assert!(requests.last().unwrap().contains("Copy-Item"));
}

#[test]
fn a_guest_with_this_copy_of_the_wrapper_gets_nothing_sent() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/nvapi64.dll", "wrapper");
    let guest = Arc::new(FilesGuest::default());
    *guest.other_answer.lock().unwrap() = "nvapi kept\r\n".into();
    context.agent = guest.clone();
    let host = RecordingHost::default();
    place_nvapi_wrapper(&mut placing_task(&context, &host)).unwrap();
    assert_eq!(guest.requests.lock().unwrap().len(), 1);
}

#[test]
fn with_the_wrapper_turned_off_nvidias_nvapi_stays_in_the_list() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    with_driver_files(&scratch, &context);
    let inf = crate::setup::guest_files::stand_in::FILES_INF.replace("one.dll", "nvapi64.dll");
    fs::write(crate::settings::driver_folder(&context).join("nv_dispi.inf"), inf).unwrap();
    scratch.write("program/guest/nvapi64.dll", "wrapper");
    let agent = Arc::new(ScriptedAgent::answering(&["", "to place 0\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    task.machine.nvapi_wrapper_off = true;
    place_driver_files(&mut task).unwrap();
    assert!(sent_list(&agent.requests.lock().unwrap()[0].2).contains("System32\tnvapi64.dll\tnvapi64.dll\n"));
}

#[test]
fn with_the_wrapper_turned_off_nvidias_goes_back_and_nothing_is_sent() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/nvapi64.dll", "wrapper");
    let guest = Arc::new(FilesGuest::default());
    *guest.other_answer.lock().unwrap() = "nvapi restored\r\n".into();
    context.agent = guest.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    task.machine.nvapi_wrapper_off = true;
    place_nvapi_wrapper(&mut task).unwrap();
    assert_eq!(task.error, "");
    assert!(guest.files.lock().unwrap().is_empty(), "the wrapper isn't sent");
    let requests = guest.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("HostDriverStore") && requests[0].contains("Copy-Item"));
}

#[test]
fn nvidias_nvapi_that_does_not_go_back_is_said() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let guest = Arc::new(FilesGuest::default());
    *guest.other_answer.lock().unwrap() = "FAILED nvapi: in use\r\n".into();
    context.agent = guest.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    task.machine.nvapi_wrapper_off = true;
    place_nvapi_wrapper(&mut task).unwrap();
    assert!(task.error.contains("NVAPI wrapper may still be there"));
}

#[test]
fn with_no_wrapper_in_the_program_the_guest_is_not_asked_about_it() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let guest = Arc::new(FilesGuest::default());
    context.agent = guest.clone();
    let host = RecordingHost::default();
    place_nvapi_wrapper(&mut placing_task(&context, &host)).unwrap();
    assert!(guest.requests.lock().unwrap().is_empty());
}

#[test]
fn with_no_card_known_the_guest_is_not_asked() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&[]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    place_driver_files(&mut placing_task(&context, &host)).unwrap();
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn with_no_script_the_start_goes_on_and_says_so() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    with_driver_files(&scratch, &context);
    fs::remove_file(scratch.path("program/guest/nv-files.ps1")).unwrap();
    context.agent = Arc::new(ScriptedAgent::answering(&["", "to place 4\r\n"]));
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    place_driver_files(&mut task).unwrap();
    assert!(task.error.contains("no script"));
}

#[test]
fn the_profile_store_script_goes_to_the_guest_and_a_failure_is_said() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/nv-profiles.ps1", "make the store");
    let agent = Arc::new(ScriptedAgent::answering(&["kept the store\r\n", "FAILED the store: locked\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    place_profile_store(&mut task).unwrap();
    assert_eq!(task.error, "");
    task.machine.window_speed_off = true;
    place_profile_store(&mut task).unwrap();
    assert!(task.error.contains("settings store"));
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked.len(), 2);
    // The present method follows "Faster programs in a window".
    assert_eq!(asked[0].2, "$on = $true\nmake the store");
    assert_eq!(asked[1].2, "$on = $false\nmake the store");
}

#[test]
fn the_display_is_named_by_the_vendor_number_windows_sees() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/display-name.ps1", "name it");
    scratch.write("program/guest/setmode.ps1", "set the mode");
    let agent = Arc::new(ScriptedAgent::answering(&[
        "kept RDPUDD Chained DD\r\n",
        "script placed\r\n",
        "named Guesthouse Display from the next start (was RDPUDD Chained DD)\r\n",
        "script placed\r\n",
        "FAILED display name: denied\r\n",
    ]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    name_display(&mut task).unwrap();
    let maker = IdentityState { shown: Shown::Maker, ..Default::default() };
    write_state(&context, &task.machine, &maker).unwrap();
    name_display(&mut task).unwrap();
    assert_eq!(task.error, "");
    name_display(&mut task).unwrap();
    assert!(task.error.contains("display's name"), "{}", task.error);
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked[0].2, "$name = 'RDPUDD Chained DD'\nname it");
    // The renamed display needs the logon task's script that knows the new name.
    assert!(asked[1].2.contains("C:\\gpupv\\setmode.ps1"), "{}", asked[1].2);
    assert_eq!(asked[2].2, "$name = 'Guesthouse Display'\nname it");
}

#[test]
fn with_no_profile_store_script_the_guest_is_not_asked() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&[]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    place_profile_store(&mut placing_task(&context, &host)).unwrap();
    assert!(agent.requests.lock().unwrap().is_empty());
}

fn setup_chose_card(context: &Context, card: serde_json::Value) {
    let fields = serde_json::json!({ "card": card });
    crate::settings::write_setup(context, fields.as_object().cloned().unwrap()).unwrap();
}

#[test]
fn the_transforms_script_goes_to_the_guest_and_only_a_change_is_logged() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    setup_chose_card(&context, serde_json::json!({"device": 0x2B85}));
    scratch.write("program/guest/nv-media-foundation.ps1", "register the transforms");
    let answers = [
        "kept 12 values of 2 transforms in 380 ms\r\n",
        "nothing to register: the INF has no transforms for the card, read in 350 ms\r\n",
        "skipped a line that can't be read: HKCR,one\r\nregistered 11 of 11 values of 2 transforms, failed 0, in 412 ms\r\n",
        "left HKLM\\a\\key Name as it is set\r\nregistered 0 of 12, failed 0, left 1 of the switch's as set, in 9 ms\r\n",
    ];
    let agent = Arc::new(ScriptedAgent::answering(&answers));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    register_media_transforms(&mut task).unwrap();
    register_media_transforms(&mut task).unwrap();
    assert!(!crate::log::read_log(&context).contains("media transforms"));
    register_media_transforms(&mut task).unwrap();
    let log = crate::log::read_log(&context);
    assert!(log.contains("media transforms: skipped a line that can't be read: HKCR,one"), "{log}");
    assert!(log.contains("registered 11 of 11 values of 2 transforms, failed 0, in 412 ms"), "{log}");
    assert!(!log.contains("the switch's"), "{log}");
    register_media_transforms(&mut task).unwrap();
    let log = crate::log::read_log(&context);
    assert!(log.contains("media transforms: left HKLM\\a\\key Name as it is set"), "{log}");
    assert!(log.contains("failed 0, left 1 of the switch's as set"), "{log}");
    assert_eq!(task.error, "", "a guest without them is as it was");
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked.len(), 4);
    assert_eq!(asked[0].2, "$card = '2B85'\nregister the transforms", "setup's card, as the INF writes it");
}

/// The driver's INF gives each card its own transforms, so a machine on the PC's second NVIDIA card
/// must not be given the ones of the card setup chose.
#[test]
fn the_transforms_script_is_told_the_card_the_machine_runs_on() {
    use crate::setup::cards::stand_in::{NAMES, fake_pci};
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    context.pci_devices = scratch.path("pci");
    context.pci_names = scratch.write("pci.ids", NAMES);
    fake_pci(&scratch, "0000:01:00.0", 0x2B85, "0x10de", "0x030000");
    fake_pci(&scratch, "0000:02:00.0", 0x2C02, "0x10de", "0x030000");
    setup_chose_card(&context, serde_json::json!({"device": 0x2B85, "pci": "0000:01:00.0"}));
    scratch.write("program/guest/nv-media-foundation.ps1", "register the transforms");
    let agent = Arc::new(ScriptedAgent::answering(&["kept\r\n", "kept\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    task.machine.use_windows_card(GpuMaker::Nvidia, "0000:02:00.0");
    register_media_transforms(&mut task).unwrap();
    task.machine.use_windows_card(GpuMaker::Nvidia, "0000:03:00.0");
    register_media_transforms(&mut task).unwrap();
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked.len(), 1, "a card that has gone has no number to give");
    assert!(asked[0].2.starts_with("$card = '2C02'\n"), "{}", asked[0].2);
    assert!(crate::log::read_log(&context).contains("media transforms: the machine's card isn't known"));
}

#[test]
fn with_no_transforms_script_or_no_card_the_guest_is_not_asked() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&[]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    scratch.write("program/guest/nv-media-foundation.ps1", "register the transforms");
    register_media_transforms(&mut placing_task(&context, &host)).unwrap();
    assert!(crate::log::read_log(&context).contains("media transforms: the machine's card isn't known"));
    fs::remove_file(context.paths.guest_files().join("nv-media-foundation.ps1")).unwrap();
    setup_chose_card(&context, serde_json::json!({"device": 0x2B85}));
    register_media_transforms(&mut placing_task(&context, &host)).unwrap();
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn the_window_speed_script_carries_the_machines_setting() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/window-speed.ps1", "write the entries");
    let agent = Arc::new(ScriptedAgent::answering(&["on for C:\\Games\\\r\n", "off, entries removed\r\n"]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    apply_window_speed(&mut task).unwrap();
    task.machine.window_speed_off = true;
    apply_window_speed(&mut task).unwrap();
    assert_eq!(task.error, "");
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked[0].2, "$on = $true\nwrite the entries");
    assert_eq!(asked[1].2, "$on = $false\nwrite the entries");
}

#[test]
fn a_window_speed_failure_is_said_and_the_start_goes_on() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/window-speed.ps1", "write the entries");
    context.agent = Arc::new(ScriptedAgent::answering(&[
        "kept 3 folders\r\n",
        "FAILED window speed: access denied\r\n",
        "FAILED window speed: access denied\r\n",
    ]));
    let host = RecordingHost::default();
    let mut task = placing_task(&context, &host);
    apply_window_speed(&mut task).unwrap();
    assert_eq!(task.error, "");
    apply_window_speed(&mut task).unwrap();
    assert!(task.error.contains("could not be told"), "{}", task.error);
    task.machine.window_speed_off = true;
    apply_window_speed(&mut task).unwrap();
    assert!(task.error.contains("still in Windows"), "{}", task.error);
}

#[test]
fn with_no_window_speed_script_the_guest_is_not_asked() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&[]));
    context.agent = agent.clone();
    let host = RecordingHost::default();
    apply_window_speed(&mut placing_task(&context, &host)).unwrap();
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn an_install_with_no_room_to_boot_ends_well_and_stays_off() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    context.ram_limit = Some(1);
    let host = RecordingHost::default();
    let mut vm = machine(&context);
    vm.installing = false;
    let mut task = Task::new(&context, &host, "install", vm);
    start_if_there_is_room(&mut task, "Windows").unwrap();
    assert_eq!(task.error, "Windows is installed. There isn't room to start it right now.");
    assert!(host.said().is_empty(), "nothing may start");
}

#[test]
fn a_run_that_ends_is_kept_where_a_reboot_leaves_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    let run = context.run_folder(None);
    fs::create_dir_all(&run).unwrap();
    fs::write(run.join(crate::vm_host::MACHINE_NOTE), &vm.id).unwrap();
    fs::write(run.join("daemon.out"), "the daemon's last words").unwrap();
    let host = RecordingHost::default();
    run_task(&context, &host, "clean_up", &vm.id, &[]).unwrap();
    let kept = crate::kept_logs::kept_folder(&context, &vm.id).join("daemon.out");
    assert_eq!(fs::read_to_string(kept).unwrap(), "the daemon's last words");
}

#[test]
fn an_intel_machine_is_given_neither_nvidias_files_nor_amds_only_its_device_key() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&["device key set\r\n"]));
    context.agent = agent.clone();
    let mut intel = machine(&context);
    intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
    crate::agent::make_token(&context, &intel).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", intel);
    place_files_for_maker(&mut task).unwrap();
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked.len(), 1, "the device key alone");
    assert!(asked[0].2.contains("OpenCLDriverName"), "{}", asked[0].2);
    assert_eq!(task.error, "");
}

#[test]
fn an_intel_machine_is_given_the_loaders_from_the_drivers_list_out_of_intels_store_folder() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    crate::setup::intel::stand_in::lay_out_driver(&scratch, "intel-driver");
    let folder = scratch.path("intel-driver");
    scratch.write("intel-driver/system-files.txt", "System32\tvulkan-1-64.dll\tvulkan-1.dll\n");
    crate::setup::intel::write_intel_setup(&context, &folder, "32.0.101.9175").unwrap();
    scratch.write("program/guest/nv-files.ps1", "copies files");
    let agent = Arc::new(ScriptedAgent::answering(&[
        "device key set\r\n",
        "",
        "to place 1\r\n",
        "",
        "copied vulkan-1.dll\r\n",
    ]));
    context.agent = agent.clone();
    let mut intel = machine(&context);
    intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
    crate::agent::make_token(&context, &intel).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", intel);
    place_files_for_maker(&mut task).unwrap();
    assert_eq!(task.error, "");
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked.len(), 5, "the device key, the list, the count, the script and the run");
    assert!(asked[4].2.contains("-StoreFolder iigd_dch.inf_amd64_gpupv"), "{}", asked[4].2);
}

/// A machine on an AMD card whose driver has the media runtime, and a guest that answers as told.
fn amd_runtime_machine(scratch: &Scratch, answers: &[&str]) -> (Context, Arc<ScriptedAgent>, Machine) {
    let mut context = plain_task_context(scratch);
    let files = ["amfrt64.dll", "amfrt32.dll"];
    let folder = crate::setup::amd_system_files::stand_in::driver_with_runtime(scratch, &files);
    crate::setup::amd::write_amd_setup(&context, &folder, "32.0.32015.12012").unwrap();
    scratch.write("program/guest/nv-files.ps1", "copies files");
    let agent = Arc::new(ScriptedAgent::answering(answers));
    context.agent = agent.clone();
    let mut amd = machine(&context);
    amd.use_windows_card(GpuMaker::Amd, "0000:7c:00.0");
    crate::agent::make_token(&context, &amd).unwrap();
    (context, agent, amd)
}

#[test]
fn an_amd_machine_is_given_the_media_runtime_in_its_system_folders_from_amds_store_folder() {
    let scratch = Scratch::new();
    let answers = ["", "to place 2\r\n", "", "copied System32\\amfrt64.dll\r\n"];
    let (context, agent, amd) = amd_runtime_machine(&scratch, &answers);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", amd);
    place_amd_runtime(&mut task).unwrap();
    assert_eq!(task.error, "");
    let asked = agent.requests.lock().unwrap();
    assert_eq!(asked.len(), 4, "the list, the count, the script and the run");
    assert!(asked[0].2.contains(&crate::text::base64_encode(
        b"System32\tamfrt64.dll\tamfrt64.dll\nSysWOW64\tamfrt32.dll\tamfrt32.dll\n"
    )));
    assert!(asked[3].2.contains("-StoreFolder amdxc.inf_amd64_gpupv"), "{}", asked[3].2);
}

#[test]
fn an_amd_guest_that_has_the_media_runtime_is_only_asked_what_it_lacks() {
    let scratch = Scratch::new();
    let (context, agent, amd) = amd_runtime_machine(&scratch, &["", "to place 0\r\n"]);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", amd);
    place_amd_runtime(&mut task).unwrap();
    assert_eq!((task.error.as_str(), agent.requests.lock().unwrap().len()), ("", 2));
}

#[test]
fn a_driver_with_no_media_runtime_is_said_and_the_guest_is_asked_nothing() {
    let scratch = Scratch::new();
    let (context, agent, amd) = amd_runtime_machine(&scratch, &[]);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", amd);
    fs::remove_file(scratch.path("Display/WT6A_INF/B026716/amfrt64.dll")).unwrap();
    fs::remove_file(scratch.path("Display/WT6A_INF/B026716/amfrt32.dll")).unwrap();
    place_amd_runtime(&mut task).unwrap();
    assert!(agent.requests.lock().unwrap().is_empty());
    assert_eq!(task.error, "");
    assert!(crate::log::read_log(&context).contains("no media runtime"));
}

#[test]
fn an_intel_guest_whose_list_predates_the_64_bit_runtime_is_sent_it_for_system32() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    crate::setup::intel::stand_in::lay_out_driver(&scratch, "intel-driver");
    scratch.write("intel-driver/msvcp140.dll", "runtime");
    let old = "System32\tIntel_OpenCL_ICD64.dll\tOpenCL.dll\n";
    scratch.write("intel-driver/system-files.txt", old);
    crate::setup::intel::write_intel_setup(&context, &scratch.path("intel-driver"), "32.0.101.9175").unwrap();
    let agent = Arc::new(ScriptedAgent::answering(&["device key set\r\n", "", "to place 0\r\n"]));
    context.agent = agent.clone();
    let mut intel = machine(&context);
    intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
    crate::agent::make_token(&context, &intel).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", intel);
    place_files_for_maker(&mut task).unwrap();
    let sent = crate::text::base64_encode(format!("{old}System32\tmsvcp140.dll\tmsvcp140.dll\n").as_bytes());
    let asked = agent.requests.lock().unwrap();
    assert!(asked[1].2.contains(&sent), "the list the guest counts from has the runtime: {}", asked[1].2);
}

#[test]
fn an_intel_device_key_that_does_not_take_is_said_and_the_start_goes_on() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&["Access is denied\r\n"]));
    context.agent = agent.clone();
    let mut intel = machine(&context);
    intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
    crate::agent::make_token(&context, &intel).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", intel);
    place_files_for_maker(&mut task).unwrap();
    assert!(task.error.contains("registry key"), "{}", task.error);
}

fn intel_start<'a>(context: &'a Context, host: &'a RecordingHost) -> Task<'a> {
    let mut intel = machine(context);
    intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
    crate::agent::make_token(context, &intel).unwrap();
    Task::new(context, host, "start", intel)
}

#[test]
fn a_vm_shut_down_from_windows_before_the_start_is_done_is_asked_nothing_more() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::default());
    context.agent = agent.clone();
    let run = context.run_folder(None);
    fs::create_dir_all(&run).unwrap();
    fs::write(run.join("ended-as"), crate::ending::GUEST_SHUTDOWN).unwrap();
    let host = RecordingHost::default();
    let mut task = intel_start(&context, &host);
    finish_on_desktop(&mut task).unwrap();
    assert!(agent.requests.lock().unwrap().is_empty(), "no guest to ask");
    assert_eq!(task.error, "", "a guest that powered itself off is nothing gone wrong");
    assert!(crate::log::read_log(&context).contains("was shut down from inside the guest."));
}

#[test]
fn a_vm_that_went_by_itself_before_the_start_is_done_says_so_and_not_what_the_guest_missed() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    context.agent = Arc::new(ScriptedAgent::default());
    let host = RecordingHost::default();
    let mut task = intel_start(&context, &host);
    finish_on_desktop(&mut task).unwrap();
    assert!(task.error.contains("this app didn't stop it"), "{}", task.error);
    assert!(!task.error.contains("registry key"), "{}", task.error);
}

#[test]
fn a_vm_still_running_at_the_end_of_its_start_is_given_its_device_key() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&["", "", "device key set\r\n"]));
    context.agent = agent.clone();
    with_vm_running(&context);
    let host = RecordingHost::default();
    let mut task = intel_start(&context, &host);
    finish_on_desktop(&mut task).unwrap();
    let asked = agent.requests.lock().unwrap();
    assert!(asked.iter().any(|(_, _, script)| script.contains("OpenCLDriverName")), "{asked:?}");
}

/// A start of a machine with a real disk, in a PC that offers the card, as far as the guest.
fn start_ready_machine(scratch: &Scratch) -> (Context, Machine) {
    let mut context = plain_task_context(scratch);
    context.pci_names = scratch.write("pci.ids", crate::setup::cards::stand_in::NAMES);
    crate::setup::cards::stand_in::fake_pci(scratch, "0000:01:00.0", 0x2704, "0x10de", "0x030000");
    fs::create_dir_all(crate::settings::driver_folder(&context)).unwrap();
    let card = serde_json::json!({"card": {"device": 1}});
    crate::settings::write_setup(&context, card.as_object().cloned().unwrap()).unwrap();
    context.agent = Arc::new(ScriptedAgent::answering(&[]));
    let mut vm = machine(&context);
    assert!(
        Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2", &vm.disk, "1G"])
            .status()
            .unwrap()
            .success()
    );
    vm.vars = scratch.write("vars.fd", "firmware").display().to_string();
    vm.save(&context).unwrap();
    crate::agent::make_token(&context, &vm).unwrap();
    (context, vm)
}

#[test]
fn a_start_that_fails_after_its_snapshot_offers_going_back_to_it() {
    let scratch = Scratch::new();
    let (context, vm) = start_ready_machine(&scratch);
    let vm = Machine { auto_snapshot: Some(true), ..vm };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    let failed = start(&mut task).unwrap_err();
    assert!(!failed.0.is_empty());
    assert_eq!(host.said()[0], "start gpu", "the snapshot is before the VM");
    let offered = status(&context, &vm).restore_point.expect("offered");
    assert!(offered.name.starts_with("Automatic: before "), "{}", offered.name);
    assert_eq!(crate::snapshots::list_snapshots(&context, &vm).len(), 1);
    let mut second = Task::new(&context, &host, "start", vm.clone());
    let _ = start(&mut second);
    assert_eq!(crate::snapshots::list_snapshots(&context, &vm).len(), 1, "a retry takes no other");
    assert_eq!(status(&context, &vm).restore_point, Some(offered));
}

#[test]
fn a_start_with_the_switch_off_takes_no_snapshot() {
    let scratch = Scratch::new();
    let (context, vm) = start_ready_machine(&scratch);
    let vm = Machine { auto_snapshot: Some(false), ..vm };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    let _ = start(&mut task);
    assert!(crate::snapshots::list_snapshots(&context, &vm).is_empty());
    assert_eq!(status(&context, &vm).restore_point, None);
}
