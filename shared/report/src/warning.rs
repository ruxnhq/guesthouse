//! What to tell the person before they send, if the report would be thin: no logs of the machine
//! running, or a machine that isn't running to ask. One message for both, and none for a machine
//! that couldn't be running for the problem they have. Sending anyway is always theirs to choose.
//! These are the Linux host's words.

use super::kind::ProblemKind;

/// The message, if there's one. `machine` is whether the report is about a machine, `running`
/// whether it was running when the report was gathered, and `has_run_logs` whether the report has
/// any logs of it running.
pub fn before_sending(machine: bool, running: bool, has_run_logs: bool, kind: ProblemKind) -> Option<String> {
    if !machine {
        return None;
    }
    let can_run = !matches!(kind, ProblemKind::WontStart | ProblemKind::InstallFailed);
    let not_running = can_run && !running;
    let no_logs = !has_run_logs;
    let text = match (no_logs, not_running) {
        (true, true) => {
            "This report has no logs of the machine running, and logs from inside the guest need it running. \
             The fullest report comes with the machine running: start it, make the problem happen again if \
             you can, then send."
        }
        (true, false) => {
            "This report has no logs of the machine running. Start it, make the problem happen again, then \
             send. If it won't start at all, send anyway."
        }
        (false, true) => {
            "The machine isn't running, so the report has nothing from inside the guest. The fullest report \
             comes with it running: start it, make the problem happen if you can, then send."
        }
        (false, false) => return None,
    };
    Some(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_said_without_a_machine_or_with_a_full_report() {
        for kind in ProblemKind::ALL {
            assert_eq!(before_sending(false, false, false, kind), None);
            assert_eq!(before_sending(true, true, true, kind), None);
        }
    }

    #[test]
    fn a_machine_that_cant_be_running_for_the_problem_isnt_asked_to_be() {
        assert_eq!(before_sending(true, false, true, ProblemKind::WontStart), None);
        assert_eq!(before_sending(true, false, true, ProblemKind::InstallFailed), None);
        let said = before_sending(true, false, false, ProblemKind::WontStart).unwrap();
        assert!(said.ends_with("If it won't start at all, send anyway."));
    }
}
