use super::describe::describe;
use super::message::*;

fn cells() -> Vec<Cell> {
    let layout = r##"{"t":"layout","seq":1,"screen":{"w":1920,"h":1080},"cells":[
{"id":"65814","pid":4120,"exe":"notepad.exe","cls":"Notepad","title":"Untitled - Notepad",
 "cell":{"x":0,"y":41,"w":886,"h":593},"frame":{"x":0,"y":41,"w":886,"h":593},
 "popup":false,"parent":"","rel":{"x":0,"y":0},"state":"shown","overflow":false,"focus":true,
 "caption":{"h":32,"buttons_w":138},"resizable":true},
{"id":"131100","pid":4120,"exe":"notepad.exe","cls":"#32768","title":"",
 "cell":{"x":900,"y":100,"w":190,"h":280},"frame":{"x":5,"y":113,"w":190,"h":280},
 "popup":true,"parent":"65814","rel":{"x":5,"y":72},"state":"shown","overflow":false,"focus":false,
 "caption":{"h":0,"buttons_w":0},"resizable":false},
{"id":"262000","pid":6001,"exe":"","cls":"Chrome_WidgetWin_1",
 "title":"He said \"hi\" \\ ünï — 日本語 😀\ttab\nline",
 "cell":{"x":0,"y":700,"w":1200,"h":380},"frame":{"x":0,"y":0,"w":1920,"h":1040},
 "popup":false,"parent":"","rel":{"x":0,"y":0},"state":"covered","overflow":true,"focus":false,
 "caption":{"h":32,"buttons_w":138},"resizable":true},
{"id":"197000","pid":5200,"exe":"CalculatorApp.exe","cls":"ApplicationFrameWindow","title":"Calculator",
 "cell":{"x":0,"y":0,"w":160,"h":28},"frame":{"x":0,"y":0,"w":160,"h":28},
 "popup":false,"parent":"","rel":{"x":0,"y":0},"state":"minimized","overflow":false,"focus":false,
 "caption":{"h":0,"buttons_w":0},"resizable":true}
]}"##;
    match parse_guest_line(&layout.replace('\n', ""), MOST_LINE_BYTES).unwrap() {
        GuestMessage::Layout(layout) => layout.cells,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_cell_is_one_line_however_odd_its_title() {
    let lines: Vec<String> = cells().iter().map(describe).collect();
    assert_eq!(lines[0], "65814 notepad.exe \"Untitled - Notepad\" 886x593 at 0,41 [focus]");
    assert_eq!(
        lines[1],
        "131100 notepad.exe \"\" 190x280 at 5,113 [popup of 65814 at 5,72, picture 190x280 at 900,100]"
    );
    assert!(
        lines[2].contains("(unknown program)")
            && lines[2].contains("[covered, overflow, picture 1200x380 at 0,700]"),
        "{}",
        lines[2]
    );
    assert!(lines[2].contains(r#""He said \"hi\" \\ ünï — 日本語 😀\ttab\nline""#), "{}", lines[2]);
    assert!(lines[3].ends_with("[minimized]"), "{}", lines[3]);
    assert!(lines.iter().all(|line| !line.contains('\n')));
}
