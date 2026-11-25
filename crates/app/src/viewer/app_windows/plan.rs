//! What the viewer does about what the helper and the user say, as plain functions: which host window
//! a change opens, updates or closes, which request an event gives, and where a point of a host
//! surface goes in the guest. No toolkit in here, so it is all tested without a display.

use guesthouse_app_core::app_windows::{
    Cell, Change, Desktop, Fields, Model, Point, Request, Size, State, Surface, guest_point, nearest_step,
};
use guesthouse_app_core::input::TABLET_RANGE;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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
            // The hub reads Windows' scale and the screen it needs from the model after each look.
            Change::Display(_) => {}
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

/// What the guest is to be told of a change of the host window's visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostChange {
    Nothing,
    /// The user brought a minimized window back.
    Restore,
    /// The user minimized the window: the compositor took it out of sight with nothing asked of it.
    Minimize,
}

/// What the host window's activity and visibility mean. A window that goes out of sight without the
/// guest having asked is the user's minimize, and the guest's window follows it. One the guest asked to
/// minimize (`Minimizing`) is not told back, which keeps the two from answering each other for ever.
pub fn follow_host_state(presence: Presence, active: bool, suspended: bool) -> (Presence, HostChange) {
    match presence {
        Presence::Minimizing if !active || suspended => (Presence::Minimized, HostChange::Nothing),
        Presence::Minimized if active && !suspended => (Presence::Up, HostChange::Restore),
        Presence::Up if suspended && !active => (Presence::Minimized, HostChange::Minimize),
        presence => (presence, HostChange::Nothing),
    }
}

/// The request that tells the guest a host window got the focus, if it has not been told already.
/// Never for a popup: a change of the foreground window closes the menu the user is looking at.
pub fn focus_request(already: Option<&str>, cell: &Cell) -> Option<Request> {
    (!cell.popup && already != Some(cell.id.as_str())).then(|| Request::Focus { id: cell.id.clone() })
}

/// How long the focused window must stay on a monitor that asks for another step before Windows is
/// asked for it. Every change makes each program in the guest lay itself out again.
pub const SCALE_STILL: Duration = Duration::from_millis(1000);
/// How long an asked step that Windows has not taken is left before it is asked for again.
pub const SCALE_ASK_AGAIN: Duration = Duration::from_secs(10);

/// Windows' scale follows the monitor of the host window that has the focus: it has one scale for its
/// one screen, and the monitor the user works on is the one it should be right for. The other monitors
/// show their windows at the size Windows means, scaled by the compositor, until one of their windows
/// gets the focus.
#[derive(Debug, Default)]
pub struct ScaleFollow {
    /// The step the focused window's monitor asks for, and since when.
    wanted: Option<(u32, Instant)>,
    /// The step last asked for, and when.
    asked: Option<(u32, Instant)>,
}

impl ScaleFollow {
    /// The step to ask Windows for now, if any. `focus_scale` is the scale of the surface of the window
    /// that has the focus (`None` when no host window has it, which leaves Windows as it is), `guest` is
    /// Windows' percentage now and `offered` the steps it offers (empty: the helper takes no scale).
    pub fn look(
        &mut self,
        focus_scale: Option<f64>,
        guest: u32,
        offered: &[u32],
        now: Instant,
    ) -> Option<u32> {
        let step = focus_scale.and_then(|scale| nearest_step(scale, offered))?;
        if self.wanted.is_none_or(|(wanted, _)| wanted != step) {
            self.wanted = Some((step, now));
        }
        let (_, since) = self.wanted?;
        if step == guest || now.duration_since(since) < SCALE_STILL {
            return None;
        }
        if self.asked.is_some_and(|(asked, at)| asked == step && now.duration_since(at) < SCALE_ASK_AGAIN) {
            return None;
        }
        self.asked = Some((step, now));
        Some(step)
    }
}

/// How long the screen the guest needs must stay the same before a larger one is set: a window being
/// dragged larger in the guest asks for many.
pub const GROW_STILL: Duration = Duration::from_millis(500);
/// And before a smaller one is: each new size takes Windows' monitor away for a moment and lays every
/// window out again, so a window closed and opened again soon should not cost two.
pub const SHRINK_STILL: Duration = Duration::from_secs(5);
/// A size asked for that the guest has not taken is asked for again after this.
pub const SIZE_ASK_AGAIN: Duration = Duration::from_secs(15);

/// Nobody picks the guest's screen: it is what the helper says its windows need, which it works out
/// between the floor and the ceiling the host gave it.
#[derive(Debug, Default)]
pub struct SizeFollow {
    seen: Option<(Size, Instant)>,
    asked: Option<(Size, Instant)>,
}

impl SizeFollow {
    /// The size to set now, if any, given what the helper needs and the screen it has.
    pub fn look(&mut self, need: Option<Size>, screen: Size, now: Instant) -> Option<Size> {
        let need = need.filter(|need| need.w > 0 && need.h > 0)?;
        if self.seen.is_none_or(|(seen, _)| seen != need) {
            self.seen = Some((need, now));
        }
        let (_, since) = self.seen?;
        if need == screen {
            return None;
        }
        if self.asked.is_some_and(|(asked, at)| asked == need && now.duration_since(at) < SIZE_ASK_AGAIN) {
            return None;
        }
        let wait = if need.w > screen.w || need.h > screen.h { GROW_STILL } else { SHRINK_STILL };
        if now.duration_since(since) < wait {
            return None;
        }
        self.asked = Some((need, now));
        Some(need)
    }
}

/// The guest point for a point of a host surface. `offset` is how far the part of the cell that is
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

    #[test]
    fn windows_follows_the_focused_monitor_once_it_has_stayed() {
        let steps = [100, 125, 150, 175, 200];
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut follow = ScaleFollow::default();
        assert_eq!(follow.look(Some(1.5), 100, &steps, at(0)), None, "just seen");
        assert_eq!(follow.look(Some(1.5), 100, &steps, at(900)), None, "not still long enough");
        assert_eq!(follow.look(Some(1.5), 100, &steps, at(1000)), Some(150));
        assert_eq!(follow.look(Some(1.5), 100, &steps, at(1300)), None, "asked once");
        assert_eq!(follow.look(Some(1.5), 150, &steps, at(1500)), None, "taken");
        assert_eq!(follow.look(Some(1.3), 150, &steps, at(2000)), None);
        assert_eq!(follow.look(Some(1.3), 150, &steps, at(3000)), Some(125), "the nearest step");
        assert_eq!(follow.look(Some(1.3), 150, &steps, at(14000)), Some(125), "asked again when not taken");
    }

    #[test]
    fn windows_is_left_alone_with_no_focus_or_no_steps() {
        let start = Instant::now();
        let later = start + Duration::from_secs(5);
        let mut follow = ScaleFollow::default();
        assert_eq!(follow.look(None, 100, &[100, 125], start), None);
        assert_eq!(follow.look(None, 100, &[100, 125], later), None);
        assert_eq!(follow.look(Some(2.0), 100, &[], start), None);
        assert_eq!(follow.look(Some(2.0), 100, &[], later), None);
        let mut follow = ScaleFollow::default();
        assert_eq!(follow.look(Some(1.5), 100, &[100, 125, 150], start), None);
        assert_eq!(follow.look(Some(1.0), 100, &[100, 125, 150], start + Duration::from_millis(800)), None);
        assert_eq!(
            follow.look(Some(1.5), 100, &[100, 125, 150], start + Duration::from_millis(1600)),
            None,
            "a focus that went back and forth starts again"
        );
    }

    #[test]
    fn the_screen_grows_soon_and_shrinks_late_to_what_the_windows_need() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let small = Size { w: 1920, h: 1080 };
        let large = Size { w: 3200, h: 1080 };
        let mut follow = SizeFollow::default();
        assert_eq!(follow.look(None, small, at(0)), None, "the helper has not said");
        assert_eq!(follow.look(Some(small), small, at(0)), None, "it has what it needs");
        assert_eq!(follow.look(Some(large), small, at(100)), None, "just seen");
        assert_eq!(follow.look(Some(large), small, at(600)), Some(large), "grows after half a second");
        assert_eq!(follow.look(Some(large), small, at(700)), None, "asked once");
        assert_eq!(follow.look(Some(small), large, at(1000)), None);
        assert_eq!(follow.look(Some(small), large, at(4000)), None, "shrinks only after five seconds");
        assert_eq!(follow.look(Some(small), large, at(6000)), Some(small));
        assert_eq!(follow.look(Some(small), large, at(22000)), Some(small), "asked again when not taken");
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    fn window(id: &str, at: Rect) -> Cell {
        Cell {
            id: id.into(),
            pid: 1,
            exe: "x.exe".into(),
            app: String::new(),
            name: String::new(),
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
            corner: 0,
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
        let (presence, change) = follow_host_state(presence, false, true);
        assert_eq!((presence, change), (Presence::Minimized, HostChange::Nothing));
        let (presence, change) = follow_host_state(presence, true, false);
        assert_eq!((presence, change), (Presence::Up, HostChange::Restore));
        // The guest's state says shown after the restore: nothing more to do.
        assert_eq!(follow_guest_state(presence, State::Shown), (Presence::Up, Order::Nothing));
        // The guest's window came back by itself while the host's was minimized.
        let (presence, order) = follow_guest_state(Presence::Minimized, State::Shown);
        assert_eq!((presence, order), (Presence::Up, Order::Present));
    }

    #[test]
    fn a_window_asked_to_minimize_that_is_not_active_still_counts_once_it_is_out_of_sight() {
        assert_eq!(
            follow_host_state(Presence::Minimizing, true, false),
            (Presence::Minimizing, HostChange::Nothing)
        );
        assert_eq!(
            follow_host_state(Presence::Minimizing, false, true),
            (Presence::Minimized, HostChange::Nothing)
        );
        assert_eq!(follow_host_state(Presence::Up, false, false), (Presence::Up, HostChange::Nothing));
    }

    #[test]
    fn the_users_minimize_is_sent_to_the_guest_once_and_does_not_come_back() {
        let (presence, change) = follow_host_state(Presence::Up, false, true);
        assert_eq!((presence, change), (Presence::Minimized, HostChange::Minimize));
        // The guest's state says minimized now: the host window is already where it is told to be.
        assert_eq!(follow_guest_state(presence, State::Minimized), (Presence::Minimized, Order::Nothing));
        // Still out of sight, still nothing to send.
        assert_eq!(follow_host_state(presence, false, true), (Presence::Minimized, HostChange::Nothing));
        let (presence, change) = follow_host_state(presence, true, false);
        assert_eq!((presence, change), (Presence::Up, HostChange::Restore));
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
            side: false,
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
