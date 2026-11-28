//! The WDDM capability queries Direct3D asks before it makes a device. Unanswered, the guest's
//! kernel returns STATUS_NOT_SUPPORTED, and Intel's Direct3D 11 driver gives the device up at once:
//! a tester's guest asked for the WDDM 3.0 caps three times and dropped the device each time.
//!
//! The layouts are the WDK's D3DKMT_WDDM_2_0_CAPS, D3DKMT_QUERY_GPUMMU_CAPS and
//! D3DKMT_WDDM_3_0_CAPS. Each bit set is something this backend, or the record it gives, already
//! says. A bit that can't be grounded that way stays clear: a clear bit only makes the guest do
//! without.

use super::bytes::dword;
use guesthouse_backend::{GPU_RANGES, Status, VendorQuery};

/// KMTQAITYPE_WDDM_2_0_CAPS.
pub const QUERY_WDDM_2_0_CAPS: u32 = 24;
/// KMTQAITYPE_QUERY_GPUMMU_CAPS.
pub const QUERY_GPU_MMU_CAPS: u32 = 34;
/// KMTQAITYPE_WDDM_3_0_CAPS.
pub const QUERY_WDDM_3_0_CAPS: u32 = 77;

/// The adapter has a page table of its own for each process. The record says so to Intel's driver
/// with FtrWddm2GpuMmu, and the core keeps each process's GPU addresses apart. 64-bit atomics,
/// the IOMMU, flip overwrite and contextless present are left out: nothing here shows them true.
const GPU_MMU_SUPPORTED: u32 = 1 << 1;
const WDDM_2_0_CAPS: u32 = GPU_MMU_SUPPORTED;
/// The one WDDM 3.0 bit worth having is hardware flip queues, which nothing here gives.
const WDDM_3_0_CAPS: u32 = 0;
/// Read-only, no-execute and cache-coherent memory are all left out: this stage maps nothing on
/// the card, so it keeps none of those promises.
const GPU_MMU_FLAGS: u32 = 0;
/// D3DKMT_QUERY_GPUMMU_CAPS: the physical adapter's index, the flags, then the address bits.
const GPU_MMU_CAPS_SIZE: usize = 12;

/// The bits of GPU address a process has: up to the end of the core's upper range, where the
/// partition the record gives ends too (1 TiB, so 40 bits). The card has more, which the core has
/// no addresses for.
fn address_bits() -> u32 {
    let end = GPU_RANGES[GPU_RANGES.len() - 1].end;
    u64::BITS - (end - 1).leading_zeros()
}

/// Answers one of the three queries. Any other is left alone.
pub fn answer(query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<Status> {
    let words = match query.kind {
        QUERY_WDDM_2_0_CAPS => vec![WDDM_2_0_CAPS],
        QUERY_WDDM_3_0_CAPS => vec![WDDM_3_0_CAPS],
        QUERY_GPU_MMU_CAPS if query.size >= GPU_MMU_CAPS_SIZE => {
            // The index is the guest's own, as it came: there is one physical adapter.
            vec![dword(query.data, 0).unwrap_or(0), GPU_MMU_FLAGS, address_bits()]
        }
        QUERY_GPU_MMU_CAPS => return Some(Status::INVALID_PARAMETER),
        _ => return None,
    };
    if query.size < words.len() * size_of::<u32>() {
        return Some(Status::INVALID_PARAMETER);
    }
    let start = out.len();
    words.iter().for_each(|word| out.extend_from_slice(&word.to_le_bytes()));
    out.resize(start + query.size, 0);
    Some(Status::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(kind: u32, size: usize, data: &[u8]) -> (Option<Status>, Vec<u8>) {
        let mut out = Vec::new();
        let status =
            answer(&VendorQuery { kind, size, data, process_is_32_bit: false, program: "" }, &mut out);
        (status, out)
    }

    fn words(bytes: &[u8]) -> Vec<u32> {
        bytes.chunks(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect()
    }

    #[test]
    fn the_wddm_3_0_caps_are_answered_with_nothing_promised() {
        assert_eq!(ask(QUERY_WDDM_3_0_CAPS, 4, &[0; 4]), (Some(Status::SUCCESS), vec![0; 4]));
    }

    #[test]
    fn wddm_2_0_promises_the_gpu_mmu_alone() {
        let (status, out) = ask(QUERY_WDDM_2_0_CAPS, 4, &[0; 4]);
        assert_eq!((status, words(&out)), (Some(Status::SUCCESS), vec![GPU_MMU_SUPPORTED]));
    }

    #[test]
    fn the_gpu_mmu_caps_say_the_cores_forty_bits_and_no_flags() {
        let (status, out) = ask(QUERY_GPU_MMU_CAPS, 12, &[0; 12]);
        assert_eq!((status, words(&out)), (Some(Status::SUCCESS), vec![0, 0, 40]));
    }

    #[test]
    fn a_short_buffer_is_refused_and_other_queries_are_left_alone() {
        assert_eq!(ask(QUERY_WDDM_2_0_CAPS, 2, &[]).0, Some(Status::INVALID_PARAMETER));
        assert_eq!(ask(QUERY_GPU_MMU_CAPS, 8, &[]).0, Some(Status::INVALID_PARAMETER));
        assert_eq!(ask(65, 8192, &[]), (None, Vec::new()));
    }
}
