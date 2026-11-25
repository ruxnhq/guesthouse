use super::message::*;
use serde_json::{Value, json};

const HELLO: &str = r#"{"t":"hello","v":1,"session":"a1b2","screen":{"w":1920,"h":1080},"scale":125}"#;

/// A window, a popup of it, a minimized window and an overflow cell, as the helper writes them. The
/// second window's title has quotes, a backslash, a tab and text from other scripts.
const LAYOUT: &str = r##"{"t":"layout","seq":7,"screen":{"w":1920,"h":1080},"cells":[
{"id":"65814","pid":4120,"exe":"notepad.exe","cls":"Notepad","title":"Untitled - Notepad",
 "cell":{"x":0,"y":41,"w":886,"h":593},"frame":{"x":0,"y":41,"w":886,"h":593},
 "popup":false,"parent":"","rel":{"x":0,"y":0},"state":"shown","overflow":false,"focus":true,
 "caption":{"h":32,"buttons_w":138},"resizable":true},
{"id":"131100","pid":4120,"exe":"notepad.exe","cls":"#32768","title":"",
 "cell":{"x":900,"y":100,"w":190,"h":280},"frame":{"x":5,"y":113,"w":190,"h":280},
 "popup":true,"parent":"65814","rel":{"x":5,"y":72},"state":"shown","overflow":false,"focus":false,
 "caption":{"h":0,"buttons_w":0},"resizable":false},
{"id":"197000","pid":5200,"exe":"CalculatorApp.exe","cls":"ApplicationFrameWindow","title":"Calculator",
 "cell":{"x":0,"y":0,"w":0,"h":0},"frame":{"x":-32000,"y":-32000,"w":160,"h":28},
 "popup":false,"parent":"","rel":{"x":0,"y":0},"state":"minimized","overflow":false,"focus":false,
 "caption":{"h":0,"buttons_w":0},"resizable":true},
{"id":"262000","pid":6001,"exe":"msedge.exe","cls":"Chrome_WidgetWin_1",
 "title":"He said \"hi\" \\ ünï — 日本語 😀\ttab","future_field":[1,2],
 "cell":{"x":0,"y":700,"w":1200,"h":380},"frame":{"x":0,"y":0,"w":1920,"h":1040},
 "popup":false,"parent":"","rel":{"x":0,"y":0},"state":"covered","overflow":true,"focus":false,
 "caption":{"h":32,"buttons_w":138},"resizable":true}
]}"##;

fn layout() -> Layout {
    match parse_guest_line(&LAYOUT.replace('\n', ""), MOST_LINE_BYTES).unwrap() {
        GuestMessage::Layout(layout) => layout,
        other => panic!("{other:?}"),
    }
}

#[test]
fn hello_carries_the_session_the_screen_and_the_scale() {
    let message = parse_guest_line(HELLO, MOST_LINE_BYTES).unwrap();
    let hello = Hello { session: "a1b2".into(), screen: Size { w: 1920, h: 1080 }, scale: 125 };
    assert_eq!(message, GuestMessage::Hello(hello));
}

#[test]
fn a_hello_of_another_version_is_refused_with_the_version() {
    for text in [HELLO.replace("\"v\":1", "\"v\":2"), HELLO.replace("\"v\":1", "\"v\":0")] {
        assert!(matches!(parse_guest_line(&text, MOST_LINE_BYTES), Err(LineProblem::Version(_))), "{text}");
    }
    let without = HELLO.replace("\"v\":1,", "");
    assert!(matches!(parse_guest_line(&without, MOST_LINE_BYTES), Err(LineProblem::Malformed { .. })));
}

#[test]
fn a_layout_has_its_popup_with_its_parent_and_where_it_lies_against_it() {
    let layout = layout();
    assert_eq!((layout.seq, layout.cells.len()), (7, 4));
    let popup = &layout.cells[1];
    assert!(popup.popup);
    assert_eq!((popup.parent.as_str(), popup.rel), ("65814", Point { x: 5, y: 72 }));
    assert_eq!(popup.cell, Rect { x: 900, y: 100, w: 190, h: 280 }, "the picture is not where the window is");
    assert_ne!(popup.cell, popup.frame);
    assert_eq!(layout.cells[0].caption, Caption { h: 32, buttons_w: 138 });
}

#[test]
fn a_minimized_window_and_an_overflow_cell_are_read_for_what_they_are() {
    let layout = layout();
    assert_eq!(layout.cells[2].state, State::Minimized);
    assert_eq!(layout.cells[2].frame.x, -32000);
    let overflow = &layout.cells[3];
    assert!(overflow.overflow);
    assert_eq!(overflow.state, State::Covered);
}

#[test]
fn a_title_with_quotes_and_other_scripts_comes_through_whole_and_an_unknown_field_is_ignored() {
    assert_eq!(layout().cells[3].title, "He said \"hi\" \\ ünï — 日本語 😀\ttab");
}

#[test]
fn a_layout_written_and_read_again_is_the_same() {
    let layout = layout();
    let text = layout_line(&layout);
    assert!(!text.contains('\n'), "a line stays one line however odd a title is");
    assert_eq!(parse_guest_line(&text, MOST_LINE_BYTES).unwrap(), GuestMessage::Layout(layout));
}

#[test]
fn a_cell_that_lacks_a_field_the_contract_names_is_refused() {
    let one = LAYOUT.replace('\n', "").replacen("\"resizable\":true", "\"resizabel\":true", 1);
    let problem = parse_guest_line(&one, MOST_LINE_BYTES).unwrap_err();
    assert!(
        matches!(&problem, LineProblem::Malformed { kind, why } if kind == "layout" && why.contains("resizable")),
        "{problem}"
    );
    let wrong_kind = LAYOUT.replace('\n', "").replacen("\"popup\":false", "\"popup\":\"no\"", 1);
    assert!(matches!(parse_guest_line(&wrong_kind, MOST_LINE_BYTES), Err(LineProblem::Malformed { .. })));
    let bad_state = LAYOUT.replace('\n', "").replacen("\"shown\"", "\"hidden\"", 1);
    assert!(matches!(parse_guest_line(&bad_state, MOST_LINE_BYTES), Err(LineProblem::Malformed { .. })));
}

#[test]
fn a_line_over_the_limit_is_refused_before_it_is_read() {
    let long = format!(r#"{{"t":"desktop","kind":"normal","pad":"{}"}}"#, "x".repeat(MOST_LINE_BYTES));
    let problem = parse_guest_line(&long, MOST_LINE_BYTES).unwrap_err();
    assert!(matches!(problem, LineProblem::TooLong { most: MOST_LINE_BYTES, .. }), "{problem:?}");
    assert_eq!(
        parse_guest_line(&long, MOST_APPS_LINE_BYTES).unwrap(),
        GuestMessage::Desktop(Desktop::Normal),
        "an apps answer's room is bigger"
    );
}

#[test]
fn what_is_not_a_message_is_told_apart() {
    assert!(matches!(parse_guest_line("not json", MOST_LINE_BYTES), Err(LineProblem::NotJson(_))));
    assert!(matches!(parse_guest_line("[1]", MOST_LINE_BYTES), Err(LineProblem::NoType)));
    assert!(matches!(parse_guest_line(r#"{"n":1}"#, MOST_LINE_BYTES), Err(LineProblem::NoType)));
    assert_eq!(
        parse_guest_line(r#"{"t":"clipboard","x":1}"#, MOST_LINE_BYTES).unwrap(),
        GuestMessage::Other("clipboard".into()),
        "a later helper's message is passed over, not an error"
    );
}

#[test]
fn answers_have_their_error_and_their_apps_only_when_there_are_any() {
    let ok = parse_guest_line(r#"{"t":"answer","re":4,"ok":true}"#, MOST_LINE_BYTES).unwrap();
    assert_eq!(ok, GuestMessage::Answer(Answer { re: 4, ok: true, error: String::new(), apps: None }));
    let failed = parse_guest_line(r#"{"t":"answer","re":5,"ok":false,"error":"no such window"}"#, 99999);
    let GuestMessage::Answer(failed) = failed.unwrap() else { panic!() };
    assert_eq!((failed.ok, failed.error.as_str()), (false, "no such window"));
    let apps = r#"{"t":"answer","re":7,"ok":true,"apps":[
        {"id":"notepad","name":"Notepad","target":"C:\\x.exe","icon":"AAAA"},
        {"id":"unins","name":"Uninstall","minor":true}]}"#
        .replace('\n', "");
    let GuestMessage::Answer(apps) = parse_guest_line(&apps, MOST_APPS_LINE_BYTES).unwrap() else { panic!() };
    let apps = apps.apps.unwrap();
    assert_eq!((apps[0].name.as_str(), apps[0].minor, apps[0].icon.as_str()), ("Notepad", false, "AAAA"));
    assert!(apps[1].minor && apps[1].icon.is_empty());
    assert!(matches!(
        parse_guest_line(r#"{"t":"answer","ok":true}"#, 999),
        Err(LineProblem::Malformed { .. })
    ));
}

#[test]
fn the_desktop_message_names_which_desktop_takes_the_input() {
    for (kind, wanted) in
        [("normal", Desktop::Normal), ("secure", Desktop::Secure), ("locked", Desktop::Locked)]
    {
        let line = desktop_line(kind);
        assert_eq!(parse_guest_line(&line, 999).unwrap(), GuestMessage::Desktop(wanted));
    }
    assert!(matches!(parse_guest_line(&desktop_line("other"), 999), Err(LineProblem::Malformed { .. })));
}

fn value(request: &Request, n: u64) -> Value {
    let line = request.line(n);
    assert!(!line.contains('\n'));
    serde_json::from_str(&line).unwrap()
}

#[test]
fn each_request_is_written_as_the_contract_has_it() {
    let id = || "65814".to_owned();
    assert_eq!(value(&Request::Focus { id: id() }, 1), json!({"t":"focus","n":1,"id":"65814"}));
    assert_eq!(value(&Request::Minimize { id: id() }, 2), json!({"t":"minimize","n":2,"id":"65814"}));
    assert_eq!(value(&Request::Restore { id: id() }, 3), json!({"t":"restore","n":3,"id":"65814"}));
    assert_eq!(value(&Request::Close { id: id() }, 4), json!({"t":"close","n":4,"id":"65814"}));
    assert_eq!(
        value(&Request::Resize { id: id(), w: 900, h: 700 }, 5),
        json!({"t":"resize","n":5,"id":"65814","w":900,"h":700})
    );
    assert_eq!(
        value(&Request::Launch(Launch::App("notepad".into())), 6),
        json!({"t":"launch","n":6,"app":"notepad"})
    );
    let path = Launch::Path { path: r"C:\Program Files\x.exe".into(), args: "\"a b\"".into() };
    assert_eq!(
        value(&Request::Launch(path), 6),
        json!({"t":"launch","n":6,"path":"C:\\Program Files\\x.exe","args":"\"a b\""})
    );
    assert_eq!(value(&Request::Apps { icons: true }, 7), json!({"t":"apps","n":7,"icons":true}));
    assert_eq!(value(&Request::Layout, 8), json!({"t":"layout","n":8}));
    assert!(Request::Apps { icons: false }.answer_is_long() && !Request::Layout.answer_is_long());
}

#[test]
fn the_token_goes_first_on_a_line_of_its_own() {
    assert_eq!(token_line("abc123"), "abc123\n");
}

#[test]
fn a_side_connection_says_so_after_the_token() {
    assert_eq!(side_token_line("abc123"), "abc123 side\n");
}
