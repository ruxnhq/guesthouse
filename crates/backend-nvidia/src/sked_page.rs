//! The SKED page: where the guest's own kernels launch work from the GPU.
//!
//! Direct3D 12 runs an indirect DispatchRays and shader execution reordering through kernels of its
//! own driver, which launch further work by adding a launch word (a QMD address and a scheduling
//! action) to one GPU address. The guest's driver reads that address from a hardware queue's private
//! data. Writes there only launch work if the page is mapped with the SKED message kind, which has
//! no memory behind it and hands each write to the GPU's scheduler. CUDA on Linux gets such a page
//! from the UVM driver for device-side launches. The RM gives one for every compute object: mapping
//! the object maps its SKED page. Which object backs the page doesn't matter, as the scheduler runs
//! the launch on the channel that wrote it, so each address space needs one, from any of its
//! graphics channels. Unmapped, the write stalls the shader and the channel times out.

use crate::rm::device::{AddressSpace, Device, Mapping};
use crate::rm::{Handle, Rm, RmError};
use guesthouse_backend::SpaceId;
use std::collections::HashMap;

/// In the hole every address space leaves around 4 GiB, past the half the RM keeps for itself.
/// Windows puts its page at 4 GiB, which is the RM's here.
pub const SKED_PAGE_ADDRESS: u64 = 0x1_2000_0000;
const SKED_PAGE_SIZE: u64 = 0x1000;
/// The range reserved for the page, a large page's worth so nothing else shares its page table.
pub const SKED_RANGE: (u64, u64) = (SKED_PAGE_ADDRESS, SKED_PAGE_ADDRESS + 0x20_0000);

/// Which queue's compute object backs each address space's page.
#[derive(Debug, Default)]
pub struct SkedPages {
    owners: HashMap<SpaceId, (u32, Handle)>,
}

impl SkedPages {
    /// Maps the page from this queue's compute object, unless the space has it already.
    pub fn give<R: Rm>(
        &mut self,
        device: &mut Device<R>,
        id: SpaceId,
        space: &AddressSpace,
        queue: u32,
        compute: Handle,
    ) {
        if self.owners.contains_key(&id) {
            return;
        }
        match map(device, space, compute) {
            Ok(()) => {
                log::info!("SKED page at {SKED_PAGE_ADDRESS:#x} from queue {queue:#x}'s compute object");
                self.owners.insert(id, (queue, compute));
            }
            Err(error) => log::warn!("no SKED page for queue {queue:#x}: {error}"),
        }
    }

    /// Before this queue's channel is freed: takes the page down if the queue's object backs it.
    /// Returns whether it did, so the caller can map it again from another channel.
    pub fn queue_closing<R: Rm>(
        &mut self,
        device: &mut Device<R>,
        id: SpaceId,
        space: &AddressSpace,
        queue: u32,
    ) -> bool {
        let Some(&(owner, compute)) = self.owners.get(&id) else { return false };
        if owner != queue {
            return false;
        }
        self.owners.remove(&id);
        if let Err(error) = device.unmap(space, compute, SKED_PAGE_ADDRESS, SKED_PAGE_SIZE) {
            log::warn!("unmapping the SKED page: {error}");
        }
        true
    }

    pub fn forget_space(&mut self, id: SpaceId) {
        self.owners.remove(&id);
    }
}

fn map<R: Rm>(device: &mut Device<R>, space: &AddressSpace, compute: Handle) -> Result<(), RmError> {
    let mapping = Mapping {
        memory: compute,
        offset: 0,
        length: SKED_PAGE_SIZE,
        address: SKED_PAGE_ADDRESS,
        small_pages: true,
        gpu_cached: false,
        page_kind: None,
    };
    device.map(space, &mapping)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::HOST_615;
    use crate::rm::fake::{Call, FakeRm};

    const COMPUTE: Handle = 0xC000_2000;
    const OTHER_COMPUTE: Handle = 0xC000_3000;

    fn device() -> (Device<FakeRm>, AddressSpace) {
        let mut device = Device::open(FakeRm::new(), &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000), SKED_RANGE]).unwrap();
        (device, space)
    }

    fn sked_maps(device: &Device<FakeRm>) -> Vec<Handle> {
        let calls = device.rm.calls.iter();
        calls
            .filter_map(|call| match call {
                Call::MapDma(map) if map.address == SKED_PAGE_ADDRESS => Some(map.memory),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_first_compute_object_backs_the_page_as_a_small_page() {
        let (mut device, space) = device();
        let mut pages = SkedPages::default();
        pages.give(&mut device, SpaceId(1), &space, 7, COMPUTE);
        pages.give(&mut device, SpaceId(1), &space, 8, OTHER_COMPUTE);
        assert_eq!(sked_maps(&device), [COMPUTE]);
        let map = device.rm.calls.iter().find_map(|call| match call {
            Call::MapDma(map) if map.address == SKED_PAGE_ADDRESS => Some(*map),
            _ => None,
        });
        let map = map.unwrap();
        assert!(map.small_pages && map.length == 0x1000 && map.page_kind.is_none());
    }

    #[test]
    fn only_the_owner_closing_takes_the_page_down() {
        let (mut device, space) = device();
        let mut pages = SkedPages::default();
        pages.give(&mut device, SpaceId(1), &space, 7, COMPUTE);
        assert!(!pages.queue_closing(&mut device, SpaceId(1), &space, 8));
        assert!(pages.queue_closing(&mut device, SpaceId(1), &space, 7));
        let unmapped =
            device.rm.calls.iter().any(|call| {
                matches!(call, Call::UnmapDma { memory: COMPUTE, address: SKED_PAGE_ADDRESS, .. })
            });
        assert!(unmapped);
        pages.give(&mut device, SpaceId(1), &space, 8, OTHER_COMPUTE);
        assert_eq!(sked_maps(&device), [COMPUTE, OTHER_COMPUTE]);
    }

    #[test]
    fn the_page_is_in_the_hole_and_past_the_rms_half() {
        let hole_start = guesthouse_backend::GPU_RANGES[0].end;
        let hole_end = guesthouse_backend::GPU_RANGES[1].start;
        assert!(SKED_RANGE.0 >= hole_start + 0x2000_0000 && SKED_RANGE.1 <= hole_end);
    }
}
