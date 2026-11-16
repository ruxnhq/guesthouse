//! The page of the GPU's timer, which the OpenCL runtime asks to have mapped into the program.
//!
//! The runtime reads the time from it to stamp when work was queued and sent. It checks only that
//! the escape went through, so a missing page makes it read from near address zero and crash. A
//! guest can't be handed the card's own registers, so the answer is a page every Windows program
//! can read, at a spot that holds nothing. The time read there stays at zero, and the start and
//! end times, which come from the GPU, are unchanged.

use super::put_long;

const OPERATION: usize = 0x30;
const ADDRESS: usize = 0x34;

const MAP: u8 = 0;

/// The tail of the page Windows shares read-only with every program. The runtime reads at 0x400
/// and 0x410 past what it is given, which lands in the zeroed end of that page.
const STAND_IN: u64 = 0x7FFE_0BE0;

/// Fills in where the page is. Letting go of it needs nothing.
pub(super) fn answer(data: &mut [u8]) {
    if data.get(OPERATION) == Some(&MAP) {
        put_long(data, ADDRESS, STAND_IN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_gives_the_stand_in_page() {
        let mut data = vec![0; ADDRESS + 8];
        answer(&mut data);
        assert_eq!(data[ADDRESS..].to_vec(), STAND_IN.to_le_bytes().to_vec());
    }

    #[test]
    fn the_reads_stay_in_that_page() {
        let page = STAND_IN & !0xFFF;
        for offset in [0x400, 0x410 + 4] {
            assert_eq!((STAND_IN + offset - 1) & !0xFFF, page);
        }
    }

    #[test]
    fn letting_go_writes_nothing() {
        let mut data = vec![0; ADDRESS + 8];
        data[OPERATION] = 1;
        answer(&mut data);
        assert!(data[ADDRESS..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_short_buffer_is_left_alone() {
        let mut data = vec![0; ADDRESS + 4];
        answer(&mut data);
        assert!(data.iter().all(|&byte| byte == 0));
    }
}
