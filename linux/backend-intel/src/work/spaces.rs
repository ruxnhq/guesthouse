//! Address spaces: one xe VM for each guest process, with the guest's mappings bound at the
//! guest's own addresses, so every address in its batches works as it is.
//!
//! A process that submits a ring (see [`super::choices::Rings`]) gets a second VM beside the
//! first, a long-running one, holding the same mappings. xe decides at a VM's making whether its
//! work may run without end, so the two kinds of work can't share one.

use super::CardWork;
use super::choices;
use crate::xe::abi::{BindOp, Sync};
use crate::xe::plan::{self, BindVerdict};
use guesthouse_backend::{BackendError, GpuRange, MapRequest, MemoryId, SpaceId, UnmapRequest};
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

/// What is mapped at an address.
#[derive(Debug, Clone, Copy)]
pub(super) struct Mapping {
    pub length: u64,
    pub memory: MemoryId,
    /// Bytes into the memory.
    pub offset: u64,
    /// The bind that made it, kept to make it again in the ring's VM.
    op: BindOp,
}

/// How many changes to a space's mappings are remembered, so that a banned run can be told what
/// moved under it. The oldest go first.
pub(super) const CHANGES_KEPT: usize = 128;

/// One change to a space's mappings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Change {
    /// Counts up with each change to the space, from one.
    pub number: u64,
    pub address: u64,
    pub length: u64,
    pub now_mapped: bool,
    /// What lay behind the memory that was mapped or unmapped.
    pub backing: &'static str,
}

/// A process's long-running VM.
#[derive(Debug)]
pub(super) struct Ring {
    pub vm: u32,
    /// Where xe writes the number of each bind once it is in place. Such a VM signals no sync
    /// object, so this is how a bind is known to be done.
    ///
    /// The word is never freed. xe writes it from a worker of its own some time after the bind
    /// call returns, and nothing says when the last write has landed, not even the VM's end. A
    /// word freed with the ring would be written while something else owned its memory.
    in_place: &'static AtomicU64,
    sent: u64,
}

impl Ring {
    /// Whether every bind sent is in place, so work may be sent after them.
    pub fn binds_in_place(&self) -> bool {
        self.in_place.load(Ordering::Acquire) >= self.sent
    }
}

#[derive(Debug)]
pub(super) struct Space {
    pub vm: u32,
    /// The timeline every bind signals a point of, which work then waits for.
    pub binds: u32,
    pub bind_point: u64,
    pub ring: Option<Ring>,
    ranges: Vec<GpuRange>,
    pub mappings: BTreeMap<u64, Mapping>,
    /// A queue of this process was banned, or its work was lost: nothing more of it runs.
    pub failed: bool,
    /// Its xe address spaces were destroyed, as xe would not take back a mapping.
    gone: bool,
    /// The latest changes to `mappings`, at most [`CHANGES_KEPT`], and how many were ever made.
    pub changes: VecDeque<Change>,
    pub changes_made: u64,
}

impl Space {
    /// How many xe address spaces it holds.
    pub(super) fn xe_vms(&self) -> usize {
        if self.gone { 0 } else { 1 + usize::from(self.ring.is_some()) }
    }

    pub(super) fn note(&mut self, address: u64, length: u64, now_mapped: bool, backing: &'static str) {
        self.changes_made += 1;
        if self.changes.len() == CHANGES_KEPT {
            self.changes.pop_front();
        }
        self.changes.push_back(Change { number: self.changes_made, address, length, now_mapped, backing });
    }

    /// How many bytes lie mapped from an address to the end of its mapping.
    pub fn mapped_from(&self, address: u64) -> Option<u64> {
        let (&start, mapping) = self.mappings.range(..=address).next_back()?;
        (start + mapping.length).checked_sub(address).filter(|&left| left > 0)
    }

    fn overlaps(&self, address: u64, length: u64) -> bool {
        let before = self.mappings.range(..address.saturating_add(length)).next_back();
        before.is_some_and(|(&start, mapping)| start.saturating_add(mapping.length) > address)
    }

    /// The memory holding all of `length` bytes at an address, and the offset into it.
    pub fn holding(&self, address: u64, length: u64) -> Option<(MemoryId, u64)> {
        let (&start, mapping) = self.mappings.range(..=address).next_back()?;
        let end = address.checked_add(length)?;
        (end <= start + mapping.length).then_some((mapping.memory, mapping.offset + (address - start)))
    }
}

/// What the log says of memory that is no longer there to ask.
const GONE: &str = "memory that has gone";

fn failed(what: &str, error: &io::Error) -> BackendError {
    match plan::bind_verdict(error) {
        BindVerdict::NoRoom => BackendError::new(format!("{what}: xe has no room for it ({error})")),
        BindVerdict::Wrong => {
            log::error!(
                "intel work: {what}: xe found the request wrong, which is this backend's fault: {error}"
            );
            BackendError::new(format!("{what}: xe found the request wrong"))
        }
        BindVerdict::Failed => BackendError::new(format!("{what}: {error}")),
    }
}

impl CardWork {
    /// A VM for a guest process. An xe that refuses the scratch page gives one without it, where
    /// a read past a buffer's end costs the process its device, which is better than no work.
    fn create_vm(&mut self) -> io::Result<u32> {
        self.room_for_vm()?;
        match self.xe.create_vm(choices::VM_FLAGS) {
            Err(error) if plan::bind_verdict(&error) == BindVerdict::Wrong => {
                log::warn!(
                    "intel work: xe refused an address space with a scratch page ({error}); made without"
                );
                self.xe.create_vm(choices::VM_FLAGS & !crate::xe::abi::VM_SCRATCH_PAGE)
            }
            made => made,
        }
    }

    pub fn create_space(&mut self, ranges: &[GpuRange]) -> Result<SpaceId, BackendError> {
        let vm = self
            .create_vm()
            .map_err(|error| BackendError::new(format!("xe made no address space: {error}")))?;
        let binds = match self.xe.create_syncobj() {
            Ok(binds) => binds,
            Err(error) => {
                let _ = self.xe.destroy_vm(vm);
                return Err(BackendError::new(format!(
                    "xe made no sync object for an address space: {error}"
                )));
            }
        };
        let space = SpaceId(self.new_id());
        let ranges = ranges.to_vec();
        self.spaces.insert(
            space,
            Space {
                vm,
                binds,
                bind_point: 0,
                ring: None,
                ranges,
                mappings: BTreeMap::new(),
                failed: false,
                gone: false,
                changes: VecDeque::new(),
                changes_made: 0,
            },
        );
        Ok(space)
    }

    /// Destroys a space and whatever of xe's still runs in it, the copy engine's queue among them.
    pub fn destroy_space(&mut self, space: SpaceId) {
        if self.copy_engine_in(space) {
            self.close_copy_engine();
        }
        let queues: Vec<u32> =
            self.queues.iter().filter(|(_, queue)| queue.space == space).map(|(id, _)| *id).collect();
        for queue in queues {
            self.close_queue(queue);
        }
        let Some(gone) = self.spaces.remove(&space) else { return };
        let vms = if gone.gone { [None, None] } else { [Some(gone.vm), gone.ring.map(|ring| ring.vm)] };
        for vm in vms.into_iter().flatten() {
            if let Err(error) = self.xe.destroy_vm(vm) {
                log::warn!("intel work: destroying an address space failed: {error}");
            }
        }
        if let Err(error) = self.xe.destroy_syncobj(gone.binds) {
            log::warn!("intel work: destroying an address space's sync object failed: {error}");
        }
    }

    /// Sends one change to a space's mappings: to its VM, signalling the next point of its bind
    /// timeline, and to its ring VM if it has one.
    fn bind(&mut self, space_id: SpaceId, op: &BindOp) -> io::Result<()> {
        self.bind_in_vm(space_id, op)?;
        let space = self.spaces.get_mut(&space_id).ok_or(io::ErrorKind::NotFound)?;
        if let Some(ring) = space.ring.as_mut() {
            Self::bind_in_ring(self.xe.as_mut(), ring, op)?;
        }
        Ok(())
    }

    /// Sends one change to a space's own VM alone.
    fn bind_in_vm(&mut self, space_id: SpaceId, op: &BindOp) -> io::Result<()> {
        let space = self.spaces.get_mut(&space_id).ok_or(io::ErrorKind::NotFound)?;
        if space.gone {
            return Err(io::ErrorKind::NotFound.into());
        }
        let point = space.bind_point + 1;
        self.xe.bind(space.vm, op, Some(Sync::Signal { timeline: space.binds, point }))?;
        space.bind_point = point;
        Ok(())
    }

    /// Ends what a space's work can reach, as xe did not take a mapping back: its queues go, its
    /// address spaces are destroyed, and nothing of it runs again. Its books are emptied, as no
    /// address is bound any more.
    fn lose_bindings(&mut self, space_id: SpaceId, why: &str) {
        let queues: Vec<u32> =
            self.queues.iter().filter(|(_, queue)| queue.space == space_id).map(|(id, _)| *id).collect();
        for queue in queues {
            self.close_queue(queue);
        }
        let Some(space) = self.spaces.get_mut(&space_id) else { return };
        if space.gone {
            return;
        }
        space.gone = true;
        let vms = [Some(space.vm), space.ring.take().map(|ring| ring.vm)];
        let mapped: Vec<(u64, Mapping)> = std::mem::take(&mut space.mappings).into_iter().collect();
        for (start, mapping) in mapped {
            let backing = self.blocks.get(&mapping.memory).map_or(GONE, |block| block.backing.kind());
            if let Some(space) = self.spaces.get_mut(&space_id) {
                space.note(start, mapping.length, false, backing);
            }
        }
        for vm in vms.into_iter().flatten() {
            if let Err(error) = self.xe.destroy_vm(vm) {
                log::warn!("intel work: destroying an address space failed: {error}");
            }
        }
        log::warn!("intel work: space {space_id:?} lost its address spaces: {why}");
        self.fail_space(space_id, why);
    }

    fn bind_in_ring(xe: &mut dyn crate::xe::device::Xe, ring: &mut Ring, op: &BindOp) -> io::Result<()> {
        let number = ring.sent + 1;
        let address = std::ptr::from_ref::<AtomicU64>(ring.in_place) as u64;
        xe.bind(ring.vm, op, Some(Sync::UserFence { address, value: number }))?;
        ring.sent = number;
        Ok(())
    }

    /// Maps memory at the guest's address. Fails if the addresses are taken, as the trait asks.
    pub fn map(&mut self, request: &MapRequest) -> Result<(), BackendError> {
        let Some(block) = self.blocks.get(&request.memory) else {
            return Err(BackendError::new("mapping memory that does not exist"));
        };
        let Some(space) = self.spaces.get(&request.space) else {
            return Err(BackendError::new("this address space is closed"));
        };
        if !space.ranges.iter().any(|range| range.contains(request.address)) {
            return Err(BackendError::new("no range holds this address"));
        }
        if space.overlaps(request.address, request.length) {
            return Err(BackendError::new("these addresses are taken"));
        }
        let backing = block.backing.kind();
        let op = plan::bind_for(request, &block.backing).map_err(|refused| {
            BackendError::new(format!("a mapping at {:#x}: {refused}", request.address))
        })?;
        self.bind_in_vm(request.space, &op).map_err(|error| failed("a mapping", &error))?;
        let ring_bound = self.spaces.get_mut(&request.space).and_then(|space| {
            space.ring.as_mut().map(|ring| Self::bind_in_ring(self.xe.as_mut(), ring, &op))
        });
        if let Some(Err(error)) = ring_bound {
            let undo = UnmapRequest {
                space: request.space,
                memory: request.memory,
                address: request.address,
                length: request.length,
            };
            if let Err(undo_error) = self.bind_in_vm(request.space, &plan::unbind_for(&undo)) {
                self.lose_bindings(
                    request.space,
                    &format!("a mapping could not be taken back: {undo_error}"),
                );
            }
            return Err(failed("a mapping in a long-running address space", &error));
        }
        let mapping = Mapping { length: request.length, memory: request.memory, offset: request.offset, op };
        let space = self.spaces.get_mut(&request.space).expect("found above");
        space.mappings.insert(request.address, mapping);
        space.note(request.address, request.length, true, backing);
        Ok(())
    }

    pub fn unmap(&mut self, unmap: &UnmapRequest) -> Result<(), BackendError> {
        if !self.blocks.contains_key(&unmap.memory) {
            return Err(BackendError::new("unmapping memory that does not exist"));
        }
        let Some(space) = self.spaces.get_mut(&unmap.space) else { return Ok(()) };
        let end = unmap.address.saturating_add(unmap.length);
        let removed: Vec<(u64, Mapping)> = space
            .mappings
            .iter()
            .filter(|&(&start, mapping)| start >= unmap.address && start + mapping.length <= end)
            .map(|(&start, &mapping)| (start, mapping))
            .collect();
        if removed.is_empty() || unmap.length == 0 {
            return Ok(());
        }
        if let Err(error) = self.bind(unmap.space, &plan::unbind_for(unmap)) {
            self.lose_bindings(
                unmap.space,
                &format!("an unmapping at {:#x} was refused: {error}", unmap.address),
            );
            return Err(failed("an unmapping", &error));
        }
        self.forget(unmap.space, &removed);
        Ok(())
    }

    /// Takes mappings out of a space's books, once xe has unbound them.
    fn forget(&mut self, space: SpaceId, removed: &[(u64, Mapping)]) {
        for (start, mapping) in removed {
            let backing = self.blocks.get(&mapping.memory).map_or(GONE, |block| block.backing.kind());
            if let Some(space) = self.spaces.get_mut(&space) {
                space.mappings.remove(start);
                space.note(*start, mapping.length, false, backing);
            }
        }
    }

    /// Takes a block out of every space it is mapped in.
    pub(super) fn unmap_everywhere(&mut self, memory: MemoryId) {
        let mapped: Vec<UnmapRequest> = self
            .spaces
            .iter()
            .flat_map(|(&space, held)| {
                held.mappings.iter().filter(move |(_, mapping)| mapping.memory == memory).map(
                    move |(&address, mapping)| UnmapRequest {
                        space,
                        memory,
                        address,
                        length: mapping.length,
                    },
                )
            })
            .collect();
        for unmap in mapped {
            let Some(mapping) =
                self.spaces.get(&unmap.space).and_then(|held| held.mappings.get(&unmap.address))
            else {
                continue;
            };
            let mapping = *mapping;
            match self.bind(unmap.space, &plan::unbind_for(&unmap)) {
                Ok(()) => self.forget(unmap.space, &[(unmap.address, mapping)]),
                Err(error) => {
                    log::warn!("intel work: unmapping freed memory at {:#x} failed: {error}", unmap.address);
                    self.lose_bindings(unmap.space, "freed memory could not be unmapped");
                }
            }
        }
    }

    /// A space's long-running VM, made on its first use with every mapping the space has.
    pub(super) fn ring_vm(&mut self, space_id: SpaceId) -> Result<u32, BackendError> {
        let space =
            self.spaces.get_mut(&space_id).ok_or(BackendError::new("this address space is closed"))?;
        if let Some(ring) = &space.ring {
            return Ok(ring.vm);
        }
        let vm = self
            .xe
            .create_vm(choices::RING_VM_FLAGS)
            .map_err(|error| BackendError::new(format!("xe made no long-running address space: {error}")))?;
        let mut ring = Ring { vm, in_place: Box::leak(Box::new(AtomicU64::new(0))), sent: 0 };
        for mapping in space.mappings.values() {
            if let Err(error) = Self::bind_in_ring(self.xe.as_mut(), &mut ring, &mapping.op) {
                let _ = self.xe.destroy_vm(vm);
                return Err(failed("a mapping in a long-running address space", &error));
            }
        }
        space.ring = Some(ring);
        Ok(vm)
    }

    /// Whether a space's work has been given up.
    pub fn space_failed(&self, space: SpaceId) -> bool {
        self.spaces.get(&space).is_none_or(|space| space.failed)
    }

    pub(super) fn fail_space(&mut self, space: SpaceId, why: &str) {
        if let Some(found) = self.spaces.get_mut(&space)
            && !found.failed
        {
            found.failed = true;
            self.faults.push_back(space);
            log::warn!(
                "intel work: space {space:?} runs nothing more: {why}. Its guest process loses its device; \
                 the others go on"
            );
        }
    }

    pub fn live_spaces(&self) -> usize {
        self.spaces.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::Stage;
    use crate::work::testing::{self, Pages, map_request};
    use crate::xe::abi;
    use crate::xe::fake::Call;
    use crate::xe::plan::{BufferUse, pat};
    use guesthouse_backend::GPU_RANGES;

    #[test]
    fn a_space_is_a_vm_with_a_scratch_page_and_goes_whole() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let space = work.create_space(&GPU_RANGES).unwrap();
        assert_eq!(xe.calls()[0], Call::CreateVm { flags: abi::VM_SCRATCH_PAGE });
        assert_eq!((work.live_spaces(), xe.live()), (1, 2), "the VM and its bind timeline");
        work.destroy_space(space);
        assert_eq!((work.live_spaces(), xe.live()), (0, 0));
        assert!(work.space_failed(space), "a space that is gone runs nothing");
    }

    #[test]
    fn an_xe_that_refuses_the_scratch_page_gives_a_space_without_one() {
        let (mut work, xe) = testing::work(Stage::Compute);
        xe.fail_call(xe.calls().len(), libc::EINVAL);
        work.create_space(&GPU_RANGES).unwrap();
        let made: Vec<_> =
            xe.calls().into_iter().filter(|call| matches!(call, Call::CreateVm { .. })).collect();
        assert_eq!(made, [Call::CreateVm { flags: abi::VM_SCRATCH_PAGE }, Call::CreateVm { flags: 0 }]);
        assert_eq!(work.live_spaces(), 1);
    }

    #[test]
    fn a_space_xe_refuses_leaves_nothing_behind() {
        let (mut work, xe) = testing::work(Stage::Compute);
        xe.fail("create_syncobj", libc::ENOMEM);
        assert!(work.create_space(&GPU_RANGES).is_err());
        assert_eq!(xe.live(), 0);
    }

    #[test]
    fn video_memory_and_guest_pages_are_bound_at_the_guests_addresses() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (video, _) = work.allocate(0x4000, BufferUse::Card).unwrap();
        let pages = Pages::new(0x2000);
        let guest = work.adopt(pages.address(), 0x2000).unwrap();
        work.map(&map_request(space, video, 0x1_4000_0000, 0x1000, 0x2000)).unwrap();
        work.map(&map_request(space, guest, 0x400_0000, 0, 0x2000)).unwrap();
        let binds = xe.calls_named("bind");
        let Call::Bind { op: first, signal: first_signal, .. } = &binds[0] else { panic!() };
        assert_eq!((first.op, first.address, first.length), (abi::BIND_MAP, 0x1_4000_0000, 0x2000));
        assert_eq!((first.offset_or_host_address, first.pat_index), (0x1000, pat::CARD));
        let Call::Bind { op: second, signal: second_signal, .. } = &binds[1] else { panic!() };
        assert_eq!((second.op, second.address), (abi::BIND_MAP_USERPTR, 0x400_0000));
        assert_eq!(
            (second.offset_or_host_address, second.pat_index),
            (pages.address(), pat::SHARED_WITH_CPU)
        );
        // Each bind signals the next point of the space's timeline, for work to wait on.
        let point = |signal: &Option<Sync>| match signal {
            Some(Sync::Signal { point, .. }) => *point,
            other => panic!("{other:?}"),
        };
        assert_eq!((point(first_signal), point(second_signal)), (1, 2));
    }

    #[test]
    fn taken_addresses_and_addresses_outside_the_ranges_are_refused_before_xe() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let space = work.create_space(&[GpuRange { start: 0x1000, end: 0x10_0000 }]).unwrap();
        let (memory, _) = work.allocate(0x4000, BufferUse::Card).unwrap();
        work.map(&map_request(space, memory, 0x2000, 0, 0x2000)).unwrap();
        assert!(work.map(&map_request(space, memory, 0x3000, 0, 0x1000)).unwrap_err().0.contains("taken"));
        assert!(work.map(&map_request(space, memory, 0x10_0000, 0, 0x1000)).is_err());
        assert!(work.map(&map_request(space, memory, 0x8800, 0, 0x1000)).unwrap_err().0.contains("page"));
        assert!(work.map(&map_request(space, MemoryId(99), 0x8000, 0, 0x1000)).is_err());
        assert!(work.map(&map_request(SpaceId(99), memory, 0x8000, 0, 0x1000)).is_err());
        assert_eq!(xe.calls_named("bind").len(), 1);
        let unmap = UnmapRequest { space, memory, address: 0x2000, length: 0x2000 };
        work.unmap(&unmap).unwrap();
        work.map(&map_request(space, memory, 0x3000, 0, 0x1000)).expect("free again once unmapped");
        let Some(Call::Bind { op, .. }) = xe.calls_named("bind").get(1).cloned() else { panic!() };
        assert_eq!((op.op, op.address, op.length), (abi::BIND_UNMAP, 0x2000, 0x2000));
    }

    #[test]
    fn a_bind_xe_has_no_room_for_fails_the_map_and_the_addresses_stay_free() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (memory, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
        xe.fail("bind", libc::ENOMEM);
        let refused = work.map(&map_request(space, memory, 0x400_0000, 0, 0x1000)).unwrap_err();
        assert!(refused.0.contains("no room"), "{refused}");
        xe.heal();
        work.map(&map_request(space, memory, 0x400_0000, 0, 0x1000)).unwrap();
    }

    #[test]
    fn freeing_memory_unbinds_it_everywhere_first() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let (first, second) =
            (work.create_space(&GPU_RANGES).unwrap(), work.create_space(&GPU_RANGES).unwrap());
        let (memory, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
        work.map(&map_request(first, memory, 0x400_0000, 0, 0x1000)).unwrap();
        work.map(&map_request(second, memory, 0x500_0000, 0, 0x1000)).unwrap();
        work.release(memory);
        let calls = xe.calls();
        let unbinds = calls
            .iter()
            .filter(|call| matches!(call, Call::Bind { op, .. } if op.op == abi::BIND_UNMAP))
            .count();
        assert_eq!(unbinds, 2);
        assert!(matches!(calls.last(), Some(Call::CloseBuffer(_))), "the buffer goes last");
        let (other, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
        work.map(&map_request(first, other, 0x400_0000, 0, 0x1000)).expect("the addresses are free");
    }

    #[test]
    fn a_ring_vm_is_long_running_and_gets_every_mapping_old_and_new() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (memory, _) = work.allocate(0x2000, BufferUse::Card).unwrap();
        work.map(&map_request(space, memory, 0x400_0000, 0, 0x1000)).unwrap();
        let ring = work.ring_vm(space).unwrap();
        assert_eq!(xe.vm_flags(ring), Some(abi::VM_LONG_RUNNING));
        assert!(xe.is_bound(ring, 0x400_0000), "what was mapped before it was made");
        work.map(&map_request(space, memory, 0x500_0000, 0x1000, 0x1000)).unwrap();
        assert!(xe.is_bound(ring, 0x500_0000), "and what is mapped after");
        assert_eq!(work.ring_vm(space).unwrap(), ring, "one for the space");
        assert!(work.spaces[&space].ring.as_ref().unwrap().binds_in_place());
        let unmap = UnmapRequest { space, memory, address: 0x400_0000, length: 0x1000 };
        work.unmap(&unmap).unwrap();
        assert!(!xe.is_bound(ring, 0x400_0000));
        work.destroy_space(space);
        assert_eq!(xe.live(), 1, "only the buffer is left");
    }

    #[test]
    fn a_bind_xe_finishes_after_the_space_is_gone_writes_a_word_that_is_still_there() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (memory, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
        work.map(&map_request(space, memory, 0x400_0000, 0, 0x1000)).unwrap();
        work.ring_vm(space).unwrap();
        let word = xe
            .calls_named("bind")
            .into_iter()
            .find_map(|call| match call {
                Call::Bind { signal: Some(Sync::UserFence { address, .. }), .. } => Some(address),
                _ => None,
            })
            .expect("the ring's bind names a word");
        work.destroy_space(space);
        // SAFETY: the word is never freed, which is what this test is for.
        let word = unsafe { &*(word as *const AtomicU64) };
        word.store(7, Ordering::Release);
        assert_eq!(word.load(Ordering::Acquire), 7);
    }
}
