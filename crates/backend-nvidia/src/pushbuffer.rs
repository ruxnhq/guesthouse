//! Ring entries and method headers: the words the GPU's command processor reads.
//!
//! The formats are in the open kernel module's class headers for channels.

/// Bytes in a ring entry.
pub const ENTRY_SIZE: usize = 8;

const ENTRY_ADDRESS_HIGH_MASK: u32 = 0xFF;
const ENTRY_LENGTH_SHIFT: u32 = 10;
const ENTRY_LENGTH_MASK: u32 = 0x1F_FFFF;
const ENTRY_NO_PREFETCH_OR_FETCH: u64 = 1 << 41;
const ENTRY_LENGTH_SHIFT_IN_U64: u32 = 42;

/// An entry with no length carries an opcode where the address's high bits would be. This one sets
/// the extended base for the entries after it, here to zero.
const ENTRY_OPCODE_SET_EXTENDED_BASE: u64 = 4;
const ENTRY_OPCODE_SHIFT_IN_U64: u32 = 32;
pub const ZERO_EXTENDED_BASE: u64 = ENTRY_OPCODE_SET_EXTENDED_BASE << ENTRY_OPCODE_SHIFT_IN_U64;

/// The channel classes made here whose headers have no such opcode: Turing's, and the one Ampere
/// and Ada share. It came with a later class.
const CHANNELS_WITH_NO_EXTENDED_BASE: [u32; 2] = [0xC46F, 0xC56F];

const HEADER_INCREASING: u32 = 0x2000_0000;
const HEADER_COUNT_SHIFT: u32 = 16;
const HEADER_SUBCHANNEL_SHIFT: u32 = 13;
const HEADER_MODE_SHIFT: u32 = 29;
const HEADER_COUNT_MASK: u32 = 0x1FFF;
const HEADER_METHOD_MASK: u32 = 0xFFF;
const HEADER_SUBCHANNEL_MASK: u32 = 7;
/// Modes whose header is followed by that many data words: increasing, non-increasing, increase-once.
const MODES_WITH_DATA: [u32; 3] = [1, 3, 5];

/// What a ring entry points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingEntry {
    pub address: u64,
    /// How many words.
    pub length: u32,
}

impl RingEntry {
    pub fn parse(entry: [u8; 8]) -> Self {
        let low = u32::from_le_bytes(entry[..4].try_into().expect("four bytes"));
        let high = u32::from_le_bytes(entry[4..].try_into().expect("four bytes"));
        Self {
            address: u64::from(low & !3) | u64::from(high & ENTRY_ADDRESS_HIGH_MASK) << 32,
            length: high >> ENTRY_LENGTH_SHIFT & ENTRY_LENGTH_MASK,
        }
    }
}

/// Whether a channel of this class has no opcode for the extended base, and faults on an entry
/// that sets it.
pub fn has_no_extended_base(channel_class: u32) -> bool {
    CHANNELS_WITH_NO_EXTENDED_BASE.contains(&channel_class)
}

/// Builds a ring entry for `words` commands at a GPU address.
pub fn ring_entry(address: u64, words: u32) -> u64 {
    (address & !3) | u64::from(words) << ENTRY_LENGTH_SHIFT_IN_U64 | ENTRY_NO_PREFETCH_OR_FETCH
}

/// A header for `count` words to consecutive methods.
pub fn increasing(subchannel: u32, method: u32, count: u32) -> u32 {
    HEADER_INCREASING | count << HEADER_COUNT_SHIFT | subchannel << HEADER_SUBCHANNEL_SHIFT | method >> 2
}

/// What a header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub mode: u32,
    pub count: u32,
    pub method: u32,
    pub subchannel: u32,
}

impl Header {
    pub fn parse(word: u32) -> Self {
        Self {
            mode: word >> HEADER_MODE_SHIFT,
            count: word >> HEADER_COUNT_SHIFT & HEADER_COUNT_MASK,
            method: (word & HEADER_METHOD_MASK) << 2,
            subchannel: word >> HEADER_SUBCHANNEL_SHIFT & HEADER_SUBCHANNEL_MASK,
        }
    }

    pub fn has_data(&self) -> bool {
        MODES_WITH_DATA.contains(&self.mode)
    }

    /// Words from this header to the next.
    pub fn stride(&self) -> usize {
        1 + if self.has_data() { self.count as usize } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_reads_back() {
        let entry = RingEntry::parse(ring_entry(0x12_3456_7890, 7).to_le_bytes());
        assert_eq!(entry, RingEntry { address: 0x12_3456_7890, length: 7 });
    }

    #[test]
    fn an_entry_that_sets_the_extended_base_points_at_no_commands() {
        assert_eq!(RingEntry::parse(ZERO_EXTENDED_BASE.to_le_bytes()).length, 0);
    }

    #[test]
    fn only_the_older_channel_classes_have_no_extended_base() {
        use crate::profile::{ADA, AMPERE, BLACKWELL, TURING};
        for generation in [&ADA, &AMPERE, &TURING] {
            assert!(has_no_extended_base(generation.channel_class), "{}", generation.name);
        }
        assert!(!has_no_extended_base(BLACKWELL.channel_class));
    }

    #[test]
    fn a_header_reads_back() {
        let header = Header::parse(increasing(4, 0x304, 1));
        assert_eq!(header, Header { mode: 1, count: 1, method: 0x304, subchannel: 4 });
        assert_eq!(header.stride(), 2);
    }

    #[test]
    fn a_header_with_immediate_data_has_nothing_after_it() {
        assert_eq!(Header::parse(4 << 29 | 3 << 16).stride(), 1);
    }
}
