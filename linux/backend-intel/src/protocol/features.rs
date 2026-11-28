//! The feature bits of the adapter record that this backend sets or clears.
//!
//! The record carries a feature table, a long run of one-bit flags saying what the hardware can do.
//! For a card Intel's kernel driver knows, the table is that driver's, [`super::card_data`], with
//! the flags here set or cleared by this backend on top. The flags here are under gmmlib's names;
//! a card the data doesn't list gets these alone and the rest of the table stays zero. The table's
//! workaround counterpart is set entry by entry with [`set_workaround`], from [`super::workarounds`].

/// One flag: the byte it is in, counted from the start of the record, and its bit there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    pub name: &'static str,
    pub byte: usize,
    pub bit: u8,
}

/// The flag at `bit` of the word that starts at byte `word` of the record.
const fn at(name: &'static str, word: usize, bit: usize) -> Feature {
    Feature { name, byte: word + bit / 8, bit: (bit % 8) as u8 }
}

/// The flag at bit `index` of the table, counting from its first byte's lowest bit.
pub const fn bit(name: &'static str, index: usize) -> Feature {
    at(name, WORD_FIRST, index)
}

/// Where the table starts in the record.
pub const TABLE: usize = WORD_FIRST;

/// Where three words of the table start.
const WORD_FIRST: usize = 0x2B4;
const WORD_A: usize = 0x2BC;
const WORD_B: usize = 0x2E4;
const WORD_C: usize = 0x2EC;

/// Per-process GPU page tables.
pub const PPGTT: Feature = at("FtrPPGTT", WORD_B, 22);
/// The GPU sharing the CPU's pointers.
pub const SVM: Feature = at("FtrSVM", WORD_B, 27);
/// The WDDM 2 model of GPU addresses, where each process has its own.
pub const WDDM2_GPU_MMU: Feature = at("FtrWddm2GpuMmu", WORD_B, 46);
/// The WDDM 2 form of [`SVM`].
pub const WDDM2_SVM: Feature = at("FtrWddm2Svm", WORD_B, 47);
/// 64 KiB pages in the GPU's page tables.
pub const WDDM2_64KB_PAGES: Feature = at("FtrWddm2_1_64kbPages", WORD_B, 50);
/// Memory on the card itself, which an integrated card hasn't got.
pub const LOCAL_MEMORY: Feature = at("FtrLocalMemory", WORD_B, 58);
/// The older Y tiling, which Xe2 has dropped.
pub const TILE_Y: Feature = at("FtrTileY", WORD_B, 62);
/// A 64-bit semaphore wait in the command streamer.
pub const HW_SEMAPHORE_64: Feature = at("FtrHwSemaphore64", WORD_A, 6);

/// The command streamer for the blitter, which copies.
pub const BLITTER_RING: Feature = at("FtrBlitterRing", WORD_FIRST, 14);
/// The ring for compute, and the node that submits to it.
pub const CCS_RING: Feature = at("FtrCCSRing", WORD_A, 8);
pub const CCS_NODE: Feature = at("FtrCCSNode", WORD_A, 12);
/// More than one compute engine.
pub const CCS_MULTI_INSTANCE: Feature = at("FtrCCSMultiInstance", WORD_A, 14);
/// Scheduling in hardware, which is what makes the guest's drivers use hardware queues.
pub const HW_SCHEDULING: Feature = at("FtrHwScheduling", WORD_C, 49);
/// Xe2's tiling formats.
pub const XE2_TILING: Feature = at("FtrXe2PlusTiling", WORD_C, 0);
/// Five levels of GPU page tables.
pub const PML5: Feature = at("FtrPml5Support", WORD_C, 16);
/// The walker's mid-thread preemption.
pub const WALKER_MTP: Feature = at("FtrWalkerMTP", WORD_B, 19);
/// Tiled resources mapped through the page tables, the hardware's translation table for them, that
/// table kept by the user-mode driver, and page-table entries that map nothing.
pub const TILE_MAPPED_RESOURCE: Feature = at("FtrTileMappedResource", WORD_B, 29);
pub const TRANSLATION_TABLE: Feature = at("FtrTranslationTable", WORD_B, 30);
pub const USER_MODE_TRANSLATION_TABLE: Feature = at("FtrUserModeTranslationTable", WORD_B, 31);
pub const NULL_PAGES: Feature = at("FtrNullPages", WORD_B, 32);
/// Memory on the card that may be mapped in 4 KiB pages.
pub const LOCAL_MEMORY_ALLOWS_4KB: Feature = at("FtrLocalMemoryAllows4KB", WORD_C, 4);
/// The kernel driver's own reset of the function.
pub const DRIVER_FLR: Feature = at("FtrDriverFLR", WORD_C, 40);
/// Render compression of surfaces the display shows.
pub const RENDER_COMPRESSION: Feature = bit("FtrRendComp", 969);
/// Preemption of compute work at a batch boundary, a thread group boundary and mid-thread.
pub const GPGPU_MID_BATCH_PREEMPT: Feature = at("FtrGpGpuMidBatchPreempt", WORD_B, 9);
pub const GPGPU_THREAD_GROUP_PREEMPT: Feature = at("FtrGpGpuThreadGroupLevelPreempt", WORD_B, 10);
pub const GPGPU_MID_THREAD_PREEMPT: Feature = at("FtrGpGpuMidThreadLevelPreempt", WORD_B, 11);

/// The 3D pipeline levels Intel's Direct3D 10 and 11 driver reports, from 10_0 up, which have no
/// names in gmmlib. Its 12_0 and 12_1 come from [`WDDM2_GPU_MMU`]. Direct3D 11 refuses a driver
/// whose levels have a gap, and with these clear the gap is below 12_0, so no Direct3D 10 or 11
/// device could be made on the adapter, the desktop's compositor's included.
pub const PIPELINE_LEVELS: [Feature; 4] = [
    at("3D pipeline 10_0", WORD_FIRST, 648),
    at("3D pipeline 10_1", WORD_FIRST, 649),
    at("3D pipeline 11_0", WORD_FIRST, 651),
    at("3D pipeline 11_1", WORD_FIRST, 652),
];

/// The Direct3D 11 interface versions past 11.0's first two, 0x000B0023 to 0x000B002D, one bit
/// each, which have no names in gmmlib. Intel's driver offers a version only when its bit is set,
/// and the newer ones carry the runtime's callbacks for hardware queues, which the driver calls on a
/// card with hardware scheduling.
pub const DDI_VERSIONS: [Feature; 11] = [
    at("D3D11 DDI 0x000B0023", WORD_FIRST, 1413),
    at("D3D11 DDI 0x000B0024", WORD_FIRST, 1414),
    at("D3D11 DDI 0x000B0025", WORD_FIRST, 1415),
    at("D3D11 DDI 0x000B0026", WORD_FIRST, 1416),
    at("D3D11 DDI 0x000B0027", WORD_FIRST, 1417),
    at("D3D11 DDI 0x000B0028", WORD_FIRST, 1418),
    at("D3D11 DDI 0x000B0029", WORD_FIRST, 1419),
    at("D3D11 DDI 0x000B002A", WORD_FIRST, 1420),
    at("D3D11 DDI 0x000B002B", WORD_FIRST, 1421),
    at("D3D11 DDI 0x000B002C", WORD_FIRST, 1422),
    at("D3D11 DDI 0x000B002D", WORD_FIRST, 1423),
];

/// The four flags that let the guest compress surfaces. Compression needs the host to set up
/// memory the same way, which this backend can't yet, so all four stay clear.
pub const COMPRESSION: [Feature; 4] = [
    at("FtrE2ECompression", WORD_B, 52),
    at("FtrLinearCCS", WORD_B, 54),
    at("FtrFlatPhysCCS", WORD_C, 1),
    at("FtrXe2Compression", WORD_C, 2),
];

/// Sets or clears a flag in a record.
pub fn set(record: &mut [u8], feature: Feature, on: bool) {
    let mask = 1 << feature.bit;
    if on {
        record[feature.byte] |= mask;
    } else {
        record[feature.byte] &= !mask;
    }
}

/// Where the workaround table starts in the record. Its entries are four bits wide, two to a byte,
/// the even one in the low half.
const WORKAROUND_TABLE: usize = 0x3A0;

/// Sets workaround entry `entry` to `value`, whose four low bits are kept. An entry past the
/// record is not set.
pub fn set_workaround(record: &mut [u8], entry: usize, value: u8) {
    let Some(byte) = record.get_mut(WORKAROUND_TABLE + entry / 2) else { return };
    let shift = 4 * (entry % 2);
    *byte = *byte & !(0xF << shift) | (value & 0xF) << shift;
}

pub fn workaround(record: &[u8], entry: usize) -> Option<u8> {
    let byte = record.get(WORKAROUND_TABLE + entry / 2)?;
    Some(byte >> (4 * (entry % 2)) & 0xF)
}

pub fn is_set(record: &[u8], feature: Feature) -> bool {
    record.get(feature.byte).is_some_and(|byte| byte & (1 << feature.bit) != 0)
}

/// The five Direct3D 11 interface versions below 0x000B0023, the low bits of the same dword, which
/// have no names in gmmlib either. Intel's kernel driver offers them on every card its data lists.
pub const OLDER_DDI_VERSIONS: [Feature; 5] = [
    at("an older D3D11 DDI, bit 0", WORD_FIRST, 1408),
    at("an older D3D11 DDI, bit 1", WORD_FIRST, 1409),
    at("an older D3D11 DDI, bit 2", WORD_FIRST, 1410),
    at("an older D3D11 DDI, bit 3", WORD_FIRST, 1411),
    at("an older D3D11 DDI, bit 4", WORD_FIRST, 1412),
];

/// The newest Direct3D 11 interface version, which carries the hardware-queue callbacks the
/// compositor needs. Every record offers it.
pub const NEWEST_DDI_VERSION: Feature = DDI_VERSIONS[DDI_VERSIONS.len() - 1];

/// Every Direct3D 11 interface version a real card offers: Intel's kernel driver offers the five
/// older ones and the newest from its data and the rest as it starts, and its own workaround rules
/// for an Arc B580 come out as a real card's only with all of them offered.
pub fn every_ddi_version() -> impl Iterator<Item = Feature> {
    OLDER_DDI_VERSIONS.into_iter().chain(DDI_VERSIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::bytes::{dword, qword};

    #[test]
    fn a_flag_lands_on_its_bit_of_its_word() {
        let mut record = vec![0u8; 0x400];
        set(&mut record, WDDM2_GPU_MMU, true);
        assert_eq!(qword(&record, WORD_B), Some(1 << 46));
        set(&mut record, COMPRESSION[3], true);
        assert_eq!(dword(&record, WORD_C), Some(1 << 2));
        set(&mut record, WDDM2_GPU_MMU, false);
        assert_eq!(qword(&record, WORD_B), Some(0));
        assert!(is_set(&record, COMPRESSION[3]));
    }

    #[test]
    fn the_flags_each_have_a_place_of_their_own() {
        let all = [
            PPGTT,
            SVM,
            WDDM2_GPU_MMU,
            WDDM2_SVM,
            WDDM2_64KB_PAGES,
            LOCAL_MEMORY,
            TILE_Y,
            HW_SEMAPHORE_64,
            BLITTER_RING,
            CCS_RING,
            CCS_NODE,
            CCS_MULTI_INSTANCE,
            HW_SCHEDULING,
            XE2_TILING,
            PML5,
            WALKER_MTP,
            GPGPU_MID_BATCH_PREEMPT,
            GPGPU_THREAD_GROUP_PREEMPT,
            GPGPU_MID_THREAD_PREEMPT,
            COMPRESSION[0],
            COMPRESSION[1],
            COMPRESSION[2],
            COMPRESSION[3],
            PIPELINE_LEVELS[0],
            PIPELINE_LEVELS[1],
            PIPELINE_LEVELS[2],
            PIPELINE_LEVELS[3],
            TILE_MAPPED_RESOURCE,
            TRANSLATION_TABLE,
            USER_MODE_TRANSLATION_TABLE,
            NULL_PAGES,
            LOCAL_MEMORY_ALLOWS_4KB,
            DRIVER_FLR,
            RENDER_COMPRESSION,
        ];
        for (at, one) in all.iter().enumerate() {
            for other in &all[at + 1..] {
                assert_ne!((one.byte, one.bit), (other.byte, other.bit), "{} {}", one.name, other.name);
            }
        }
        assert_eq!((HW_SCHEDULING.byte, HW_SCHEDULING.bit), (0x2EC + 6, 1));
        assert_eq!((BLITTER_RING.byte, BLITTER_RING.bit), (0x2B4 + 1, 6));
        // By their bit in the table, as the data counts them.
        for (feature, index) in [
            (TILE_MAPPED_RESOURCE, 413),
            (TRANSLATION_TABLE, 414),
            (USER_MODE_TRANSLATION_TABLE, 415),
            (NULL_PAGES, 416),
            (LOCAL_MEMORY_ALLOWS_4KB, 452),
            (DRIVER_FLR, 488),
            (SVM, 411),
            (HW_SCHEDULING, 497),
        ] {
            assert_eq!(feature, bit(feature.name, index), "{}", feature.name);
        }
    }

    #[test]
    fn the_ddi_versions_are_bits_5_to_15_of_the_dword_at_0x364() {
        let mut record = vec![0u8; 0x400];
        for version in DDI_VERSIONS {
            set(&mut record, version, true);
        }
        assert_eq!(dword(&record, 0x364), Some(0xFFE0));
        assert_eq!(record.iter().filter(|&&byte| byte != 0).count(), 2);
    }

    #[test]
    fn the_older_ddi_versions_are_the_low_five_bits_and_every_version_is_the_low_sixteen() {
        let mut record = vec![0u8; 0x400];
        for version in OLDER_DDI_VERSIONS {
            set(&mut record, version, true);
        }
        assert_eq!(dword(&record, 0x364), Some(0x1F));
        for version in every_ddi_version() {
            set(&mut record, version, true);
        }
        assert_eq!(dword(&record, 0x364), Some(0xFFFF));
        assert_eq!((NEWEST_DDI_VERSION.byte, NEWEST_DDI_VERSION.bit), (0x365, 7));
    }

    #[test]
    fn the_pipeline_levels_are_bits_0_1_3_and_4_of_byte_0x305() {
        let mut record = vec![0u8; 0x400];
        for level in PIPELINE_LEVELS {
            set(&mut record, level, true);
        }
        assert_eq!(record[0x305], 0x1B);
        assert_eq!(record.iter().filter(|&&byte| byte != 0).count(), 1);
    }

    #[test]
    fn a_workaround_entry_is_a_nibble() {
        let mut record = vec![0u8; 0x600];
        record[0x584] = 0xA0;
        set_workaround(&mut record, 968, 1);
        assert_eq!(record[0x584], 0xA1, "the low half, the high half kept");
        set_workaround(&mut record, 969, 0x1F);
        assert_eq!((record[0x584], workaround(&record, 969)), (0xF1, Some(0xF)));
        set_workaround(&mut record, 100_000, 1);
        assert_eq!(workaround(&record, 100_000), None);
    }
}
