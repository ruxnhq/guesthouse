//! A log's lines as the window keeps them: the newest `limit`, each with an ID that stays with it
//! while it's kept, so the window's list can draw only the lines in view and leave a line it has
//! drawn alone while others come and go. The lines are shared, not copied, with what was last
//! drawn, as drawing happens later, from the list, as lines scroll into view. Nothing of Windows is
//! in it, so its tests run on any system.

// Off Windows only the tests use these.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// IDs and revisions both come from here, so no two lines and no two states of any log share one,
/// even a log made afresh in place of another.
static NEXT: AtomicU64 = AtomicU64::new(0);

fn next() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The lines, oldest first, each with its ID.
pub type Lines<T> = Rc<VecDeque<(u64, Rc<T>)>>;

#[derive(Clone, PartialEq, Eq)]
pub struct Log<T> {
    lines: Lines<T>,
    limit: usize,
    revision: u64,
}

impl<T> Log<T> {
    pub fn new(limit: usize) -> Log<T> {
        Log { lines: Rc::default(), limit, revision: next() }
    }

    /// Adds lines at the foot, letting the oldest go past the limit.
    pub fn extend(&mut self, new: impl IntoIterator<Item = T>) {
        let mut new = new.into_iter().peekable();
        if new.peek().is_none() {
            return;
        }
        // A copy of the IDs and pointers only, when the last drawing still holds these.
        let lines = Rc::make_mut(&mut self.lines);
        for line in new {
            if lines.len() == self.limit {
                lines.pop_front();
            }
            lines.push_back((next(), Rc::new(line)));
        }
        self.revision = next();
    }

    pub fn push(&mut self, line: T) {
        self.extend([line]);
    }

    pub fn clear(&mut self) {
        if !self.lines.is_empty() {
            self.lines = Rc::default();
            self.revision = next();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.lines.iter().map(|(_, line)| line.as_ref())
    }

    /// Different after every change, and never the same as any other log's.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The lines as they are now, unchanged by what's added after.
    pub fn lines(&self) -> Lines<T> {
        Rc::clone(&self.lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(log: &Log<u32>) -> Vec<u64> {
        log.lines().iter().map(|(id, _)| *id).collect()
    }

    #[test]
    fn the_oldest_go_past_the_limit() {
        let mut log = Log::new(3);
        log.extend(1..=5);
        assert_eq!(log.iter().copied().collect::<Vec<_>>(), [3, 4, 5]);
        log.push(6);
        assert_eq!(log.iter().copied().collect::<Vec<_>>(), [4, 5, 6]);
    }

    #[test]
    fn a_line_keeps_its_id_while_others_come_and_go() {
        let mut log = Log::new(3);
        log.extend([1, 2, 3]);
        let before = ids(&log);
        log.push(4);
        let after = ids(&log);
        assert_eq!(before[1..], after[..2]);
        assert!(after[2] > before[2]);
    }

    #[test]
    fn ids_are_never_used_twice_even_after_clearing() {
        let mut log = Log::new(10);
        log.extend([1, 2]);
        let first = ids(&log);
        log.clear();
        log.extend([1, 2]);
        let second = ids(&log);
        assert!(first.iter().all(|id| !second.contains(id)));
    }

    #[test]
    fn every_change_has_a_new_revision_and_nothing_else_does() {
        let mut log = Log::new(2);
        let empty = log.revision();
        log.clear();
        log.extend([]);
        assert_eq!(log.revision(), empty);
        log.extend([1, 2]);
        let full = log.revision();
        assert_ne!(full, empty);
        // Still two lines, but other ones.
        log.push(3);
        assert_ne!(log.revision(), full);
        assert_ne!(Log::<u32>::new(2).revision(), log.revision());
    }

    #[test]
    fn what_was_taken_stays_as_it_was() {
        let mut log = Log::new(2);
        log.extend([1, 2]);
        let drawn = log.lines();
        log.push(3);
        assert_eq!(drawn.iter().map(|(_, line)| **line).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(log.iter().copied().collect::<Vec<_>>(), [2, 3]);
    }
}
