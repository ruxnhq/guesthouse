//! Takes a reading of a running machine: the host reads the VM's QEMU, and for a Windows guest the
//! agent runs the idle script. Nothing here changes anything in the machine or its settings.

use super::parse_report;
use super::vcpu::{LEAST_WINDOW, first_look};
use super::{GuestKind, IdleReading, Unread};
use crate::agent::guest_script;
use crate::context::Context;
use crate::machine::{Kind, Machine};
use crate::slots;
use crate::text::base64_encode;
use std::fs;

const SCRIPT: &str = "idle.ps1";
const HELPER: &str = "idle-input.ps1";
/// It starts a task in the user's session and waits up to ten seconds for it.
const SCRIPT_SECONDS: u64 = 30;

/// Reads the machine that runs in `slot` as the QEMU of `qemu_pid`. The CPU time is a rate over the
/// time the guest takes to answer, and at least [`LEAST_WINDOW`].
pub fn take(context: &Context, machine: &Machine, slot: u8, qemu_pid: u32) -> IdleReading {
    let first = first_look(context, qemu_pid);
    let kind = if machine.kind == Kind::Windows { GuestKind::Windows } else { GuestKind::Other };
    let (remote_sessions, power_requests, input) = match kind {
        GuestKind::Windows => {
            let report = parse_report(&ask(context, machine, slot).unwrap_or_default());
            (report.remote_sessions, report.power_requests, report.input)
        }
        GuestKind::Other => {
            let why = Unread::new("this kind of guest has nothing in it that answers");
            (Err(why.clone()), Err(why.clone()), Err(why))
        }
    };
    let at = context.clock.now();
    if let Ok(first) = &first {
        let waited = at.saturating_sub(first.at());
        context.clock.sleep(LEAST_WINDOW.saturating_sub(waited));
    }
    let measured = first.and_then(|first| first.since(context));
    let (vcpu_cores, vcpu_window_seconds) = match measured {
        Ok((cores, seconds)) => (Ok(cores), seconds),
        Err(unread) => (Err(unread), 0.0),
    };
    IdleReading { kind, input, remote_sessions, power_requests, vcpu_cores, vcpu_window_seconds }
}

/// What the idle script said. Nothing when the script or its helper can't be found here, or the agent
/// didn't answer, which the parser reads the same way.
fn ask(context: &Context, machine: &Machine, slot: u8) -> Option<String> {
    let helper = fs::read(context.paths.guest_files().join(HELPER)).ok()?;
    let script = format!("$helper = '{}'\n{}", base64_encode(&helper), guest_script(context, SCRIPT).ok()?);
    Some(context.ask_guest(machine, &script, SCRIPT_SECONDS, Some(slots::number(slot))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::make_token;
    use crate::agent::stand_in::ScriptedAgent;
    use crate::clock::stand_in::Ticking;
    use crate::idle::write_guest_stat;
    use crate::idle::{Input, PowerKind};
    use crate::testing::{Scratch, scratch_context};
    use std::sync::Arc;

    const POWER_NOTHING: &str =
        "p|DISPLAY:\np|None.\np|\np|SYSTEM:\np|None.\np|\np|AWAYMODE:\np|None.\np|\np|EXECUTION:\np|None.\n";
    const POWER_ASKING: &str = "p|DISPLAY:\np|None.\np|\np|SYSTEM:\np|[PROCESS] vlc.exe\np|\np|AWAYMODE:\np|None.\np|\np|EXECUTION:\np|None.\n";

    fn answer(power: &str, input: &str) -> String {
        format!("sessions ok\ns|0|4|Services\ns|1|1|Console\npower ok\n{power}input {input}\n")
    }

    fn rig(scratch: &Scratch, agent: ScriptedAgent, kind: Kind) -> (Context, Machine, Arc<ScriptedAgent>) {
        let mut context = scratch_context(scratch);
        let agent = Arc::new(agent);
        context.agent = agent.clone();
        context.clock = Arc::new(Ticking::default());
        scratch.write("program/guest/idle.ps1", "read it");
        scratch.write("program/guest/idle-input.ps1", "the helper");
        let machine = Machine { kind, ..Machine::new("m", "M", "/d", "/v") };
        machine.save(&context).unwrap();
        make_token(&context, &machine).unwrap();
        write_guest_stat(&context, 800, 5, 100);
        (context, machine, agent)
    }

    #[test]
    fn a_windows_guest_is_read_through_its_agent_and_its_qemu() {
        let scratch = Scratch::new();
        let (context, machine, agent) =
            rig(&scratch, ScriptedAgent::answering(&[&answer(POWER_ASKING, "42")]), Kind::Windows);
        let reading = take(&context, &machine, 1, 800);
        assert_eq!(reading.kind, GuestKind::Windows);
        assert_eq!(reading.input, Ok(Input::Seconds(42)));
        assert_eq!(reading.remote_sessions, Ok(0));
        let requests = reading.power_requests.unwrap();
        assert_eq!((requests.len(), requests[0].kind), (1, PowerKind::System));
        // No guest time went by, and the window is at least the least there is.
        assert_eq!(reading.vcpu_cores, Ok(0.0));
        assert!(reading.vcpu_window_seconds >= 2.0, "{}", reading.vcpu_window_seconds);
        let asked = agent.requests.lock().unwrap();
        assert_eq!(asked.len(), 1, "one call to the guest");
        let monitor = context.run_folder(Some(slots::number(1))).join(crate::monitor::MAIN_SOCKET);
        assert_eq!(asked[0].0, monitor, "through the slot it runs in");
        assert!(asked[0].2.starts_with("$helper = 'dGhlIGhlbHBlcg=='\nread it"), "{}", asked[0].2);
    }

    #[test]
    fn a_silent_agent_leaves_the_guests_signals_unread_and_the_hosts_read() {
        let scratch = Scratch::new();
        let (context, machine, _) = rig(&scratch, ScriptedAgent::default(), Kind::Windows);
        let reading = take(&context, &machine, 0, 800);
        assert!(
            reading.input.is_err() && reading.remote_sessions.is_err() && reading.power_requests.is_err()
        );
        assert_eq!(reading.vcpu_cores, Ok(0.0));
    }

    #[test]
    fn missing_scripts_are_signals_not_read_and_not_quiet() {
        let scratch = Scratch::new();
        let (context, machine, agent) = rig(&scratch, ScriptedAgent::answering(&["x"]), Kind::Windows);
        std::fs::remove_file(context.paths.guest_files().join(HELPER)).unwrap();
        let reading = take(&context, &machine, 0, 800);
        assert!(reading.input.is_err() && reading.power_requests.is_err());
        assert!(agent.requests.lock().unwrap().is_empty(), "nothing was sent without the helper");
    }

    #[test]
    fn a_guest_that_is_not_windows_is_asked_nothing() {
        for kind in [Kind::Linux, Kind::Plain] {
            let scratch = Scratch::new();
            let (context, machine, agent) = rig(&scratch, ScriptedAgent::default(), kind);
            let reading = take(&context, &machine, 0, 800);
            assert_eq!(reading.kind, GuestKind::Other);
            assert!(reading.input.is_err() && reading.remote_sessions.is_err());
            assert!(reading.vcpu_cores.is_ok(), "CPU time reads for any guest");
            assert!(agent.requests.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn a_vm_that_has_gone_has_no_cpu_time() {
        let scratch = Scratch::new();
        let (context, machine, _) =
            rig(&scratch, ScriptedAgent::answering(&[&answer(POWER_NOTHING, "nobody")]), Kind::Windows);
        let reading = take(&context, &machine, 0, 999);
        assert!(reading.vcpu_cores.is_err());
        assert_eq!(reading.input, Ok(Input::NobodyLoggedIn));
    }
}
