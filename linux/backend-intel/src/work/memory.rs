//! Memory behind the names the core is given: buffers xe makes for video memory, and the guest's own
//! pages, which are host memory bound by address.

use super::CardWork;
use crate::xe::device::CpuMap;
use crate::xe::plan::{Backing, BufferUse};
use guesthouse_backend::{BackendError, MemoryId};

/// A block of memory the core holds a name for.
#[derive(Debug)]
pub(super) struct Block {
    pub backing: Backing,
    /// The CPU's view of a buffer, made the first time something reads or writes it.
    pub map: Option<CpuMap>,
}

impl CardWork {
    /// Makes video memory: a buffer on the card, counted against the guest's cap.
    pub fn allocate(&mut self, size: u64, usage: BufferUse) -> Result<(MemoryId, u64), BackendError> {
        let request = self.placements.buffer(size, usage);
        if self.held.checked_add(request.size).is_none_or(|held| held > self.cap) {
            return Err(BackendError::new(format!(
                "{:#x} more bytes of video memory would pass the guest's cap of {:#x}, with {:#x} held",
                request.size, self.cap, self.held
            )));
        }
        let handle = self.xe.create_buffer(&request).map_err(|error| {
            BackendError::new(format!("xe made no buffer of {:#x} bytes: {error}", request.size))
        })?;
        self.held += request.size;
        let memory = MemoryId(self.new_id());
        let backing = Backing::Buffer {
            handle,
            size: request.size,
            page: self.placements.page(usage),
            usage,
            cpu_caching: request.cpu_caching,
        };
        self.blocks.insert(memory, Block { backing, map: None });
        Ok((memory, request.size))
    }

    /// Takes the guest's own pages, which are already mapped in this process. Nothing is made:
    /// they are bound by their host address wherever the guest maps them.
    pub fn adopt(&mut self, host_address: u64, size: u64) -> Result<MemoryId, BackendError> {
        if host_address == 0 || size == 0 {
            return Err(BackendError::new("no host pages to adopt"));
        }
        let memory = MemoryId(self.new_id());
        self.blocks
            .insert(memory, Block { backing: Backing::HostPages { address: host_address, size }, map: None });
        Ok(memory)
    }

    /// Frees a block. Every mapping of it goes first, as a binding of freed memory must not stay.
    pub fn release(&mut self, memory: MemoryId) {
        let Some(block) = self.blocks.remove(&memory) else { return };
        self.unmap_everywhere(memory);
        if let Backing::Buffer { handle, size, .. } = block.backing {
            self.held = self.held.saturating_sub(size);
            // The CPU's view goes before the buffer.
            drop(block.map);
            if let Err(error) = self.xe.close_buffer(handle) {
                log::warn!("intel work: closing a buffer failed: {error}");
            }
        }
    }

    /// Blocks held, for a test to see that frees arrive.
    pub fn live_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// Bytes of buffers held against the cap.
    pub fn held(&self) -> u64 {
        self.held
    }

    /// The CPU's view of a block: a buffer's own mapping, made on first use, or the host pages
    /// where they already are.
    pub(super) fn cpu_view(&mut self, memory: MemoryId) -> Result<&mut CpuMap, BackendError> {
        let block = self.blocks.get_mut(&memory).ok_or(BackendError::new("this memory is gone"))?;
        if block.map.is_none() {
            block.map = Some(match block.backing {
                Backing::Buffer { handle, size, .. } => {
                    self.xe.map_buffer(handle, size).map_err(|error| {
                        BackendError::new(format!("xe gave the CPU no view of a buffer: {error}"))
                    })?
                }
                // SAFETY: the core adopted these pages as mapped in this process for as long as the
                // block lives, and the view goes with the block.
                Backing::HostPages { address, size } => unsafe {
                    CpuMap::borrowed(address as *mut u8, size as usize)
                },
            });
        }
        Ok(block.map.as_mut().expect("just made"))
    }

    /// Copies what the card may have written from one block to another, by the CPU, which reads
    /// the card's memory uncached: see [`super::moves`].
    pub(super) fn move_by_cpu(
        &mut self,
        from: MemoryId,
        to: MemoryId,
        size: u64,
    ) -> Result<(), BackendError> {
        const STEP: usize = 1 << 20;
        let mut step = vec![0u8; STEP.min(size as usize)];
        let mut done = 0u64;
        while done < size {
            let length = step.len().min((size - done) as usize);
            let chunk = &mut step[..length];
            if !self.cpu_view(from)?.read(done, chunk) {
                return Err(BackendError::new("the memory moved from is smaller than what is moved"));
            }
            if !self.cpu_view(to)?.write(done, chunk) {
                return Err(BackendError::new("the memory moved to is smaller than what is moved"));
            }
            done += length as u64;
        }
        Ok(())
    }

    /// Reads the first bytes of a block.
    pub fn read_start(&mut self, memory: MemoryId, out: &mut [u8]) -> Result<(), BackendError> {
        if self.cpu_view(memory)?.read(0, out) {
            Ok(())
        } else {
            Err(BackendError::new("the memory is smaller than what was asked of its start"))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::stage::Stage;
    use crate::work::testing;
    use crate::xe::abi;
    use crate::xe::fake::Call;
    use crate::xe::plan::BufferUse;

    #[test]
    fn video_memory_is_a_buffer_on_the_card_that_goes_when_freed() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let (memory, size) = work.allocate(0x1800, BufferUse::Card).unwrap();
        assert_eq!(size, 0x2000, "whole pages");
        let Some(Call::CreateBuffer(made)) = xe.calls_named("create_buffer").pop() else { panic!() };
        assert_eq!((made.size, made.cpu_caching), (0x2000, abi::CPU_CACHING_WRITE_COMBINED));
        assert_eq!(made.placement.count_ones(), 2);
        assert_eq!((work.live_blocks(), work.held()), (1, 0x2000));
        work.release(memory);
        assert_eq!((work.live_blocks(), work.held(), xe.live()), (0, 0, 0));
        work.release(memory);
        assert_eq!(xe.calls_named("close_buffer").len(), 1, "a second free does nothing");
    }

    #[test]
    fn video_memory_past_the_cap_is_refused_and_nothing_is_made() {
        let (mut work, xe) = testing::work_with_cap(Stage::Compute, 0x3000);
        let (first, _) = work.allocate(0x2000, BufferUse::Card).unwrap();
        let refused = work.allocate(0x2000, BufferUse::Card).unwrap_err();
        assert!(refused.0.contains("cap"), "{refused}");
        assert_eq!(xe.calls_named("create_buffer").len(), 1);
        work.allocate(0x1000, BufferUse::Card).unwrap();
        work.release(first);
        work.allocate(0x2000, BufferUse::Card).expect("room again once the first is freed");
    }

    #[test]
    fn a_buffer_xe_refuses_is_a_failure_and_counts_for_nothing() {
        let (mut work, xe) = testing::work(Stage::Compute);
        xe.fail("create_buffer", libc::ENOMEM);
        assert!(work.allocate(0x1000, BufferUse::Card).is_err());
        assert_eq!((work.live_blocks(), work.held()), (0, 0));
    }

    #[test]
    fn guest_pages_make_nothing_on_the_card() {
        let (mut work, xe) = testing::work(Stage::Compute);
        let pages = testing::Pages::new(0x2000);
        let memory = work.adopt(pages.address(), 0x2000).unwrap();
        assert!(xe.calls().is_empty());
        assert!(work.adopt(0, 0x1000).is_err());
        work.release(memory);
        assert!(xe.calls().is_empty(), "nothing to close either");
    }

    #[test]
    fn contents_move_from_the_card_to_guest_pages_by_the_cpu() {
        let (mut work, _xe) = testing::work(Stage::Compute);
        let (from, _) = work.allocate(0x2000, BufferUse::Card).unwrap();
        let pages = testing::Pages::new(0x2000);
        let to = work.adopt(pages.address(), 0x2000).unwrap();
        let picture: Vec<u8> = (0..0x2000u32).map(|byte| byte as u8).collect();
        assert!(work.cpu_view(from).unwrap().write(0, &picture));
        work.move_by_cpu(from, to, 0x2000).unwrap();
        assert_eq!(pages.bytes(), picture);
        let mut start = [0u8; 16];
        work.read_start(from, &mut start).unwrap();
        assert_eq!(start[..], picture[..16]);
        assert!(work.move_by_cpu(from, to, 0x3000).is_err(), "more than either holds");
    }
}
