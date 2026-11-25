use super::windows_tests::{Rig, rig, rig_with};
use super::*;
use crate::app_windows::{Look, answer_line};
use crate::testing::Scratch;

const ICON: &str = "iVBORw0KGgo=";

fn lists(rig: &Rig, apps: &str) {
    let apps = apps.to_owned();
    rig.helper.respond_with(move |request, _| {
        let n = request["n"].as_u64().unwrap_or(0);
        if request["t"] == "apps" {
            let apps = apps.replace("ICON", ICON).replace('\n', "");
            vec![format!("{{\"t\":\"answer\",\"re\":{n},\"ok\":true,\"apps\":{apps}}}")]
        } else {
            vec![answer_line(n, true, "")]
        }
    });
}

const SOME: &str = r#"[{"id":"Paint","name":"Paint","target":"C:\\Windows\\mspaint.exe","icon":"ICON","minor":false},
{"id":"Paint Help","name":"Paint Help","target":"","icon":"","minor":true}]"#;

#[test]
fn a_look_gets_the_programs_with_their_icons_and_the_windows_that_are_open() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    lists(&rig, SOME);
    let open = |id: &str, popup: bool| crate::app_windows::Cell {
        id: id.into(),
        pid: 5,
        exe: r"C:\Windows\mspaint.exe".into(),
        app: String::new(),
        name: String::new(),
        cls: String::new(),
        title: String::new(),
        cell: crate::app_windows::Rect { x: 0, y: 0, w: 9, h: 9 },
        frame: crate::app_windows::Rect { x: 0, y: 0, w: 9, h: 9 },
        popup,
        parent: String::new(),
        rel: crate::app_windows::Point::default(),
        state: crate::app_windows::State::Shown,
        overflow: false,
        focus: false,
        caption: crate::app_windows::Caption::default(),
        resizable: true,
        corner: 0,
    };
    rig.helper.with(|inner| inner.layout.cells = vec![open("1", false), open("2", false), open("3", true)]);
    let found = look(&rig.context, &rig.machine);
    let Look::Listed(found) = found else { panic!("not listed: {found:?}") };
    assert_eq!(found.apps.len(), 2);
    assert_eq!(found.windows.len(), 2, "the popup is left out");
    assert_eq!(found.apps[0].icon, ICON);
    assert!(found.apps[1].minor);
    assert_eq!(rig.helper.requests().iter().filter(|request| request["t"] == "apps").count(), 1);
    assert_eq!(rig.helper.requests().iter().find(|r| r["t"] == "apps").unwrap()["icons"], true);
}

#[test]
fn a_look_is_made_beside_a_viewer_and_does_not_take_its_connection() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    lists(&rig, SOME);
    let _ = look(&rig.context, &rig.machine);
    assert_eq!(rig.helper.connections(), 0, "the driving connection is the viewer's");
    assert!(rig.helper.with(|inner| inner.side_connections) >= 1);
}

#[test]
fn a_helper_that_refuses_is_a_failure_with_the_helpers_words() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    rig.helper.respond_with(|request, _| {
        vec![answer_line(request["n"].as_u64().unwrap(), false, "the Start menu is locked")]
    });
    assert_eq!(
        look(&rig.context, &rig.machine),
        Look::Failed("Office PC said: the Start menu is locked.".into())
    );
}

#[test]
fn a_helper_that_says_nothing_is_a_failure_that_names_the_wait() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    rig.helper.respond_with(|_, _| Vec::new());
    let Look::Failed(text) = look(&rig.context, &rig.machine) else { panic!("not a failure") };
    assert!(text.contains("did not answer in time"), "{text}");
}

#[test]
fn a_guest_nobody_is_logged_in_to_is_not_ready_yet_and_not_a_failure() {
    let scratch = Scratch::new();
    let rig = super::windows_tests::rig_listening(
        &scratch,
        Kind::Windows,
        true,
        &["nobody is logged in"; 40],
        false,
    );
    let Look::NotYet(text) = look(&rig.context, &rig.machine) else { panic!("not 'not yet'") };
    assert!(text.contains("Nobody is logged in to Office PC"), "{text}");
}

#[test]
fn a_machine_that_is_off_or_has_the_switch_off_is_not_asked() {
    let scratch = Scratch::new();
    let off = rig_with(&scratch, Kind::Windows, true, &[], true, false);
    let Look::Failed(text) = look(&off.context, &off.machine) else { panic!() };
    assert!(text.contains("is not running"), "{text}");
    assert_eq!(off.helper.connections() + off.helper.with(|inner| inner.side_connections), 0);
    let scratch = Scratch::new();
    let unset = rig(&scratch, Kind::Windows, false, &[]);
    let Look::Failed(text) = look(&unset.context, &unset.machine) else { panic!() };
    assert!(text.starts_with("App windows is off for Office PC."), "{text}");
    assert_eq!(unset.helper.with(|inner| inner.side_connections), 0);
}

#[test]
fn a_look_on_a_machine_that_is_not_windows_fails_before_anything_is_sent() {
    let scratch = Scratch::new();
    let plain = rig(&scratch, Kind::Plain, true, &[]);
    assert!(matches!(look(&plain.context, &plain.machine), Look::Failed(_)));
}
