// SPDX-License-Identifier: GPL-2.0-or-later
//! Places a developer's own builds of the programs the app runs can be found, tried after every
//! other place. A public build has none: the lab's list lives in the lab repo's copy of this file,
//! behind a `lab` feature that is not part of this tree.

use crate::tools::Tool;
use std::path::{Path, PathBuf};

/// The developer's own builds of `tool` under `home`, newest first. None here.
pub fn builds(tool: Tool, home: &Path) -> Vec<PathBuf> {
    let _ = (tool, home);
    Vec::new()
}

/// A developer's folder of QEMU's firmware. None here.
pub fn firmware_dir(home: &Path) -> Option<PathBuf> {
    let _ = home;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_build_names_no_place_of_its_own() {
        for tool in [Tool::Qemu, Tool::Rmhost, Tool::PvgHost, Tool::PvgQemu] {
            assert!(builds(tool, Path::new("/h")).is_empty());
        }
        assert!(firmware_dir(Path::new("/h")).is_none());
    }
}
