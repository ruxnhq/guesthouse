//! The cells of one machine, as the helper's layouts arrive. Each layout is the whole state, so the
//! model keeps the last one and says what the new one changed: which cells are new, which changed and
//! in what, which are gone. A viewer opens, updates and closes its host windows from those changes
//! and never has to diff two layouts itself.
//!
//! Three things the contract leaves open are decided here.
//!
//! - A new `session` in `hello` is a helper that was started again, and its window handles mean
//!   nothing next to the old ones, which Windows may give to other windows. Every old cell is
//!   reported gone, with [`Change::Restarted`] before them, and the new layout then adds its own
//!   cells, even one with an id that was there before. The same `session` is a reconnect: the cells
//!   stay until the layout that follows `hello` says what became of them.
//! - A `seq` that goes backwards inside a session is applied all the same and reported in
//!   [`Applied::seq_went_back`]. The stream is ordered, so it is not a late line, and a layout is the
//!   whole state: the newest one is the truth, and ignoring it would freeze the windows for as long
//!   as the helper counts wrong. An equal `seq` is what a re-sent layout has, and is applied.
//! - A dropped connection changes nothing here. The windows may well still be there, and it is the
//!   viewer that decides whether to show them dimmed or to close them: [`Model::clear`] is what it
//!   calls to close them.

use super::message::{Cell, Desktop, Hello, Layout, Size, State};

/// How deep a popup of a popup of a popup may go before it is taken for a loop in the helper's data.
const MOST_DEPTH: usize = 8;

/// What differs between two states of one cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fields {
    /// Where the picture is: the host window's size, and what it crops.
    pub cell: bool,
    /// Where the real window is, which is where input goes.
    pub frame: bool,
    pub title: bool,
    pub state: bool,
    pub focus: bool,
    /// Whether it is a popup, whose, and where against it.
    pub popup_link: bool,
    pub caption: bool,
    /// The program, the class, the overflow and resizable marks.
    pub other: bool,
}

impl Fields {
    pub fn any(self) -> bool {
        self != Self::default()
    }

    fn between(old: &Cell, new: &Cell) -> Self {
        Self {
            cell: old.cell != new.cell,
            frame: old.frame != new.frame,
            title: old.title != new.title,
            state: old.state != new.state,
            focus: old.focus != new.focus,
            popup_link: (old.popup, &old.parent, old.rel) != (new.popup, &new.parent, new.rel),
            caption: old.caption != new.caption,
            other: (old.pid, &old.exe, &old.cls, old.overflow, old.resizable)
                != (new.pid, &new.exe, &new.cls, new.overflow, new.resizable),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The helper was started again: what follows are the old cells going, then the new ones.
    Restarted,
    /// A cell that was not there. A window comes before its popups.
    Added(Cell),
    Changed {
        id: String,
        what: Fields,
    },
    /// What the cell last was. A popup comes before the cell it belongs to.
    Removed(Cell),
    Screen(Size),
    Desktop(Desktop),
}

/// What a layout did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Applied {
    pub changes: Vec<Change>,
    pub seq_went_back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    session: Option<String>,
    seq: Option<u64>,
    screen: Size,
    scale: u32,
    desktop: Desktop,
    cells: Vec<Cell>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            session: None,
            seq: None,
            screen: Size::default(),
            scale: 100,
            desktop: Desktop::Normal,
            cells: Vec::new(),
        }
    }
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    /// The helper said hello, on a first connection or on a reconnect.
    pub fn hello(&mut self, hello: &Hello) -> Vec<Change> {
        let mut changes = Vec::new();
        let started_again = self.session.as_deref().is_some_and(|old| old != hello.session);
        if started_again {
            changes.push(Change::Restarted);
            changes.extend(self.clear());
            self.seq = None;
            self.desktop = Desktop::Normal;
        }
        if self.session.as_deref() != Some(hello.session.as_str()) {
            self.session = Some(hello.session.clone());
        }
        if self.screen != hello.screen && self.screen != Size::default() {
            changes.push(Change::Screen(hello.screen));
        }
        self.screen = hello.screen;
        self.scale = hello.scale;
        changes
    }

    pub fn layout(&mut self, layout: &Layout) -> Applied {
        let seq_went_back = self.seq.is_some_and(|last| layout.seq < last);
        self.seq = Some(layout.seq);
        let mut changes = Vec::new();
        if self.screen != layout.screen {
            // The first layout of a model that has had no hello is not a change of screen.
            if self.screen != Size::default() {
                changes.push(Change::Screen(layout.screen));
            }
            self.screen = layout.screen;
        }
        let old = std::mem::take(&mut self.cells);
        let mut gone: Vec<Cell> =
            old.iter().filter(|cell| !layout.cells.iter().any(|new| new.id == cell.id)).cloned().collect();
        gone.sort_by_key(|cell| std::cmp::Reverse(depth(&old, cell)));
        changes.extend(gone.into_iter().map(Change::Removed));
        let mut added: Vec<&Cell> =
            layout.cells.iter().filter(|new| !old.iter().any(|cell| cell.id == new.id)).collect();
        added.sort_by_key(|cell| depth(&layout.cells, cell));
        changes.extend(added.into_iter().cloned().map(Change::Added));
        for new in &layout.cells {
            let Some(before) = old.iter().find(|cell| cell.id == new.id) else { continue };
            let what = Fields::between(before, new);
            if what.any() {
                changes.push(Change::Changed { id: new.id.clone(), what });
            }
        }
        self.cells = layout.cells.clone();
        Applied { changes, seq_went_back }
    }

    /// The input desktop changed: UAC's, the lock screen's, or the user's again.
    pub fn desktop(&mut self, desktop: Desktop) -> Vec<Change> {
        if self.desktop == desktop {
            return Vec::new();
        }
        self.desktop = desktop;
        vec![Change::Desktop(desktop)]
    }

    /// Forgets every cell, as when the viewer has chosen to close its windows after the connection
    /// was lost. The session and the sequence are kept, so a reconnect to the same helper adds them back.
    pub fn clear(&mut self) -> Vec<Change> {
        let all = std::mem::take(&mut self.cells);
        let mut gone = all.clone();
        gone.sort_by_key(|cell| std::cmp::Reverse(depth(&all, cell)));
        gone.into_iter().map(Change::Removed).collect()
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub fn seq(&self) -> Option<u64> {
        self.seq
    }

    pub fn screen(&self) -> Size {
        self.screen
    }

    /// Windows' percentage for its monitor. 100 until a hello says.
    pub fn scale_percent(&self) -> u32 {
        self.scale
    }

    pub fn desktop_now(&self) -> Desktop {
        self.desktop
    }

    /// Every cell, in the order the helper gave them.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn get(&self, id: &str) -> Option<&Cell> {
        self.cells.iter().find(|cell| cell.id == id)
    }

    /// The cells that are not popups: one host toplevel each.
    pub fn windows(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter().filter(|cell| !cell.popup)
    }

    /// The popups that name `id` as their parent, which are the ones a host window of `id` places.
    /// A submenu names the menu it opened from, so it is one of that menu's.
    pub fn popups_of(&self, id: &str) -> impl Iterator<Item = &Cell> {
        self.cells.iter().filter(move |cell| cell.popup && cell.parent == id)
    }

    /// The popups whose parent is not a cell of this layout. A viewer has nothing to place them
    /// against and leaves them out.
    pub fn orphans(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter().filter(|cell| cell.popup && self.get(&cell.parent).is_none())
    }

    /// The window a cell belongs to: itself, or the first cell up its parents that is not a popup.
    pub fn window_of(&self, id: &str) -> Option<&Cell> {
        let mut cell = self.get(id)?;
        for _ in 0..MOST_DEPTH {
            if !cell.popup {
                return Some(cell);
            }
            cell = self.get(&cell.parent)?;
        }
        None
    }

    /// The cell that has the guest's focus.
    pub fn focused(&self) -> Option<&Cell> {
        self.cells.iter().find(|cell| cell.focus)
    }

    /// The cells with something to show: not minimized, not hidden under an overflow cell.
    pub fn shown(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter().filter(|cell| cell.state == State::Shown)
    }
}

/// How many parents a cell has in `cells`, as far as they are there.
fn depth(cells: &[Cell], cell: &Cell) -> usize {
    let mut depth = 0;
    let mut at = cell;
    while at.popup && depth < MOST_DEPTH {
        match cells.iter().find(|other| other.id == at.parent) {
            Some(parent) => at = parent,
            None => break,
        }
        depth += 1;
    }
    depth
}
