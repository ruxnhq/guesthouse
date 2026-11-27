use super::*;
use crate::agent::make_token;
use crate::agent::stand_in::ScriptedAgent;
use crate::app_windows::stand_in::FakeHelper;
use crate::app_windows::{
    Caption, Cell, Layout, LinkSettings, Point, Rect, Size, State, Timing, answer_line, forward_word,
    host_port, layout_line, set_on,
};
use std::sync::Arc;
use std::time::Duration;

pub(super) fn refused(context: &Context, words: &str) -> String {
    match said(context, words).1 {
        Err(Problem::Failed(failure)) => failure.0,
        other => panic!("{words:?} gave {other:?}"),
    }
}

fn quick(port: u16) -> LinkSettings {
    LinkSettings {
        port: Some(port),
        timing: Timing {
            connect: Duration::from_millis(500),
            hello: Duration::from_millis(300),
            write: Duration::from_millis(500),
            reconnect_first: Duration::from_millis(20),
            reconnect_most: Duration::from_millis(100),
            start_cooldown: Duration::from_millis(300),
            request: Duration::from_millis(500),
            apps_request: Duration::from_millis(800),
        },
        patience: Duration::from_secs(2),
    }
}

fn spare_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

fn cell(id: &str, title: &str, popup_of: Option<&str>) -> Cell {
    Cell {
        id: id.into(),
        pid: 10,
        exe: "notepad.exe".into(),
        app: String::new(),
        name: String::new(),
        cls: "Notepad".into(),
        title: title.into(),
        cell: rect(10, 20, 100, 200),
        frame: rect(10, 20, 100, 200),
        popup: popup_of.is_some(),
        parent: popup_of.unwrap_or_default().into(),
        owner: String::new(),
        rel: Point { x: 3, y: 4 },
        state: State::Shown,
        overflow: false,
        focus: popup_of.is_none(),
        caption: Caption { h: 32, buttons_w: 138 },
        resizable: true,
        corner: 0,
    }
}

pub(super) struct Rig {
    pub context: Context,
    pub agent: Arc<ScriptedAgent>,
    pub helper: FakeHelper,
    pub machine: Machine,
}

/// A Windows machine that runs as pid 40, whose helper is a pretend one on a port of its own, and whose
/// agent answers from a list. With `on` false the switch was never set.
pub(super) fn rig(scratch: &Scratch, kind: Kind, on: bool, answers: &[&str]) -> Rig {
    rig_listening(scratch, kind, on, answers, true)
}

/// With `listening` false the machine's helper port is one that nothing listens on.
pub(super) fn rig_listening(
    scratch: &Scratch,
    kind: Kind,
    on: bool,
    answers: &[&str],
    listening: bool,
) -> Rig {
    rig_with(scratch, kind, on, answers, listening, true)
}

/// With `up` false the machine is off: its process is not there until `start_process` makes it.
pub(super) fn rig_with(
    scratch: &Scratch,
    kind: Kind,
    on: bool,
    answers: &[&str],
    listening: bool,
    up: bool,
) -> Rig {
    let mut context = scratch_context(scratch);
    let agent = Arc::new(ScriptedAgent::answering(answers));
    context.agent = agent.clone();
    for name in ["app-windows.ps1", "app-windows.cs", "app-windows-start.ps1", "helper-stop.ps1"] {
        scratch.write(&format!("program/guest/{name}"), "script");
    }
    let machine = make(&context, "work", "Office PC", kind);
    make_token(&context, &machine).unwrap();
    let token = std::fs::read_to_string(machine.token_file(&context)).unwrap();
    let helper = FakeHelper::start(token.trim());
    context.app_windows = quick(if listening { helper.port } else { spare_port() });
    if on && kind == Kind::Windows {
        set_on(&context, &machine, true).unwrap();
    }
    if up {
        start_process(&context, &machine);
    }
    Rig { context, agent, helper, machine }
}

/// The machine's QEMU, as the system would show it.
pub(super) fn start_process(context: &Context, machine: &Machine) {
    let qemu = context.paths.qemu.display().to_string();
    let monitor = format!("unix:{}/qmp.sock,server=on", context.run_folder(None).display());
    let disk = format!("file={},format=qcow2", machine.disk);
    let nic = format!("user,{}", forward_word(host_port(context, None)));
    crate::processes::stand_in::fake_process(
        context,
        40,
        &[&qemu, "-qmp", &monitor, &disk, "-nic", &nic],
        &[],
        "",
    );
}

fn two_cells() -> Layout {
    let cells = vec![cell("4096", "a \"b\" ü", None), cell("8192", "", Some("4096"))];
    Layout { seq: 5, screen: Size { w: 1920, h: 1080 }, cells }
}

#[test]
fn each_cell_is_a_line_and_json_is_the_helpers_layout_as_it_came() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    rig.helper.with(|inner| inner.layout = two_cells());
    let (out, result) = said(&rig.context, "windows work");
    assert_eq!(result, Ok(()));
    assert_eq!(
        out,
        "4096 notepad.exe \"a \\\"b\\\" ü\" 100x200 at 10,20 [focus]\n\
         8192 notepad.exe \"\" 100x200 at 10,20 [popup of 4096 at 3,4]\n"
    );
    let (json, result) = said(&rig.context, "windows work --json");
    assert_eq!(result, Ok(()));
    assert_eq!(json, format!("{}\n", layout_line(&two_cells())), "the line as the helper sent it");
    assert!(rig.agent.requests.lock().unwrap().is_empty(), "a helper that answers is not started or stopped");
}

#[test]
fn a_machine_that_has_none_open_says_so() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    assert_eq!(said(&rig.context, "windows work").0, "Office PC has no app windows open.\n");
}

#[test]
fn a_helper_that_was_started_for_the_look_is_ended_after() {
    let scratch = Scratch::new();
    let rig = rig_listening(&scratch, Kind::Windows, true, &["started\r\n", ""], false);
    let (_, result) = said(&rig.context, "windows work");
    assert!(result.is_err(), "the pretend start leaves the port empty");
    let requests = rig.agent.requests.lock().unwrap().clone();
    assert!(requests[0].2.contains("$helperName = 'app-windows'"), "{}", requests[0].2);
    let last = requests.last().unwrap();
    assert_eq!(last.2, "$helperName = 'app-windows'\nscript", "what it started is ended after the look");
}

#[test]
fn only_a_running_windows_machine_that_has_app_windows_on_is_asked() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Plain, true, &[]);
    assert_eq!(
        refused(&rig.context, "windows work"),
        "Office PC is not a Windows machine, and only Windows has a helper for this."
    );
    make(&rig.context, "off", "Off PC", Kind::Windows);
    assert_eq!(refused(&rig.context, "windows off"), "Off PC is not running, so it has no windows.");
    assert!(rig.agent.requests.lock().unwrap().is_empty());
    assert_eq!(rig.helper.connections(), 0);
}

#[test]
fn with_the_switch_off_nothing_is_started_or_sent_and_the_words_say_how_to_turn_it_on() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, false, &["started\r\n"]);
    let words = "App windows is off for Office PC. guesthouse-vm set work app-windows on turns it on, and the \
                 machine gets what it needs at its next start.";
    assert_eq!(refused(&rig.context, "windows work"), words);
    assert_eq!(refused(&rig.context, "windows work --json"), words);
    assert_eq!(refused(&rig.context, r"app work C:\Windows\notepad.exe"), words);
    assert!(rig.agent.requests.lock().unwrap().is_empty(), "no agent request, so no helper starts");
    assert_eq!(rig.helper.connections(), 0, "and nothing connects");
}

#[test]
fn app_launches_a_program_by_its_path_and_leaves_the_helper_running() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let (out, result) = said(&rig.context, r"app work C:\Windows\notepad.exe");
    assert_eq!(result, Ok(()));
    assert_eq!(out, "C:\\Windows\\notepad.exe was started in Office PC.\n");
    let seen = rig.helper.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        (seen[0]["t"].as_str(), seen[0]["path"].as_str(), seen[0]["args"].as_str()),
        (Some("launch"), Some("C:\\Windows\\notepad.exe"), Some(""))
    );
    assert!(rig.agent.requests.lock().unwrap().is_empty(), "nothing was started or ended");
}

#[test]
fn app_says_the_helpers_reason_when_it_cannot_start_it() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    rig.helper.respond_with(|request, _| {
        vec![answer_line(request["n"].as_u64().unwrap(), false, "the system cannot find the file")]
    });
    assert_eq!(
        refused(&rig.context, r"app work C:\nope.exe"),
        "Office PC could not start C:\\nope.exe: the system cannot find the file."
    );
    rig.helper.respond_with(|_, _| Vec::new());
    assert!(refused(&rig.context, r"app work C:\slow.exe").contains("did not take C:\\slow.exe"));
}

#[test]
fn the_words_after_windows_and_app_are_checked() {
    assert!(usage_of("windows").contains("windows needs a machine."));
    assert!(usage_of("windows a b").contains("windows takes only a machine."));
    assert!(usage_of("windows a --yes").contains("windows has no option --yes."));
    assert!(usage_of("app").contains("app needs a machine and a program."));
    assert!(usage_of("app a").contains("app needs a machine and a program."));
    assert!(usage_of("app a b c").contains("app takes only a machine and a program."));
    assert!(usage_of("app a b --json").contains("app has no option --json."));
    assert!(
        USAGE.contains("windows MACHINE [--json]") && USAGE.contains("app MACHINE PROGRAM [--start-menu]")
    );
    assert!(USAGE.contains("remote-desktop, ssh,\n                             app-windows,"));
}
