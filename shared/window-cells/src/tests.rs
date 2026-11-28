//! The crate's own hold on what it does, against the same golden files as the Linux host's tests in
//! `app-core/tests/app_windows_lines.rs`, so that it is held on every host the crate builds on.

use super::*;
use std::fs;
use std::path::Path;

const HELPER_LINES: &str = include_str!("../../../linux/app-core/tests/app_windows_lines/helper-lines.jsonl");

fn golden(name: &str) -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../linux/app-core/tests/app_windows_lines").join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Today's text against the golden copy of it.
fn same(name: &str, text: &str) {
    assert_eq!(text, golden(name), "{name}");
}

fn read(line: &str) -> GuestMessage {
    parse_guest_line(line, MOST_LINE_BYTES).unwrap_or_else(|problem| panic!("{problem}: {line}"))
}

fn refused(line: &str) -> String {
    parse_guest_line(line, MOST_LINE_BYTES).unwrap_err().to_string()
}

fn helper_messages() -> Vec<GuestMessage> {
    HELPER_LINES.lines().map(read).collect()
}

fn layout_of(message: &GuestMessage) -> Layout {
    match message {
        GuestMessage::Layout(layout) => layout.clone(),
        other => panic!("not a layout: {other:?}"),
    }
}

// From the host to the helper.

fn requests() -> Vec<Request> {
    let id = "65814".to_owned();
    vec![
        Request::Focus { id: id.clone() },
        Request::Minimize { id: id.clone() },
        Request::Restore { id: id.clone() },
        Request::Close { id: id.clone() },
        Request::Resize { id: id.clone(), w: 1280, h: -1 },
        Request::Launch(Launch::App("Accessories/Notepad".into())),
        Request::Launch(Launch::App("A \"quoted\" name\\ with a\nline and \u{1} \u{e9}t\u{e9}".into())),
        Request::Launch(Launch::Path { path: r"C:\Program Files\App\app.exe".into(), args: String::new() }),
        Request::Launch(Launch::Path {
            path: r"C:\Tools\t.exe".into(),
            args: "--open \"D:\\a b.txt\"\t2".into(),
        }),
        Request::Apps { icons: true },
        Request::Apps { icons: false },
        Request::Icon { id, size: 48 },
        Request::Layout,
        Request::Scale { percent: 150 },
        Request::Screen { floor: Size { w: 1280, h: 720 }, ceiling: Size { w: 7680, h: 4320 } },
    ]
}

#[test]
fn every_line_the_host_writes_is_as_it_was() {
    let mut text = String::new();
    for (n, request) in requests().iter().enumerate() {
        let n = [0, 1, 2, 41, u64::MAX][n % 5];
        text.push_str(&format!("{} {} {}\n", request.kind(), request.answer_is_long(), request.line(n)));
    }
    same("host-lines.txt", &text);
    assert_eq!(token_line("0123456789abcdef"), "0123456789abcdef\n");
    assert_eq!(side_token_line("0123456789abcdef"), "0123456789abcdef side\n");
    assert_eq!(SIDE_WORD, "side");
    assert_eq!((PROTOCOL_VERSION, MOST_LINE_BYTES, MOST_APPS_LINE_BYTES), (1, 1 << 20, 16 << 20));
}

// From the helper to the host.

#[test]
fn each_line_the_helper_writes_is_read_as_it_was() {
    let read: String = helper_messages().iter().map(|message| format!("{message:?}\n")).collect();
    same("helper-lines.read.txt", &read);
}

#[test]
fn the_stand_in_helper_writes_its_lines_as_it_did() {
    let messages = helper_messages();
    let mut text = String::new();
    text.push_str(&hello_line("3f2a9c1e4b5d46e8a7c0b1d2e3f4a5b6", Size { w: 1920, h: 1080 }, 125));
    text.push('\n');
    text.push_str(&layout_line(&layout_of(&messages[2])));
    text.push('\n');
    text.push_str(&answer_line(3, true, "ignored when it went well"));
    text.push('\n');
    text.push_str(&answer_line(5, false, "a \"quoted\" reason\nover two lines"));
    text.push('\n');
    text.push_str(&display_line(&Display { scale: 150, steps: vec![100, 125, 150], need: None }));
    text.push('\n');
    text.push_str(&display_line(&Display {
        scale: 100,
        steps: vec![],
        need: Some(Size { w: 2576, h: 1440 }),
    }));
    text.push('\n');
    text.push_str(&desktop_line("secure"));
    text.push('\n');
    same("stand-in-lines.txt", &text);
    // What the stand-in writes, the host reads back as it was meant.
    for line in text.lines() {
        read(line);
    }
    assert_eq!(read(&layout_line(&layout_of(&messages[2]))), messages[2]);
}

#[test]
fn what_a_line_may_leave_out_or_add_is_as_it_was() {
    let bare_cell = r#"{"id":"1","pid":2,"exe":"a.exe","cls":"A","title":"t","cell":{"x":0,"y":0,"w":10,"h":10},"frame":{"x":0,"y":0,"w":10,"h":10},"popup":false,"parent":"","rel":{"x":0,"y":0},"state":"shown","overflow":false,"focus":false,"caption":{"h":0,"buttons_w":0},"resizable":true}"#;
    let layout = read(&format!(
        r#"{{"t":"layout","seq":2,"screen":{{"w":800,"h":600}},"cells":[{bare_cell}],"more":1}}"#
    ));
    let cell = &layout_of(&layout).cells[0];
    assert_eq!((cell.app.as_str(), cell.name.as_str(), cell.owner.as_str(), cell.corner), ("", "", "", 0));
    let extra = bare_cell.replacen("{\"id\"", "{\"later\":[1,2],\"id\"", 1);
    let layout = read(&format!(r#"{{"t":"layout","seq":2,"screen":{{"w":800,"h":600}},"cells":[{extra}]}}"#));
    assert_eq!(&layout_of(&layout).cells[0], cell);
    assert_eq!(
        read(r#"{"t":"answer","re":9,"ok":false}"#),
        GuestMessage::Answer(Answer { re: 9, ok: false, error: String::new(), apps: None, icon: None })
    );
    assert_eq!(
        read(r#"{"t":"answer","re":9,"ok":true,"apps":[{"id":"x","name":"X"}],"extra":"y"}"#),
        GuestMessage::Answer(Answer {
            re: 9,
            ok: true,
            error: String::new(),
            apps: Some(vec![App {
                id: "x".into(),
                name: "X".into(),
                target: String::new(),
                icon: String::new(),
                minor: false
            }]),
            icon: None,
        })
    );
    assert_eq!(
        read(r#"{"t":"display","scale":125}"#),
        GuestMessage::Display(Display { scale: 125, steps: vec![], need: None })
    );
    assert_eq!(
        read(r#"{"t":"display","scale":125,"need":null}"#),
        GuestMessage::Display(Display { scale: 125, steps: vec![], need: None })
    );
    assert_eq!(
        read(r#"{"t":"hello","v":1,"session":"s","screen":{"w":1,"h":2},"scale":100,"build":"later"}"#),
        GuestMessage::Hello(Hello { session: "s".into(), screen: Size { w: 1, h: 2 }, scale: 100 })
    );
    assert_eq!(read(r#"{"t":"desktop","kind":"locked","why":"x"}"#), GuestMessage::Desktop(Desktop::Locked));
    assert_eq!(read(r#"{"t":"ping"}"#), GuestMessage::Other("ping".into()));
    assert_eq!(read(r#"{"t":"","x":1}"#), GuestMessage::Other(String::new()));
    assert_eq!(read(r#"{"t":"Layout"}"#), GuestMessage::Other("Layout".into()));
    assert_eq!(read("  {\"t\":\"desktop\",\"kind\":\"normal\"}\r"), GuestMessage::Desktop(Desktop::Normal));
}

#[test]
fn every_line_refused_says_why_as_it_did() {
    let longest = format!(r#"{{"t":"ping","pad":"{}"}}"#, "x".repeat(MOST_LINE_BYTES - 21));
    assert_eq!(longest.len(), MOST_LINE_BYTES);
    assert_eq!(read(&longest), GuestMessage::Other("ping".into()));
    let over = format!("{longest} ");
    assert_eq!(refused(&over), "a line of 1048577 bytes, where 1048576 is the most");
    assert_eq!(
        parse_guest_line(&over, MOST_LINE_BYTES).unwrap_err(),
        LineProblem::TooLong { length: MOST_LINE_BYTES + 1, most: MOST_LINE_BYTES }
    );
    assert_eq!(parse_guest_line(&over, MOST_APPS_LINE_BYTES), Ok(GuestMessage::Other("ping".into())));
    assert_eq!(
        parse_guest_line("{\"t\":\"ping\"}", 5).unwrap_err().to_string(),
        "a line of 12 bytes, where 5 is the most"
    );
    assert_eq!(refused(""), "a line that is not JSON: EOF while parsing a value at line 1 column 0");
    assert_eq!(refused("hello"), "a line that is not JSON: expected value at line 1 column 1");
    assert_eq!(
        refused("{\"t\":\"hello\""),
        "a line that is not JSON: EOF while parsing an object at line 1 column 12"
    );
    assert_eq!(
        refused("{\"t\":\"x\"} {}"),
        "a line that is not JSON: trailing characters at line 1 column 11"
    );
    assert_eq!(refused("{}"), "a line with no type");
    assert_eq!(refused("{\"t\":7}"), "a line with no type");
    assert_eq!(refused("{\"t\":null}"), "a line with no type");
    assert_eq!(refused("{\"type\":\"hello\"}"), "a line with no type");
    assert_eq!(refused("[\"hello\"]"), "a line with no type");
    assert_eq!(refused("\"hello\""), "a line with no type");
    assert_eq!(
        refused(r#"{"t":"hello","v":2,"session":"s","screen":{"w":1,"h":1},"scale":100}"#),
        "version 2 of the messages, where 1 is read"
    );
    assert_eq!(
        parse_guest_line(
            r#"{"t":"hello","v":0,"session":"s","screen":{"w":1,"h":1},"scale":100}"#,
            MOST_LINE_BYTES
        ),
        Err(LineProblem::Version(0))
    );
    assert_eq!(
        refused(r#"{"t":"hello","v":2}"#),
        "a hello message that cannot be read: missing field `session`",
        "a hello is read whole before its version is looked at"
    );
    assert_eq!(
        refused(r#"{"t":"hello","session":"s","screen":{"w":1,"h":1},"scale":100}"#),
        "a hello message that cannot be read: missing field `v`"
    );
    assert_eq!(
        refused(r#"{"t":"hello","v":1,"session":"s","screen":{"w":1},"scale":100}"#),
        "a hello message that cannot be read: missing field `h`"
    );
    assert_eq!(
        refused(r#"{"t":"hello","v":1,"session":"s","screen":{"w":1,"h":1},"scale":-1}"#),
        "a hello message that cannot be read: invalid value: integer `-1`, expected u32"
    );
    assert_eq!(
        refused(r#"{"t":"layout","seq":1,"screen":{"w":1,"h":1}}"#),
        "a layout message that cannot be read: missing field `cells`"
    );
    assert_eq!(
        refused(r#"{"t":"layout","seq":1,"screen":{"w":1,"h":1},"cells":[{"id":"1"}]}"#),
        "a layout message that cannot be read: missing field `pid`"
    );
    let cell = r#"{"id":"1","pid":2,"exe":"a.exe","cls":"A","title":"t","cell":{"x":0,"y":0,"w":10,"h":10},"frame":{"x":0,"y":0,"w":10,"h":10},"popup":false,"parent":"","rel":{"x":0,"y":0},"state":"hidden","overflow":false,"focus":false,"caption":{"h":0,"buttons_w":0},"resizable":true}"#;
    assert_eq!(
        refused(&format!(r#"{{"t":"layout","seq":1,"screen":{{"w":1,"h":1}},"cells":[{cell}]}}"#)),
        "a layout message that cannot be read: unknown variant `hidden`, expected one of `shown`, `minimized`, \
         `covered`"
    );
    assert_eq!(
        refused(r#"{"t":"layout","seq":-1,"screen":{"w":1,"h":1},"cells":[]}"#),
        "a layout message that cannot be read: invalid value: integer `-1`, expected u64"
    );
    assert_eq!(
        refused(r#"{"t":"answer","ok":true}"#),
        "a answer message that cannot be read: missing field `re`"
    );
    assert_eq!(
        refused(r#"{"t":"answer","re":1}"#),
        "a answer message that cannot be read: missing field `ok`"
    );
    assert_eq!(
        refused(r#"{"t":"answer","re":1,"ok":"yes"}"#),
        "a answer message that cannot be read: invalid type: string \"yes\", expected a boolean"
    );
    assert_eq!(
        refused(r#"{"t":"answer","re":1,"ok":true,"apps":[{"id":"x"}]}"#),
        "a answer message that cannot be read: missing field `name`"
    );
    assert_eq!(
        refused(r#"{"t":"answer","re":1,"ok":true,"icon":7}"#),
        "a answer message that cannot be read: invalid type: integer `7`, expected a string"
    );
    assert_eq!(refused(r#"{"t":"desktop"}"#), "a desktop message that cannot be read: missing field `kind`");
    assert_eq!(
        refused(r#"{"t":"desktop","kind":"Secure"}"#),
        "a desktop message that cannot be read: unknown variant `Secure`, expected one of `normal`, `secure`, \
         `locked`"
    );
    assert_eq!(refused(r#"{"t":"display"}"#), "a display message that cannot be read: missing field `scale`");
    assert_eq!(
        refused(r#"{"t":"display","scale":100,"steps":"100"}"#),
        "a display message that cannot be read: invalid type: string \"100\", expected a sequence"
    );
    assert_eq!(
        LineProblem::Malformed { kind: "layout".into(), why: "why".into() }.to_string(),
        "a layout message that cannot be read: why"
    );
    assert_eq!(LineProblem::NotJson("why".into()).to_string(), "a line that is not JSON: why");
}

// What is made of the lines.

/// The changes each message makes in a model, as they came.
fn changes_of(model: &mut Model, message: &GuestMessage) -> String {
    match message {
        GuestMessage::Hello(hello) => format!("hello {:?}", model.hello(hello)),
        GuestMessage::Layout(layout) => {
            let Applied { changes, seq_went_back } = model.layout(layout);
            format!("layout {seq_went_back} {changes:?}")
        }
        GuestMessage::Desktop(desktop) => format!("desktop {:?}", model.desktop(*desktop)),
        GuestMessage::Display(display) => format!("display {:?}", model.display(display)),
        other => format!("nothing for {other:?}"),
    }
}

#[test]
fn the_model_makes_the_same_changes_of_the_same_lines() {
    let messages = helper_messages();
    let full = layout_of(&messages[2]);
    let mut moved = full.clone();
    moved.seq = 8;
    moved.cells[0].title = "Notes - Notepad".into();
    moved.cells[0].focus = false;
    moved.cells[1].rel = Point { x: 6, y: 72 };
    moved.cells[2].state = State::Shown;
    moved.cells[2].cell = Rect { x: 0, y: 600, w: 160, h: 28 };
    moved.cells.remove(3);
    let mut back = moved.clone();
    back.seq = 3;
    back.cells.retain(|cell| cell.id != "65814");
    let restarted = GuestMessage::Hello(Hello {
        session: "another".into(),
        screen: Size { w: 1280, h: 720 },
        scale: 100,
    });
    let sequence: Vec<GuestMessage> = vec![
        messages[0].clone(),
        messages[1].clone(),
        GuestMessage::Layout(full.clone()),
        GuestMessage::Layout(full.clone()),
        GuestMessage::Layout(moved),
        GuestMessage::Layout(back),
        messages[3].clone(),
        messages[10].clone(),
        messages[10].clone(),
        messages[11].clone(),
        messages[13].clone(),
        messages[13].clone(),
        messages[12].clone(),
        messages[0].clone(),
        restarted,
        GuestMessage::Layout(full),
    ];
    let mut model = Model::new();
    let mut text =
        format!("new {:?} {:?} {} {:?}\n", model.seq(), model.screen(), model.scale_percent(), model.steps());
    for message in &sequence {
        text.push_str(&changes_of(&mut model, message));
        text.push('\n');
        let windows: Vec<&str> = model.windows().map(|cell| cell.id.as_str()).collect();
        let shown: Vec<&str> = model.shown().map(|cell| cell.id.as_str()).collect();
        let popups: Vec<&str> = model.popups_of("65814").map(|cell| cell.id.as_str()).collect();
        text.push_str(&format!(
            "  seq {:?} screen {:?} scale {} steps {:?} need {:?} cells {} windows {windows:?} shown {shown:?} \
             popups {popups:?} get {:?}\n",
            model.seq(),
            model.screen(),
            model.scale_percent(),
            model.steps(),
            model.need(),
            model.cells().len(),
            model.get("131100").map(|cell| cell.rel),
        ));
    }
    text.push_str(&format!("clear {:?}\n", model.clear()));
    text.push_str(&format!("after {:?} {}\n", model.seq(), model.cells().len()));
    same("model-changes.txt", &text);
    let fields = Fields { title: true, ..Fields::default() };
    assert!(fields.any() && !Fields::default().any());
    assert!(matches!(Change::Restarted, Change::Restarted));
}

#[test]
fn each_cell_is_described_as_it_was() {
    let layout = layout_of(&helper_messages()[2]);
    let text: String = layout.cells.iter().map(|cell| describe(cell) + "\n").collect();
    same("describe.txt", &text);
    let mut nameless = layout.cells[3].clone();
    nameless.exe = String::new();
    nameless.title = "a\"b\\c\nd".into();
    assert_eq!(
        describe(&nameless),
        "393410 (unknown program) \"a\\\"b\\\\c\\nd\" 1200x800 at 1700,900 [covered, overflow, picture 860x540 at 1700,900]"
    );
}

#[test]
fn the_geometry_is_as_it_was() {
    assert_eq!(WINDOWS_STEPS, [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500]);
    assert_eq!(nearest_step(1.3, &WINDOWS_STEPS), Some(125));
    assert_eq!(nearest_step(1.375, &WINDOWS_STEPS), Some(125), "the smaller of two as near");
    assert_eq!(nearest_step(2.0, &[100, 150]), Some(150));
    assert_eq!(nearest_step(1.0, &[]), None);
    assert_eq!(shown_ratio(1.25, 125, &WINDOWS_STEPS, Fit::Sharp), 1.25);
    assert_eq!(shown_ratio(1.3, 125, &WINDOWS_STEPS, Fit::Sharp), 1.3);
    assert_eq!(shown_ratio(1.3, 125, &WINDOWS_STEPS, Fit::Size), 1.25);
    assert_eq!(shown_ratio(2.0, 125, &WINDOWS_STEPS, Fit::Sharp), 1.25);
    assert_eq!(shown_ratio(0.0, 100, &WINDOWS_STEPS, Fit::Size), 1.0);
    assert_eq!(shown_ratio(1.0, 0, &[], Fit::Size), 0.01);
    assert_eq!(
        [logical_length(886, 1.25), logical_length(10, 1.0), logical_length(0, 2.0), logical_length(3, 1.5)],
        [709, 10, 1, 2]
    );
    let layout = layout_of(&helper_messages()[2]);
    let (window, popup, minimized, covered) =
        (&layout.cells[0], &layout.cells[1], &layout.cells[2], &layout.cells[3]);
    let surface = Surface::shown(window, 1.25);
    assert_eq!(surface, Surface { w: 708.8, h: 474.4 });
    assert_eq!(Surface::shown(window, 0.0), Surface { w: 886.0, h: 593.0 });
    let screen = layout.screen;
    assert_eq!(guest_point(window, surface, 0.0, 0.0, screen), Point { x: 0, y: 41 });
    assert_eq!(guest_point(window, surface, 100.4, 50.9, screen), Point { x: 125, y: 104 });
    assert_eq!(guest_point(window, surface, 9000.0, -5.0, screen), Point { x: 885, y: 41 });
    assert_eq!(guest_point(popup, Surface::shown(popup, 1.0), 3.0, 4.0, screen), Point { x: 8, y: 117 });
    assert_eq!(
        guest_point(minimized, Surface::shown(minimized, 1.0), 3.0, 4.0, screen),
        Point { x: 0, y: 0 }
    );
    assert_eq!(crop(window.cell, screen), Some(Crop { source: window.cell, offset: Point { x: 0, y: 0 } }));
    assert_eq!(
        crop(Rect { x: -10, y: -5, w: 100, h: 50 }, screen),
        Some(Crop { source: Rect { x: 0, y: 0, w: 90, h: 45 }, offset: Point { x: 10, y: 5 } })
    );
    assert_eq!(
        crop(covered.cell, Size { w: 2000, h: 1000 }),
        Some(Crop { source: Rect { x: 1700, y: 900, w: 300, h: 100 }, offset: Point { x: 0, y: 0 } })
    );
    assert_eq!(crop(minimized.cell, screen), None);
    assert_eq!(popup_place(popup, window, surface), Rect { x: 4, y: 58, w: 152, h: 224 });
    assert_eq!(popup_place(popup, window, Surface::shown(window, 1.0)), Rect { x: 5, y: 72, w: 190, h: 280 });
    assert_eq!(drag_strip(window), Some(Rect { x: 0, y: 0, w: 748, h: 32 }));
    assert_eq!(drag_strip(popup), None);
    assert_eq!(drag_strip(minimized), None);
    let mut narrow = window.clone();
    narrow.caption = Caption { h: 32, buttons_w: 900 };
    assert_eq!(drag_strip(&narrow), None);
    assert!(on_drag_strip(window, surface, 10.0, 10.0));
    assert!(!on_drag_strip(window, surface, 600.0, 10.0), "on the buttons");
    assert!(!on_drag_strip(window, surface, 10.0, 26.0), "under the title bar");
    assert!(!on_drag_strip(popup, Surface::shown(popup, 1.0), 1.0, 1.0));
}

#[test]
fn the_values_of_a_cell_keep_their_names() {
    let cell: Cell = serde_json::from_str(
        r#"{"id":"1","pid":2,"exe":"a","cls":"b","title":"c","cell":{"x":1,"y":2,"w":3,"h":4},"frame":{"x":5,"y":6,"w":7,"h":8},"popup":true,"parent":"9","rel":{"x":1,"y":1},"state":"covered","overflow":true,"focus":true,"caption":{"h":3,"buttons_w":4},"resizable":false}"#,
    )
    .unwrap();
    same("cell.json", &(serde_json::to_string(&cell).unwrap() + "\n"));
    assert_eq!(serde_json::to_string(&State::Minimized).unwrap(), "\"minimized\"");
    assert_eq!(serde_json::to_string(&Display::default()).unwrap(), r#"{"scale":0,"steps":[],"need":null}"#);
}

#[test]
fn a_layout_holds_no_more_cells_than_the_limit() {
    let first = layout_of(
        &helper_messages()
            .iter()
            .find(|m| matches!(m, GuestMessage::Layout(l) if !l.cells.is_empty()))
            .unwrap()
            .clone(),
    );
    let template = first.cells[0].clone();
    let cells =
        (0..MOST_CELLS + 50).map(|n| Cell { id: n.to_string(), ..template.clone() }).collect::<Vec<_>>();
    let mut model = Model::default();
    let applied = model.layout(&Layout { seq: 1, screen: first.screen, cells });
    assert_eq!(model.cells().len(), MOST_CELLS);
    assert_eq!(
        applied.changes.iter().filter(|change| matches!(change, Change::Added(_))).count(),
        MOST_CELLS
    );
}
