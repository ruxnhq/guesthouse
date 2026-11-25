use super::windows_tests::{Rig, refused, rig, rig_with};
use super::*;
use crate::app_windows::launcher::{self, Program};
use crate::app_windows::{Launch, answer_line};
use crate::testing::Scratch;
use std::fs;
use std::path::Path;

/// A small PNG with no picture in it (see `small_png`), in Base64, as an icon of a program is sent.
const ICON: &str = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAYAAAAAAAAAAAAAAElFTkSuQmCC";

fn applications(scratch: &Scratch) -> std::path::PathBuf {
    scratch.path("home/.local/share/applications")
}

fn files_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .map(|found| found.filter_map(|entry| Some(entry.ok()?.file_name().to_str()?.to_owned())).collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// A helper that lists these programs, as `{"id","name","target","icon","minor"}`.
fn lists(rig: &Rig, apps: &str) {
    let apps = apps.to_owned();
    rig.helper.respond_with(move |request, _| {
        let n = request["n"].as_u64().unwrap_or(0);
        if request["t"] == "apps" {
            let icons = request["icons"].as_bool().unwrap_or(false);
            let apps = apps.replace("ICON", if icons { ICON } else { "" }).replace('\n', "");
            vec![format!("{{\"t\":\"answer\",\"re\":{n},\"ok\":true,\"apps\":{apps}}}")]
        } else {
            vec![answer_line(n, true, "")]
        }
    });
}

const PAINT: &str = r#"[{"id":"Accessories/Paint","name":"Paint","target":"C:\\Windows\\System32\\mspaint.exe","icon":"ICON","minor":false},
{"id":"Accessories/Paint Help","name":"Paint Help","target":"C:\\x\\help.exe","icon":"ICON","minor":true}]"#;

fn binary(rig: &Rig) -> String {
    rig.context.paths.binary("guesthouse").display().to_string()
}

#[test]
fn apps_lists_the_start_menu_a_line_each() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    lists(&rig, PAINT);
    let (out, result) = said(&rig.context, "apps work");
    assert_eq!(result, Ok(()));
    assert_eq!(
        out,
        "Accessories/Paint      Paint\n\
         Accessories/Paint Help Paint Help  (uninstaller or help)\n\
         launcher work add takes an id from this list, or a program's path or name.\n"
    );
    assert_eq!(rig.helper.requests()[0]["icons"], false, "no icons are fetched to be listed");
    rig.helper.respond_with(|request, _| {
        let n = request["n"].as_u64().unwrap();
        vec![answer_line(n, false, "the Start menu is locked")]
    });
    assert_eq!(
        refused(&rig.context, "apps work"),
        "Office PC could not list its programs: the Start menu is locked."
    );
}

#[test]
fn a_start_menu_program_becomes_an_entry_with_its_icon_and_the_exact_text() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    lists(&rig, PAINT);
    let (out, result) = said(&rig.context, "launcher work add paint");
    assert_eq!(result, Ok(()));
    let file = applications(&scratch).join("guesthouse-work-mspaint-130b95.desktop");
    assert_eq!(out, format!("Added Paint (Office PC) to the launcher: {}.\n", file.display()));
    let icon = scratch.path("home/.local/share/icons/hicolor/48x48/apps/guesthouse-work-mspaint-130b95.png");
    assert_eq!(fs::read(&icon).unwrap(), small_png());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Paint (Office PC)\n\
             Comment=A program of Office PC, in a window of its own on this desktop.\n\
             Exec={} --app work --start-menu Accessories/Paint\nIcon={}\nTerminal=false\n\
             Categories=Utility;\nStartupNotify=false\nStartupWMClass=guesthouse-work-mspaint-130b95\n\
             X-Guesthouse-Machine=work\nX-Guesthouse-Program=Accessories/Paint\nX-Guesthouse-Target=C:\\\\Windows\\\\System32\\\\mspaint.exe\nX-Guesthouse-Start-Menu=true\n",
            binary(&rig),
            icon.display()
        )
    );
    let (out, _) = said(&rig.context, "launcher work list");
    assert_eq!(
        out,
        "guesthouse-work-mspaint-130b95 Paint (Office PC) Start menu program Accessories/Paint\n"
    );
}

#[test]
fn a_program_that_is_in_no_start_menu_is_added_by_its_path_or_name() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    lists(&rig, PAINT);
    let (out, result) = said(&rig.context, r"launcher work add C:\Windows\notepad.exe");
    assert_eq!(result, Ok(()));
    assert!(
        out.ends_with(".desktop.  It is not a Start menu program, so it has no icon.\n")
            || out.contains("so it has no icon."),
        "{out}"
    );
    let text =
        fs::read_to_string(applications(&scratch).join("guesthouse-work-notepad-4e600e.desktop")).unwrap();
    assert!(text.contains("Name=Notepad (Office PC)\n"), "{text}");
    // A backslash in a path is four in the file: the Exec rules and the file's own each double it.
    assert!(
        text.contains(&format!(
            "Exec={} --app work \"C:\\\\\\\\Windows\\\\\\\\notepad.exe\"\n",
            binary(&rig)
        )),
        "{text}"
    );
    assert!(!text.contains("Icon="), "{text}");
    assert!(
        text.contains("StartupWMClass=guesthouse-work-notepad-4e600e\n"),
        "the file name and the class agree"
    );
    // A name with no path is taken the same way.
    let (_, result) = said(&rig.context, "launcher work add calc.exe");
    assert_eq!(result, Ok(()));
    assert_eq!(
        files_in(&applications(&scratch)),
        ["guesthouse-work-calc.desktop", "guesthouse-work-notepad-4e600e.desktop"]
    );
}

#[test]
fn a_machine_that_is_off_takes_a_program_as_a_path_and_says_so() {
    let scratch = Scratch::new();
    let rig = rig_with(&scratch, Kind::Windows, true, &[], true, false);
    let (out, result) = said(&rig.context, "launcher work add Paint");
    assert_eq!(result, Ok(()));
    assert!(
        out.starts_with(
            "Office PC is not running, so Paint is taken as a program's path or name, with no icon."
        ),
        "{out}"
    );
    assert_eq!(rig.helper.connections(), 0, "nothing was asked of a machine that is off");
    assert_eq!(files_in(&applications(&scratch)), ["guesthouse-work-paint.desktop"]);
}

#[test]
fn a_name_that_fits_two_programs_asks_for_an_id() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    lists(
        &rig,
        r#"[{"id":"A/Tool","name":"Tool","target":"a.exe","icon":"","minor":false},
            {"id":"B/Tool","name":"Tool","target":"b.exe","icon":"","minor":false}]"#,
    );
    assert_eq!(
        refused(&rig.context, "launcher work add tool"),
        "tool fits more than one program: A/Tool, B/Tool. Use an id."
    );
    assert!(files_in(&applications(&scratch)).is_empty());
}

#[test]
fn what_the_guest_says_cannot_break_out_of_the_entry_or_the_folders() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    lists(
        &rig,
        r#"[{"id":"../../evil","name":"Evil\nExec=rm -rf ~\u0007","target":"..\\..\\..\\x.exe","icon":"ICON","minor":false}]"#,
    );
    let (_, result) = said(&rig.context, "launcher work add ../../evil");
    assert_eq!(result, Ok(()));
    let data = scratch.path("home/.local/share");
    let written = files_in(&applications(&scratch));
    assert_eq!(
        written,
        ["guesthouse-work-x-6126cf.desktop"],
        "the file name is made here, from letters and dashes"
    );
    let text = fs::read_to_string(applications(&scratch).join(&written[0])).unwrap();
    assert_eq!(text.lines().filter(|line| line.starts_with("Exec=")).count(), 1, "{text}");
    assert!(
        text.contains("Name=EvilExec=rm -rf ~ (Office PC)\n"),
        "newline and bell dropped, so no second key: {text}"
    );
    // Nothing but the applications and icons folders was made in the data folder.
    assert_eq!(files_in(&data), ["applications", "guesthouse", "icons"], "guesthouse is the machine store");
    assert_eq!(files_in(&scratch.path("home")), [".local"]);
}

#[test]
fn an_entry_goes_by_its_name_program_or_id_and_a_key_that_fits_none_says_so() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    lists(&rig, PAINT);
    assert_eq!(said(&rig.context, "launcher work add Accessories/Paint").1, Ok(()));
    assert_eq!(said(&rig.context, "launcher work add notepad.exe").1, Ok(()));
    let (out, result) = said(&rig.context, "launcher work remove PAINT");
    assert_eq!((out.as_str(), result), ("Removed Paint (Office PC) from the launcher.\n", Ok(())));
    assert_eq!(files_in(&applications(&scratch)), ["guesthouse-work-notepad.desktop"]);
    assert!(
        !scratch
            .path("home/.local/share/icons/hicolor/48x48/apps/guesthouse-work-mspaint-130b95.png")
            .exists(),
        "its icon goes too"
    );
    assert_eq!(
        refused(&rig.context, "launcher work remove paint"),
        "Office PC has no launcher entry for paint."
    );
    assert_eq!(said(&rig.context, "launcher work remove guesthouse-work-notepad").1, Ok(()));
    assert_eq!(said(&rig.context, "launcher work list").0, "Office PC has no launcher entries.\n");
}

#[test]
fn only_a_windows_machine_with_app_windows_on_is_given_entries() {
    let scratch = Scratch::new();
    let off = rig(&scratch, Kind::Windows, false, &[]);
    assert!(
        refused(&off.context, "launcher work add notepad.exe")
            .starts_with("App windows is off for Office PC.")
    );
    let other = Scratch::new();
    let plain = rig(&other, Kind::Plain, true, &[]);
    assert_eq!(
        refused(&plain.context, "launcher work add x"),
        "Office PC is not a Windows machine, and only Windows has a helper for this."
    );
    assert!(files_in(&applications(&scratch)).is_empty() && files_in(&applications(&other)).is_empty());
}

/// The smallest thing the launcher takes as an icon: a PNG's frame of 16 by 16, with no picture in it.
fn small_png() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    bytes.extend(b"IHDR");
    bytes.extend([0, 0, 0, 16, 0, 0, 0, 16, 8, 6, 0, 0, 0, 0, 0, 0, 0]);
    bytes.extend([0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]);
    bytes
}

fn program(name: &str, exe: &str) -> Program {
    Program {
        name: name.into(),
        exe: exe.into(),
        launch: Launch::Path { path: exe.into(), args: String::new() },
        icon: Some(small_png()),
    }
}

#[test]
fn a_window_with_no_entry_gets_a_hidden_one_and_a_chosen_entry_is_left_alone() {
    let scratch = Scratch::new();
    let rig = rig_with(&scratch, Kind::Windows, true, &[], true, false);
    let (context, machine) = (&rig.context, &rig.machine);
    let id = launcher::ensure_for_window(context, machine, &program("Calculator", "Calculator.exe")).unwrap();
    assert_eq!(id, "guesthouse-work-calculator");
    let file = applications(&scratch).join("guesthouse-work-calculator.desktop");
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains("NoDisplay=true\n") && text.contains("Icon="), "{text}");
    assert_eq!(
        said(context, "launcher work list").0,
        "Office PC has no launcher entries.\n",
        "hidden is not listed"
    );
    // The user's own entry for it replaces the hidden one, and a window after it changes nothing.
    launcher::add(context, machine, &program("Calculator", "Calculator.exe")).unwrap();
    let chosen = fs::read_to_string(&file).unwrap();
    assert!(!chosen.contains("NoDisplay"), "{chosen}");
    launcher::ensure_for_window(context, machine, &program("Other", "Calculator.exe")).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), chosen);
}

#[test]
fn a_hidden_entry_with_no_icon_is_written_again_once_there_is_one() {
    let scratch = Scratch::new();
    let rig = rig_with(&scratch, Kind::Windows, true, &[], true, false);
    let (context, machine) = (&rig.context, &rig.machine);
    let bare = Program { icon: None, ..program("Notes", "notes.exe") };
    launcher::ensure_for_window(context, machine, &bare).unwrap();
    let file = applications(&scratch).join("guesthouse-work-notes.desktop");
    assert!(!fs::read_to_string(&file).unwrap().contains("Icon="));
    launcher::ensure_for_window(context, machine, &program("Notes", "notes.exe")).unwrap();
    assert!(fs::read_to_string(&file).unwrap().contains("Icon="));
}

#[test]
fn removing_a_machine_removes_its_entries_and_no_other_machines() {
    let scratch = Scratch::new();
    let rig = rig_with(&scratch, Kind::Windows, true, &[], true, false);
    let (context, machine) = (&rig.context, &rig.machine);
    let sibling = make(context, "work-2", "Second", Kind::Windows);
    launcher::add(context, machine, &program("Notepad", "notepad.exe")).unwrap();
    launcher::ensure_for_window(context, machine, &program("Calculator", "calc.exe")).unwrap();
    launcher::add(context, &sibling, &program("Notepad", "notepad.exe")).unwrap();
    assert_eq!(
        files_in(&applications(&scratch)),
        [
            "guesthouse-work-2-notepad.desktop",
            "guesthouse-work-calc.desktop",
            "guesthouse-work-notepad.desktop"
        ]
    );
    let (out, result) = said(context, "remove work --yes");
    assert_eq!(result, Ok(()), "{out}");
    assert_eq!(files_in(&applications(&scratch)), ["guesthouse-work-2-notepad.desktop"]);
    assert_eq!(
        files_in(&scratch.path("home/.local/share/icons/hicolor/48x48/apps")),
        ["guesthouse-work-2-notepad.png"]
    );
}

#[test]
fn an_icon_that_is_not_a_small_png_is_not_written() {
    let scratch = Scratch::new();
    let rig = rig_with(&scratch, Kind::Windows, true, &[], true, false);
    let mut bad = program("Paint", "mspaint.exe");
    bad.icon = Some(b"<svg>not a picture</svg>".to_vec());
    crate::app_windows::launcher::add(&rig.context, &rig.machine, &bad).unwrap();
    assert!(
        !scratch.path("home/.local/share/icons/hicolor/48x48/apps").exists()
            || files_in(&scratch.path("home/.local/share/icons/hicolor/48x48/apps")).is_empty(),
        "no icon file came of it"
    );
}
