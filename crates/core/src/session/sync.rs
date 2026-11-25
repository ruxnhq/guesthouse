//! Handlers for sync objects and fences.

use super::{Handled, Session, answer_status};
use crate::deferred::DeferredAction;
use crate::fences::{FENCE_PAGE, Fence, FenceBlock, FencePiece, piece_size};
use crate::sync_table::SyncObject;
use guesthouse_backend::{
    Backend, BackendError, GuestMemory, MapMemoryType, MapRequest, SpaceId, UnmapRequest,
};
use guesthouse_messages::notice::Notice;
use guesthouse_messages::sync::{
    CreateSyncObject, KIND_CPU_NOTIFICATION, KIND_MONITORED_FENCE, SignalFence, first_handle, open_answer,
    opened_sync_object, paging_queue_answer,
};
use guesthouse_messages::wire::{Malformed, put_u32};
use guesthouse_messages::{MessageHeader, Status};

/// The mapping of a piece of the fences' block, which starts at `gpu_address`, in `space`.
fn fence_mapping(space: SpaceId, gpu_address: u64, piece: FencePiece) -> MapRequest {
    MapRequest {
        space,
        memory: piece.memory,
        offset: 0,
        length: piece.length,
        address: gpu_address + piece.offset,
        small_pages: true,
        gpu_cached: false,
        readable: true,
        writable: true,
        executable: false,
        memory_type: MapMemoryType::Default,
        driver_protection: 0,
    }
}

fn fence_unmapping(space: SpaceId, gpu_address: u64, piece: FencePiece) -> UnmapRequest {
    UnmapRequest { space, memory: piece.memory, address: gpu_address + piece.offset, length: piece.length }
}

impl<B: Backend, M: GuestMemory> Session<B, M> {
    /// The GPU address of the fences' block, which is made the first time something needs it. Its
    /// first piece is adopted with it, which for most backends is all of it.
    pub(crate) fn fence_block(&mut self) -> Result<u64, BackendError> {
        if let Some(block) = &self.fences.block {
            return Ok(block.gpu_address);
        }
        let start = self.placed_io_space()?.start();
        let piece = piece_size(self.backend.fence_piece());
        // The whole area has to be there, whichever of its pieces come to be used.
        let host_address = self.memory.host_address(start, crate::FENCE_AREA)?;
        let memory = self.backend.adopt_host_pages(host_address, piece)?;
        let gpu_address = self.take_gpu_addresses(crate::FENCE_AREA, crate::ALLOCATION_ALIGN)?;
        let mut block = FenceBlock::new(gpu_address, piece);
        block.adopted(0, memory);
        self.fences.block = Some(block);
        Ok(gpu_address)
    }

    /// Maps the fence pieces the GPU reaches into a new address space, which gets the later ones as
    /// they come.
    pub(crate) fn map_fences(&mut self, space: SpaceId) -> Result<(), BackendError> {
        let gpu_address = self.fence_block()?;
        let Some(block) = &self.fences.block else { return Ok(()) };
        for piece in block.reached() {
            self.backend.map(&fence_mapping(space, gpu_address, piece))?;
        }
        if let Some(block) = &mut self.fences.block {
            block.spaces.push(space);
        }
        Ok(())
    }

    /// Takes the fences out of an address space that is closing.
    pub(crate) fn unmap_fences(&mut self, space: SpaceId) {
        let Some(block) = &mut self.fences.block else { return };
        let Some(at) = block.spaces.iter().position(|&mapped| mapped == space) else { return };
        block.spaces.remove(at);
        let gpu_address = block.gpu_address;
        for piece in block.reached() {
            if let Err(error) = self.backend.unmap(&fence_unmapping(space, gpu_address, piece)) {
                log::warn!("closing an address space: {error}");
            }
        }
    }

    /// Has the GPU reach the fence at `offset` into the area, in every address space: adopts the
    /// piece that holds it and maps that everywhere, unless it is there already. A piece that
    /// can't go everywhere goes nowhere.
    fn reach_fence(&mut self, offset: u64) -> Result<(), BackendError> {
        let gpu_address = self.fence_block()?;
        let Some(block) = &self.fences.block else { return Ok(()) };
        let Some((piece_offset, length)) = block.missing_piece(offset) else { return Ok(()) };
        let spaces = block.spaces.clone();
        let start = self.placed_io_space()?.start();
        let host_address = self.memory.host_address(start + piece_offset, length)?;
        let memory = self.backend.adopt_host_pages(host_address, length)?;
        let piece = FencePiece { offset: piece_offset, length, memory };
        for (mapped, &space) in spaces.iter().enumerate() {
            let Err(error) = self.backend.map(&fence_mapping(space, gpu_address, piece)) else { continue };
            for &space in &spaces[..mapped] {
                if let Err(error) = self.backend.unmap(&fence_unmapping(space, gpu_address, piece)) {
                    log::warn!("taking back a piece of the fences: {error}");
                }
            }
            self.backend.release_memory(memory);
            return Err(error);
        }
        if let Some(block) = &mut self.fences.block {
            block.adopted(piece_offset, memory);
        }
        Ok(())
    }

    fn fresh_fence_page(&mut self) -> Result<u64, BackendError> {
        let start = self.placed_io_space()?.start();
        Ok(start + self.fences.fresh_page()?)
    }

    /// A zeroed fence page, reusing freed ones.
    pub(crate) fn take_fence(&mut self) -> Result<Fence, BackendError> {
        if let Some(fence) = self.fences.reuse() {
            self.memory.zero(fence.0, FENCE_PAGE)?;
            return Ok(fence);
        }
        let fence = self.fresh_fence_page()?;
        let offset = fence - self.placed_io_space()?.start();
        match self.fence_block().and_then(|block| self.reach_fence(offset).map(|()| block)) {
            Ok(gpu_address) => Ok((fence, gpu_address + offset)),
            Err(error) => {
                self.fences.unused_fresh_page(offset);
                Err(error)
            }
        }
    }

    pub(super) fn create_paging_queue(&mut self, out: &mut Vec<u8>) -> Handled {
        let queue = self.handles.take();
        let sync = self.handles.take();
        let fence = match self.fences.reuse_paging() {
            Some(fence) => {
                self.memory.zero(fence, FENCE_PAGE)?;
                fence
            }
            None => self.fresh_fence_page()?,
        };
        self.paging_queues.insert(queue, fence);
        paging_queue_answer(queue, sync, fence, out);
        Ok(())
    }

    pub(super) fn destroy_paging_queue(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        if let Some(fence) = self.paging_queues.remove(&first_handle(message)?) {
            self.fences.give_back_paging(fence);
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(super) fn create_sync_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = CreateSyncObject::parse(message)?;
        let handle = self.handles.take();
        let monitored = request.kind == KIND_MONITORED_FENCE;
        let (fence, gpu_address) = if monitored { self.take_fence()? } else { (0, 0) };
        let event = if request.kind == KIND_CPU_NOTIFICATION {
            Some(request.initial.ok_or(Malformed)?)
        } else {
            None
        };
        self.sync_objects.insert(handle, fence, gpu_address, event);
        self.sync_objects.note_holder(handle, MessageHeader::process_of(message)?);
        if let Some((free, most)) = request.tokens() {
            self.sync_objects.set_tokens(handle, free, most);
        }
        if monitored {
            // Some swapchains start their fences above zero, and wait for that before any signal.
            self.memory.write_u64(fence, request.initial.ok_or(Malformed)?)?;
        }
        CreateSyncObject::answer(handle, gpu_address, fence, out);
        Ok(())
    }

    pub(super) fn open_sync_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let existing = opened_sync_object(message)?;
        if self.sync_objects.get(existing).is_none() {
            open_answer(0, Status::INVALID_PARAMETER, 0, 0, out);
            return Ok(());
        }
        let handle = self.handles.take();
        let object = self.sync_objects.open(existing, handle).expect("checked just above");
        self.sync_objects.note_holder(handle, MessageHeader::process_of(message)?);
        open_answer(handle, Status::SUCCESS, object.gpu_address, object.fence, out);
        Ok(())
    }

    pub(super) fn destroy_sync_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let closed = self.sync_objects.close(first_handle(message)?);
        if let Some(object) = closed.filter(|object| object.fence != 0) {
            self.defer(DeferredAction::ReturnFence((object.fence, object.gpu_address)));
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(super) fn create_shared_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let object = first_handle(message)?;
        let handle = self.handles.take();
        self.shared_objects.insert(handle, object);
        put_u32(out, handle);
        put_u32(out, 0);
        Ok(())
    }

    pub(super) fn destroy_shared_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        self.shared_objects.remove(&first_handle(message)?);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    fn resolve_shared(&self, handle: u32) -> u32 {
        self.shared_objects.get(&handle).copied().unwrap_or(handle)
    }

    /// Raises a fence, wakes a notification's event, or gives back what a wait took. Fences never go
    /// backwards.
    pub(crate) fn write_fence(&mut self, handle: u32, value: u64) {
        let handle = self.resolve_shared(handle);
        let Some(&object) = self.sync_objects.get(handle) else {
            return;
        };
        if object.tokens.is_some() {
            self.sync_objects.give_token(handle);
        } else if let Some(event) = object.event {
            self.notices.push(Notice::WakeEvent { event, release: 0 });
        } else if object.fence != 0 && value > self.fence_value(object.fence) {
            self.store_fence(object.fence, value);
        }
    }

    /// Whether a fence handle is one this session tracks. One it doesn't know counts as reached.
    pub(crate) fn fence_known(&self, handle: u32) -> bool {
        self.sync_objects.get(self.resolve_shared(handle)).is_some()
            || self.queue_progress.contains_key(&handle)
    }

    pub(crate) fn fence_reached(&self, handle: u32, value: u64) -> bool {
        let fence = match self.sync_objects.get(self.resolve_shared(handle)) {
            Some(SyncObject { tokens: Some(tokens), .. }) => {
                return tokens.free > 0
                    && self.sync_objects.next_in_line(self.resolve_shared(handle)).is_none();
            }
            Some(object) => object.fence,
            None => self.queue_progress.get(&handle).copied().unwrap_or(0),
        };
        fence == 0 || self.fence_value(fence) >= value
    }

    /// Takes what a finished wait holds, for those objects that are held. A wait for any one of them
    /// takes only the one that let it through.
    pub(crate) fn take_tokens(&mut self, signals: &[(u32, u64)], wake_on_any: bool) {
        let free = |handle: &u32| {
            let object = self.sync_objects.get(*handle);
            object.is_some_and(|object| object.tokens.is_some_and(|tokens| tokens.free > 0))
        };
        let handles = signals.iter().map(|&(handle, _)| self.resolve_shared(handle));
        let free: Vec<u32> = handles.filter(free).collect();
        let taken = if wake_on_any { &free[..free.len().min(1)] } else { &free };
        taken.iter().for_each(|&handle| self.sync_objects.take_token(handle));
    }

    /// Puts a gate's wait in line for each object that has tokens.
    pub(crate) fn join_lines(&mut self, signals: &[(u32, u64)], wait: u64) {
        for &(handle, _) in signals {
            self.sync_objects.join_line(self.resolve_shared(handle), wait);
        }
    }

    pub(crate) fn leave_lines(&mut self, signals: &[(u32, u64)], wait: u64) {
        for &(handle, _) in signals {
            self.sync_objects.leave_line(self.resolve_shared(handle), wait);
        }
    }

    /// Whether a wait that's in line can go: it's first for everything held, and fences have moved.
    pub(crate) fn wait_can_go(&self, signals: &[(u32, u64)], wait: u64) -> bool {
        signals.iter().all(|&(handle, value)| {
            let handle = self.resolve_shared(handle);
            match self.sync_objects.get(handle).and_then(|object| object.tokens) {
                Some(tokens) => tokens.free > 0 && self.sync_objects.next_in_line(handle) == Some(wait),
                None => self.fence_reached(handle, value),
            }
        })
    }

    pub(super) fn signal_fence(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let signal = SignalFence::parse(message)?;
        self.write_fence(signal.handle, signal.value);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }
}
