use super::message::*;
use super::model::*;

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

fn window(id: &str, title: &str) -> Cell {
    Cell {
        id: id.into(),
        pid: 10,
        exe: "app.exe".into(),
        app: String::new(),
        name: String::new(),
        cls: "Frame".into(),
        title: title.into(),
        cell: rect(0, 0, 400, 300),
        frame: rect(0, 0, 400, 300),
        popup: false,
        parent: String::new(),
        rel: Point::default(),
        state: State::Shown,
        overflow: false,
        focus: false,
        caption: Caption { h: 32, buttons_w: 138 },
        resizable: true,
        corner: 0,
    }
}

fn popup(id: &str, parent: &str) -> Cell {
    Cell { popup: true, parent: parent.into(), rel: Point { x: 5, y: 40 }, ..window(id, "") }
}

fn layout(seq: u64, cells: Vec<Cell>) -> Layout {
    Layout { seq, screen: Size { w: 1920, h: 1080 }, cells }
}

fn hello(session: &str) -> Hello {
    Hello { session: session.into(), screen: Size { w: 1920, h: 1080 }, scale: 100 }
}

fn ids(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|change| match change {
            Change::Added(cell) => format!("+{}", cell.id),
            Change::Removed(cell) => format!("-{}", cell.id),
            Change::Changed { id, .. } => format!("~{id}"),
            Change::Restarted => "restart".into(),
            Change::Screen(_) => "screen".into(),
            Change::Desktop(_) => "desktop".into(),
            Change::Display(_) => "display".into(),
        })
        .collect()
}

fn ready() -> Model {
    let mut model = Model::new();
    model.hello(&hello("one"));
    model
}

#[test]
fn the_first_layout_adds_every_cell_a_window_before_its_popups() {
    let mut model = ready();
    let applied = model.layout(&layout(1, vec![popup("p", "w"), window("w", "A"), popup("q", "p")]));
    assert_eq!(ids(&applied.changes), ["+w", "+p", "+q"]);
    assert!(!applied.seq_went_back);
    assert_eq!(model.cells().len(), 3);
}

#[test]
fn what_changed_in_a_cell_is_named() {
    let mut model = ready();
    model.layout(&layout(1, vec![window("w", "A")]));
    let moved = Cell { frame: rect(10, 0, 400, 300), cell: rect(10, 0, 400, 300), ..window("w", "A") };
    let applied = model.layout(&layout(2, vec![moved]));
    assert_eq!(
        applied.changes,
        [Change::Changed { id: "w".into(), what: Fields { cell: true, frame: true, ..Fields::default() } }]
    );
    let retitled = Cell { title: "B".into(), focus: true, state: State::Minimized, ..window("w", "A") };
    let moved_back = Cell { frame: rect(10, 0, 400, 300), cell: rect(10, 0, 400, 300), ..retitled };
    let applied = model.layout(&layout(3, vec![moved_back]));
    let Change::Changed { what, .. } = &applied.changes[0] else { panic!("{applied:?}") };
    assert_eq!(*what, Fields { title: true, focus: true, state: true, ..Fields::default() });
}

#[test]
fn a_layout_that_says_nothing_new_changes_nothing() {
    let mut model = ready();
    model.layout(&layout(1, vec![window("w", "A"), popup("p", "w")]));
    let again = model.layout(&layout(1, vec![window("w", "A"), popup("p", "w")]));
    assert_eq!(again, Applied::default(), "a layout asked for again has the same seq and is no change");
}

#[test]
fn a_popup_that_moves_against_its_parent_is_a_change_of_its_link() {
    let mut model = ready();
    model.layout(&layout(1, vec![window("w", "A"), popup("p", "w")]));
    let moved = Cell { rel: Point { x: 9, y: 40 }, ..popup("p", "w") };
    let applied = model.layout(&layout(2, vec![window("w", "A"), moved]));
    assert_eq!(
        applied.changes,
        [Change::Changed { id: "p".into(), what: Fields { popup_link: true, ..Fields::default() } }]
    );
}

#[test]
fn cells_that_are_not_in_the_layout_are_gone_a_popup_before_its_window() {
    let mut model = ready();
    model.layout(&layout(1, vec![window("w", "A"), window("v", "B"), popup("p", "w")]));
    let applied = model.layout(&layout(2, vec![window("v", "B")]));
    assert_eq!(ids(&applied.changes), ["-p", "-w"]);
    assert_eq!(model.cells().len(), 1);
    let Change::Removed(last) = &applied.changes[1] else { panic!() };
    assert_eq!(last.title, "A", "what it last was, for the viewer's last word on it");
}

#[test]
fn the_popups_of_a_window_are_found() {
    let mut model = ready();
    model.layout(&layout(
        1,
        vec![window("w", "A"), popup("p", "w"), popup("q", "p"), popup("lost", "nobody"), window("v", "B")],
    ));
    let of = |id: &str| model.popups_of(id).map(|cell| cell.id.clone()).collect::<Vec<_>>();
    assert_eq!(of("w"), ["p"]);
    assert_eq!(of("p"), ["q"], "a submenu belongs to the menu it came from");
    assert_eq!(model.windows().count(), 2);
}

#[test]
fn a_parent_that_leads_round_to_itself_does_not_loop() {
    let mut model = ready();
    model.layout(&layout(1, vec![popup("a", "b"), popup("b", "a")]));
    let applied = model.layout(&layout(2, vec![]));
    assert_eq!(applied.changes.len(), 2);
}

#[test]
fn a_reconnect_to_the_same_helper_keeps_the_cells_until_the_layout_after_it() {
    let mut model = ready();
    model.layout(&layout(4, vec![window("w", "A")]));
    assert!(model.hello(&hello("one")).is_empty());
    assert_eq!(model.cells().len(), 1, "not shown as gone, then new");
    let applied = model.layout(&layout(5, vec![window("w", "A"), window("v", "B")]));
    assert_eq!(ids(&applied.changes), ["+v"]);
}

#[test]
fn a_helper_that_was_started_again_takes_every_old_cell_with_it_even_one_with_the_same_id() {
    let mut model = ready();
    model.layout(&layout(40, vec![window("w", "A"), popup("p", "w")]));
    let changes = model.hello(&hello("two"));
    assert_eq!(ids(&changes), ["restart", "-p", "-w"]);
    assert!(model.cells().is_empty());
    assert_eq!(model.seq(), None, "the new helper counts from its own start");
    let applied = model.layout(&layout(1, vec![window("w", "Another window with the old handle")]));
    assert_eq!(ids(&applied.changes), ["+w"]);
    assert!(!applied.seq_went_back, "1 after 40 is a new session's, not a step back");
    assert_eq!(model.session(), Some("two"));
}

#[test]
fn a_seq_that_goes_backwards_in_a_session_is_applied_and_said() {
    let mut model = ready();
    model.layout(&layout(9, vec![window("w", "A")]));
    let applied = model.layout(&layout(3, vec![window("w", "B")]));
    assert!(applied.seq_went_back);
    assert_eq!(model.get("w").unwrap().title, "B");
    assert_eq!(model.seq(), Some(3));
    assert!(!model.layout(&layout(4, vec![window("w", "B")])).seq_went_back, "it counts on from there");
}

#[test]
fn a_change_of_screen_or_of_desktop_is_reported_once() {
    let mut model = ready();
    let wider = Layout { screen: Size { w: 3840, h: 1080 }, ..layout(1, vec![]) };
    assert_eq!(model.layout(&wider).changes, [Change::Screen(Size { w: 3840, h: 1080 })]);
    assert!(model.layout(&Layout { seq: 2, ..wider }).changes.is_empty());
    assert_eq!(model.desktop(Desktop::Secure), [Change::Desktop(Desktop::Secure)]);
    assert!(model.desktop(Desktop::Secure).is_empty());
    assert_eq!(model.desktop_now(), Desktop::Secure);
}

#[test]
fn clearing_closes_everything_and_a_reconnect_adds_it_back() {
    let mut model = ready();
    model.layout(&layout(1, vec![window("w", "A"), popup("p", "w")]));
    assert_eq!(ids(&model.clear()), ["-p", "-w"]);
    assert!(model.hello(&hello("one")).is_empty());
    assert_eq!(ids(&model.layout(&layout(1, vec![window("w", "A")])).changes), ["+w"]);
}

#[test]
fn the_shown_cells_are_found() {
    let mut model = ready();
    let focused = Cell { focus: true, ..window("v", "B") };
    let minimized = Cell { state: State::Minimized, ..window("m", "C") };
    model.layout(&layout(1, vec![window("w", "A"), focused, minimized]));
    assert_eq!(model.shown().count(), 2);
}

#[test]
fn a_display_message_sets_the_scale_and_is_a_change_only_when_it_differs() {
    let mut model = ready();
    let display =
        Display { scale: 125, steps: vec![100, 125, 150, 175], need: Some(Size { w: 2560, h: 1440 }) };
    assert_eq!(ids(&model.display(&display)), ["display"]);
    assert_eq!(model.scale_percent(), 125);
    assert_eq!(model.steps(), Some(&[100, 125, 150, 175][..]));
    assert_eq!(model.need(), Some(Size { w: 2560, h: 1440 }));
    assert!(model.display(&display).is_empty(), "the same again changes nothing");
}
