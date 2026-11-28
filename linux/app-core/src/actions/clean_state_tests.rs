use super::*;
use crate::actions::tests::{plain_task_context, with_vm_running};
use crate::context::Context;
use crate::library::{NewMachine, create};
use crate::machine::{Kind, Machine};
use crate::snapshots::list_snapshots;
use crate::testing::Scratch;
use crate::throwaway::{clean_folder, set_throwaway};
use crate::vm_args::VmPlan;
use crate::vm_host::VmHost;
use std::process::Command;
use std::sync::Mutex;

const NOTE: &str = "display.json";

fn fill(disk: &str, byte: u8) {
    let write = format!("write -P {byte} 0 64k");
    let done = Command::new("qemu-io").args(["-f", "qcow2", "-c", &write, disk]).output().unwrap();
    assert!(done.status.success());
}

fn holds(disk: &str, byte: u8) -> bool {
    let read = format!("read -P {byte} 0 64k");
    Command::new("qemu-io").args(["-f", "qcow2", "-r", "-c", &read, disk]).output().unwrap().status.success()
}

/// A throwaway whose clean disk holds 0x11, with a clean note that says "clean".
fn throwaway(context: &Context, scratch: &Scratch) -> Machine {
    let iso = scratch.write("linux.iso", "disc").display().to_string();
    let new = NewMachine {
        name: "Clean".into(),
        iso,
        kind: Kind::Plain,
        windows_disc: false,
        cores: 1,
        ram_gib: 1,
        disk_gib: 1,
        refresh_hz: None,
        user: String::new(),
        password: String::new(),
        amd_gpu: None,
        disk_place: String::new(),
        gpu_maker: None,
        access: crate::remote_access::Access::NEW,
    };
    let mut machine = create(context, &new).unwrap();
    fill(&machine.disk, 0x11);
    fs::write(machine.folder(context).join(NOTE), "clean").unwrap();
    set_throwaway(context, &mut machine, true).unwrap();
    // What the last run left beside its settings, which the refresh must not take for the clean state.
    fs::write(machine.folder(context).join(NOTE), "last run").unwrap();
    machine
}

/// A VM whose run writes 0x22 to the disk it is given and a note beside the settings, and is then
/// up, or gone if `stays` is false.
struct Run<'a> {
    context: &'a Context,
    stays: bool,
    disks: Mutex<Vec<String>>,
}

impl VmHost for Run<'_> {
    fn start(&self, plan: &VmPlan, machine: &Machine) -> Result<()> {
        self.disks.lock().unwrap().push(plan.disk.clone());
        fill(&plan.disk, 0x22);
        fs::write(machine.folder(self.context).join(NOTE), "refreshed").unwrap();
        if self.stays {
            with_vm_running(self.context);
        }
        Ok(())
    }

    fn stop(&self, _machine: &Machine, _seconds: u64) -> Result<Stopped> {
        Ok(Stopped::Cleanly)
    }

    fn release(&self) -> Result<()> {
        Ok(())
    }
}

fn clean_note(context: &Context, machine: &Machine) -> String {
    fs::read_to_string(clean_folder(context, machine).join(NOTE)).unwrap()
}

#[test]
fn only_a_throwaway_has_a_clean_state_to_refresh() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let mut machine = throwaway(&context, &scratch);
    set_throwaway(&context, &mut machine, false).unwrap();
    let host = Run { context: &context, stays: true, disks: Mutex::default() };
    let mut task = Task::new(&context, &host, REFRESH_CLEAN_TASK, machine);
    assert_eq!(refresh_clean_state(&mut task).unwrap_err().0, NOT_A_THROWAWAY);
    assert!(host.disks.lock().unwrap().is_empty());
}

#[test]
fn the_run_writes_to_the_disk_itself_and_that_is_the_clean_state_after() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let machine = throwaway(&context, &scratch);
    let host = Run { context: &context, stays: true, disks: Mutex::default() };
    let mut task = Task::new(&context, &host, REFRESH_CLEAN_TASK, machine.clone());
    refresh_clean_state(&mut task).unwrap();
    assert_eq!(
        *host.disks.lock().unwrap(),
        std::slice::from_ref(&machine.disk),
        "its own disk, not an overlay"
    );
    assert!(holds(&machine.disk, 0x22));
    assert_eq!(clean_note(&context, &machine), "refreshed", "the run's notes are the clean ones now");
    assert!(task.machine.throwaway, "it stays a throwaway");
    assert!(crate::machine::load(&context, &machine.id).unwrap().throwaway);
    assert!(list_snapshots(&context, &machine).is_empty(), "the snapshot taken before goes");
    assert!(!machine.folder(&context).join(MARK_FILE).exists());
}

#[test]
fn a_refresh_that_fails_leaves_the_old_clean_state() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let machine = throwaway(&context, &scratch);
    let host = Run { context: &context, stays: false, disks: Mutex::default() };
    let mut task = Task::new(&context, &host, REFRESH_CLEAN_TASK, machine.clone());
    let failure = refresh_clean_state(&mut task).unwrap_err();
    assert!(failure.0.ends_with(KEPT), "{}", failure.0);
    assert!(holds(&machine.disk, 0x11), "the disk is the old clean state");
    assert_eq!(clean_note(&context, &machine), "clean");
    assert!(task.machine.throwaway);
    assert!(list_snapshots(&context, &machine).is_empty());
    assert!(!machine.folder(&context).join(MARK_FILE).exists());
}

/// The app went while a refresh ran: the next start of the throwaway goes back first.
#[test]
fn a_refresh_cut_short_is_put_back_before_the_next_run() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let machine = throwaway(&context, &scratch);
    let host = Run { context: &context, stays: false, disks: Mutex::default() };
    let mut task = Task::new(&context, &host, REFRESH_CLEAN_TASK, machine.clone());
    let tag = make_snapshot(&mut task, SNAPSHOT_NAME, true).unwrap();
    fs::write(machine.folder(&context).join(MARK_FILE), &tag).unwrap();
    fill(&machine.disk, 0x22);
    put_back_cut_short(&mut task).unwrap();
    assert!(holds(&machine.disk, 0x11));
    assert!(list_snapshots(&context, &machine).is_empty());
    assert!(!machine.folder(&context).join(MARK_FILE).exists());
    put_back_cut_short(&mut task).unwrap();
}
