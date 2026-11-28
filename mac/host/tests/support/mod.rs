// SPDX-License-Identifier: GPL-2.0-or-later
//! Stand-ins for what an install finds on a developer's Mac, so the tests need nothing outside
//! the checkout: QEMU's firmware files.

use std::fs;
use std::path::{Path, PathBuf};

/// Makes `root/firmware` with the files an install looks for. Returns its path.
pub fn firmware_dir(root: &Path) -> PathBuf {
    let firmware = root.join("firmware");
    fs::create_dir_all(&firmware).unwrap();
    for name in ["edk2-aarch64-code.fd", "edk2-aarch64-vars.fd", "pvgfb.rom"] {
        fs::write(firmware.join(name), "x").unwrap();
    }
    firmware
}
