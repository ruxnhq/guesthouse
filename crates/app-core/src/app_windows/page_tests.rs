use super::launcher::Listed;
use super::message::{App, Caption, Cell, Point, Rect, State as WindowState};
use super::page::*;
use crate::machine::Machine;
use crate::tasks::State;
use std::path::PathBuf;

fn situation(on: bool, state: State, has_forward: Option<bool>) -> Situation {
    Situation { on, state, has_forward }
}

fn machine() -> Machine {
    Machine::new("work", "Office PC", "/d", "/v")
}

fn app(id: &str, name: &str, target: &str, minor: bool) -> App {
    App { id: id.into(), name: name.into(), target: target.into(), icon: String::new(), minor }
}

fn cell(id: &str, exe: &str, popup: bool) -> Cell {
    let rect = Rect { x: 0, y: 0, w: 10, h: 10 };
    Cell {
        id: id.into(),
        pid: 1,
        exe: exe.into(),
        app: String::new(),
        name: String::new(),
        cls: String::new(),
        title: String::new(),
        cell: rect,
        frame: rect,
        popup,
        parent: String::new(),
        rel: Point::default(),
        state: WindowState::Shown,
        overflow: false,
        focus: false,
        caption: Caption::default(),
        resizable: true,
    }
}

fn entry(machine: &Machine, name: &str, program: &str, start_menu: bool) -> Listed {
    Listed {
        file: PathBuf::from("/x"),
        app_id: super::launcher::app_id(machine, program),
        name: format!("{name} ({})", machine.name),
        program: program.into(),
        target: String::new(),
        start_menu,
        hidden: false,
    }
}

fn no_icons(_: &str) -> Option<Vec<u8>> {
    None
}

#[test]
fn each_state_of_the_machine_has_its_own_body() {
    use Body::*;
    assert_eq!(body(&situation(false, State::Off, None)), SwitchedOff);
    assert_eq!(body(&situation(false, State::Running, Some(true))), SwitchedOff);
    assert_eq!(body(&situation(true, State::Off, None)), MachineOff);
    for waiting in [State::Starting, State::Installing, State::Stopping] {
        assert_eq!(body(&situation(true, waiting, None)), Waiting(waiting));
    }
    assert_eq!(body(&situation(true, State::Running, Some(true))), Ask);
    assert_eq!(
        body(&situation(true, State::Running, None)),
        Ask,
        "a machine that can't be looked at is asked"
    );
    assert_eq!(body(&situation(true, State::Running, Some(false))), NeedsRestart);
}

#[test]
fn a_running_machine_that_differs_from_its_start_says_the_switch_counts_from_the_next_one() {
    let on_late = start_note(&situation(true, State::Running, Some(false))).unwrap();
    assert!(on_late.contains("started with app windows off") && on_late.contains("next start"));
    let off_late = start_note(&situation(false, State::Running, Some(true))).unwrap();
    assert!(off_late.contains("started with app windows on") && off_late.contains("next start"));
    for none in [
        situation(true, State::Running, Some(true)),
        situation(false, State::Running, Some(false)),
        situation(true, State::Running, None),
        situation(true, State::Off, Some(false)),
        situation(false, State::Starting, Some(true)),
    ] {
        assert_eq!(start_note(&none), None, "{none:?}");
    }
}

#[test]
fn a_body_with_no_list_says_why_and_one_with_a_list_says_nothing() {
    let name = "Office PC";
    assert!(placeholder(Body::SwitchedOff, name).unwrap().starts_with("App windows is off"));
    assert!(placeholder(Body::Waiting(State::Starting), name).unwrap().contains("is starting"));
    assert!(placeholder(Body::Waiting(State::Installing), name).unwrap().contains("still being installed"));
    assert!(placeholder(Body::Waiting(State::Stopping), name).unwrap().contains("shutting down"));
    for listed in [Body::MachineOff, Body::NeedsRestart, Body::Ask] {
        assert_eq!(placeholder(listed, name), None);
    }
    assert!(off_note(name).contains("The full list shows when the machine runs."));
}

#[test]
fn a_look_that_found_nothing_is_worded_and_says_whether_it_tries_again() {
    let (text, again) = look_problem("Office PC", &Look::NotYet("Nobody is logged in.".into())).unwrap();
    assert!(again && text.contains("Nobody is logged in.") && text.ends_with("asks again by itself."));
    let (text, again) = look_problem("Office PC", &Look::Failed("It timed out.".into())).unwrap();
    assert!(!again && text.starts_with("The programs of Office PC could not be listed."));
    assert_eq!(look_problem("Office PC", &Look::Listed(Found { apps: vec![], windows: vec![] })), None);
}

#[test]
fn the_second_line_counts_windows() {
    assert_eq!(open_text(Some(0), "x"), "Not running");
    assert_eq!(open_text(Some(1), "x"), "Open now, 1 window");
    assert_eq!(open_text(Some(2), "x"), "Open now, 2 windows");
    assert_eq!(open_text(None, "Starts the machine to open"), "Starts the machine to open");
    assert_eq!(launcher_count_text(3), "3 in the Linux launcher");
    assert_eq!(more_text(31), "31 more, such as uninstallers and help files");
}

#[test]
fn windows_are_matched_to_programs_by_file_and_popups_are_not_counted() {
    let windows = [
        cell("1", r"C:\Program Files\Edge\msedge.exe", false),
        cell("2", r"c:\program files\edge\MSEDGE.EXE", false),
        cell("3", r"C:\Program Files\Edge\msedge.exe", true),
        cell("4", "notepad.exe", false),
        cell("5", r"D:\Other\msedge.exe", false),
    ];
    assert_eq!(open_windows(r"C:\Program Files\Edge\msedge.exe", &windows), 2);
    assert_eq!(open_windows("notepad.exe", &windows), 1);
    assert_eq!(open_windows(r"C:\Windows\notepad.exe", &windows), 1, "a name with no folder in the window");
    assert_eq!(open_windows("Accessories/Paint", &windows), 0);
    assert_eq!(open_windows("", &windows), 0);
}

#[test]
fn rows_are_sorted_and_carry_windows_and_launcher_state() {
    let machine = machine();
    let apps = [
        app("Edge", "Microsoft Edge", r"C:\Edge\msedge.exe", false),
        app("Notepad", "notepad", "notepad.exe", false),
        app("Edge/Uninstall", "Uninstall Edge", r"C:\Edge\setup.exe", true),
    ];
    let windows = [cell("1", r"C:\Edge\msedge.exe", false), cell("2", r"C:\Edge\msedge.exe", false)];
    let entries = [entry(&machine, "notepad", "Notepad", true)];
    let rows = rows_found(&machine, &apps, Some(&windows), &entries, &no_icons);
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["Microsoft Edge", "notepad", "Uninstall Edge"]);
    assert_eq!(rows[0].open, Some(2));
    assert!(!rows[0].in_launcher);
    assert_eq!(rows[1].open, Some(0));
    assert!(rows[1].in_launcher, "an entry for the program's id");
    assert!(rows[2].minor);
}

#[test]
fn an_entry_that_is_not_in_the_list_is_a_row_of_its_own_and_counts() {
    let machine = machine();
    let apps = [app("Paint", "Paint", r"C:\Windows\mspaint.exe", false)];
    let by_path = entry(&machine, "Calc", r"C:\Windows\calc.exe", false);
    let windows = [cell("1", "calc.exe", false)];
    let rows = rows_found(&machine, &apps, Some(&windows), &[by_path], &no_icons);
    assert_eq!(rows.len(), 2);
    let calc = rows.iter().find(|row| row.name == "Calc").unwrap();
    assert!(calc.in_launcher && !calc.start_menu);
    assert_eq!(calc.program, r"C:\Windows\calc.exe");
    assert_eq!(calc.open, Some(1));
}

#[test]
fn two_shortcuts_of_one_file_share_the_windows_and_the_entry_is_one_programs() {
    let machine = machine();
    let apps = [
        app("Word", "Word", r"C:\Office\winword.exe", false),
        app("Word Safe", "Word (Safe Mode)", r"C:\Office\winword.exe", false),
    ];
    let windows = [cell("1", r"C:\Office\winword.exe", false)];
    let entries = [entry(&machine, "Word", "Word", true)];
    let rows = rows_found(&machine, &apps, Some(&windows), &entries, &no_icons);
    let word = rows.iter().find(|row| row.program == "Word").unwrap();
    let safe = rows.iter().find(|row| row.program == "Word Safe").unwrap();
    assert_eq!((word.open, safe.open), (Some(1), Some(1)), "one file has one count");
    assert!(word.app_id == safe.app_id);
}

#[test]
fn the_icon_is_the_helpers_and_a_launcher_entry_keeps_its_own_when_the_helper_sent_none() {
    let machine = machine();
    let mut with = app("Paint", "Paint", "paint.exe", false);
    with.icon = "iVBORw0KGgo=".into();
    let without = app("Calc", "Calc", "calc.exe", false);
    let entries = [entry(&machine, "Calc", "Calc", true)];
    let kept = |_: &str| Some(vec![1, 2, 3]);
    let rows = rows_found(&machine, &[with, without], None, &entries, &kept);
    let paint = rows.iter().find(|row| row.name == "Paint").unwrap();
    let calc = rows.iter().find(|row| row.name == "Calc").unwrap();
    assert_eq!(paint.icon.as_deref(), Some(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a][..]));
    assert_eq!(calc.icon, Some(vec![1, 2, 3]));
    assert_eq!(paint.open, None, "no windows known");
}

#[test]
fn a_machine_that_is_off_shows_its_entries_with_the_machines_name_taken_off() {
    let machine = machine();
    let entries = [entry(&machine, "Zip", r"C:\z.exe", false), entry(&machine, "Edge", "Edge", true)];
    let rows = rows_launcher(&machine, &entries, &no_icons);
    assert_eq!(rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(), ["Edge", "Zip"]);
    assert!(rows.iter().all(|row| row.in_launcher && row.open.is_none()));
    assert!(rows[0].start_menu && !rows[1].start_menu);
}

#[test]
fn minor_programs_are_left_out_and_counted_unless_in_use_or_in_the_launcher() {
    let machine = machine();
    let apps: Vec<App> =
        (0..4).map(|n| app(&format!("m{n}"), &format!("Minor {n}"), &format!("m{n}.exe"), true)).collect();
    let mut all = apps.clone();
    all.push(app("main", "Main", "main.exe", false));
    let windows = [cell("1", "m1.exe", false)];
    let entries = [entry(&machine, "Minor 2", "m2", true)];
    let rows = rows_found(&machine, &all, Some(&windows), &entries, &no_icons);
    let (kept, left_out) = shown(&rows, false);
    let names: Vec<&str> = kept.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["Main", "Minor 1", "Minor 2"], "Minor 1 is open, Minor 2 is in the launcher");
    assert_eq!(left_out, 2);
    let (everything, left_out) = shown(&rows, true);
    assert_eq!((everything.len(), left_out), (5, 0));
}

#[test]
fn a_list_of_hundreds_is_sorted_and_cheap_to_build() {
    let machine = machine();
    let apps: Vec<App> = (0..800)
        .rev()
        .map(|n| app(&format!("a{n}"), &format!("Program {n:04}"), &format!("p{n}.exe"), n % 3 == 0))
        .collect();
    let rows = rows_found(&machine, &apps, Some(&[]), &[], &no_icons);
    assert_eq!(rows.len(), 800);
    assert_eq!(rows[0].name, "Program 0000");
    assert!(rows.windows(2).all(|pair| pair[0].name <= pair[1].name));
}

#[test]
fn a_start_menu_listing_becomes_the_program_a_launcher_entry_is_made_from() {
    let listing = app("Accessories/Paint", "Paint", r"C:\Windows\mspaint.exe", false);
    let program = program_of(&listing, Some(vec![1]));
    assert_eq!(program.exe, r"C:\Windows\mspaint.exe");
    assert_eq!(program.launch, super::message::Launch::App("Accessories/Paint".into()));
    let bare = program_of(&app("Tool", "Tool", "", false), None);
    assert_eq!(bare.exe, "Tool", "with no target the id stands for the file");
}

#[test]
fn the_labels_carry_the_programs_name() {
    assert_eq!(open_label("Notepad"), "Open Notepad");
    assert_eq!(launcher_label("Notepad"), "Notepad in the launcher");
}

#[test]
fn a_start_menu_entry_is_matched_to_its_windows_by_the_slug_of_its_file() {
    let machine = machine();
    let mut paint = entry(&machine, "Paint", r"C:\Windows\mspaint.exe", true);
    paint.program = "Accessories/Paint".into();
    paint.target = r"C:\Windows\mspaint.exe".into();
    let windows = [cell("1", r"C:\Windows\mspaint.exe", false)];
    let rows = rows_launcher(&machine, std::slice::from_ref(&paint), &no_icons);
    assert_eq!(rows[0].open, None);
    let row = entry_row(&machine, &paint, Some(&windows), &no_icons);
    assert_eq!((row.open, row.target.as_str()), (Some(1), r"C:\Windows\mspaint.exe"));
    let program = row.to_program();
    assert_eq!(program.launch, super::message::Launch::App("Accessories/Paint".into()));
    assert_eq!(
        super::launcher::app_id(&machine, &program.exe),
        paint.app_id,
        "it makes the same entry again"
    );
}

#[test]
fn a_window_that_says_which_program_it_is_of_is_matched_by_that_and_not_by_its_host_process() {
    let frame_host = |app: &str| Cell { app: app.into(), ..cell("1", "ApplicationFrameHost.exe", false) };
    let windows = [
        frame_host("aumid:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
        frame_host("aumid:Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"),
        Cell { app: r"C:\Users\me\custom\winver.exe".into(), ..cell("3", "winver.exe", false) },
    ];
    assert_eq!(open_windows("aumid:microsoft.windowscalculator_8wekyb3d8bbwe!app", &windows), 1);
    assert_eq!(
        open_windows(r"C:\Windows\System32\winver.exe", &windows),
        0,
        "another folder, another program"
    );
    assert_eq!(open_windows(r"c:\users\me\custom\winver.exe", &windows), 1);
    assert_eq!(open_windows("ApplicationFrameHost.exe", &windows), 0, "a Store app is not the host");
}
