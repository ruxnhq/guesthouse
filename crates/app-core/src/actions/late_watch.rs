//! A start on an Intel card that showed its picture first is reported done once the picture is up,
//! and the watch of [`super::hardware_desktop`] goes on after that, in the same process but outside
//! the task: the machine shows as running, and its controls work, while Windows is watched to the
//! end of the time a failing card is stopped in. The watch holds no lock and writes no step.
//!
//! A desktop that fails meanwhile is handled as it would be inside the start: the watch takes the
//! slot as a start again, once no other task has it, puts the machine on the software desktop and
//! starts it again there. A VM that has gone, or another one in the slot, ends the watch.

use super::turn_off_after_failed_start;
use super::{KEY_POLL, Task, hardware_desktop, start_again_on_software_desktop, take_lock};
use crate::log::write_log;
use crate::tasks::{read_task, task_alive};
use std::fs::File;
use std::time::Duration;

/// How long a failed desktop waits for another task in the slot to end, such as an API check the
/// user started meanwhile.
const TAKE_OVER_WAIT: Duration = Duration::from_secs(600);

/// The watch, once the start has been reported done. What it found is in the app's log.
pub(super) fn watch_after_start(task: &mut Task) {
    let Some(qemu) = task.pid() else { return };
    note(task, "the start is done, and the desktop on the card is watched on");
    let Some(why) = hardware_desktop::looks(task, qemu) else {
        let end = if task.pid() == Some(qemu) { "the desktop stayed on the card" } else { "the VM has gone" };
        note(task, &format!("the watch after the start ended: {end}"));
        return;
    };
    note(task, &format!("the watch after the start found that {why}"));
    let Some(_lock) = take_slot(task, qemu) else {
        note(task, "the slot was not free, or the VM went, so the machine was left as it is");
        return;
    };
    task.error.clear();
    if let Err(failure) = start_again_on_software_desktop(task, &why) {
        turn_off_after_failed_start(task);
        task.error = failure.0;
    }
    task.finish();
}

/// The slot's lock, once nothing else runs there, for as long as `qemu` is the VM in it.
fn take_slot(task: &Task, qemu: u32) -> Option<File> {
    let end = task.now() + TAKE_OVER_WAIT;
    while task.now() < end && task.pid() == Some(qemu) {
        let other_task = task_alive(task.context, &read_task(task.context, None));
        if !other_task && let Ok(lock) = take_lock(task.context) {
            return Some(lock);
        }
        task.sleep(KEY_POLL);
    }
    None
}

fn note(task: &Task, text: &str) {
    // The watch goes on without its line in the log.
    let _ = write_log(task.context, &format!("hardware desktop: {text}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::stand_in::ScriptedAgent;
    use crate::failure::Failure;
    use crate::gpu_maker::GpuMaker;
    use crate::intel_desktop;
    use crate::log::read_log;
    use crate::processes::stand_in::fake_process;
    use crate::tasks::write_task;
    use crate::testing::Scratch;
    use crate::vm_host::stand_in::RecordingHost;
    use guesthouse_backend_intel::stage::Desktop;
    use std::sync::Arc;

    const STOPPED: &str = "hardware-desktop:60 1 1 0\r\n";
    const STAYED: &str = "hardware-desktop:160 0 1 0\r\n";

    fn intel_task<'a>(context: &'a crate::context::Context, host: &'a RecordingHost) -> Task<'a> {
        let mut intel = super::super::tests::machine(context);
        intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
        crate::agent::make_token(context, &intel).unwrap();
        Task::new(context, host, "start", intel)
    }

    fn context_answering(scratch: &Scratch, answers: &[&str]) -> crate::context::Context {
        let mut context = super::super::tests::plain_task_context(scratch);
        context.agent = Arc::new(ScriptedAgent::answering(answers));
        super::super::tests::with_vm_running(&context);
        context
    }

    #[test]
    fn a_desktop_that_stays_on_the_card_is_left_as_it_is() {
        let scratch = Scratch::new();
        let context = context_answering(&scratch, &[STAYED]);
        let host = RecordingHost::default();
        let mut task = intel_task(&context, &host);
        watch_after_start(&mut task);
        assert!(host.said().is_empty(), "nothing stopped or started");
        assert_eq!(read_task(&context, None), crate::tasks::TaskRecord::default(), "no step written");
        assert!(read_log(&context).contains("the desktop stayed on the card"));
    }

    #[test]
    fn a_desktop_that_fails_after_the_start_goes_to_the_software_desktop_as_a_start() {
        let scratch = Scratch::new();
        let context = context_answering(&scratch, &[STOPPED]);
        let host = RecordingHost::default();
        // The start again fails at once, which keeps the test short; the fallback has happened by then.
        *host.start_failure.lock().unwrap() = Some(Failure::new("QEMU did not start."));
        let mut task = intel_task(&context, &host);
        watch_after_start(&mut task);
        assert_eq!(intel_desktop::for_start(&context, &task.machine), Desktop::Software);
        assert_eq!(host.said()[..2], ["stop 100", "start gpu"]);
        let record = read_task(&context, None);
        assert_eq!((record.action.as_str(), record.pid), ("start", 0), "{record:?}");
        assert_eq!(record.error, "QEMU did not start.");
        assert!(read_log(&context).contains("found that Windows stopped the session's use of the card"));
    }

    #[test]
    fn a_failed_desktop_waits_for_the_task_in_the_slot_and_in_time_leaves_the_machine_as_it_is() {
        let scratch = Scratch::new();
        let context = context_answering(&scratch, &[STOPPED]);
        fake_process(&context, 79, &["guesthouse-task", "api-check", "x"], &[], "");
        let busy = crate::tasks::TaskRecord { action: "api-check".into(), pid: 79, ..Default::default() };
        write_task(&context, None, &busy).unwrap();
        let host = RecordingHost::default();
        let mut task = intel_task(&context, &host);
        let qemu = task.pid().unwrap();
        assert!(take_slot(&task, qemu + 1).is_none(), "another VM's slot is not taken");
        watch_after_start(&mut task);
        assert!(host.said().is_empty(), "the API check's slot was left alone");
        assert_eq!(read_task(&context, None), busy);
        assert_eq!(intel_desktop::for_start(&context, &task.machine), Desktop::Hardware);
        assert!(read_log(&context).contains("the slot was not free"));
    }
}
