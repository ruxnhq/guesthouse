//! A Windows guest's screen, through remoteman. What has no Windows in it lives in
//! `guesthouse-remote-view-core`, which every host builds on; this keeps the names the Windows host
//! has always used for it, and the one thing that is about its own disks.

use std::path::{Path, PathBuf};

pub use guesthouse_remote_view_core::{
    GUEST_FOLDER, GUEST_SETUP, PORT, SCRIPT, SETUP, encoder_for, fingerprint, fingerprint_in, trust_list,
    viewer_identity,
};

/// The viewer's state folder for a machine whose disk is `disk`, beside it.
pub fn state_beside(disk: &Path) -> PathBuf {
    disk.with_extension("screen")
}
