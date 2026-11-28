//! Handlers for processes, contexts, queues and work.

use super::waiting::GpuTook;
use super::{Failure, Handled, Session, answer_status};
use crate::deferred::DeferredAction;
use crate::pending::GateStep;
use crate::tables::{Queue, ReservedRange};
use guesthouse_backend::{Backend, CommandBuffer, GuestMemory, SpaceId, UnmapRequest, WorkSource};
use guesthouse_messages::queue::{
    CreateContext, CreateHardwareQueue, DEFAULT_QUEUED_PRESENT_LIMIT, QueuedLimit, SubmitToContext,
    SubmitToQueue,
};
use guesthouse_messages::sync::first_handle;
use guesthouse_messages::wire::{Malformed, put_u32, u8_at, u32_at};
use guesthouse_messages::{MessageHeader, Status};

/// Where a new process's PID in the guest is, where its flags are, and the flag for a 32-bit one.
const PROCESS_ID: usize = 0x20;
const PROCESS_FLAGS: usize = 0x232;
const PROCESS_32_BIT: u8 = 4;
/// Between the PID and the flags, the process's name as UTF-16 ending in a zero, when the guest
/// gives one: the 261 units of MAX_PATH and its zero.
const PROCESS_NAME: std::ops::Range<usize> = 0x28..PROCESS_FLAGS;

/// The program a new process runs, by its file name alone: the folders before it can name the
/// guest's user, and the log has no use for them. Empty when the guest sent no name.
fn program_name(message: &[u8]) -> String {
    let field = message.get(PROCESS_NAME).unwrap_or_default();
    let units: Vec<u16> = field
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    let path = String::from_utf16_lossy(&units);
    path.rsplit(['\\', '/']).next().unwrap_or_default().to_owned()
}

impl<B: Backend, M: GuestMemory> Session<B, M> {
    pub(super) fn create_process(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        if self.guest_pids.len() >= super::limits::MOST_PROCESSES {
            return Err(
                guesthouse_backend::BackendError::new("this guest has as many processes as it may").into()
            );
        }
        let handle = self.take_handle()?;
        if u8_at(message, PROCESS_FLAGS)? & PROCESS_32_BIT != 0 {
            self.processes_32_bit.insert(handle);
        }
        let pid = u32_at(message, PROCESS_ID)?;
        self.guest_pids.insert(handle, pid);
        // So a later line about this process can be tied to a program in the guest. One line a
        // process, at info level, as the other lines name processes by handle alone.
        match program_name(message) {
            name if name.is_empty() => {
                log::info!("process {handle:#x} is guest pid {pid}, which gave no name")
            }
            name => {
                log::info!("process {handle:#x} is guest pid {pid}, {name}");
                self.programs.insert(handle, name);
            }
        }
        put_u32(out, handle);
        Ok(())
    }

    /// The address space closes at the next chance to run deferred work, not now.
    pub(super) fn destroy_process(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let process = MessageHeader::process_of(message)?;
        self.end_process(process);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    /// Forgets a process, and closes its address space once nothing waits for it.
    fn end_process(&mut self, process: u32) {
        self.processes_32_bit.remove(&process);
        self.guest_pids.remove(&process);
        self.programs.remove(&process);
        self.process_going(process);
        self.context_processes.retain(|_, owner| *owner != process);
        self.reservations.retain(|&(owner, _), _| owner != process);
        self.described_surfaces.remove(&process);
        for object in self.sync_objects.close_held_by(process).into_iter().filter(|object| object.fence != 0)
        {
            self.defer(DeferredAction::ReturnFence((object.fence, object.gpu_address)));
        }
        if self.newest_description.is_some_and(|(owner, _, _)| owner == process) {
            self.newest_description = None;
        }
        self.surface_range_sizes.retain(|&(owner, _), _| owner != process);
        self.backend.process_gone(process);
        if let Some(space) = self.spaces.remove(&process) {
            let action = DeferredAction::CloseSpace(space);
            let runs = self.runs_on_card();
            self.deferred.push(crate::deferred::Deferred { waits: Vec::new(), runs, action });
        }
    }

    /// A guest that restarted without saying goodbye destroys nothing, so what its last start had
    /// would stay until the machine stops: the card's memory, and pins on guest pages the new start
    /// uses again. It all goes here, the way a guest's own destroys would take it: the allocations
    /// once the GPU is done with them, then the processes and their address spaces. One a copy still
    /// uses stays, as a destroy would wait for that copy and the guest won't send one again.
    pub(super) fn end_the_last_start(&mut self) {
        let mut handles: Vec<u32> =
            self.allocations.keys().copied().filter(|&handle| !self.copy_uses(handle)).collect();
        handles.sort_unstable();
        let mut processes: Vec<u32> =
            self.spaces.keys().chain(self.guest_pids.keys()).chain(self.programs.keys()).copied().collect();
        processes.sort_unstable();
        processes.dedup();
        if handles.is_empty() && processes.is_empty() {
            return;
        }
        log::info!(
            "the guest restarted: freeing {} allocations and {} processes of its last start",
            handles.len(),
            processes.len()
        );
        self.run_deferred();
        self.free_allocations(handles);
        self.resources.clear();
        self.global_shares.clear();
        self.resource_users.clear();
        self.shared_objects.clear();
        processes.into_iter().for_each(|process| self.end_process(process));
        self.run_deferred();
    }

    /// Frees an address space, and anything still running or mapped there.
    pub(crate) fn close_space(&mut self, space: SpaceId) {
        let mut owned: Vec<u32> =
            self.queues.iter().filter(|(_, queue)| queue.space == space).map(|(&handle, _)| handle).collect();
        // Oldest first, as each gives back a fence and the order decides who gets which one next.
        owned.sort_unstable();
        owned.into_iter().for_each(|queue| self.close_queue(queue));
        self.failed_queues.remove(&space);
        // Oldest first again, as each gives back addresses.
        let mut mapped: Vec<(u64, u32)> = self
            .allocations
            .iter()
            .filter(|(_, allocation)| allocation.mappings.iter().any(|mapping| mapping.space == space))
            .filter_map(|(&handle, allocation)| Some((allocation.first_mapped?, handle)))
            .collect();
        mapped.sort_unstable();
        for (_, handle) in mapped {
            self.close_mappings(handle, space);
        }
        let (closing, kept) =
            std::mem::take(&mut self.reserved_addresses).into_iter().partition(|range| range.space == space);
        self.reserved_addresses = kept;
        let closing: Vec<ReservedRange> = closing;
        closing.into_iter().for_each(|range| self.give_back_reserved(range));
        self.closed_spaces.insert(space);
        if let Some(block) = self.fences.block {
            let unmap = UnmapRequest {
                space,
                memory: block.memory,
                address: block.gpu_address,
                length: crate::FENCE_AREA,
            };
            if let Err(error) = self.backend.unmap(&unmap) {
                log::warn!("closing an address space: {error}");
            }
        }
        self.backend.destroy_address_space(space);
    }

    /// Removes an allocation's mappings in one address space.
    fn close_mappings(&mut self, handle: u32, space: SpaceId) {
        let allocation = self.allocations.get_mut(&handle).expect("the caller found it");
        let Some(memory) = allocation.memory else { return };
        let system_memory = allocation.in_system_memory();
        let (closing, kept) = std::mem::take(&mut allocation.mappings)
            .into_iter()
            .partition(|mapping: &crate::tables::Mapping| mapping.space == space);
        allocation.mappings = kept;
        for mapping in closing {
            let unmap = UnmapRequest { space, memory, address: mapping.address, length: mapping.length };
            if let Err(error) = self.backend.unmap(&unmap) {
                log::warn!("closing an address space: {error}");
            }
            self.give_back_mapping(mapping.address, system_memory);
        }
    }

    pub(super) fn create_context(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = CreateContext::parse(message)?;
        // Video work sent to a copy channel would be a bad push buffer on the card the host's desktop
        // uses, so it stops here. A handle of zero fails the context, and the program can fall back.
        if !self.backend.context_allowed(request.node, request.client_hint) {
            log::info!(
                "refused a context on node {}, which has no channel here (hint {:#x})",
                request.node,
                request.client_hint
            );
            request.answer(message, 0, out);
            return Ok(());
        }
        let process = MessageHeader::process_of(message)?;
        let private = message.get(CreateContext::PRIVATE..).unwrap_or_default();
        let private = &private[..request.private_size.min(private.len())];
        if !self.backend.context_data_valid(process, request.node, private) {
            log::info!(
                "refused a context on node {} whose private data the vendor's driver wouldn't take",
                request.node
            );
            request.answer(message, 0, out);
            return Ok(());
        }
        if self.context_processes.len() >= super::limits::MOST_CONTEXTS {
            log::warn!("refused a context, as this guest has {} already", super::limits::MOST_CONTEXTS);
            request.answer(message, 0, out);
            return Ok(());
        }
        let handle = self.take_handle()?;
        log::debug!("context {handle:#x} on node {}, hint {:#x}", request.node, request.client_hint);
        self.context_nodes.insert(handle, request.node);
        self.context_hints.insert(handle, request.client_hint);
        self.context_processes.insert(handle, process);
        self.backend.context_made(process, request.node);
        let answer_at = out.len();
        request.answer(message, handle, out);
        if let Some(private) = out.get_mut(answer_at + CreateContext::PRIVATE..) {
            self.backend.finish_context_data(request.node, private);
        }
        Ok(())
    }

    /// A device's limit on queued presents. A set is kept for that device, and a read answers it, or
    /// the default until the device has set one.
    pub(super) fn queued_limit(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = QueuedLimit::parse(message)?;
        let room = self.present_limits.len() < super::limits::MOST_KEYED;
        if request.is_set() && (room || self.present_limits.contains_key(&request.device)) {
            self.present_limits.insert(request.device, request.limit);
        }
        let limit = self.present_limits.get(&request.device).copied().unwrap_or(DEFAULT_QUEUED_PRESENT_LIMIT);
        request.answer(message, limit, out);
        Ok(())
    }

    /// The device's limit goes with it. A message too short to name one has nothing to forget.
    pub(super) fn destroy_device(&mut self, message: &[u8], out: &mut Vec<u8>) {
        if let Ok(device) = first_handle(message) {
            self.present_limits.remove(&device);
            self.devices.remove(&device);
        }
        answer_status(Status::SUCCESS, out);
    }

    /// An app that exits destroys its contexts and never names their queues.
    pub(super) fn destroy_context(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let context = first_handle(message)?;
        for queue in self.context_queues.get(&context).cloned().unwrap_or_default() {
            self.close_queue(queue);
        }
        self.context_queues.remove(&context);
        let node = self.context_nodes.remove(&context);
        self.context_hints.remove(&context);
        if let (Some(node), Some(process)) = (node, self.context_processes.remove(&context)) {
            self.backend.context_gone(process, node);
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(super) fn create_hardware_queue(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = CreateHardwareQueue::parse(message)?;
        request.begin_answer(message, out);
        let process = MessageHeader::process_of(message)?;
        if self.fence_share_taken(process) {
            return Err(
                guesthouse_backend::BackendError::new("this process holds its share of fence pages").into()
            );
        }
        let queue = self.take_handle()?;
        let progress = self.take_handle()?;
        let (fence, fence_gpu_address) = self.take_fence()?;
        let space = self.space_for(process)?;
        self.queues.insert(queue, Queue::new(fence, fence_gpu_address, space, request.context));
        self.context_queues.entry(request.context).or_default().push(queue);
        self.queue_progress.insert(progress, fence);
        self.queues_made = self.queues_made.wrapping_add(1);
        let finished =
            self.backend.finish_queue_data(queue, &mut out[CreateHardwareQueue::PRIVATE..], self.queues_made);
        if let Err(error) = finished {
            // The guest gets a failure and never names this queue, so nothing of it may stay.
            self.close_queue(queue);
            self.queue_progress.remove(&progress);
            return Err(error.into());
        }
        CreateHardwareQueue::finish_answer(queue, progress, fence, fence_gpu_address, out);
        Ok(())
    }

    pub(super) fn destroy_hardware_queue(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        self.close_queue(first_handle(message)?);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(crate) fn close_queue(&mut self, handle: u32) {
        let closed = self.queues.remove(&handle);
        self.rings_not_before.remove(&handle);
        for step in self.gates.remove(&handle).unwrap_or_default() {
            if let GateStep::Wait { signals, id, .. } = step {
                self.leave_wait(id, &signals, false);
            }
        }
        for queues in self.context_queues.values_mut() {
            queues.retain(|&queue| queue != handle);
        }
        self.backend.close_queue(handle);
        for item in &mut self.deferred {
            item.waits.retain(|&(queue, _)| queue != handle);
        }
        if let Some(queue) = closed {
            // Its progress handle goes with it, as the fence it names is given back.
            self.queue_progress.retain(|_, fence| *fence != queue.fence);
            // The queue's runs may still write its fence, so the page waits for them like memory.
            self.defer(DeferredAction::ReturnFence((queue.fence, queue.fence_gpu_address)));
        }
    }

    pub(super) fn submit_to_hardware_queue(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        self.run_deferred();
        let request = SubmitToQueue::parse(message)?;
        let value = request.fence_value;
        self.note_batch(request.queue, request.commands, request.commands_length);
        self.note_waiting_updates(request.queue);
        let commands = CommandBuffer { address: request.commands, length: request.commands_length };
        match self.start_on_gpu(
            request.queue,
            WorkSource::HardwareQueue,
            request.private,
            Some(commands),
            value,
        ) {
            GpuTook::Work(work) => self.hold_if_ring_is_full(request.queue, value, work),
            GpuTook::NoRoom => {
                self.retry_later(message);
                return Ok(());
            }
            took @ (GpuTook::Nothing | GpuTook::Dropped) => {
                if let Some(queue) = self.queues.get(&request.queue) {
                    // Nothing ran, so stand in for the GPU where the guest would notice.
                    self.memory.write_u64(queue.fence, value)?;
                }
                answer_status(took.status(), out);
                return Ok(());
            }
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    /// A context that has no queue gets one here, so what waits for it can treat it like any other.
    pub(super) fn queue_of_context(&mut self, context: u32, message: &[u8]) -> Result<u32, Failure> {
        if self.context_queues.get(&context).is_none_or(Vec::is_empty) {
            let handle = self.take_handle()?;
            let (fence, fence_gpu_address) = self.take_fence()?;
            let space = self.space_for(MessageHeader::process_of(message)?)?;
            self.queues.insert(handle, Queue::new(fence, fence_gpu_address, space, context));
            self.context_queues.entry(context).or_default().push(handle);
        }
        Ok(self.context_queues[&context][0])
    }

    pub(super) fn submit_to_context(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = SubmitToContext::parse(message)?;
        let queue = self.queue_of_context(request.context, message)?;
        let record = self.queues.get(&queue).ok_or(Malformed)?;
        let (fence, value) = (record.fence, record.last_fence.unwrap_or(0).saturating_add(1));
        self.note_waiting_updates(queue);
        // Only a backend that runs them is given the commands. For it, each submission has a value
        // of its own, as one finished unrun may come before one that runs.
        let runs_commands = self.backend.runs_context_commands();
        let commands = (runs_commands && request.commands_length != 0)
            .then_some(CommandBuffer { address: request.commands, length: request.commands_length });
        if let Some(commands) = commands {
            self.note_batch(queue, commands.address, commands.length);
        }
        match self.start_on_gpu(queue, WorkSource::Context, request.private, commands, value) {
            GpuTook::Work(_) => {}
            GpuTook::NoRoom => {
                self.retry_later(message);
                return Ok(());
            }
            took @ (GpuTook::Nothing | GpuTook::Dropped) => {
                if runs_commands && let Some(record) = self.queues.get_mut(&queue) {
                    record.last_fence = Some(value);
                }
                self.memory.write_u64(fence, value)?;
                answer_status(took.status(), out);
                return Ok(());
            }
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_process(name: &str) -> Vec<u8> {
        let mut message = vec![0; PROCESS_FLAGS + 1];
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        message[PROCESS_NAME.start..PROCESS_NAME.start + units.len()].copy_from_slice(&units);
        message
    }

    #[test]
    fn a_process_is_named_by_its_programs_file_name_alone() {
        assert_eq!(program_name(&create_process("C:\\Users\\ana\\Games\\game.exe")), "game.exe");
        assert_eq!(program_name(&create_process("dwm.exe")), "dwm.exe");
        assert_eq!(program_name(&create_process("")), "");
        assert_eq!(program_name(&[0; 0x30]), "", "a message too short for the name has none");
    }
}
