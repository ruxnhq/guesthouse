//! The driver-update query, KMTQAITYPE_CHECKDRIVERUPDATESTATUS: whether the adapter's driver is
//! being updated. The core answers no.
//!
//! While the desktop is in software the compositor is refused Intel's drivers, but it still tries a
//! Direct3D 11 device on the Intel adapter, which is the first one it sees, for each thing it draws
//! on its default device. Each failure counts against it, and past a limit it fails fast, so a burst
//! of tries at logon or as an app starts kills it over and over until Windows stops the adapter. It
//! doesn't count failures while a driver update is in progress, so the compositor alone is told one
//! is, and draws in software as it would during an update.

use crate::stage::Desktop;
use guesthouse_backend::{Status, VendorQuery};

/// KMTQAITYPE_CHECKDRIVERUPDATESTATUS.
pub const QUERY_DRIVER_UPDATE_STATUS: u32 = 11;
const ANSWER_SIZE: usize = 4;
const IN_PROGRESS: u32 = 1;

/// Says an update is in progress, to the compositor on the software desktop. `None` leaves every
/// other asker to the core.
pub fn answer(query: &VendorQuery<'_>, desktop: Desktop, out: &mut Vec<u8>) -> Option<Status> {
    let ours = query.kind == QUERY_DRIVER_UPDATE_STATUS
        && query.size == ANSWER_SIZE
        && desktop.tells_compositor_of_an_update(query.program);
    if !ours {
        return None;
    }
    out.extend_from_slice(&IN_PROGRESS.to_le_bytes());
    Some(Status::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(kind: u32, size: usize, program: &str) -> VendorQuery<'_> {
        VendorQuery { kind, size, data: &[0; 4], process_is_32_bit: false, program }
    }

    #[test]
    fn only_the_compositor_on_the_software_desktop_hears_of_an_update() {
        let mut out = Vec::new();
        let query = asked(QUERY_DRIVER_UPDATE_STATUS, 4, "dwm.exe");
        assert_eq!(answer(&query, Desktop::Software, &mut out), Some(Status::SUCCESS));
        assert_eq!(out, 1u32.to_le_bytes());
        out.clear();
        assert_eq!(answer(&query, Desktop::Hardware, &mut out), None, "the card's desktop");
        let others = [
            asked(QUERY_DRIVER_UPDATE_STATUS, 4, "explorer.exe"),
            asked(QUERY_DRIVER_UPDATE_STATUS, 8, "dwm.exe"),
        ];
        for query in others {
            assert_eq!(answer(&query, Desktop::Software, &mut out), None);
        }
        assert_eq!(answer(&asked(12, 4, "dwm.exe"), Desktop::Software, &mut out), None);
        assert!(out.is_empty());
    }
}
