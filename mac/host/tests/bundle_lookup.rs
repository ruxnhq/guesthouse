// SPDX-License-Identifier: GPL-2.0-or-later
#![cfg(target_os = "macos")]
//! The app finds its helpers inside its bundle, by the same call the app makes: with
//! GUESTHOUSE_BUNDLE_APP naming a Guesthouse.app that mac/nightly.sh made with --helpers, QEMU (the
//! one with the protected graphics device), the graphics process and rmhost are found in
//! Contents/Helpers, and QEMU's firmware in Contents/Resources/qemu. Without the variable there is
//! nothing to look in, and the test says so and passes. mac/Nightly/check-bundle.sh --lookup runs it.

use guesthouse_core::install::steps::{find_firmware, firmware_dirs_of};
use guesthouse_core::tools::{Places, Tool, locate_checked};
use std::path::PathBuf;

#[test]
fn the_helpers_and_the_firmware_are_found_inside_the_bundle() {
    let Some(app) = std::env::var_os("GUESTHOUSE_BUNDLE_APP").map(PathBuf::from) else {
        eprintln!("\n*** bundle_lookup SKIPPED: GUESTHOUSE_BUNDLE_APP names no app ***\n");
        return;
    };
    let helpers = app.join("Contents/Helpers");
    for tool in [Tool::Qemu, Tool::PvgQemu, Tool::PvgHost, Tool::Rmhost] {
        let bundled = helpers.join(tool.file_name());
        // Nothing else offered: no setting, no environment, and no developer's place.
        let places = Places { bundled: Some(bundled.clone()), ..Places::default() };
        let found = locate_checked(tool, &places).unwrap_or_else(|why| panic!("{tool:?}: {why}"));
        assert_eq!(found.path, bundled, "{tool:?}");
        assert_eq!(found.source.name(), "bundled", "{tool:?}");
    }
    let (code, vars) = find_firmware(&firmware_dirs_of(&helpers.join("qemu-system-aarch64")))
        .unwrap_or_else(|why| panic!("firmware: {}", why.message));
    assert!(!code.is_empty() && !vars.is_empty());
}
