//! An Intel machine started on the hardware desktop is watched until it has been up long enough to
//! show whether Windows keeps the card. When the compositor can't use the card, Windows stops the
//! card for the session within a minute or two, and a tester would be left with no desktop. Then the
//! machine is put on the software desktop, and started again on it in the same start: see
//! [`crate::intel_desktop`].
//!
//! A machine that already has the display driver shows its picture first and is watched after,
//! so nobody looks at a black screen for the length of the watch. One that still has to get the
//! driver is watched first, as putting a driver into a desktop that is about to go helps nobody.
//!
//! A compositor that stays up is not yet a desktop: on a card where what it draws doesn't run, it
//! lives on and shows nothing. So once the display is set up, the watch's last look is for a
//! picture: the display driver's ring, with a frame on it. One with none fails the same way.

use super::{STOP_WAIT_SECONDS, Task};
use crate::context::Context;
use crate::display::{DisplayState, Driver, DriverPackage, read_state, ring_in, write_state};
use crate::failure::Result;
use crate::gpu_maker::GpuMaker;
use crate::intel_desktop;
use crate::log::write_log;
use crate::machine::Machine;
use crate::vm_host::Stopped;
use guesthouse_backend_intel::stage::Desktop;
use guesthouse_daemon::display_note::ScreenNote;
use std::time::Duration;

/// How long after Windows boots the desktop must have stayed on the card. The lab's and a tester's
/// stops came 50 to 80 seconds in.
const WATCHED_UPTIME_SECONDS: u64 = 150;
const POLL: Duration = Duration::from_secs(15);
/// How many looks there are at most, for a guest that stops answering.
const LOOKS: u32 = 16;
const LOOK_SECONDS: u64 = 30;
/// More compositor crashes than this since boot is a failure. On the card's desktop the compositor
/// only crashes when it can't make its device there; after a few it settles on software and looks up,
/// until something makes it try the card again. One is let pass, for a crash that has another cause.
const CRASH_LIMIT: u32 = 1;
const MARK: &str = "hardware-desktop:";
/// How long a picture gets once the display is set up. By then Windows has been up past the watched
/// uptime, and the display step has given the driver its own wait for a frame.
const PICTURE_WAIT: Duration = Duration::from_secs(60);
const PICTURE_POLL: Duration = Duration::from_secs(5);
const NO_RING: &str = "no ring for the display driver";
const NO_FRAME: &str = "no frame came to the display driver's ring";

/// What the guest says about its desktop since it booted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Look {
    uptime_seconds: u64,
    /// Winlogon asked to stop the session's use of the GPU (DxgKrnl-Admin 465).
    session_stops: u32,
    /// Render devices Windows reports with no problem.
    working_render_devices: u32,
    compositor_crashes: u32,
}

impl Look {
    /// Why the hardware desktop has failed, if it has.
    fn failure(&self) -> Option<String> {
        if self.session_stops > 0 {
            return Some("Windows stopped the session's use of the card".to_owned());
        }
        if self.working_render_devices == 0 {
            return Some("Windows turned the card off".to_owned());
        }
        (self.compositor_crashes > CRASH_LIMIT)
            .then(|| format!("the desktop's compositor crashed {} times", self.compositor_crashes))
    }

    fn parse(said: &str) -> Option<Self> {
        let words: Vec<u64> = said
            .split(MARK)
            .nth(1)?
            .split_whitespace()
            .take(4)
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()
            .ok()?;
        let [uptime_seconds, stops, working, crashes] = words[..] else { return None };
        let count = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
        Some(Self {
            uptime_seconds,
            session_stops: count(stops),
            working_render_devices: count(working),
            compositor_crashes: count(crashes),
        })
    }
}

/// The guest's answer, as four numbers after [`MARK`].
fn look_script() -> String {
    format!(
        "$boot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime; \
         $up = [int]((Get-Date) - $boot).TotalSeconds; \
         $stops = @(Get-WinEvent -FilterHashtable @{{LogName='Microsoft-Windows-DxgKrnl-Admin'; Id=465; StartTime=$boot}} -ea 0).Count; \
         $working = @(Get-CimInstance Win32_VideoController | Where-Object {{ ({}) -and $_.ConfigManagerErrorCode -eq 0 }}).Count; \
         $crashes = @(Get-WinEvent -FilterHashtable @{{LogName='Application'; Id=1000; StartTime=$boot}} -ea 0 | \
         Where-Object {{ $_.Message -match 'dwm\\.exe' }}).Count; \
         \"{MARK}$up $stops $working $crashes\"",
        super::gpu_check::is_render_device()
    )
}

/// Whether this start put the desktop on an Intel card, so that it has to be watched.
fn watched(task: &Task) -> bool {
    task.machine.windows_maker() == Some(GpuMaker::Intel)
        && intel_desktop::for_start(task.context, &task.machine) == Desktop::Hardware
}

/// Looks until the guest has been up long enough, or the desktop has failed. A guest that stops
/// answering is not taken as a failure on its own: the look after the GPU check decides. One whose
/// VM has gone, as Windows shut down from its own menu does, is not looked at any longer, and the
/// start says how it went.
fn watch(task: &Task) -> Option<String> {
    task.step("Watching the desktop on the card");
    for attempt in 0..LOOKS {
        if attempt > 0 {
            task.sleep(POLL);
        }
        let Some(look) = Look::parse(&task.ask_guest(&look_script(), LOOK_SECONDS)) else {
            // A VM that has gone is the end of the watch.
            task.pid()?;
            continue;
        };
        if let Some(why) = look.failure() {
            return Some(why);
        }
        if look.uptime_seconds >= WATCHED_UPTIME_SECONDS {
            return None;
        }
    }
    None
}

/// Whether a display driver in this state can show a picture as soon as the display is set up.
fn shows_at_once(display: &DisplayState, has_package: bool) -> bool {
    display.driver == Driver::Installed && !display.restart_owed && has_package
}

/// Whether this start shows its picture before the desktop is watched: one on an Intel card whose
/// guest already has the display driver.
pub(super) fn picture_first(task: &Task) -> bool {
    watched(task)
        && shows_at_once(
            &read_state(task.context, &task.machine),
            DriverPackage::find(task.context).is_some(),
        )
}

/// Whether the hardware desktop failed on this boot, and why. `gpu_missing` is the after-boot check
/// having found no card. With `picture_first` the watch is left for [`failed_with_picture`].
pub(super) fn failed(task: &Task, gpu_missing: bool, picture_first: bool) -> Option<String> {
    if !watched(task) {
        return None;
    }
    if gpu_missing {
        return Some("Windows started without the card".to_owned());
    }
    if picture_first {
        return None;
    }
    watch(task)
}

/// The watch of a start that showed its picture first, once the picture is up.
pub(super) fn failed_with_picture(task: &Task) -> Option<String> {
    if !watched(task) {
        return None;
    }
    watch(task)
}

/// Why there is no picture, from what is on the display driver's ring.
fn picture_failure(ring: Option<ScreenNote>) -> Option<&'static str> {
    match ring {
        None => Some(NO_RING),
        Some(ring) if !ring.frames => Some(NO_FRAME),
        Some(_) => None,
    }
}

/// Whether a desktop on the card shows no picture, and why, once the display is set up. A display
/// driver that waits on a restart, or a build with none to give, is judged on another boot.
pub(super) fn no_picture(task: &Task) -> Option<String> {
    if !watched(task) {
        return None;
    }
    if read_state(task.context, &task.machine).restart_owed || DriverPackage::find(task.context).is_none() {
        return None;
    }
    task.step("Waiting for a picture from the card");
    let end = task.now() + PICTURE_WAIT;
    loop {
        let failure = picture_failure(ring_in(task.context, None))?;
        if task.now() >= end {
            return Some(failure.to_owned());
        }
        task.sleep(PICTURE_POLL);
    }
}

/// A display driver that showed nothing on a desktop the card couldn't draw was not at fault, so the
/// next hardware desktop gets it again rather than going without it for good.
fn forgive_display_driver(context: &Context, machine: &Machine) -> Result<()> {
    let mut state = read_state(context, machine);
    if state.failed_package.is_empty() {
        return Ok(());
    }
    state.failed_package.clear();
    write_state(context, machine, &state)
}

/// What the user is told once the machine is up on the software desktop.
fn told(why: &str) -> String {
    format!(
        "The desktop didn't come up on the Intel card ({why}), so this machine is on the software \
         desktop until the next build."
    )
}

/// Puts the machine on the software desktop and stops it, ready to start again. Says why in the log.
pub(super) fn fall_back_and_stop(task: &mut Task, why: &str) -> Result<String> {
    write_log(task.context, &format!("hardware desktop: {why}; back to the software desktop"))?;
    intel_desktop::fall_back(task.context, &task.machine, why)?;
    forgive_display_driver(task.context, &task.machine)?;
    task.step("Starting again on the software desktop");
    if task.host.stop(&task.machine, STOP_WAIT_SECONDS)? == Stopped::TurnedOff {
        write_log(
            task.context,
            "hardware desktop: Windows did not shut down in time, so the VM was turned off",
        )?;
    }
    // The next start replaces the daemon's log, and a report needs this one's.
    if let Err(error) = intel_desktop::keep_hardware_log(task.context, &task.machine) {
        write_log(task.context, &format!("hardware desktop: its daemon log could not be kept: {error}"))?;
    }
    Ok(told(why))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(stops: u32, working: u32, crashes: u32) -> Look {
        Look {
            uptime_seconds: 60,
            session_stops: stops,
            working_render_devices: working,
            compositor_crashes: crashes,
        }
    }

    #[test]
    fn a_watch_ends_once_the_vm_has_gone() {
        use crate::agent::stand_in::ScriptedAgent;
        use crate::vm_host::stand_in::RecordingHost;
        use std::sync::Arc;
        let scratch = crate::testing::Scratch::new();
        let mut context = super::super::tests::plain_task_context(&scratch);
        let agent = Arc::new(ScriptedAgent::default());
        context.agent = agent.clone();
        let mut intel = super::super::tests::machine(&context);
        intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
        crate::agent::make_token(&context, &intel).unwrap();
        let host = RecordingHost::default();
        let task = Task::new(&context, &host, "start", intel);
        assert_eq!(watch(&task), None);
        assert_eq!(agent.requests.lock().unwrap().len(), 1, "one look, and no more once QEMU is gone");
    }

    #[test]
    fn a_stop_a_card_turned_off_or_a_crash_loop_is_a_failure() {
        assert_eq!(look(0, 1, 0).failure(), None);
        assert_eq!(look(0, 1, CRASH_LIMIT).failure(), None, "one crash with another cause");
        assert!(look(1, 1, 0).failure().unwrap().contains("stopped"));
        assert!(look(0, 0, 0).failure().unwrap().contains("turned the card off"));
        assert!(look(0, 1, CRASH_LIMIT + 1).failure().unwrap().contains("crashed 2 times"));
    }

    #[test]
    fn the_guests_answer_is_read_after_its_mark() {
        let said = format!("noise\n{MARK}151 0 1 3\n");
        let expected =
            Look { uptime_seconds: 151, session_stops: 0, working_render_devices: 1, compositor_crashes: 3 };
        assert_eq!(Look::parse(&said), Some(expected));
        assert_eq!(Look::parse("ERROR: the agent said nothing"), None);
        assert_eq!(Look::parse(&format!("{MARK}12 0")), None);
    }

    #[test]
    fn only_a_display_driver_that_is_in_and_ready_shows_a_picture_at_once() {
        let installed = DisplayState { driver: Driver::Installed, ..DisplayState::default() };
        assert!(shows_at_once(&installed, true));
        assert!(!shows_at_once(&installed, false), "this build has no driver to check it against");
        assert!(!shows_at_once(&DisplayState { restart_owed: true, ..installed.clone() }, true));
        assert!(!shows_at_once(&DisplayState::default(), true), "it still has to be put in");
        assert!(!shows_at_once(&DisplayState { driver: Driver::Failed, ..DisplayState::default() }, true));
    }

    #[test]
    fn a_desktop_with_no_ring_or_no_frame_has_no_picture() {
        let ring = |frames| ScreenNote { width: 1920, height: 1080, hdr: false, daemon: 1, frames };
        assert_eq!(picture_failure(None), Some(NO_RING));
        assert_eq!(picture_failure(Some(ring(false))), Some(NO_FRAME));
        assert_eq!(picture_failure(Some(ring(true))), None);
    }

    #[test]
    fn falling_back_lets_the_display_driver_be_tried_again() {
        let scratch = crate::testing::Scratch::new();
        let context = crate::testing::scratch_context(&scratch);
        let machine = Machine::new("arc", "Arc", "/m/disk.qcow2", "");
        std::fs::create_dir_all(machine.folder(&context)).unwrap();
        let mut state = read_state(&context, &machine);
        state.failed_package = "guesthouse-display-1".to_owned();
        write_state(&context, &machine, &state).unwrap();
        forgive_display_driver(&context, &machine).unwrap();
        assert!(read_state(&context, &machine).failed_package.is_empty());
    }

    #[test]
    fn the_message_says_why_and_until_when() {
        let message = told("Windows stopped the session's use of the card");
        assert!(message.contains("(Windows stopped the session's use of the card)"));
        assert!(message.contains("until the next build"));
    }
}
