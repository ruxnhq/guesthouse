//! What the adapter record leaves at zero because nothing here knows the right value or place yet.
//!
//! Each is a part of the record the guest's driver may read. None is made up: a field stays zero
//! until the card, Intel's open sources or a guest's own traffic says what belongs there and
//! where. The log names every one at start, so a tester's log says what the guest was not told.

/// The parts of the record always left at zero, in words for the log.
pub const RECORD: [&str; 13] = [
    "the kernel driver's interface version (the record's first dword)",
    "the PCH family and the chipset's LPC device, which the kernel driver reads from the chipset",
    "the platform word between the display and render IP versions",
    "the core and bus frequencies, the power limit and the fill rate",
    "the command buffer, patch list and allocation list sizes",
    "the kernel driver's capability bits, but the one that says it runs from the driver store",
    "the cache sizes and bank count",
    "the slice array's slots past the ones seen: dual subslices past the sixth in a slice, and the \
     flag that says slices are populated dynamically",
    "the video and video-enhancement engines' SFC support bits, and the multi-tile block of the GPU's shape",
    "the GPU address partition's eleventh range, which has no name",
    "the adapter's bus, device and function",
    "the names, counts and flags at the record's end",
    "the deferred wait block",
];

/// What is left at zero on a card the kernel driver's data doesn't list, which the data fills on
/// one it does ([`super::card_data`]).
const WITHOUT_DATA: [&str; 6] = [
    "the display IP version",
    "the media IP version",
    "the per-adapter block after the platform",
    "the hwconfig fields this backend doesn't name",
    "the feature flags whose places aren't known: Tile64Optimization, the video and video-enhancement \
     ring flags, and the rest of the table",
    "the feature flags whose places are only inferred: 3D mid-batch preemption, 3D object-level \
     preemption and the per-context preemption granularity control",
];

/// What is left at zero on a card the data lists, in place of [`WITHOUT_DATA`].
const WITH_DATA: [&str; 2] = [
    "the per-adapter block past the per-IP core families, and those core families on Alchemist",
    "the feature flags the kernel driver's data doesn't know, Tile64Optimization's place among them",
];

/// The slice, subslice and EU arrays, left empty when the card gave no topology or no hwconfig
/// that says how its cores divide into slices.
const SLICE_ARRAYS: &str = "the slice, subslice and EU arrays of the GPU's shape, which compute-runtime \
     needs to open a device (the card did not say how its cores divide into slices)";

/// What a card with memory of its own is not told about it. An integrated card has no use for these.
pub const LOCAL_MEMORY: &str = "what goes with memory on the card: which kinds of memory may or should be \
     placed there, and on a card the kernel driver's data doesn't list, whether it takes 4 KiB pages";

/// Everything left at zero in a record, given whether its slice arrays were filled and whether the
/// kernel driver's data listed the card.
pub fn unset(slice_arrays_filled: bool, from_data: bool) -> Vec<&'static str> {
    let mut gaps = RECORD.to_vec();
    gaps.extend(if from_data { &WITH_DATA[..] } else { &WITHOUT_DATA[..] });
    if !slice_arrays_filled {
        gaps.push(SLICE_ARRAYS);
    }
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slice_arrays_are_a_gap_only_when_not_filled() {
        assert_eq!(unset(true, true).len(), RECORD.len() + WITH_DATA.len());
        assert_eq!(unset(false, true).len(), RECORD.len() + WITH_DATA.len() + 1);
        assert!(unset(false, true).last().is_some_and(|gap| gap.contains("slice")));
    }

    #[test]
    fn a_card_the_data_lists_has_fewer_gaps() {
        assert_eq!(unset(true, false).len(), RECORD.len() + WITHOUT_DATA.len());
        assert!(unset(true, false).contains(&"the media IP version"));
        assert!(!unset(true, true).contains(&"the media IP version"));
    }
}
