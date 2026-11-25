//! What a machine's Apps page shows, as plain values: which of its bodies it has for the machine's state,
//! the rows of programs with how many windows each has open, and every sentence the page says. The
//! page itself (`crates/app/src/settings/apps.rs`) only draws these, so each state can be tested here
//! without a display.
//!
//! Where the programs come from depends on the machine. Only a running machine whose helper can be
//! reached is asked, and what it lists is a snapshot: the open windows are as they were when it
//! answered. For a machine that is off, or one that cannot be asked, the programs already in the Linux
//! launcher are shown from their `.desktop` entries, which still open and start the machine.

use super::launcher::{self, Listed, Program};
use super::message::{App, Cell, Launch};
use crate::machine::Machine;
use crate::tasks::State;

pub const SWITCH_TITLE: &str = "Show apps as their own windows";
pub const SWITCH_SUBTITLE: &str =
    "A Windows program opens like a Linux one, without the Windows desktop around it.";
pub const SWITCH_INFO: &str = "With this on, the machine starts with a way for Guesthouse to reach a helper in \
                               Windows. The helper shows one program at a time, and Guesthouse puts it in a \
                               window of its own. The switch counts from the machine's next start.";
pub const HEADING_FOUND: &str = "Programs found in Windows";
pub const HEADING_LAUNCHER: &str = "Programs in the launcher";
pub const OPEN: &str = "Open";
pub const IN_LAUNCHER: &str = "In launcher";
pub const SHOW_ALL: &str = "Show all";
pub const TRY_AGAIN: &str = "Try again";
pub const FOOTNOTE: &str = "Opening an app starts this machine if it is off. The whole desktop stays one click \
                            away on the machine's card.";
pub const LOOKING: &str = "Looking for programs in Windows.";
pub const NOT_RUNNING: &str = "Not running";
/// The second line of a program whose open windows are not known.
pub const STARTS_MACHINE: &str = "Starts the machine to open";
pub const AFTER_RESTART: &str = "Opens after the machine's next start";
/// Why Open waits, on a running machine that cannot be reached yet.
pub const OPEN_WAITS: &str = "The machine has to start again before apps can open as windows.";
pub const SWITCH_OFF_BODY: &str = "App windows is off for this machine. Turn it on above, and the programs of \
                                   Windows are listed here.";

/// What the machine is doing, as far as this page cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Situation {
    pub on: bool,
    pub state: State,
    /// Whether the running machine has the way in to its helper: `None` when it can't be looked at.
    pub has_forward: Option<bool>,
}

/// What the page shows under its switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// The switch is off: no list, and why.
    SwitchedOff,
    /// The machine is off: the launcher's programs, and that the full list shows when it runs.
    MachineOff,
    /// The machine is coming up, being installed or going down: nothing can be asked yet.
    Waiting(State),
    /// The machine runs and has no way in to its helper until it starts again. The launcher's programs
    /// are shown, and cannot be opened yet.
    NeedsRestart,
    /// The helper is asked for the full list.
    Ask,
}

pub fn body(situation: &Situation) -> Body {
    if !situation.on {
        return Body::SwitchedOff;
    }
    match situation.state {
        State::Off => Body::MachineOff,
        State::Starting | State::Installing | State::Stopping => Body::Waiting(situation.state),
        State::Running if situation.has_forward == Some(false) => Body::NeedsRestart,
        State::Running => Body::Ask,
    }
}

/// The sentence for a running machine whose state differs from what it was started with. The switch
/// counts from the next start, and the page says so rather than letting it look broken.
pub fn start_note(situation: &Situation) -> Option<&'static str> {
    if situation.state != State::Running {
        return None;
    }
    match (situation.on, situation.has_forward) {
        (true, Some(false)) => Some(
            "This machine started with app windows off. The switch counts from its next start, \
             and apps cannot open as windows until then.",
        ),
        (false, Some(true)) => Some(
            "This machine started with app windows on. The switch counts from its next start, \
             and until then its helper stays reachable.",
        ),
        _ => None,
    }
}

/// What the page says in place of a list, for a body that has none. `None` for a body that has a list.
pub fn placeholder(body: Body, machine_name: &str) -> Option<String> {
    Some(match body {
        Body::SwitchedOff => SWITCH_OFF_BODY.to_owned(),
        Body::Waiting(State::Starting) => {
            format!("{machine_name} is starting. Its programs are listed here once it is up.")
        }
        Body::Waiting(State::Installing) => {
            format!(
                "Windows is still being installed on {machine_name}. Its programs are listed here after that."
            )
        }
        Body::Waiting(_) => {
            format!("{machine_name} is shutting down. Its programs are listed here when it runs.")
        }
        Body::MachineOff | Body::NeedsRestart | Body::Ask => return None,
    })
}

/// The note over the launcher's programs when the machine is off.
pub fn off_note(machine_name: &str) -> String {
    format!(
        "{machine_name} is off, so Windows cannot list its programs. These are the ones already in the \
         launcher, and each starts the machine when opened. The full list shows when the machine runs."
    )
}

/// What the page says when the machine is off and the launcher has none.
pub fn off_and_none(machine_name: &str) -> String {
    format!(
        "{machine_name} is off, and no program of it is in the launcher yet. Start it, and its programs are \
         listed here."
    )
}

/// What a look at the machine's helper came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Look {
    Listed(Found),
    /// The helper is not up yet, as a guest that has only just started has no session for it. Asking
    /// again a little later can work. The text says why.
    NotYet(String),
    /// Asking failed in a way that waiting does not explain. The text says why.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub apps: Vec<App>,
    /// The windows open in the guest when it answered, popups left out.
    pub windows: Vec<Cell>,
}

/// What a look leaves the page saying, with whether it asks again by itself.
pub fn look_problem(machine_name: &str, look: &Look) -> Option<(String, bool)> {
    match look {
        Look::Listed(_) => None,
        Look::NotYet(why) => Some((
            format!(
                "{machine_name} is not ready to list its programs yet. {why} Guesthouse asks again by itself."
            ),
            true,
        )),
        Look::Failed(why) => {
            Some((format!("The programs of {machine_name} could not be listed. {why}"), false))
        }
    }
}

/// What the page says for a helper that answered with no programs.
pub fn nothing_found(machine_name: &str) -> String {
    format!("{machine_name} has no programs in its Start menu.")
}

/// A row of the list. `open` is `None` when the machine's windows are not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// What opens it: a Start menu id or a path.
    pub program: String,
    pub start_menu: bool,
    /// What the helper's windows of this program are matched by: the program's file, or for an entry of
    /// the launcher whose file is not known, the word its app id ends in.
    pub target: String,
    /// The launcher's app id for it, which is also what its windows get.
    pub app_id: String,
    /// The PNG of its icon.
    pub icon: Option<Vec<u8>>,
    pub open: Option<usize>,
    pub in_launcher: bool,
    pub minor: bool,
}

/// The line under a program's name.
pub fn open_text(open: Option<usize>, otherwise: &str) -> String {
    match open {
        None => otherwise.to_owned(),
        Some(0) => NOT_RUNNING.to_owned(),
        Some(1) => "Open now, 1 window".to_owned(),
        Some(count) => format!("Open now, {count} windows"),
    }
}

/// The count at the right of the heading.
pub fn launcher_count_text(count: usize) -> String {
    format!("{count} in the Linux launcher")
}

pub fn more_text(count: usize) -> String {
    format!("{count} more, such as uninstallers and help files")
}

/// The name Open and the switch carry for a screen reader.
pub fn open_label(name: &str) -> String {
    format!("Open {name}")
}

pub fn launcher_label(name: &str) -> String {
    format!("{name} in the launcher")
}

/// `C:\Program Files\Edge\msedge.exe` as one comparable word.
fn normal(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// Whether a window's program is this one. Two paths are the same file when they read the same; a name
/// with no folder, which is also all the launcher's entry and the viewer go by, is the same program as
/// any file of that name.
pub fn same_program(target: &str, window_exe: &str) -> bool {
    if target.is_empty() || window_exe.is_empty() {
        return false;
    }
    let has_folder = |text: &str| text.contains(['\\', '/']);
    if has_folder(target) && has_folder(window_exe) {
        return normal(target) == normal(window_exe);
    }
    launcher::slug_of(target) == launcher::slug_of(window_exe)
}

/// How many windows of the guest belong to the program. A popup is part of its window.
pub fn open_windows(target: &str, windows: &[Cell]) -> usize {
    windows.iter().filter(|cell| !cell.popup && same_program(target, &cell.exe)).count()
}

/// What an entry in the launcher is, back as a program's id or path.
fn entry_names(entry: &Listed, machine: &Machine) -> String {
    let suffix = format!(" ({})", machine.name);
    entry.name.strip_suffix(&suffix).unwrap_or(&entry.name).to_owned()
}

/// The program a Start menu listing stands for, as `launcher add` takes it.
pub fn program_of(app: &App, icon: Option<Vec<u8>>) -> Program {
    let target = if app.target.is_empty() { &app.id } else { &app.target };
    Program { name: app.name.clone(), exe: target.clone(), launch: Launch::App(app.id.clone()), icon }
}

/// The rows of the list a running machine gave, sorted by name. A program that has a launcher entry is
/// marked, and an entry of a program that is not in the list is a row of its own, so the count of the
/// launcher and what the list shows agree. `windows` is what the guest has open, if it is known.
pub fn rows_found(
    machine: &Machine,
    apps: &[App],
    windows: Option<&[Cell]>,
    entries: &[Listed],
    icon_of: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Vec<Row> {
    let mut rows: Vec<Row> = apps
        .iter()
        .map(|app| {
            let target = if app.target.is_empty() { &app.id } else { &app.target };
            let app_id = launcher::app_id(machine, target);
            // Two Start menu programs of one file share an entry, and the entry is one program's.
            let in_launcher = entries
                .iter()
                .any(|entry| entry.app_id == app_id && entry.start_menu && entry.program == app.id);
            Row {
                name: app.name.clone(),
                program: app.id.clone(),
                start_menu: true,
                target: target.clone(),
                icon: crate::text::base64_decode(&app.icon)
                    .filter(|png| !png.is_empty())
                    .or_else(|| in_launcher.then(|| icon_of(&app_id)).flatten()),
                app_id,
                open: windows.map(|windows| open_windows(target, windows)),
                in_launcher,
                minor: app.minor,
            }
        })
        .collect();
    for entry in entries {
        if rows.iter().any(|row| row.in_launcher && row.app_id == entry.app_id) {
            continue;
        }
        rows.push(entry_row(machine, entry, windows, icon_of));
    }
    sort(&mut rows);
    rows
}

/// What an entry's windows are matched by: its path, or for a Start menu program, the word its app id
/// ends in, which is the slug of the program's file.
fn entry_target(machine: &Machine, entry: &Listed) -> String {
    if !entry.start_menu {
        return entry.program.clone();
    }
    launcher::slug_in_app_id(machine, &entry.app_id).unwrap_or(&entry.app_id).to_owned()
}

/// A row for a launcher entry.
pub fn entry_row(
    machine: &Machine,
    entry: &Listed,
    windows: Option<&[Cell]>,
    icon_of: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Row {
    let target = entry_target(machine, entry);
    Row {
        name: entry_names(entry, machine),
        program: entry.program.clone(),
        start_menu: entry.start_menu,
        app_id: entry.app_id.clone(),
        icon: icon_of(&entry.app_id),
        open: windows.map(|windows| open_windows(&target, windows)),
        target,
        in_launcher: true,
        minor: false,
    }
}

impl Row {
    /// What the launcher makes its entry from.
    pub fn to_program(&self) -> Program {
        let launch = if self.start_menu {
            Launch::App(self.program.clone())
        } else {
            Launch::Path { path: self.program.clone(), args: String::new() }
        };
        Program { name: self.name.clone(), exe: self.target.clone(), launch, icon: self.icon.clone() }
    }
}

/// The launcher's programs alone, for a machine that cannot be asked.
pub fn rows_launcher(
    machine: &Machine,
    entries: &[Listed],
    icon_of: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Vec<Row> {
    let mut rows: Vec<Row> = entries.iter().map(|entry| entry_row(machine, entry, None, icon_of)).collect();
    sort(&mut rows);
    rows
}

fn sort(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.app_id.cmp(&b.app_id))
    });
}

/// The rows to show, and how many were left out as minor. A minor program that is in the launcher, or
/// is open, is shown all the same, as it is something the user did or is using.
pub fn shown(rows: &[Row], all: bool) -> (Vec<&Row>, usize) {
    let mut left_out = 0;
    let kept = rows
        .iter()
        .filter(|row| {
            let keep = all || !row.minor || row.in_launcher || row.open.is_some_and(|open| open > 0);
            if !keep {
                left_out += 1;
            }
            keep
        })
        .collect();
    (kept, left_out)
}
