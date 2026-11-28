// SPDX-License-Identifier: GPL-2.0-or-later
//! Builds the Objective-C file that calls Apple's framework, on a macOS target only. Elsewhere the
//! crate is its guard and its memory, which need no framework.
//!
//! The compiler is called as it is, with no crate between: it is one file, and macOS has `xcrun`
//! wherever it has the framework's headers.

use std::env;
use std::path::PathBuf;
use std::process::Command;

const SOURCE: &str = "shim/pvg_shim.m";
const FRAMEWORKS: [&str; 5] = ["Foundation", "Metal", "QuartzCore", "IOSurface", "ParavirtualizedGraphics"];

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|error| panic!("{command:?} did not start: {error}"));
    assert!(status.success(), "{command:?} failed");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={SOURCE}");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let object = out.join("pvg_shim.o");
    let library = out.join("libpvg_shim.a");
    // The object goes into an archive of one, which is the form rustc links.
    run(Command::new("xcrun")
        .args(["clang", "-fobjc-arc", "-O2", "-g", "-Wall", "-Wextra", "-Werror", "-c", SOURCE, "-o"])
        .arg(&object));
    let _ = std::fs::remove_file(&library);
    run(Command::new("xcrun").args(["ar", "crs"]).arg(&library).arg(&object));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=pvg_shim");
    for framework in FRAMEWORKS {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
