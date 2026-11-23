use super::super::tests::{machine, plain_task_context, with_vm_running};
use super::*;
use crate::guest_shell::stand_in::ScriptedShell;
use crate::testing::Scratch;
use crate::vm_host::stand_in::RecordingHost;
use std::process::Command;
use std::sync::Arc;

fn running_context(scratch: &Scratch, answers: Vec<ShellAnswer>) -> (Context, Arc<ScriptedShell>) {
    let mut context = plain_task_context(scratch);
    with_vm_running(&context);
    scratch.write("program/linux/copy/new-identity.sh", "SCRIPT\n");
    let shell = Arc::new(ScriptedShell::answering(answers));
    context.shell = shell.clone();
    (context, shell)
}

/// An Ubuntu machine with a key of its own, a host key learned, and the mark a copy has.
fn copy_of_ubuntu(context: &Context) -> Machine {
    let vm = Machine { kind: Kind::Linux, ..machine(context) };
    let ssh = vm.folder(context).join("ssh");
    fs::create_dir_all(&ssh).unwrap();
    fs::write(ssh.join("id"), "key").unwrap();
    fs::write(ssh.join("known_hosts"), "[127.0.0.1]:2222 ssh-ed25519 THE-ORIGINALS\n").unwrap();
    plan_at(&vm.folder(context), vm.kind).unwrap();
    vm
}

fn known_hosts(context: &Context, vm: &Machine) -> PathBuf {
    vm.folder(context).join("ssh/known_hosts")
}

fn mark_says(context: &Context, vm: &Machine) -> String {
    fs::read_to_string(vm.folder(context).join(MARK)).unwrap_or_default()
}

fn ran(status: i32, output: &str) -> ShellAnswer {
    ShellAnswer::Ran { status, output: output.into() }
}

#[test]
fn only_an_ubuntu_copy_is_marked() {
    let scratch = Scratch::new();
    let folder = scratch.path("copy");
    fs::create_dir_all(&folder).unwrap();
    for kind in [Kind::Windows, Kind::Plain] {
        plan_at(&folder, kind).unwrap();
        assert!(!folder.join(MARK).exists());
    }
    plan_at(&folder, Kind::Linux).unwrap();
    assert_eq!(fs::read_to_string(folder.join(MARK)).unwrap(), "owed");
}

#[test]
fn a_copys_guest_gets_new_identities_once_it_answers_and_the_host_learns_its_new_host_key() {
    let scratch = Scratch::new();
    let refused = ShellAnswer::Unreachable("Connection refused".into());
    let (context, shell) = running_context(&scratch, vec![refused, ran(0, "renewed\n")]);
    let vm = copy_of_ubuntu(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    renew_if_owed(&mut task);
    let asked = shell.asked();
    assert_eq!(asked.len(), 2, "asked again once the guest is up");
    assert!(asked.iter().all(|asked| asked.command == COMMAND && asked.input == b"SCRIPT\n"));
    assert!(!known_hosts(&context, &vm).exists(), "the original's host key is forgotten");
    assert_eq!(mark_says(&context, &vm), "done");
    assert_eq!(task.error, "");
    assert!(task.said.lock().unwrap().iter().any(|step| step == STEP));
    renew_if_owed(&mut task);
    assert_eq!(shell.asked().len(), 2, "done once");
}

#[test]
fn a_change_that_does_not_happen_is_told_and_tried_again_at_the_next_start() {
    let scratch = Scratch::new();
    let (context, _) = running_context(&scratch, vec![ran(1, "ssh-keygen: no room\n")]);
    let vm = copy_of_ubuntu(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    let gpu = "Starting without the GPU: no nvfwd device.";
    task.error = gpu.into();
    renew_if_owed(&mut task);
    assert_eq!(mark_says(&context, &vm), "owed");
    assert!(task.error.starts_with(&format!("{gpu} Its guest still has")), "{}", task.error);
    assert!(task.error.ends_with("ssh-keygen: no room The next start tries again."), "{}", task.error);
}

#[test]
fn a_first_boot_or_gpu_step_under_way_is_asked_again_and_then_left_for_the_next_start() {
    let scratch = Scratch::new();
    let (context, shell) = running_context(&scratch, Vec::new());
    *shell.otherwise.lock().unwrap() = Some(ran(3, "Its GPU step or a restart is still under way.\n"));
    let vm = copy_of_ubuntu(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    renew_if_owed(&mut task);
    assert!(shell.asked().len() > 10, "asked again while it waits");
    assert!(mark_says(&context, &vm) == "owed" && task.error.is_empty(), "{}", task.error);
}

#[test]
fn a_throwaway_copy_waits_until_it_is_one_no_more_and_a_machine_that_is_no_copy_never_does_it() {
    let scratch = Scratch::new();
    let (context, shell) = running_context(&scratch, vec![ran(0, "renewed\n")]);
    let vm = Machine { throwaway: true, ..copy_of_ubuntu(&context) };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    renew_if_owed(&mut task);
    assert!(shell.asked().is_empty(), "its disk would not keep it, and each run would restart");
    assert!(mark_says(&context, &vm) == "owed" && task.error.is_empty());
    let ordinary = Machine { throwaway: false, ..vm };
    let mut task = Task::new(&context, &host, "start", ordinary.clone());
    renew_if_owed(&mut task);
    assert_eq!(shell.asked().len(), 1);
    fs::remove_file(ordinary.folder(&context).join(MARK)).unwrap();
    let mut task = Task::new(&context, &host, "start", ordinary);
    renew_if_owed(&mut task);
    assert_eq!(shell.asked().len(), 1, "a machine that is no copy never does it");
}

#[test]
fn a_guest_never_up_or_a_machine_that_stopped_ends_the_wait() {
    let scratch = Scratch::new();
    let (context, shell) = running_context(&scratch, Vec::new());
    let vm = copy_of_ubuntu(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    renew_if_owed(&mut task);
    assert!(task.error.contains("could not be reached in 5 minutes"), "{}", task.error);
    assert!(shell.asked().len() > 20, "it kept asking, seconds apart");
    let stopped = Scratch::new();
    let off = plain_task_context(&stopped);
    stopped.write("program/linux/copy/new-identity.sh", "SCRIPT\n");
    let vm = copy_of_ubuntu(&off);
    let mut task = Task::new(&off, &host, "start", vm.clone());
    renew_if_owed(&mut task);
    assert!(task.error.is_empty() && mark_says(&off, &vm) == "owed");
}

#[test]
fn going_back_to_a_snapshot_of_a_copy_makes_its_identities_owed_again_and_a_new_install_forgets_them() {
    let scratch = Scratch::new();
    let (context, shell) = running_context(&scratch, vec![ran(0, "renewed\n"), ran(0, "renewed\n")]);
    let vm = copy_of_ubuntu(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    renew_if_owed(&mut task);
    fs::write(known_hosts(&context, &vm), "[127.0.0.1]:2222 ssh-ed25519 THE-NEW-ONE\n").unwrap();
    // A snapshot from before holds the original's host key, which the one learned would refuse.
    went_back(&context, &vm);
    assert_eq!(mark_says(&context, &vm), "owed");
    assert!(!known_hosts(&context, &vm).exists());
    let mut task = Task::new(&context, &host, "start", vm.clone());
    renew_if_owed(&mut task);
    assert_eq!(shell.asked().len(), 2);
    forget(&context, &vm);
    went_back(&context, &vm);
    assert!(!vm.folder(&context).join(MARK).exists(), "a machine that is no copy has nothing owed");
}

/// The script from the repo, run for real on a folder of the test's own as the guest's root, with a
/// fake `ssh-keygen`, `systemctl` and `systemd-run` first in the path.
#[test]
fn the_script_makes_new_host_keys_and_a_machine_id_and_restarts_the_guest() {
    let scratch = Scratch::new();
    let root = scratch.path("guest");
    scratch.write("guest/etc/ssh/ssh_host_ed25519_key", "the original's");
    scratch.write("guest/etc/ssh/ssh_host_ed25519_key.pub", "the original's");
    scratch.write("guest/etc/ssh/sshd_config", "kept");
    scratch.write("guest/etc/machine-id", "0123456789abcdef0123456789abcdef\n");
    scratch.write("guest/var/lib/dbus/machine-id", "0123456789abcdef0123456789abcdef\n");
    scratch.write("uuid", "4a1d6b0e-2f6c-4d3e-9a7b-1c2d3e4f5a6b\n");
    let calls = scratch.path("calls");
    let keygen = format!(
        "#!/bin/sh\necho \"ssh-keygen $1\" >> {}\nfor end in _key _key.pub; do echo new > \"$3/etc/ssh/ssh_host_ed25519$end\"; done\n",
        calls.display()
    );
    scratch.program("bin/ssh-keygen", &keygen);
    scratch.program(
        "bin/systemctl",
        &format!("#!/bin/sh\ncat {} 2>/dev/null\n", scratch.path("jobs").display()),
    );
    scratch
        .program("bin/systemd-run", &format!("#!/bin/sh\necho \"systemd-run $*\" >> {}\n", calls.display()));
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../linux/copy/new-identity.sh");
    let run = || {
        let path = format!("{}:{}", scratch.path("bin").display(), std::env::var("PATH").unwrap());
        Command::new("sh")
            .arg("-s")
            .stdin(fs::File::open(&script).unwrap())
            .env("PATH", path)
            .env("GUESTHOUSE_GUEST_ROOT", &root)
            .env("GUESTHOUSE_UUID_SOURCE", scratch.path("uuid"))
            .output()
            .unwrap()
    };
    assert_eq!(run().status.code(), Some(NOT_YET), "no marker yet");
    scratch.write("guest/var/lib/nvfwd/first-boot-done", "");
    scratch.write("jobs", "JOB UNIT TYPE STATE\n7 nvfwd-gpu.service start running\n");
    assert_eq!(run().status.code(), Some(NOT_YET), "the GPU step is building its driver");
    assert!(!calls.exists(), "nothing is touched before then");
    scratch.write("jobs", "No jobs running.\n");
    let done = run();
    assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
    assert_eq!(String::from_utf8_lossy(&done.stdout), "renewed\n");
    assert_eq!(fs::read_to_string(root.join("etc/ssh/ssh_host_ed25519_key.pub")).unwrap(), "new\n");
    let left = fs::read_dir(root.join("etc/ssh")).unwrap().count();
    assert_eq!(left, 3, "the new keys in place of the old, and nothing left from making them");
    assert_eq!(fs::read_to_string(root.join("etc/ssh/sshd_config")).unwrap(), "kept");
    let id = fs::read_to_string(root.join("etc/machine-id")).unwrap();
    assert_eq!(id, "4a1d6b0e2f6c4d3e9a7b1c2d3e4f5a6b\n");
    assert_eq!(fs::read_link(root.join("var/lib/dbus/machine-id")).unwrap(), Path::new("/etc/machine-id"));
    let said = fs::read_to_string(&calls).unwrap();
    assert_eq!(said, "ssh-keygen -A\nsystemd-run --quiet --on-active=3s systemctl reboot\n");
}
