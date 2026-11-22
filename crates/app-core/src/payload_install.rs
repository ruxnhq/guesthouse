//! Installing the release an AppImage carries. install.sh from inside it does the work, with
//! `--payload`, so this only asks it two things: what it would do on this PC, from its dry run, and
//! then to do it. The app's installer window shows the first and runs the second.

use crate::failure::{Failure, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const INSTALL_SCRIPT: &str = "install.sh";
const PAYLOAD_OPTION: &str = "--payload=";
const DRY_RUN_OPTION: &str = "--dry-run";
/// The app has asked already, so the script doesn't, and pkexec asks for root's password in a window.
const FROM_APP_OPTION: &str = "--from-app=";
/// Where the script says what went wrong: `install.sh: ` and the reason.
const SCRIPT_PREFIX: &str = "install.sh: ";
/// Where every install puts the program, which the launcher and the app's updates go through.
const INSTALLED_PROGRAM: &str = ".local/lib/guesthouse/current";

const PC_HEADING: &str = "Checking this PC.";
const PLAN_HEADING: &str = "The plan: ";
const ROOT_HEADING: &str = "This is everything that runs as root:";
const NO_ROOT: &str = "Nothing here needs root.";
const DRY_RUN_END: &str = "That was a dry run";
/// A fact's own lines are indented by two, and a hint under it by four.
const FACT_INDENT: &str = "  ";
const HINT_INDENT: &str = "    ";

/// One thing the script found out about this PC, as `guesthouse-check` said it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fact {
    pub label: String,
    pub value: String,
    /// What to do about it, when it isn't fine.
    pub hint: String,
}

/// What installing would do here, as the script's dry run said it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// `install Guesthouse 0.7.5`, `update Guesthouse from 0.7.4 to 0.7.5` and the like.
    pub headline: String,
    pub pc: Vec<Fact>,
    /// What goes where: `Program` and its folder, and so on.
    pub places: Vec<Fact>,
    /// The one command that runs as root, if any.
    pub root_command: Option<String>,
    /// Why root is needed, and anything else the script says about it, such as packages an immutable
    /// system has to layer by itself.
    pub root_notes: Vec<String>,
}

impl Plan {
    /// The headline as a heading: `Install Guesthouse 0.7.5`.
    pub fn title(&self) -> String {
        let mut letters = self.headline.chars();
        match letters.next() {
            Some(first) => first.to_uppercase().chain(letters).collect(),
            None => String::new(),
        }
    }
}

/// `Label: value`, as the script prints a fact.
fn fact(line: &str) -> Fact {
    match line.trim().split_once(": ") {
        Some((label, value)) => {
            Fact { label: label.to_owned(), value: value.to_owned(), hint: String::new() }
        }
        None => Fact { label: line.trim().to_owned(), ..Fact::default() },
    }
}

/// A fact per line, and a hint joined to the fact over it.
fn facts<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<Fact> {
    let mut found: Vec<Fact> = Vec::new();
    for line in lines {
        match found.last_mut() {
            Some(last) if line.starts_with(HINT_INDENT) => {
                if !last.hint.is_empty() {
                    last.hint.push('\n');
                }
                last.hint.push_str(line.trim());
            }
            _ if line.starts_with(FACT_INDENT) => found.push(fact(line)),
            _ => {}
        }
    }
    found
}

/// Root's part: the command on the line under its heading, and every other line a note.
fn read_root_part(part: &str, plan: &mut Plan) {
    let mut command_next = false;
    for line in part.lines() {
        if line == ROOT_HEADING {
            command_next = true;
        } else if command_next {
            plan.root_command = Some(line.trim().to_owned());
            command_next = false;
        } else if line != NO_ROOT {
            plan.root_notes.push(line.trim().to_owned());
        }
    }
}

/// Reads the dry run. Its parts are apart by blank lines: this PC, the plan, and root's part.
pub fn read_plan(said: &str) -> Result<Plan> {
    let before_end = said.split(DRY_RUN_END).next().unwrap_or_default();
    let mut plan = Plan::default();
    let mut planned = false;
    for part in before_end.split("\n\n").map(|part| part.trim_matches('\n')) {
        let mut lines = part.lines();
        let Some(first) = lines.next() else { continue };
        if first.starts_with(PC_HEADING) {
            plan.pc = facts(lines);
        } else if let Some(headline) = first.strip_prefix(PLAN_HEADING) {
            plan.headline = headline.trim_end_matches('.').to_owned();
            plan.places = facts(lines);
            planned = true;
        } else if planned {
            read_root_part(part, &mut plan);
        }
    }
    if planned { Ok(plan) } else { Err(Failure::new(format!("install.sh said no plan:\n{said}"))) }
}

/// The script's own reason for stopping, or its last words if it gave none.
fn reason(said: &str) -> String {
    let last = said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
    said.lines().rev().find_map(|line| line.strip_prefix(SCRIPT_PREFIX)).unwrap_or(last).to_owned()
}

fn run_script(payload: &Path, app: u32, dry_run: bool) -> Result<Output> {
    let mut script = Command::new("sh");
    script
        .arg(payload.join(INSTALL_SCRIPT))
        .arg(format!("{PAYLOAD_OPTION}{}", payload.display()))
        .arg(format!("{FROM_APP_OPTION}{app}"))
        .stdin(Stdio::null());
    if dry_run {
        script.arg(DRY_RUN_OPTION);
    }
    Ok(script.output()?)
}

fn all_it_said(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

/// What installing the release in `payload` would do, with the app as process `app`. It checks this PC
/// and copies the release once, so it takes a few seconds and is for a thread.
pub fn plan(payload: &Path, app: u32) -> Result<Plan> {
    let output = run_script(payload, app, true)?;
    if !output.status.success() {
        return Err(Failure::new(reason(&all_it_said(&output))));
    }
    read_plan(&String::from_utf8_lossy(&output.stdout))
}

/// Installs the release in `payload`. If root is needed, pkexec asks for the password in a window.
/// Everything the script said comes back, for the log. This takes a minute, and is for a thread.
pub fn install(payload: &Path, app: u32) -> Result<String> {
    let output = run_script(payload, app, false)?;
    let said = all_it_said(&output);
    if output.status.success() { Ok(said) } else { Err(Failure::new(reason(&said))) }
}

/// The installed app's folder, which it's started from once the install is done.
pub fn installed_program(home: &Path) -> PathBuf {
    home.join(INSTALLED_PROGRAM)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_INSTALL: &str = "Checking this PC. Nothing here needs root or changes anything.
  Distro: Ubuntu 26.10 (should work, tested without a GPU)
  KVM: /dev/kvm can be opened
  Host NVIDIA driver: 615.71.09, and this was tested on 620.70.06.
    The guest's graphics depend on the exact version. Install 620.70.06 if they fail.

The plan: install Guesthouse 0.7.5.
  Program: /home/me/.local/lib/guesthouse/0.7.5, reached through /home/me/.local/lib/guesthouse/current
  Launcher: /home/me/.local/share/applications/io.github.xnixy.Guesthouse.desktop
  VMs: GPU machines and plain VMs

This is everything that runs as root:
  sudo sh -c 'dnf install -y swtpm && mkdir -p /usr/local/libexec/guesthouse'
That puts QEMU's bridge helper in /usr/local/libexec/guesthouse, setuid root, for bridged networking.

That was a dry run, so nothing was changed.
";

    #[test]
    fn a_first_install_reads_whole() {
        let plan = read_plan(FIRST_INSTALL).unwrap();
        assert_eq!(plan.title(), "Install Guesthouse 0.7.5");
        assert_eq!(plan.pc.len(), 3);
        assert_eq!(plan.pc[2].label, "Host NVIDIA driver");
        assert!(plan.pc[2].hint.starts_with("The guest's graphics"));
        assert_eq!(plan.places[0].label, "Program");
        assert_eq!(plan.places[2].value, "GPU machines and plain VMs");
        assert_eq!(
            plan.root_command.as_deref(),
            Some("sudo sh -c 'dnf install -y swtpm && mkdir -p /usr/local/libexec/guesthouse'")
        );
        assert_eq!(plan.root_notes.len(), 1);
        assert!(plan.root_notes[0].starts_with("That puts QEMU's bridge helper"));
    }

    #[test]
    fn a_plan_with_nothing_for_root_has_no_command() {
        let said = "The plan: update Guesthouse from 0.7.4 to 0.7.5.\n  Files: 3 changed\n\nNothing here needs root.\n\nThat was a dry run, so nothing was changed.\n";
        let plan = read_plan(said).unwrap();
        assert_eq!(plan.title(), "Update Guesthouse from 0.7.4 to 0.7.5");
        assert_eq!(plan.root_command, None);
        assert!(plan.root_notes.is_empty());
    }

    #[test]
    fn an_immutable_systems_packages_are_a_note_and_not_a_command() {
        let said = "The plan: install Guesthouse 0.7.5.\n  VMs: plain VMs only\n\n\
            This system is immutable, so I won't add packages to it. You'll need these, and a reboot:\n  \
            rpm-ostree install swtpm\n\nThat was a dry run, so nothing was changed.\n";
        let plan = read_plan(said).unwrap();
        assert_eq!(plan.root_command, None);
        assert_eq!(plan.root_notes.len(), 2);
        assert_eq!(plan.root_notes[1], "rpm-ostree install swtpm");
    }

    #[test]
    fn no_plan_is_a_failure() {
        assert!(read_plan("Checking this PC.\n  KVM: fine\n").is_err());
    }

    #[test]
    fn the_scripts_own_reason_wins() {
        assert_eq!(
            reason("Checking this PC.\ninstall.sh: this needs glibc 2.39.\n"),
            "this needs glibc 2.39."
        );
        assert_eq!(reason("Checking.\nsomething odd\n\n"), "something odd");
    }
}
