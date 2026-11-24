use super::*;
use crate::agent::make_token;
use crate::agent::stand_in::ScriptedAgent;
use crate::text::base64_encode;
use std::sync::Arc;

const LIST: &str = concat!(
    r#"{"windows":["#,
    r#"{"handle":4096,"owner":0,"pid":10,"exe":"notepad.exe","class":"Notepad","title":"a \"b\" ü","#,
    r#""frame":{"left":10,"top":20,"right":110,"bottom":220},"rect":{"left":8,"top":20,"right":112,"bottom":224},"#,
    r#""z":0,"minimized":false,"tool_window":false,"popup":false,"foreground":true}"#,
    r#"]}"#
);

fn refused(context: &Context, words: &str) -> String {
    match said(context, words).1 {
        Err(Problem::Failed(failure)) => failure.0,
        other => panic!("{words:?} gave {other:?}"),
    }
}

/// A Windows machine that runs as pid 40, whose agent answers from a list.
fn rig(scratch: &Scratch, kind: Kind, answers: &[&str]) -> (Context, Arc<ScriptedAgent>) {
    let mut context = scratch_context(scratch);
    let agent = Arc::new(ScriptedAgent::answering(answers));
    context.agent = agent.clone();
    for name in ["windows.ps1", "windows-start.ps1", "windows-ask.ps1", "helper-stop.ps1"] {
        scratch.write(&format!("program/guest/{name}"), "script");
    }
    let machine = make(&context, "work", "Office PC", kind);
    make_token(&context, &machine).unwrap();
    running(&context, &machine, 40);
    (context, agent)
}

fn answer() -> String {
    format!("answer {}\r\n", base64_encode(LIST.as_bytes()))
}

#[test]
fn each_window_is_a_line_and_json_is_the_helpers_answer_as_it_is() {
    let scratch = Scratch::new();
    let (context, agent) = rig(&scratch, Kind::Windows, &[&answer(), &answer()]);
    let (out, result) = said(&context, "windows work");
    assert_eq!(result, Ok(()));
    assert_eq!(out, "0 0x1000 notepad.exe \"a \\\"b\\\" ü\" 100x200 at 10,20 [foreground]\n");
    assert_eq!(said(&context, "windows work --json"), (format!("{LIST}\n"), Ok(())));
    assert_eq!(agent.requests.lock().unwrap().len(), 2, "a helper that answers is not started or stopped");
}

#[test]
fn a_machine_that_has_none_open_says_so() {
    let scratch = Scratch::new();
    let none = format!("answer {}\r\n", base64_encode(br#"{"windows":[]}"#));
    let (context, _) = rig(&scratch, Kind::Windows, &[&none]);
    assert_eq!(said(&context, "windows work").0, "Office PC has no windows open.\n");
}

#[test]
fn only_a_running_windows_machine_is_asked() {
    let scratch = Scratch::new();
    let (context, agent) = rig(&scratch, Kind::Plain, &[]);
    assert_eq!(
        refused(&context, "windows work"),
        "Office PC is not a Windows machine, and only Windows has a helper for this."
    );
    make(&context, "off", "Off PC", Kind::Windows);
    assert_eq!(refused(&context, "windows off"), "Off PC is not running, so it has no windows.");
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn the_words_after_windows_are_checked() {
    assert!(usage_of("windows").contains("windows needs a machine."));
    assert!(usage_of("windows a b").contains("windows takes only a machine."));
    assert!(usage_of("windows a --yes").contains("windows has no option --yes."));
    assert!(USAGE.contains("windows MACHINE [--json]"));
}
