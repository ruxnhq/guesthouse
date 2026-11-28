//! Sync objects. One object can have many handles, and goes away with the last.

use std::collections::{HashMap, VecDeque};

/// What's left of a mutex or semaphore, which engines take and give back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    pub free: u32,
    pub most: u32,
}

/// A sync object's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncObject {
    /// Guest physical address of its counter, or zero if it has none.
    pub fence: u64,
    pub gpu_address: u64,
    /// The guest event to wake instead of counting.
    pub event: Option<u64>,
    pub tokens: Option<Tokens>,
    references: u32,
}

/// Handles to sync objects.
#[derive(Debug, Default)]
pub struct SyncTable {
    /// A closed object's place is empty until a new one takes it.
    objects: Vec<Option<SyncObject>>,
    vacant: Vec<usize>,
    by_handle: HashMap<u32, usize>,
    /// Waits in line for each object that has tokens, by the object's place in `objects`.
    lines: HashMap<usize, VecDeque<u64>>,
    /// The process that made or opened each handle, where the guest said.
    holders: HashMap<u32, u32>,
}

impl SyncTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many places the table has made, used or not.
    #[cfg(test)]
    pub fn places(&self) -> usize {
        self.objects.len()
    }

    /// How many handles there are, which is what a session limits.
    pub fn handles(&self) -> usize {
        self.by_handle.len()
    }

    pub fn insert(&mut self, handle: u32, fence: u64, gpu_address: u64, event: Option<u64>) {
        let object = Some(SyncObject { fence, gpu_address, event, tokens: None, references: 1 });
        let index = match self.vacant.pop() {
            Some(index) => {
                self.objects[index] = object;
                index
            }
            None => {
                self.objects.push(object);
                self.objects.len() - 1
            }
        };
        self.by_handle.insert(handle, index);
    }

    fn object_mut(&mut self, handle: u32) -> Option<&mut SyncObject> {
        let index = *self.by_handle.get(&handle)?;
        self.objects[index].as_mut()
    }

    /// Makes `handle`, which was just inserted, an object that's held and given back.
    pub fn set_tokens(&mut self, handle: u32, free: u32, most: u32) {
        if let Some(object) = self.object_mut(handle) {
            object.tokens = Some(Tokens { free, most });
        }
    }

    /// Puts a wait in line. The guest's engines take a mutex in the order they asked for it: a copy
    /// is queued first and then the encoder that reads what it wrote, and the other way round the
    /// encoder gets an old picture.
    pub fn join_line(&mut self, handle: u32, wait: u64) {
        if let Some(&index) = self.by_handle.get(&handle) {
            self.lines.entry(index).or_default().push_back(wait);
        }
    }

    pub fn leave_line(&mut self, handle: u32, wait: u64) {
        if let Some(line) = self.by_handle.get(&handle).and_then(|index| self.lines.get_mut(index)) {
            line.retain(|&waiting| waiting != wait);
        }
    }

    /// Who is next for this object, if anyone is waiting.
    pub fn next_in_line(&self, handle: u32) -> Option<u64> {
        let line = self.by_handle.get(&handle).and_then(|index| self.lines.get(index))?;
        line.front().copied()
    }

    /// Takes one token, if this object has them. Only for an object that has one free.
    pub fn take_token(&mut self, handle: u32) {
        if let Some(tokens) = self.object_mut(handle).and_then(|object| object.tokens.as_mut()) {
            tokens.free = tokens.free.saturating_sub(1);
        }
    }

    /// Gives one token back. A signal with nothing held changes nothing.
    pub fn give_token(&mut self, handle: u32) {
        if let Some(tokens) = self.object_mut(handle).and_then(|object| object.tokens.as_mut()) {
            tokens.free = tokens.free.saturating_add(1).min(tokens.most);
        }
    }

    pub fn get(&self, handle: u32) -> Option<&SyncObject> {
        self.by_handle.get(&handle).and_then(|&index| self.objects[index].as_ref())
    }

    /// Gives an existing object one more handle. Returns the object, or `None` if `existing` is
    /// unknown.
    pub fn open(&mut self, existing: u32, handle: u32) -> Option<SyncObject> {
        let index = *self.by_handle.get(&existing)?;
        let object = self.objects[index].as_mut()?;
        object.references = object.references.saturating_add(1);
        let object = *object;
        self.by_handle.insert(handle, index);
        Some(object)
    }

    /// Notes the process a handle was made or opened for.
    pub fn note_holder(&mut self, handle: u32, process: u32) {
        if self.by_handle.contains_key(&handle) {
            self.holders.insert(handle, process);
        }
    }

    /// The counter of a monitored fence a process holds a handle to, by the fence's GPU address.
    pub fn monitored_fence(&self, process: u32, gpu_address: u64) -> Option<u64> {
        let held = |handle: &u32| self.holders.get(handle) == Some(&process);
        let handles = self.by_handle.iter().filter(|(handle, _)| held(handle));
        let mut objects = handles.filter_map(|(_, &index)| self.objects[index].as_ref());
        let monitored = |object: &&SyncObject| {
            object.fence != 0
                && object.gpu_address == gpu_address
                && object.event.is_none()
                && object.tokens.is_none()
        };
        objects.find(monitored).map(|object| object.fence)
    }

    /// How many handles to objects with a fence page a process holds.
    pub fn fences_held_by(&self, process: u32) -> usize {
        let held = self.holders.iter().filter(|(_, owner)| **owner == process);
        let index = |handle: &u32| self.by_handle.get(handle).copied();
        let objects = held.filter_map(|(handle, _)| index(handle).and_then(|at| self.objects[at].as_ref()));
        objects.filter(|object| object.fence != 0).count()
    }

    /// Whether a handle names an object here, or a process that holds one.
    pub fn mentions(&self, handle: u32) -> bool {
        self.by_handle.contains_key(&handle)
            || self.holders.contains_key(&handle)
            || self.holders.values().any(|&process| process == handle)
    }

    /// Removes every handle a process held, and returns the objects that were left with none.
    pub fn close_held_by(&mut self, process: u32) -> Vec<SyncObject> {
        let held: Vec<u32> =
            self.holders.iter().filter(|(_, owner)| **owner == process).map(|(&handle, _)| handle).collect();
        held.into_iter().filter_map(|handle| self.close(handle)).collect()
    }

    /// Removes a handle. Returns the object if that was its last.
    pub fn close(&mut self, handle: u32) -> Option<SyncObject> {
        self.holders.remove(&handle);
        let index = self.by_handle.remove(&handle)?;
        let object = self.objects[index].as_mut()?;
        object.references = object.references.saturating_sub(1);
        if object.references > 0 {
            return None;
        }
        let object = self.objects[index].take();
        self.lines.remove(&index);
        self.vacant.push(index);
        object
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_lasts_until_its_last_handle() {
        let mut table = SyncTable::new();
        table.insert(1, 0x1000, 0x2000, None);
        assert_eq!(table.open(1, 2).map(|object| object.fence), Some(0x1000));
        assert_eq!(table.close(1), None);
        assert_eq!(table.get(2).map(|object| object.gpu_address), Some(0x2000));
        assert_eq!(table.close(2).map(|object| object.fence), Some(0x1000));
        assert_eq!(table.get(2), None);
    }

    #[test]
    fn a_line_goes_in_order_and_anyone_can_leave_it() {
        let mut table = SyncTable::new();
        table.insert(1, 0, 0, None);
        table.set_tokens(1, 1, 1);
        assert_eq!(table.next_in_line(1), None);
        table.join_line(1, 7);
        table.join_line(1, 8);
        table.join_line(1, 9);
        table.leave_line(1, 8);
        assert_eq!(table.next_in_line(1), Some(7));
        table.leave_line(1, 7);
        assert_eq!(table.next_in_line(1), Some(9));
    }

    #[test]
    fn tokens_are_shared_by_every_handle_and_stop_at_the_most() {
        let mut table = SyncTable::new();
        table.insert(1, 0, 0, None);
        table.set_tokens(1, 1, 1);
        table.open(1, 2);
        table.take_token(2);
        assert_eq!(table.get(1).unwrap().tokens, Some(Tokens { free: 0, most: 1 }));
        table.give_token(1);
        table.give_token(1);
        assert_eq!(table.get(2).unwrap().tokens, Some(Tokens { free: 1, most: 1 }));
    }

    #[test]
    fn the_count_of_handles_stops_at_its_limit() {
        let mut table = SyncTable::new();
        table.insert(1, 0x1000, 0x2000, None);
        table.objects[0].as_mut().unwrap().references = u32::MAX;
        assert!(table.open(1, 2).is_some());
        assert_eq!(table.objects[0].unwrap().references, u32::MAX);
    }

    #[test]
    fn a_monitored_fence_is_found_only_for_a_process_that_holds_it() {
        let mut table = SyncTable::new();
        table.insert(1, 0x1000, 0x2000, None);
        table.note_holder(1, 0x40);
        assert_eq!(table.monitored_fence(0x40, 0x2000), Some(0x1000));
        assert_eq!(table.monitored_fence(0x40, 0x3000), None, "another address");
        assert_eq!(table.monitored_fence(0x50, 0x2000), None, "a process that holds no handle to it");
        table.open(1, 2);
        table.note_holder(2, 0x50);
        assert_eq!(table.monitored_fence(0x50, 0x2000), Some(0x1000), "opened");
        table.close(2);
        assert_eq!(table.monitored_fence(0x50, 0x2000), None, "closed again");
    }

    #[test]
    fn only_a_counting_fence_is_a_monitored_one() {
        let mut table = SyncTable::new();
        table.insert(1, 0x1000, 0x2000, Some(0x99));
        table.insert(2, 0, 0x3000, None);
        table.insert(3, 0x4000, 0x5000, None);
        table.set_tokens(3, 1, 1);
        for handle in 1..=3 {
            table.note_holder(handle, 0x40);
        }
        for address in [0x2000, 0x3000, 0x5000] {
            assert_eq!(table.monitored_fence(0x40, address), None, "{address:#x}");
        }
        table.note_holder(9, 0x40);
        assert_eq!(table.monitored_fence(0x40, 0), None, "an unknown handle holds nothing");
    }

    #[test]
    fn closed_objects_leave_nothing_behind_and_their_places_are_reused() {
        let mut table = SyncTable::new();
        for round in 0..1000 {
            table.insert(round, 0, 0, None);
            table.set_tokens(round, 1, 1);
            table.join_line(round, 7);
            assert!(table.close(round).is_some());
        }
        assert_eq!((table.objects.len(), table.lines.len(), table.handles()), (1, 0, 0));
        table.insert(5000, 0x10, 0, None);
        assert_eq!(table.next_in_line(5000), None, "a new object does not inherit a line");
        assert_eq!(table.get(5000).unwrap().tokens, None);
    }

    #[test]
    fn a_process_that_ends_gives_up_its_handles() {
        let mut table = SyncTable::new();
        table.insert(1, 0x1000, 0, None);
        table.note_holder(1, 0x40);
        table.open(1, 2);
        table.note_holder(2, 0x50);
        table.insert(3, 0x3000, 0, None);
        table.note_holder(3, 0x40);
        let gone = table.close_held_by(0x40);
        assert_eq!(gone.iter().map(|object| object.fence).collect::<Vec<_>>(), [0x3000]);
        assert_eq!(
            (table.get(1).is_some(), table.get(2).is_some(), table.get(3).is_some()),
            (false, true, false)
        );
    }

    #[test]
    fn unknown_handles_are_harmless() {
        let mut table = SyncTable::new();
        assert_eq!(table.open(9, 10), None);
        assert_eq!(table.close(9), None);
    }
}
