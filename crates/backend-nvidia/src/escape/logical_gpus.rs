//! The logical GPUs NVAPI asks the kernel driver for, and how they pair with the physical ones.
//!
//! NVAPI keeps its own table of GPUs and finds a physical handle there before any call on it. It
//! fills in each GPU's kind from the logical GPU list, so with that list empty every call on the
//! handle from the GPU list fails as an invalid handle: the card's name, its architecture, and the
//! check NGX, which runs DLSS, makes before anything else. One card is one logical GPU of one
//! physical GPU. The layouts are from NVAPI's own reading of these answers (nvapi64_impl.dll 617.42).

use super::{get, put};

const COUNT: usize = 0x34;
const ENTRIES: usize = 0x38;
const ENTRY_SIZE: usize = 0x214;
// Within an entry.
const LOGICAL_HANDLE: usize = 0x000;
const GPU_IDS: usize = 0x004;
const SUBDEVICES: usize = 0x104;
const GPU_COUNT: usize = 0x204;
/// Logical GPUs with the same key are one SLI group.
const GROUP_KEY: usize = 0x208;
const VALID: usize = 0x20C;
const KIND: usize = 0x210;
/// A GPU under the display driver, as the GPU list says too.
const KIND_DISPLAY_DRIVER: u32 = 1;

// The two lookups: the handle asked about, then the answer.
const ASKED: usize = 0x30;
const FOUND: usize = 0x34;
const FOUND_HANDLES: usize = 0x38;

/// NVAPI takes a logical handle only with this top byte.
const LOGICAL_MARK: u32 = 0xAA00_0000;

/// The logical GPU's handle, made from the physical one, which is the RM's number for the card.
pub(super) fn logical_handle(gpu_id: u32) -> u32 {
    LOGICAL_MARK | (gpu_id & 0x00FF_FFFF)
}

/// The list of logical GPUs: this card alone.
pub(super) fn list(gpu_id: u32, data: &mut [u8]) {
    if data.len() < ENTRIES + ENTRY_SIZE {
        return;
    }
    let entry = ENTRIES;
    put(data, COUNT, 1);
    put(data, entry + LOGICAL_HANDLE, logical_handle(gpu_id));
    put(data, entry + GPU_IDS, gpu_id);
    put(data, entry + SUBDEVICES, 0);
    data[entry + GPU_COUNT] = 1;
    put(data, entry + GROUP_KEY, gpu_id);
    data[entry + VALID] = 1;
    put(data, entry + KIND, KIND_DISPLAY_DRIVER);
}

/// The logical GPU of a physical one. Another handle finds none.
pub(super) fn logical_of(gpu_id: u32, data: &mut [u8]) {
    if get(data, ASKED) == Some(gpu_id) {
        put(data, FOUND, logical_handle(gpu_id));
    }
}

/// The physical GPUs of a logical one. Another handle finds none.
pub(super) fn physical_of(gpu_id: u32, data: &mut [u8]) {
    if get(data, ASKED) == Some(logical_handle(gpu_id)) {
        put(data, FOUND, 1);
        put(data, FOUND_HANDLES, gpu_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_has_one_valid_logical_gpu_of_this_card() {
        let mut data = vec![0; ENTRIES + ENTRY_SIZE * 2];
        list(0x100, &mut data);
        assert_eq!(get(&data, COUNT), Some(1));
        assert_eq!(get(&data, ENTRIES), Some(0xAA00_0100));
        assert_eq!(get(&data, ENTRIES + GPU_IDS), Some(0x100));
        assert_eq!(data[ENTRIES + GPU_COUNT], 1);
        assert_eq!(data[ENTRIES + VALID], 1);
        assert_eq!(get(&data, ENTRIES + KIND), Some(KIND_DISPLAY_DRIVER));
    }

    #[test]
    fn a_buffer_too_short_for_an_entry_is_left_alone() {
        let mut data = vec![0; ENTRIES + 8];
        list(0x100, &mut data);
        assert!(data.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn the_card_and_its_logical_gpu_find_each_other() {
        let mut data = vec![0; 0x40];
        put(&mut data, ASKED, 0x100);
        logical_of(0x100, &mut data);
        assert_eq!(get(&data, FOUND), Some(0xAA00_0100));

        let mut data = vec![0; 0x40];
        put(&mut data, ASKED, 0xAA00_0100);
        physical_of(0x100, &mut data);
        assert_eq!(get(&data, FOUND), Some(1));
        assert_eq!(get(&data, FOUND_HANDLES), Some(0x100));
    }

    #[test]
    fn another_handle_finds_nothing() {
        let mut data = vec![0; 0x40];
        put(&mut data, ASKED, 0x200);
        logical_of(0x100, &mut data);
        physical_of(0x100, &mut data);
        assert_eq!(get(&data, FOUND), Some(0));
    }
}
