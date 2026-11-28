//! A throwaway run's disk: an overlay the app opened and deleted, which QEMU finds in a set of its own.

use super::VmPlan;
use super::arguments::words;
use crate::text::qemu_value;
use std::os::fd::RawFd;

/// Where QEMU finds the overlay open: to write, and to read, as QEMU may open it again either way.
/// Clear of the drives' numbers and of the memory's.
pub const THROWAWAY_FDS: [RawFd; 2] = [50, 51];
/// Begins the note on the overlay's set, after which comes the machine's own disk. Nothing else in
/// QEMU's words names that disk then, and the app finds which machine a QEMU runs by this.
pub const THROWAWAY_MARK: &str = "guesthouse-throwaway:";

/// The name QEMU opens the overlay by.
pub fn throwaway_disk() -> String {
    format!("/dev/fdset/{}", THROWAWAY_FDS[0])
}

impl VmPlan {
    pub(super) fn throwaway_arguments(&self) -> Vec<String> {
        let Some(disk) = &self.throwaway else { return Vec::new() };
        let [writing, reading] = THROWAWAY_FDS;
        let set = writing;
        words(&[
            "-add-fd",
            &format!("fd={writing},set={set},opaque={THROWAWAY_MARK}{}", qemu_value(disk)),
            "-add-fd",
            &format!("fd={reading},set={set}"),
        ])
    }
}
