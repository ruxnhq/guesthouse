//! The card's hardware configuration table, which xe passes on from the GuC as it is.
//!
//! The table is a list of key, length and value: a dword key, a dword length in dwords, then that
//! many dwords. The key numbers are Mesa's, from `src/intel/dev/intel_hwconfig_types.h`, which is
//! MIT. Only the keys the adapter record uses are named here; the probe prints the rest by number.

use crate::protocol::bytes::dword;

pub const MAX_SLICES_SUPPORTED: u32 = 1;
pub const MAX_DUAL_SUBSLICES_SUPPORTED: u32 = 2;
pub const MAX_NUM_EU_PER_DSS: u32 = 3;
pub const NUM_THREADS_PER_EU: u32 = 15;
/// The threads each 3D stage may have across the GPU.
pub const TOTAL_VS_THREADS: u32 = 16;
pub const TOTAL_GS_THREADS: u32 = 17;
pub const TOTAL_HS_THREADS: u32 = 18;
pub const TOTAL_DS_THREADS: u32 = 19;
pub const TOTAL_VS_THREADS_POCS: u32 = 20;
pub const TOTAL_PS_THREADS: u32 = 21;
/// The most render, compute, video decode, video enhancement and copy engines, in that order.
pub const MAX_RCS: u32 = 23;
pub const MAX_CCS: u32 = 24;
pub const MAX_VCS: u32 = 25;
pub const MAX_VECS: u32 = 26;
pub const MAX_COPY_CS: u32 = 27;
/// The least and most URB entries of each stage, least first.
pub const MIN_VS_URB_ENTRIES: u32 = 29;
pub const MAX_VS_URB_ENTRIES: u32 = 30;
pub const MIN_HS_URB_ENTRIES: u32 = 33;
pub const MAX_HS_URB_ENTRIES: u32 = 34;
pub const MIN_GS_URB_ENTRIES: u32 = 35;
pub const MAX_GS_URB_ENTRIES: u32 = 36;
pub const MIN_DS_URB_ENTRIES: u32 = 37;
pub const MAX_DS_URB_ENTRIES: u32 = 38;
pub const MAX_URB_STARTING_ADDRESS: u32 = 47;
pub const URB_SIZE_PER_SLICE_IN_KB: u32 = 68;
pub const MIN_TASK_URB_ENTRIES: u32 = 77;
pub const MAX_TASK_URB_ENTRIES: u32 = 78;
pub const MIN_MESH_URB_ENTRIES: u32 = 79;
pub const MAX_MESH_URB_ENTRIES: u32 = 80;
/// A card whose table has no dual subslice keys gives the same two numbers under these, as
/// subslices. On Xe a subslice and a dual subslice are both the Xe core.
pub const MAX_SUBSLICE: u32 = 70;
pub const MAX_EU_PER_SUBSLICE: u32 = 71;

/// The names of the keys above, for the probe.
const NAMES: [(u32, &str); 6] = [
    (MAX_SLICES_SUPPORTED, "max slices supported"),
    (MAX_DUAL_SUBSLICES_SUPPORTED, "max dual subslices supported"),
    (MAX_NUM_EU_PER_DSS, "max EUs per dual subslice"),
    (NUM_THREADS_PER_EU, "threads per EU"),
    (MAX_SUBSLICE, "max subslices"),
    (MAX_EU_PER_SUBSLICE, "max EUs per subslice"),
];

/// One entry of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: u32,
    pub values: Vec<u32>,
}

impl Entry {
    pub fn name(&self) -> Option<&'static str> {
        NAMES.iter().find(|(key, _)| *key == self.key).map(|(_, name)| *name)
    }
}

/// The whole table, as xe gave it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hwconfig {
    pub entries: Vec<Entry>,
}

impl Hwconfig {
    /// Reads entries until the bytes run out. An entry whose length runs past the end is dropped,
    /// with everything after it, as there is no telling where the next would start.
    pub fn parse(table: &[u8]) -> Self {
        let mut entries = Vec::new();
        let mut at = 0;
        while let (Some(key), Some(length)) = (dword(table, at), dword(table, at + 4)) {
            let start = at + 8;
            let values: Option<Vec<u32>> =
                (0..length as usize).map(|index| dword(table, start + index * 4)).collect();
            let Some(values) = values else { break };
            entries.push(Entry { key, values });
            at = start + length as usize * 4;
        }
        Self { entries }
    }

    /// The first value of a key, if the table has it.
    pub fn value(&self, key: u32) -> Option<u32> {
        self.entries.iter().find(|entry| entry.key == key)?.values.first().copied()
    }

    /// The most Xe cores the platform has, under whichever key the table gives it.
    pub fn most_cores(&self) -> Option<u32> {
        self.value(MAX_DUAL_SUBSLICES_SUPPORTED).or_else(|| self.value(MAX_SUBSLICE))
    }

    /// The most EUs an Xe core has, under whichever key the table gives it.
    pub fn most_eus_per_core(&self) -> Option<u32> {
        self.value(MAX_NUM_EU_PER_DSS).or_else(|| self.value(MAX_EU_PER_SUBSLICE))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A table of the given keys, each with one value.
    pub fn table(pairs: &[(u32, u32)]) -> Vec<u8> {
        pairs.iter().flat_map(|&(key, value)| [key, 1, value]).flat_map(u32::to_le_bytes).collect()
    }

    #[test]
    fn entries_are_read_by_their_length() {
        let mut bytes = table(&[(MAX_SLICES_SUPPORTED, 1)]);
        bytes.extend([99u32, 2, 7, 8].iter().flat_map(|word| word.to_le_bytes()));
        bytes.extend(table(&[(NUM_THREADS_PER_EU, 8)]));
        let config = Hwconfig::parse(&bytes);
        assert_eq!(config.entries.len(), 3);
        assert_eq!(config.entries[1].values, [7, 8]);
        assert_eq!(config.value(NUM_THREADS_PER_EU), Some(8));
        assert_eq!(config.entries[2].name(), Some("threads per EU"));
        assert_eq!(config.value(MAX_NUM_EU_PER_DSS), None);
    }

    #[test]
    fn the_cores_are_found_under_either_pair_of_keys() {
        let dual = Hwconfig::parse(&table(&[(MAX_DUAL_SUBSLICES_SUPPORTED, 8), (MAX_NUM_EU_PER_DSS, 8)]));
        assert_eq!((dual.most_cores(), dual.most_eus_per_core()), (Some(8), Some(8)));
        let plain = Hwconfig::parse(&table(&[(MAX_SUBSLICE, 20), (MAX_EU_PER_SUBSLICE, 8)]));
        assert_eq!((plain.most_cores(), plain.most_eus_per_core()), (Some(20), Some(8)));
        assert_eq!(plain.entries[0].name(), Some("max subslices"));
        assert_eq!(Hwconfig::default().most_cores(), None);
    }

    #[test]
    fn an_entry_running_past_the_end_stops_the_table() {
        let mut bytes = table(&[(MAX_SLICES_SUPPORTED, 1)]);
        bytes.extend([5u32, 3, 1].iter().flat_map(|word| word.to_le_bytes()));
        assert_eq!(Hwconfig::parse(&bytes).entries.len(), 1);
    }
}
