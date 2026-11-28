//! Handles the host gives the guest.

const FIRST_HANDLE: u32 = 0x4000_1040;
const STEP: u32 = 0x40;

/// How many handles there are, from the first to the last that fits in 32 bits.
pub const HANDLE_COUNT: u32 = (u32::MAX - FIRST_HANDLE) / STEP + 1;

/// One sequence for every kind of object, so a handle never means two things at once.
///
/// Tens of millions of objects later the sequence comes to the end of the numbers and starts again.
/// From then on a number may still be in use, and only the session's tables can say: see
/// `Session::take_handle`, which skips those.
#[derive(Debug)]
pub struct HandleCounter {
    next: u32,
    started_again: bool,
}

impl HandleCounter {
    pub fn new() -> Self {
        Self { next: FIRST_HANDLE, started_again: false }
    }

    /// The next number in the sequence, and whether the sequence has handed it out before.
    pub fn take(&mut self) -> (u32, bool) {
        let handle = self.next;
        let handed_out_before = self.started_again;
        match self.next.checked_add(STEP) {
            Some(next) => self.next = next,
            None => {
                self.next = FIRST_HANDLE;
                self.started_again = true;
            }
        }
        (handle, handed_out_before)
    }

    /// A counter this many handles from the end of the numbers, for a test.
    #[cfg(test)]
    pub fn near_the_end(left: u32) -> Self {
        Self { next: FIRST_HANDLE + (HANDLE_COUNT - left) * STEP, started_again: false }
    }
}

impl Default for HandleCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_spaced_evenly() {
        let mut handles = HandleCounter::new();
        assert_eq!(handles.take(), (FIRST_HANDLE, false));
        assert_eq!(handles.take(), (FIRST_HANDLE + STEP, false));
    }

    #[test]
    fn handles_start_again_after_the_last() {
        let mut handles = HandleCounter::near_the_end(1);
        assert_eq!(handles.take(), (u32::MAX - STEP + 1, false), "the last that fits");
        assert_eq!(handles.take(), (FIRST_HANDLE, true));
        assert_eq!(handles.take(), (FIRST_HANDLE + STEP, true));
    }
}
