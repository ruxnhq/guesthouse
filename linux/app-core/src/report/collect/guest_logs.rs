//! What a running Windows guest says about itself, through its agent and `guest/windows/report.ps1`: the logs
//! Guesthouse's guest pieces write, display events, the adapter, the build, and whether a dump was
//! written. Text only, in the agent's answer size and a time limit, and a guest that doesn't answer
//! only costs this part.
//!
//! A Linux guest has no agent the host can ask over the same channel, so nothing comes from inside
//! one here. Android has none either, and its logcat, which it writes to a console of its own, goes
//! in with the run's logs instead.

use super::{Collection, Request};
use crate::agent::guest_script;
use crate::context::Context;
use crate::machine::{Kind, Machine};
use crate::processes::running_machine;
use crate::report::scrub::scrub_names;
use crate::slots;

const SCRIPT: &str = "report.ps1";
/// Long enough for the event log on a slow guest, short enough not to hold up a report for long.
const SECONDS: u64 = 30;
const SECTION_START: &str = "=== ";
const NAMES: &str = "names";
const END: &str = "end";
const LOG_START: &str = "log ";
pub(super) const GUEST_USER_MARK: &str = "<guest-user>";
pub(super) const GUEST_PC_MARK: &str = "<guest-pc>";
/// Accounts every Windows has, whose names are words in its own logs and say nothing of whose it is.
const BUILT_IN_ACCOUNTS: [&str; 4] = ["Administrator", "Guest", "DefaultAccount", "WDAGUtilityAccount"];
const ANDROID_LOGCAT: &str = "Android has no agent to ask. What its logcat wrote, errors and crashes from every \
                              app, is with the run's logs as logcat.log.";
const NEEDS_RUNNING: &str = "Logs from inside the guest need the machine running, so there are none here.";

/// What the guest's answer holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GuestAnswer {
    /// Each part, by the file it becomes in the bundle.
    pub parts: Vec<(String, String)>,
    /// The guest's computer name, then its accounts, to take out of the parts.
    pub names: Vec<String>,
    /// Whether the script got to its end, rather than the time or the size running out.
    pub whole: bool,
}

pub fn collect(context: &Context, machine: &Machine, request: &Request, found: &mut Collection) {
    let running = running_machine(context, machine);
    found.machine_running = running.is_some();
    if !request.guest_logs {
        found.notes.push("Logs from inside the guest were left out, as asked.".to_owned());
        return;
    }
    if machine.android() {
        found.notes.push(ANDROID_LOGCAT.to_owned());
        return;
    }
    if machine.kind == Kind::Linux || machine.kind == Kind::Plain {
        found.notes.push(format!(
            "A {} has no agent to ask, so there are no logs from inside it.",
            machine.kind_name()
        ));
        return;
    }
    let Some((slot, _)) = running else {
        found.notes.push(NEEDS_RUNNING.to_owned());
        return;
    };
    let script = match guest_script(context, SCRIPT) {
        Ok(script) => script,
        Err(failure) => {
            found.miss(
                "guest/",
                &format!("the guest's report script could not be read: {}", failure.0),
                None,
            );
            return;
        }
    };
    let said = context.ask_guest(machine, &script, SECONDS, Some(slots::number(slot)));
    add_answer(found, &said);
}

/// The guest's answer, with its names taken out, as files under `guest/`.
fn add_answer(found: &mut Collection, said: &str) {
    if said.trim().is_empty() {
        found.miss("guest/", &format!("the guest's agent gave no answer in {SECONDS} seconds"), None);
        return;
    }
    let answer = read_answer(said);
    let mut names: Vec<(&str, &str)> = Vec::new();
    if let Some((computer, accounts)) = answer.names.split_first() {
        names.push((GUEST_PC_MARK, computer));
        let own = accounts.iter().filter(|account| {
            !BUILT_IN_ACCOUNTS.iter().any(|built_in| built_in.eq_ignore_ascii_case(account))
        });
        names.extend(own.map(|account| (GUEST_USER_MARK, account.as_str())));
    }
    for (name, text) in &answer.parts {
        found.add(&format!("guest/{name}"), scrub_names(text, &names));
    }
    if !answer.whole {
        found.notes.push("The guest's answer stopped before its end, so part of it is missing.".to_owned());
    }
}

/// Splits the script's output into its parts.
pub fn read_answer(said: &str) -> GuestAnswer {
    let mut answer = GuestAnswer::default();
    let mut current: Option<(String, String)> = None;
    for line in said.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix(SECTION_START) {
            if let Some(part) = current.take() {
                answer.parts.push(part);
            }
            if name == END {
                answer.whole = true;
                continue;
            }
            current = Some((file_name(name), String::new()));
            continue;
        }
        match &mut current {
            Some((name, _)) if name == NAMES => {
                let word = line.trim();
                if !word.is_empty() && !answer.names.iter().any(|known| known == word) {
                    answer.names.push(word.to_owned());
                }
            }
            Some((_, text)) => {
                text.push_str(line);
                text.push('\n');
            }
            None => {}
        }
    }
    if let Some(part) = current.take() {
        answer.parts.push(part);
    }
    answer.parts.retain(|(name, _)| name != NAMES);
    answer
}

/// `log nv-files.log` is that file, and anything else is a text file of its name. Only plain names
/// are taken, so the guest can't name a path.
fn file_name(part: &str) -> String {
    let name = part.strip_prefix(LOG_START).unwrap_or(part);
    let plain: String = name
        .chars()
        .filter(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '-' | '_' | '.'))
        .collect();
    let plain = plain.trim_start_matches('.');
    if part == NAMES {
        NAMES.to_owned()
    } else if part.starts_with(LOG_START) {
        plain.to_owned()
    } else {
        format!("{plain}.txt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::stand_in::ScriptedAgent;
    use crate::testing::{Scratch, scratch_context};
    use std::sync::Arc;

    const ANSWER: &str = "=== names\r\nDESKTOP-QUILL7\r\nquillon\r\nquillon\r\nGuest\r\n\
                          === log nv-files.log\r\nplaced 41 files for quillon\r\n\
                          === log ../../evil\r\nnope\r\n\
                          === events\r\n2026-11-17T10:00:00 Display 4101 Warning: the driver stopped responding\r\n\
                          === windows\r\nMicrosoft Windows 11 Pro 10.0.26100.2314\r\n\
                          === dumps\r\nC:\\Windows\\Minidump\\111726-01.dmp, 2026-11-17T10:00:01, 1234 bytes\r\n\
                          === end\r\n";

    #[test]
    fn the_answer_splits_into_its_parts() {
        let answer = read_answer(ANSWER);
        let names: Vec<&str> = answer.parts.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["nv-files.log", "evil", "events.txt", "windows.txt", "dumps.txt"]);
        assert_eq!(answer.names, ["DESKTOP-QUILL7", "quillon", "Guest"]);
        assert!(answer.whole);
        assert_eq!(answer.parts[3].1, "Microsoft Windows 11 Pro 10.0.26100.2314\n");
    }

    #[test]
    fn an_answer_cut_short_says_so() {
        let mut found = Collection::default();
        add_answer(&mut found, "=== names\nPC\n=== events\nhalf a li");
        assert_eq!(found.names().collect::<Vec<_>>(), ["guest/events.txt"]);
        assert!(found.notes.iter().any(|note| note.contains("stopped before its end")));
    }

    #[test]
    fn the_guests_names_are_taken_out() {
        let mut found = Collection::default();
        add_answer(&mut found, ANSWER);
        let all: String = found.items.iter().map(|item| item.text.as_str()).collect();
        assert!(!all.contains("quillon") && !all.contains("DESKTOP-QUILL7"), "{all}");
        assert!(all.contains("placed 41 files for <guest-user>"));
        assert!(!all.contains("<guest-user>-user"), "a built-in account's name isn't taken out of words");
        assert!(!found.names().any(|name| name.contains("names")), "the names never go in");
    }

    fn windows(context: &Context) -> Machine {
        let machine = Machine::new("w11", "W11", "/d", "/v");
        machine.save(context).unwrap();
        machine
    }

    #[test]
    fn a_machine_that_isnt_running_is_not_asked_and_it_says_why() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let agent = Arc::new(ScriptedAgent::answering(&[ANSWER]));
        context.agent = agent.clone();
        let mut found = Collection::default();
        let request = Request { guest_logs: true, ..Request::default() };
        collect(&context, &windows(&context), &request, &mut found);
        assert!(agent.requests.lock().unwrap().is_empty());
        assert!(!found.machine_running);
        assert!(found.notes.iter().any(|note| note.contains("need the machine running")));
    }

    #[test]
    fn a_guest_that_gives_no_answer_costs_only_its_part() {
        let mut found = Collection::default();
        add_answer(&mut found, "");
        assert_eq!(found.items.len(), 0);
        assert!(found.missing[0].reason.contains("gave no answer"));
    }

    /// The repo's own script, as no PowerShell runs here to try it: it has to ask for what says why
    /// Windows stopped a display device, which the display stack's own providers in System never do.
    #[test]
    fn the_script_asks_for_why_a_display_device_was_stopped() {
        let script =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../guest/windows/report.ps1"))
                .unwrap();
        for wanted in [
            "'Microsoft-Windows-Kernel-PnP'",
            "@(411, 219)",
            r"'WUDFRd*ROOT\DISPLAY\*0xC0000365'",
            "'Microsoft-Windows-DxgKrnl-Admin'",
            "'Microsoft-Windows-DxgKrnl-Operational'",
            "$lines -join ' | '",
            "Get-PnpDevice -Class Display",
            "DEVPKEY_Device_ProblemStatus",
            "ConfigManagerErrorCode",
        ] {
            assert!(script.contains(wanted), "report.ps1 lacks {wanted}");
        }
        assert!(script.contains("Select-Object -First $mostEvents"), "still bounded");
    }

    #[test]
    fn a_plug_and_play_event_keeps_its_problem_and_the_guests_names_go() {
        let mut found = Collection::default();
        let event = "2026-11-17T19:30:00 Microsoft-Windows-Kernel-PnP 411 Error: Device PCI\\VEN_1414&DEV_008E had a \
                     problem starting. | Driver Name: oem12.inf | Problem: 0x2B | Problem Status: 0xC00000E5 | on DESKTOP-QUILL7";
        add_answer(
            &mut found,
            &format!("=== names\r\nDESKTOP-QUILL7\r\nquillon\r\n=== events\r\n{event}\r\n=== end\r\n"),
        );
        let events = &found.items[0].text;
        assert!(events.contains("Problem Status: 0xC00000E5"), "{events}");
        assert!(!events.contains("QUILL7"), "{events}");
    }
}
