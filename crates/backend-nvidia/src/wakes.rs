//! Waits the GPU can interrupt, instead of ones the daemon has to keep looking at.
//!
//! A semaphore surface is the RM's monitored-fence object: slots in host memory whose counts
//! engines raise, and waiters the kernel posts to a file descriptor when a bound engine raises its
//! non-stall interrupt. The daemon — or anything else driving the card — arms a waiter for a count
//! it wants, points a release at the slot, and sleeps on the descriptor until the interrupt says
//! the slot may have moved. A poll that times out is the backstop: it re-reads the slots the same
//! way, so a lost wakeup costs the timeout and nothing else.
//!
//! The layout of every request is in the open kernel module's public headers. Slots are 32 bytes,
//! with the count at the start; this generation's counts are 64 bits, so the surface is made
//! without the max-submitted half of the pair, which the driver refuses when it takes 64-bit
//! semaphores.

use crate::rm::classes::{
    NOTIFY_COPY, NOTIFY_FIFO_METHODS, NOTIFY_GRAPHICS, SEMAPHORE_SURFACE, SURFACE_BIND_CHANNEL,
    SURFACE_REGISTER_WAITER, SURFACE_UNBIND_CHANNEL, SURFACE_UNREGISTER_WAITER,
};
use crate::rm::device::{AddressSpace, Device, Mapping};
use crate::rm::{Handle, Rm, RmError, STATUS_ALREADY_SIGNALLED, STATUS_OK};

/// One slot, in bytes: the count at the start, the kernel's threshold in the middle.
const SLOT: usize = 32;

/// The surface, its memory, and what hangs from it.
pub struct Wakes {
    memory: Handle,
    /// The surface's own handle, for the batch's waiter arm.
    pub(crate) surface: Handle,
    /// Each bound channel and the notify index it was bound by, so what close takes off matches
    /// what bind put on.
    bound: Vec<(Handle, u32)>,
    armed: Vec<(u32, u64)>,
    gpu_address: u64,
    /// The space the page is mapped in, so close can take the mapping down before the memory.
    space: AddressSpace,
}

impl std::fmt::Debug for Wakes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wakes").field("surface", &self.surface).finish_non_exhaustive()
    }
}

impl Wakes {
    /// Makes a surface over fresh host pages mapped at `gpu_address` in the space, and tells the
    /// driver to post this client's waiters on the descriptor [`Rm::wake_fd`] names. The surface
    /// hangs from `parent`: the device on this driver. Whatever a refusal leaves behind, the page,
    /// its mapping, and the event posting, is taken back before the error goes up.
    pub fn new<R: Rm>(
        device: &mut Device<R>,
        space: &AddressSpace,
        gpu_address: u64,
        parent: Handle,
    ) -> Result<Self, RmError> {
        device.rm.alloc_os_event(device.device)?;
        let memory = match device.alloc_sysmem(4096) {
            Ok(memory) => memory,
            Err(error) => {
                let _ = device.rm.free_os_event(device.device);
                return Err(error);
            }
        };
        if let Err(error) = device.map(
            space,
            &Mapping {
                memory,
                offset: 0,
                length: 4096,
                address: gpu_address,
                small_pages: true,
                gpu_cached: false,
                page_kind: None,
            },
        ) {
            let _ = device.free(memory);
            let _ = device.rm.free_os_event(device.device);
            return Err(error);
        }
        let mut params = [0; 16];
        params[0..4].copy_from_slice(&memory.to_le_bytes());
        match device.alloc(parent, SEMAPHORE_SURFACE, &mut params) {
            Ok(surface) => Ok(Self {
                memory,
                surface,
                bound: Vec::new(),
                armed: Vec::new(),
                gpu_address,
                space: space.clone(),
            }),
            Err(error) => {
                let _ = device.unmap(space, memory, gpu_address, 4096);
                let _ = device.free(memory);
                let _ = device.rm.free_os_event(device.device);
                Err(error)
            }
        }
    }

    /// Binds a channel, so its engine's non-stall interrupts re-check the waiters. A copy channel
    /// takes the copy notifier; anything else takes the graphics one.
    pub fn bind<R: Rm>(
        &mut self,
        device: &mut Device<R>,
        channel: Handle,
        copy: bool,
    ) -> Result<(), RmError> {
        let mut params = [0; 40];
        params[0..4].copy_from_slice(&channel.to_le_bytes());
        // The runlist reports a channel's semaphore interrupt on the FIFO's index, and the engine
        // itself on its own, so a channel takes both.
        params[4..8].copy_from_slice(&2u32.to_le_bytes());
        params[8..12].copy_from_slice(&NOTIFY_FIFO_METHODS.to_le_bytes());
        let index = u32::from(!copy) * NOTIFY_GRAPHICS + u32::from(copy) * NOTIFY_COPY;
        params[12..16].copy_from_slice(&index.to_le_bytes());
        let status = device.rm.control(self.surface, SURFACE_BIND_CHANNEL, &mut params)?;
        RmError::check(|| "binding a channel to the surface".to_owned(), status)?;
        self.bound.push((channel, index));
        Ok(())
    }

    /// Arms a one-shot waiter for slot `index` reaching `value`. `Ok(false)` says the count is
    /// already there and there is nothing to wait for.
    pub fn arm<R: Rm>(
        &mut self,
        device: &mut Device<R>,
        index: u32,
        value: u64,
        fd: i32,
    ) -> Result<bool, RmError> {
        let mut params = [0; 32];
        params[0..4].copy_from_slice(&index.to_le_bytes());
        params[8..16].copy_from_slice(&value.to_le_bytes());
        params[24..28].copy_from_slice(&(fd as u32).to_le_bytes());
        let status = device.rm.control(self.surface, SURFACE_REGISTER_WAITER, &mut params)?;
        if status == STATUS_ALREADY_SIGNALLED {
            return Ok(false);
        }
        if status != STATUS_OK {
            return Err(RmError::refused(format!("arming slot {index}"), status));
        }
        self.armed.push((index, value));
        Ok(true)
    }

    /// The GPU address of a slot, for a release to write.
    pub fn slot_address(&self, index: u32) -> u64 {
        self.gpu_address + u64::from(index) * SLOT as u64
    }

    /// Takes the waiters off and the channels away. Run this while the bound channels still live,
    /// so the unbinds name live handles. The page stays mapped: a channel that is still running can
    /// end its batch by writing the slot, so the page goes only once the channel is closed.
    pub fn unbind<R: Rm>(&mut self, device: &mut Device<R>) {
        for (index, value) in self.armed.drain(..) {
            let mut params = [0; 32];
            params[0..4].copy_from_slice(&index.to_le_bytes());
            params[8..16].copy_from_slice(&value.to_le_bytes());
            let _ = device.rm.control(self.surface, SURFACE_UNREGISTER_WAITER, &mut params);
        }
        for (channel, index) in self.bound.drain(..) {
            let mut params = [0; 40];
            params[0..4].copy_from_slice(&channel.to_le_bytes());
            // The same two entries the bind made, in its order: the FIFO's index and the engine's
            // own.
            params[4..8].copy_from_slice(&2u32.to_le_bytes());
            params[8..12].copy_from_slice(&NOTIFY_FIFO_METHODS.to_le_bytes());
            params[12..16].copy_from_slice(&index.to_le_bytes());
            let _ = device.rm.control(self.surface, SURFACE_UNBIND_CHANNEL, &mut params);
        }
    }

    /// Unbinds what is still bound, unmaps the page, and frees the surface, its memory and the
    /// event posting. Run this after the channels it was bound to are closed, as a closed channel
    /// can no longer write the slot, and before the space is freed, so the unmap names a live space.
    pub fn close<R: Rm>(&mut self, device: &mut Device<R>) -> Result<(), RmError> {
        self.unbind(device);
        device.unmap(&self.space, self.memory, self.gpu_address, 4096)?;
        let surface = self.surface;
        device.free(surface)?;
        let memory = self.memory;
        device.free(memory)?;
        // The event posting came with the surface's making, so it goes with its closing; without
        // this a rebuilt surface asks for another posting every time and none comes off.
        device.rm.free_os_event(device.device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_address_steps_by_the_slot() {
        use crate::profile::HOST_615;
        use crate::rm::device::Device;
        use crate::rm::fake::FakeRm;

        let mut rm = FakeRm::new();
        rm.wake = Some(0);
        let mut device = Device::open(rm, &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
        let subdevice = device.subdevice;
        let wakes = Wakes::new(&mut device, &space, 0x400_0000, subdevice).unwrap();
        assert_eq!(wakes.slot_address(0), 0x400_0000);
        assert_eq!(wakes.slot_address(3), 0x400_0000 + 3 * SLOT as u64);
    }

    #[test]
    fn closing_takes_off_both_of_a_bound_channels_entries() {
        use crate::profile::HOST_615;
        use crate::rm::STATUS_OK;
        use crate::rm::classes::{
            NOTIFY_COPY, NOTIFY_FIFO_METHODS, SURFACE_BIND_CHANNEL, SURFACE_UNBIND_CHANNEL,
        };
        use crate::rm::device::Device;
        use crate::rm::fake::{Call, FakeRm};

        let mut rm = FakeRm::new();
        rm.wake = Some(0);
        rm.answer(SURFACE_BIND_CHANNEL, |_| STATUS_OK);
        let mut device = Device::open(rm, &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
        let subdevice = device.subdevice;
        let mut wakes = Wakes::new(&mut device, &space, 0x800_0000, subdevice).unwrap();
        wakes.bind(&mut device, 0x1234, true).unwrap();
        wakes.close(&mut device).unwrap();
        let word =
            |params: &[u8], at: usize| u32::from_le_bytes(params[at..at + 4].try_into().expect("four bytes"));
        let unbinds: Vec<_> = device
            .rm
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::Control { command, params, .. } if *command == SURFACE_UNBIND_CHANNEL => {
                    Some(params.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(unbinds.len(), 1);
        assert_eq!(word(&unbinds[0], 4), 2, "the FIFO's entry and the engine's own");
        assert_eq!(word(&unbinds[0], 8), NOTIFY_FIFO_METHODS, "the FIFO's index, as the bind sent it");
        assert_eq!(word(&unbinds[0], 12), NOTIFY_COPY, "the engine's own, second as the bind sent it");
    }

    #[test]
    fn closing_takes_the_pages_mapping_down_before_its_memory() {
        use crate::profile::HOST_615;
        use crate::rm::STATUS_OK;
        use crate::rm::classes::SURFACE_BIND_CHANNEL;
        use crate::rm::device::Device;
        use crate::rm::fake::FakeRm;

        let mut rm = FakeRm::new();
        rm.wake = Some(0);
        rm.answer(SURFACE_BIND_CHANNEL, |_| STATUS_OK);
        let mut device = Device::open(rm, &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
        let subdevice = device.subdevice;
        let mut wakes = Wakes::new(&mut device, &space, 0x800_0000, subdevice).unwrap();
        assert!(device.rm.memory_at(0x800_0000).is_some(), "the page is mapped while it lives");
        wakes.close(&mut device).unwrap();
        assert!(device.rm.memory_at(0x800_0000).is_none(), "the mapping went with the surface");
        assert_eq!(
            (device.rm.events_allocated, device.rm.events_freed),
            (1, 1),
            "the posting the surface asked for came back off"
        );
    }

    #[test]
    fn a_surface_that_will_not_make_leaves_nothing_behind() {
        use crate::profile::HOST_615;
        use crate::rm::classes::SEMAPHORE_SURFACE;
        use crate::rm::device::Device;
        use crate::rm::fake::FakeRm;

        let mut rm = FakeRm::new();
        rm.wake = Some(0);
        rm.refuse_class = Some(SEMAPHORE_SURFACE);
        let mut device = Device::open(rm, &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
        let subdevice = device.subdevice;
        assert!(Wakes::new(&mut device, &space, 0x800_0000, subdevice).is_err());
        assert!(device.rm.memory_at(0x800_0000).is_none(), "the page's mapping came down again");
        assert_eq!(device.rm.events_freed, 1, "the event posting came off with the refusal");
    }
}
