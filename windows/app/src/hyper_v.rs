//! What the Network section says of Windows' Hyper-V feature, which bridging is made on, and what it
//! offers to do, for each case the service says, with the words of the questions asked before
//! turning it on and before restarting Windows. It has nothing of Windows in it, so its tests run
//! on any system. The service decides and does it all; the app only words it.

// Off Windows only the tests use these.
#![cfg_attr(not(windows), allow(dead_code))]

use guesthouse_control::{HyperV, Machine, State, System};

/// What can be done from the section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Turn Hyper-V on, which asks first.
    TurnOn,
    /// Turn it on again, after a try that failed.
    TryAgain,
    /// Restart Windows, which asks first.
    Restart,
}

impl Action {
    /// The button's words.
    pub fn label(self) -> &'static str {
        match self {
            Action::TurnOn => "Turn on Hyper-V",
            Action::TryAgain => "Try again",
            Action::Restart => "Restart now",
        }
    }
}

/// How the section's Hyper-V part looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    /// Waiting for something the person does, as a restart.
    Caution,
    Bad,
}

/// What the section shows of Hyper-V while a machine can't be bridged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub line: String,
    /// More, under it, if there's more to say.
    pub more: Option<String>,
    /// What can be done, and whether it can be now.
    pub action: Option<(Action, bool)>,
    /// Something is going on, which a ring shows.
    pub busy: bool,
    pub tone: Tone,
}

/// Why turning Hyper-V on waits for the machines, as the service says.
const WAITS: &str = "Hyper-V is turned on only while no machine is running or being worked on, as the restart that finishes \
     it stops every machine anyway.";

/// The machines that hold turning Hyper-V on up, by name: those running, being made, or with work
/// going on, as the service counts them.
pub fn holding_up(machines: &[Machine]) -> Vec<String> {
    machines
        .iter()
        .filter(|machine| !matches!(machine.state, State::Off | State::Failed) || machine.working.is_some())
        .map(|machine| machine.name.clone())
        .collect()
}

/// `names` as a list in words: `a`, `a and b`, `a, b and c`.
pub fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the section shows of Hyper-V standing as `hyper_v` says, for a machine of `system`, with
/// `holding` the machines that hold turning it on up, `asking` whether the app is turning it on
/// now, and `reason` the service's own words, for a service from before it said `hyper_v`. `None`
/// while it's on: the adapters show then.
pub fn shown(
    hyper_v: &HyperV,
    system: System,
    holding: &[String],
    asking: bool,
    reason: Option<&str>,
) -> Option<Shown> {
    let plain = |line: String, more: Option<String>, action: Option<(Action, bool)>| Shown {
        line,
        more,
        action,
        busy: false,
        tone: Tone::Plain,
    };
    // Whether it can be turned on now, and why not if not.
    let can = holding.is_empty() && !asking;
    let held = (!holding.is_empty()).then(|| format!("Stop {} first: {WAITS}", listed(holding)));
    Some(match hyper_v {
        HyperV::On => return None,
        HyperV::Off => plain(
            "Bridging is made on Windows' Hyper-V feature, which is off. It's turned on if you choose, and then \
             Windows has to restart before a machine can be bridged."
                .to_owned(),
            held,
            Some((Action::TurnOn, can)),
        ),
        HyperV::Unknown { reason } => plain(
            format!("Bridging is made on Windows' Hyper-V feature, and whether it's on couldn't be told: {reason}"),
            held,
            Some((Action::TurnOn, can)),
        ),
        HyperV::Failed { reason, log } => Shown {
            line: format!("Hyper-V couldn't be turned on. {reason}"),
            more: match (held, log) {
                (Some(held), _) => Some(held),
                (None, Some(log)) => Some(format!("DISM's log is {log}.")),
                (None, None) => None,
            },
            action: Some((Action::TryAgain, can)),
            busy: false,
            tone: Tone::Bad,
        },
        HyperV::TurningOn { percent } => Shown {
            line: match percent {
                Some(percent) => format!("Turning on Hyper-V, {percent}% done."),
                None => "Turning on Hyper-V.".to_owned(),
            },
            more: Some(
                "This can take several minutes, and no machine can start until it's done. Windows has to restart \
                 after."
                    .to_owned(),
            ),
            action: None,
            busy: true,
            tone: Tone::Plain,
        },
        HyperV::RestartPending => Shown {
            line: "Hyper-V is turned on. Windows has to restart before a machine can be bridged.".to_owned(),
            more: Some(RESTART_MORE.to_owned()),
            action: Some((Action::Restart, true)),
            busy: false,
            tone: Tone::Caution,
        },
        HyperV::NotInEdition => plain(
            match system {
                System::Windows => "Not available in Windows Home, which has no Hyper-V. Forward ports below to reach \
                                    the guest from other devices, and Remote View shows its screen on them."
                    .to_owned(),
                System::Linux => "Not available in Windows Home, which has no Hyper-V. Forward ports below to reach \
                                  the guest from other devices."
                    .to_owned(),
            },
            None,
            None,
        ),
        HyperV::NoVirtualization => plain(
            "Bridging is made on Windows' Hyper-V feature, which needs the processor's virtualization, and this \
             PC's processor says it's off in the firmware, or it has none. Turn it on in the firmware's settings \
             (UEFI or BIOS), and Hyper-V can be turned on here after."
                .to_owned(),
            None,
            None,
        ),
        // A service from before this says only why, in its own words.
        HyperV::Unsaid => plain(reason.unwrap_or("This PC can't bridge a machine.").to_owned(), None, None),
    })
}

/// Under the line that says a restart is needed, here and on Home.
pub const RESTART_MORE: &str = "Restarting asks each running machine to shut down first. Until then, machines stay on the shared network.";

/// The line on Home while Windows has to restart to finish turning Hyper-V on.
pub const HOME_LINE: &str = "Hyper-V is turned on. Windows has to restart before a machine can be bridged.";

/// The question asked before Hyper-V is turned on: its title, what it says, and its buttons.
pub const ASK_TITLE: &str = "Turn on Hyper-V?";
pub const ASK_TURN_ON: &str = "Turn on";
pub const ASK_WORDS: [&str; 4] = [
    "Windows turns on its Hyper-V feature, Microsoft-Hyper-V, with the parts of it that feature needs, which \
     bridging is made on. Then Windows has to restart before a machine can be bridged: Guesthouse doesn't restart \
     it, you do, when it suits you. Your machines are kept, on the shared network until you choose Bridged.",
    "It takes a few minutes, and Windows may download what it needs from Windows Update. No machine can start \
     until it's done.",
    "Hyper-V runs on Windows' hypervisor, which already runs for Guesthouse's machines through Virtual Machine \
     Platform. Microsoft says some other virtualization apps, such as VMware Workstation and VirtualBox, may not \
     start their machines, or may run them unreliably, while it runs.",
    "To undo it, turn Hyper-V off in Turn Windows features on or off, and restart.",
];

/// The question asked before Windows is restarted.
pub const RESTART_TITLE: &str = "Restart Windows now?";
pub const RESTART_BUTTON: &str = "Restart";

/// What the question before a restart says, naming the machines running now, which the service asks
/// to shut down first.
pub fn restart_words(running: &[String]) -> String {
    let machines = match running {
        [] => "No machine is running now.".to_owned(),
        [one] => format!(
            "The Guesthouse service asks {one}, which is running, to shut down first, which can take a minute."
        ),
        many => format!(
            "The Guesthouse service asks {}, which are running, to shut down first, which can take a minute.",
            listed(many)
        ),
    };
    format!(
        "Windows asks each open app to close, and an app with unsaved work can hold the restart up until you say. \
         Nothing is closed without asking. {machines}"
    )
}

/// The machines running now, which a restart shuts down.
pub fn running(machines: &[Machine]) -> Vec<String> {
    machines
        .iter()
        .filter(|machine| !matches!(machine.state, State::Off | State::Failed))
        .map(|machine| machine.name.clone())
        .collect()
}

/// What the app says once its request to turn Hyper-V on has ended: `None` when it worked.
pub fn ended_words(reason: Option<&str>) -> (String, bool) {
    match reason {
        None => ("Hyper-V is turned on. Restart Windows to finish.".to_owned(), false),
        Some(reason) => (format!("Hyper-V wasn't turned on. {reason}"), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_control::{Card, NetworkMode};
    use std::path::PathBuf;

    fn machine(name: &str, state: State, working: Option<&str>) -> Machine {
        Machine {
            name: name.to_owned(),
            system: System::Windows,
            state,
            set_up: true,
            memory_mb: 8192,
            processors: 4,
            card: None,
            chosen_card: Card::Default,
            nvapi_wrapper: true,
            tpm: true,
            network: true,
            shares: Vec::new(),
            network_mode: NetworkMode::Shared,
            forwards: Vec::new(),
            nested_virtualization: false,
            ssh_port: None,
            size_gb: None,
            folder: PathBuf::new(),
            problem: None,
            snapshots: Vec::new(),
            working: working.map(str::to_owned),
            address: None,
            remote_view: false,
            remote_port: None,
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_machines_running_or_worked_on_hold_it_up_as_the_service_counts_them() {
        let machines = [
            machine("off", State::Off, None),
            machine("failed", State::Failed, None),
            machine("running", State::Running, None),
            machine("making", State::Installing, None),
            machine("moving", State::Off, Some("Moving its disks")),
            machine("stopping", State::Stopping, None),
        ];
        assert_eq!(holding_up(&machines), names(&["running", "making", "moving", "stopping"]));
        assert_eq!(running(&machines), names(&["running", "making", "stopping"]));
    }

    #[test]
    fn off_offers_turning_it_on_unless_a_machine_holds_it_up_or_it_is_being_asked() {
        let free = shown(&HyperV::Off, System::Windows, &[], false, None).unwrap();
        assert_eq!(free.action, Some((Action::TurnOn, true)));
        assert!(free.line.contains("which is off") && free.more.is_none());
        let held = shown(&HyperV::Off, System::Windows, &names(&["gaming", "media"]), false, None).unwrap();
        assert_eq!(held.action, Some((Action::TurnOn, false)));
        assert!(held.more.unwrap().starts_with("Stop gaming and media first: "));
        let asking = shown(&HyperV::Off, System::Windows, &[], true, None).unwrap();
        assert_eq!(asking.action, Some((Action::TurnOn, false)));
    }

    #[test]
    fn each_case_is_worded_on_its_own() {
        let show =
            |hyper_v: HyperV| shown(&hyper_v, System::Windows, &[], false, Some("Old words.")).unwrap();
        assert_eq!(shown(&HyperV::On, System::Windows, &[], false, None), None);
        let turning = show(HyperV::TurningOn { percent: Some(40) });
        assert!(turning.busy && turning.action.is_none() && turning.line == "Turning on Hyper-V, 40% done.");
        assert_eq!(show(HyperV::TurningOn { percent: None }).line, "Turning on Hyper-V.");
        let restart = show(HyperV::RestartPending);
        assert_eq!((restart.action, restart.tone), (Some((Action::Restart, true)), Tone::Caution));
        let failed = show(HyperV::Failed { reason: "No room.".into(), log: Some(r"C:\dism.log".into()) });
        assert_eq!(failed.line, "Hyper-V couldn't be turned on. No room.");
        assert_eq!(failed.more.as_deref(), Some(r"DISM's log is C:\dism.log."));
        assert_eq!((failed.action, failed.tone), (Some((Action::TryAgain, true)), Tone::Bad));
        let home = show(HyperV::NotInEdition);
        assert!(
            home.action.is_none() && home.line.contains("Windows Home") && home.line.contains("Remote View")
        );
        let ubuntu = shown(&HyperV::NotInEdition, System::Linux, &[], false, None).unwrap();
        assert!(!ubuntu.line.contains("Remote View"), "an Ubuntu guest has none");
        let firmware = show(HyperV::NoVirtualization);
        assert!(firmware.action.is_none() && firmware.line.contains("firmware"));
        let unknown = show(HyperV::Unknown { reason: "DISM is busy.".into() });
        assert!(unknown.line.ends_with("DISM is busy.") && unknown.action == Some((Action::TurnOn, true)));
        let older = show(HyperV::Unsaid);
        assert_eq!((older.line.as_str(), older.action), ("Old words.", None));
    }

    #[test]
    fn the_questions_say_what_changes_and_how_to_undo_it() {
        let asked = ASK_WORDS.join(" ");
        for said in
            ["Microsoft-Hyper-V", "restart", "Turn Windows features on or off", "VMware", "VirtualBox"]
        {
            assert!(asked.contains(said), "{said}");
        }
        assert!(!asked.to_lowercase().contains("anti-cheat"), "nothing unsourced");
        assert!(restart_words(&[]).ends_with("No machine is running now."));
        assert!(restart_words(&names(&["gaming"])).contains("asks gaming, which is running"));
        assert!(restart_words(&names(&["gaming", "media"])).contains("gaming and media, which are running"));
        assert!(restart_words(&[]).contains("Nothing is closed without asking."));
        assert!(!ended_words(None).1);
        assert_eq!(ended_words(Some("No room.")), ("Hyper-V wasn't turned on. No room.".to_owned(), true));
        assert_eq!(listed(&names(&["a", "b", "c"])), "a, b and c");
    }
}
