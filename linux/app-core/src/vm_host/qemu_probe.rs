//! Whether the QEMU a machine runs knows the user network setting that keeps the guest off the host's
//! loopback. It's asked at each start, of that QEMU, whatever its version says.

use crate::failure::Failure;
use crate::vm_args::VmPlan;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A value no QEMU takes, so QEMU stops at once while reading its words, with no machine made: a
/// QEMU that knows the setting says the value is wrong, and one that doesn't says so of the setting.
const QUESTION: [&str; 7] =
    ["-nodefaults", "-machine", "none", "-display", "none", "-netdev", "user,id=probe,host-loopback=ask"];
const UNKNOWN: &str = "Invalid parameter 'host-loopback'";
const ANSWER_WITHIN: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);
pub(super) const HELP: &str = "This QEMU's own network can't keep the guest off this PC's loopback, which \
                               every machine on it needs. Start the machine with the QEMU that comes with \
                               Guesthouse, or turn passt on in its network settings.";

/// Refuses the start when the plan gives QEMU's own network `host-loopback=off` and its QEMU says it
/// doesn't know the setting. A QEMU that can't be asked, or answers anything else, starts as it
/// would, and says for itself what's wrong.
pub(super) fn check(plan: &VmPlan) -> Result<(), Failure> {
    if plan.keeps_host_loopback_off() && says_unknown(&plan.qemu) {
        return Err(Failure::new(HELP));
    }
    Ok(())
}

/// Whether `qemu` answers the question by not knowing the setting. One that doesn't stop in time is
/// ended, and counts as knowing it.
fn says_unknown(qemu: &Path) -> bool {
    let Ok(mut child) = Command::new(qemu)
        .args(QUESTION)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return false;
    };
    let end = Instant::now() + ANSWER_WITHIN;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < end => std::thread::sleep(POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
    let mut said = String::new();
    let _ = child.stderr.take().map(|mut stderr| stderr.read_to_string(&mut said));
    said.contains(UNKNOWN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Kind, Machine};
    use crate::network::Mode;
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};
    use crate::vm_args::Boot;

    fn plan_with(scratch: &Scratch, qemu: &str, mode: Mode) -> VmPlan {
        let mut context = scratch_context(scratch);
        context.paths.qemu = scratch.program("stand-in-qemu", qemu);
        let machine =
            Machine { kind: Kind::Plain, network_mode: mode, ..Machine::new("vm", "VM", "/d", "/v") };
        VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0)
    }

    /// Answers as a QEMU without the setting does, and only to the question.
    const WITHOUT: &str = "#!/bin/sh\ncase \"$*\" in *host-loopback=ask*) \
                           echo \"qemu: -netdev user,id=probe,host-loopback=ask: Invalid parameter 'host-loopback'\" >&2; \
                           exit 1;; esac\nexit 0\n";
    /// Answers as a QEMU with the setting does.
    const WITH: &str = "#!/bin/sh\necho \"qemu: -netdev user,id=probe,host-loopback=ask: Parameter \
                        'host-loopback' expects 'on' or 'off'\" >&2\nexit 1\n";

    #[test]
    fn a_qemu_that_does_not_know_the_setting_is_refused_with_what_to_do() {
        let scratch = Scratch::new();
        let refused = check(&plan_with(&scratch, WITHOUT, Mode::Nat)).unwrap_err();
        assert!(refused.0.contains("the QEMU that comes with Guesthouse") && refused.0.contains("passt"));
    }

    #[test]
    fn a_qemu_that_knows_it_starts() {
        let scratch = Scratch::new();
        assert!(check(&plan_with(&scratch, WITH, Mode::Nat)).is_ok());
    }

    #[test]
    fn a_restricted_network_does_not_ask() {
        let scratch = Scratch::new();
        assert!(check(&plan_with(&scratch, WITHOUT, Mode::None)).is_ok());
    }

    #[test]
    fn a_qemu_that_never_answers_is_ended_and_starts_as_it_would() {
        let scratch = Scratch::new();
        let started = Instant::now();
        assert!(check(&plan_with(&scratch, "#!/bin/sh\nexec sleep 30\n", Mode::Nat)).is_ok());
        assert!(started.elapsed() < Duration::from_secs(15));
    }
}
