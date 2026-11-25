//! What the viewer does about what the helper and the user say, as plain functions: which host window
//! a change opens, updates or closes, which request an event gives, and where a point of a host
//! surface goes in the guest. No toolkit in here, so it is all tested without a display.

use guesthouse_app_core::app_windows::{
    Cell, Change, Desktop, Fields, Model, Point, Request, Size, State, Surface, guest_point,
};
use guesthouse_app_core::input::TABLET_RANGE;
use std::collections::BTreeMap;

/// What a cell is shown as on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hosting {
    /// A toplevel of its own.
    Window,
    /// A popup surface against its parent's.
    Popup,
    /// Nothing: it is hidden under an overflow cell, or its parent is not shown.
    Not,
}

/// How `cell` is shown, given whether the cell it names as its parent is shown (for a popup).
pub fn hosting(cell: &Cell, parent_hosted: bool) -> Hosting {
    match (cell.popup, cell.state) {
        (false, State::Shown | State::Minimized) => Hosting::Window,
        (true, State::Shown) if parent_hosted => Hosting::Popup,
        _ => Hosting::Not,
    }
}

/// One thing for the viewer to do to its host windows. Steps are in the order the changes came, so a
/// restarted helper's reused id is closed and then opened again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The helper started again: every host window goes, and the ones that follow are new.
    Restart,
    Open(String),
    /// With the popups that hang on it.
    Close(String),
    Update {
        id: String,
        what: Fields,
    },
    Screen(Size),
    Desktop(Desktop),
}

/// The steps for `changes`, which the model has taken already. `hosted` is what the viewer shows now:
/// id to whether it is a popup. It is brought up to date as the steps are made.
pub fn steps(changes: &[Change], model: &Model, hosted: &mut BTreeMap<String, bool>) -> Vec<Step> {
    let mut steps = Vec::new();
    for change in changes {
        match change {
            Change::Restarted => {
                hosted.clear();
                steps.push(Step::Restart);
            }
            Change::Added(cell) => open_if_shown(&cell.id, model, hosted, &mut steps),
            Change::Changed { id, what } => {
                let Some(cell) = model.get(id) else { continue };
                let parent_hosted = hosted.contains_key(&cell.parent);
                let wanted = hosting(cell, parent_hosted);
                let there = hosted.contains_key(id);
                match (there, wanted) {
                    (false, Hosting::Not) => {}
                    (false, _) => open_if_shown(id, model, hosted, &mut steps),
                    (true, Hosting::Not) => close(id, hosted, &mut steps),
                    (true, wanted) => {
                        // A window that became a popup or the other way is another kind of host window.
                        if hosted.get(id).copied() != Some(wanted == Hosting::Popup) {
                            close(id, hosted, &mut steps);
                            open_if_shown(id, model, hosted, &mut steps);
                        } else if what.any() {
                            steps.push(Step::Update { id: id.clone(), what: *what });
                        }
                    }
                }
            }
            Change::Removed(cell) => {
                if hosted.contains_key(&cell.id) {
                    close(&cell.id, hosted, &mut steps);
                }
            }
            Change::Screen(size) => steps.push(Step::Screen(*size)),
            Change::Desktop(desktop) => steps.push(Step::Desktop(*desktop)),
        }
    }
    steps
}

fn close(id: &str, hosted: &mut BTreeMap<String, bool>, steps: &mut Vec<Step>) {
    hosted.remove(id);
    steps.push(Step::Close(id.to_owned()));
}

/// Opens the cell if the model has it and it is to be shown, and then its popups that were waiting
/// for it.
fn open_if_shown(id: &str, model: &Model, hosted: &mut BTreeMap<String, bool>, steps: &mut Vec<Step>) {
    let Some(cell) = model.get(id) else { return };
    let kind = hosting(cell, hosted.contains_key(&cell.parent));
    if kind == Hosting::Not || hosted.contains_key(id) {
        return;
    }
    hosted.insert(id.to_owned(), kind == Hosting::Popup);
    steps.push(Step::Open(id.to_owned()));
    let waiting: Vec<String> = model.popups_of(id).map(|popup| popup.id.clone()).collect();
    for popup in waiting {
        open_if_shown(&popup, model, hosted, steps);
    }
}

/// Where the host window is as far as the guest's state of the window goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Up,
    /// Asked to minimize, and not yet seen to go out of sight.
    Minimizing,
    /// Minimized for the guest. Coming back means the user brought it back.
    Minimized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Nothing,
    Minimize,
    /// Bring the host window back, because the guest's came back without it.
    Present,
}

/// What the guest's window being in `state` means for a host window that is `presence`.
pub fn follow_guest_state(presence: Presence, state: State) -> (Presence, Order) {
    match (presence, state) {
        (Presence::Up, State::Minimized) => (Presence::Minimizing, Order::Minimize),
        (Presence::Minimizing | Presence::Minimized, State::Shown) => (Presence::Up, Order::Present),
        (presence, _) => (presence, Order::Nothing),
    }
}

/// What the host window's activity and visibility mean. The second says whether the user brought a
/// minimized window back, which the guest's has to follow.
pub fn follow_host_state(presence: Presence, active: bool, suspended: bool) -> (Presence, bool) {
    match presence {
        Presence::Minimizing if !active || suspended => (Presence::Minimized, false),
        Presence::Minimized if active && !suspended => (Presence::Up, true),
        presence => (presence, false),
    }
}

/// The request that tells the guest a host window got the focus, if it has not been told already.
/// Never for a popup: a change of the foreground window closes the menu the user is looking at.
pub fn focus_request(already: Option<&str>, cell: &Cell) -> Option<Request> {
    (!cell.popup && already != Some(cell.id.as_str())).then(|| Request::Focus { id: cell.id.clone() })
}

/// The guest's point for a point of a host surface. `offset` is how far the part of the cell that is
/// on the screen starts inside the cell, which is nothing unless the cell hangs over the screen's top
/// or left edge.
pub fn point_for(cell: &Cell, surface: Surface, offset: Point, x: f64, y: f64, screen: Size) -> Point {
    guest_point(cell, surface, x + f64::from(offset.x), y + f64::from(offset.y), screen)
}

/// The tablet's value for a pixel of the guest's screen: the middle of the pixel, so that the guest,
/// which takes the whole part, lands on it.
pub fn tablet_value(pixel: i32, side: i32) -> i64 {
    ((f64::from(pixel) + 0.5) * TABLET_RANGE as f64 / f64::from(side.max(1))) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_app_core::app_windows::{Caption, Hello, Layout, Rect};

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    fn window(id: &str, at: Rect) -> Cell {
        Cell {
            id: id.into(),
            pid: 1,
            exe: "x.exe".into(),
            cls: "X".into(),
            title: id.into(),
            cell: at,
            frame: at,
            popup: false,
            parent: String::new(),
            rel: Point::default(),
            state: State::Shown,
            overflow: false,
            focus: false,
            caption: Caption { h: 32, buttons_w: 138 },
            resizable: true,
        }
    }

    fn popup(id: &str, parent: &str, at: Rect) -> Cell {
        Cell { popup: true, parent: parent.into(), rel: Point { x: 10, y: 40 }, ..window(id, at) }
    }

    struct Rig {
        model: Model,
        seq: u64,
        hosted: BTreeMap<String, bool>,
    }

    impl Rig {
        fn new() -> Self {
            let mut model = Model::new();
            model.hello(&Hello { session: "one".into(), screen: Size { w: 1920, h: 1080 }, scale: 100 });
            Self { model, seq: 0, hosted: BTreeMap::new() }
        }

        fn show(&mut self, cells: Vec<Cell>) -> Vec<Step> {
            self.seq += 1;
            let layout = Layout { seq: self.seq, screen: Size { w: 1920, h: 1080 }, cells };
            let applied = self.model.layout(&layout);
            steps(&applied.changes, &self.model, &mut self.hosted)
        }
    }

    fn open(id: &str) -> Step {
        Step::Open(id.into())
    }

    fn close_of(id: &str) -> Step {
        Step::Close(id.into())
    }

    #[test]
    fn a_window_opens_with_its_popup_after_it() {
        let mut rig = Rig::new();
        let steps = rig.show(vec![popup("p", "w", rect(5, 5, 50, 50)), window("w", rect(0, 0, 200, 100))]);
        assert_eq!(steps, vec![open("w"), open("p")]);
        assert_eq!(rig.hosted.get("w"), Some(&false));
        assert_eq!(rig.hosted.get("p"), Some(&true));
    }

    #[test]
    fn a_popup_of_a_popup_is_hosted_against_the_popup() {
        let mut rig = Rig::new();
        let steps = rig.show(vec![
            window("w", rect(0, 0, 200, 100)),
            popup("m", "w", rect(5, 5, 50, 50)),
            popup("s", "m", rect(60, 5, 50, 50)),
        ]);
        assert_eq!(steps, vec![open("w"), open("m"), open("s")]);
    }

    #[test]
    fn a_popup_whose_window_is_not_shown_is_not_either() {
        let mut rig = Rig::new();
        let covered = Cell { state: State::Covered, ..window("w", rect(0, 0, 200, 100)) };
        assert!(rig.show(vec![covered, popup("p", "w", rect(5, 5, 50, 50))]).is_empty());
    }

    #[test]
    fn a_moved_cell_and_a_new_title_are_one_update() {
        let mut rig = Rig::new();
        rig.show(vec![window("w", rect(0, 0, 200, 100))]);
        let moved = Cell { title: "new".into(), ..window("w", rect(300, 0, 250, 100)) };
        let steps = rig.show(vec![moved]);
        let [Step::Update { id, what }] = &steps[..] else { panic!("{steps:?}") };
        assert_eq!(id, "w");
        assert!(what.cell && what.title && !what.state);
    }

    #[test]
    fn a_minimized_window_is_kept_and_a_covered_one_goes() {
        let mut rig = Rig::new();
        rig.show(vec![window("w", rect(0, 0, 200, 100))]);
        let minimized = Cell { state: State::Minimized, ..window("w", rect(0, 0, 200, 100)) };
        let steps = rig.show(vec![minimized]);
        assert!(matches!(&steps[..], [Step::Update { what, .. }] if what.state));
        let covered = Cell { state: State::Covered, ..window("w", rect(0, 0, 200, 100)) };
        assert_eq!(rig.show(vec![covered.clone()]), vec![close_of("w")]);
        // And it is opened again when the overflow cell lets go of it.
        assert_eq!(rig.show(vec![window("w", rect(0, 0, 200, 100))]), vec![open("w")]);
    }

    #[test]
    fn a_window_that_comes_back_brings_its_popups_that_waited() {
        let mut rig = Rig::new();
        let covered = Cell { state: State::Covered, ..window("w", rect(0, 0, 200, 100)) };
        rig.show(vec![covered, popup("p", "w", rect(5, 5, 50, 50))]);
        let steps = rig.show(vec![window("w", rect(0, 0, 200, 100)), popup("p", "w", rect(5, 5, 50, 50))]);
        assert_eq!(steps, vec![open("w"), open("p")]);
    }

    #[test]
    fn a_closed_popup_goes_before_its_window() {
        let mut rig = Rig::new();
        rig.show(vec![window("w", rect(0, 0, 200, 100)), popup("p", "w", rect(5, 5, 50, 50))]);
        assert_eq!(rig.show(vec![window("w", rect(0, 0, 200, 100))]), vec![close_of("p")]);
        assert_eq!(rig.show(vec![]), vec![close_of("w")]);
        assert!(rig.hosted.is_empty());
    }

    #[test]
    fn a_restarted_helper_closes_everything_and_opens_its_own_even_under_an_old_id() {
        let mut rig = Rig::new();
        rig.show(vec![window("w", rect(0, 0, 200, 100))]);
        let changes =
            rig.model.hello(&Hello { session: "two".into(), screen: Size { w: 1920, h: 1080 }, scale: 100 });
        let mut steps = steps(&changes, &rig.model, &mut rig.hosted);
        let layout = Layout {
            seq: 1,
            screen: Size { w: 1920, h: 1080 },
            cells: vec![window("w", rect(9, 9, 80, 80))],
        };
        let applied = rig.model.layout(&layout);
        steps.extend(super::steps(&applied.changes, &rig.model, &mut rig.hosted));
        // The old windows went with the restart, so nothing is closed one by one.
        assert_eq!(steps, vec![Step::Restart, open("w")]);
    }

    #[test]
    fn the_guests_minimize_and_the_users_restore_follow_each_other() {
        let (presence, order) = follow_guest_state(Presence::Up, State::Minimized);
        assert_eq!((presence, order), (Presence::Minimizing, Order::Minimize));
        // The host window goes out of sight, then the user brings it back.
        let (presence, restored) = follow_host_state(presence, false, true);
        assert_eq!((presence, restored), (Presence::Minimized, false));
        let (presence, restored) = follow_host_state(presence, true, false);
        assert_eq!((presence, restored), (Presence::Up, true));
        // The guest's state says shown after the restore: nothing more to do.
        assert_eq!(follow_guest_state(presence, State::Shown), (Presence::Up, Order::Nothing));
        // The guest's window came back by itself while the host's was minimized.
        let (presence, order) = follow_guest_state(Presence::Minimized, State::Shown);
        assert_eq!((presence, order), (Presence::Up, Order::Present));
    }

    #[test]
    fn a_window_asked_to_minimize_that_is_not_active_still_counts_once_it_is_out_of_sight() {
        assert_eq!(follow_host_state(Presence::Minimizing, true, false), (Presence::Minimizing, false));
        assert_eq!(follow_host_state(Presence::Minimizing, false, true), (Presence::Minimized, false));
        assert_eq!(follow_host_state(Presence::Up, false, false), (Presence::Up, false));
    }

    #[test]
    fn the_focus_is_sent_once_and_never_for_a_popup() {
        let w = window("w", rect(0, 0, 10, 10));
        assert_eq!(focus_request(None, &w), Some(Request::Focus { id: "w".into() }));
        assert_eq!(focus_request(Some("w"), &w), None);
        assert_eq!(focus_request(Some("other"), &w), Some(Request::Focus { id: "w".into() }));
        assert_eq!(focus_request(None, &popup("p", "w", rect(0, 0, 10, 10))), None);
    }

    #[test]
    fn a_point_is_the_frames_origin_plus_the_point_and_a_cell_over_the_edge_adds_its_offset() {
        let screen = Size { w: 1920, h: 1080 };
        let menu = Cell { frame: rect(105, 113, 190, 280), ..window("m", rect(1000, 100, 190, 280)) };
        let at = point_for(&menu, Surface::of_cell(&menu), Point::default(), 7.9, 31.2, screen);
        assert_eq!(at, Point { x: 112, y: 144 });
        let over = window("o", rect(-20, -10, 200, 100));
        let at = point_for(&over, Surface::of_cell(&over), Point { x: 20, y: 10 }, 5.0, 5.0, screen);
        assert_eq!(at, Point { x: 5, y: 5 });
    }

    #[test]
    fn the_tablet_lands_on_the_pixel_it_was_meant_for() {
        for (pixel, side) in [(0, 1920), (1, 1920), (959, 1920), (1919, 1920), (500, 1080), (1079, 1080)] {
            let value = tablet_value(pixel, side);
            assert_eq!(
                (value as f64 * f64::from(side) / TABLET_RANGE as f64) as i32,
                pixel,
                "{pixel}/{side}"
            );
            assert!(value <= TABLET_RANGE);
        }
    }

    #[test]
    fn a_layout_from_the_fake_helper_opens_windows_and_a_request_reaches_it() {
        use guesthouse_app_core::app_windows::stand_in::FakeHelper;
        use guesthouse_app_core::app_windows::{Client, LinkConfig, Timing, Update};
        use std::time::{Duration, Instant};
        let helper = FakeHelper::start("tok");
        helper.with(|inner| {
            inner.layout.cells = vec![window("w", rect(0, 0, 200, 100)), popup("p", "w", rect(5, 5, 50, 50))];
        });
        let mut client = Client::start(LinkConfig {
            port: helper.port,
            token: "tok".into(),
            starter: None,
            timing: Timing::default(),
        });
        let mut hosted = BTreeMap::new();
        let mut found = Vec::new();
        let until = Instant::now() + Duration::from_secs(3);
        while found.len() < 2 && Instant::now() < until {
            for update in client.poll() {
                if let Update::Changes(changes) = update {
                    found.extend(steps(&changes, client.model(), &mut hosted));
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let opened: Vec<_> = found.iter().filter(|step| matches!(step, Step::Open(_))).cloned().collect();
        assert_eq!(opened, vec![open("w"), open("p")]);
        let cell = client.model().get("w").unwrap().clone();
        let request = focus_request(None, &cell).unwrap();
        client.request(request);
        assert!(
            helper.wait_until(|inner| inner.requests.iter().any(|r| r["t"] == "focus" && r["id"] == "w"))
        );
    }
}
