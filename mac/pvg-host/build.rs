// SPDX-License-Identifier: GPL-2.0-or-later
//! Builds the C and Objective-C files in `shim/` into one archive, on a macOS target only: the
//! window, the frame export, and the rmframes files it speaks through. As in `guesthouse-pvg-sys`,
//! the compiler is called as it is.
//!
//! The rmframes files are plain C, the Mac app's `mac/rmframes/` with the publishing side beside it
//! (see `shim/rmframes/SOURCE`), and are built without ARC: they release their dispatch objects
//! themselves. The reading side goes into the archive too, for the crate's test; the program
//! never calls it, so the linker leaves it out.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Objective-C, with ARC.
const OBJECTIVE_C: [&str; 2] = ["shim/window.m", "shim/export.m"];
/// Plain C.
const C: [&str; 4] = [
    "shim/rmframes/rmframes_pub.c",
    "shim/rmframes/rmframes_recv.c",
    "shim/rmframes/rmframes_sub.c",
    "shim/rmframes/rmframes_peer.c",
];
/// What the C files include, for cargo to watch.
const HEADERS: [&str; 5] = [
    "shim/rmframes/rmframes.h",
    "shim/rmframes/rmframes_peer.h",
    "shim/rmframes/rmframes_pub.h",
    "shim/rmframes/rmframes_pub_internal.h",
    "shim/rmframes/rmframes_sub.h",
];
const FRAMEWORKS: [&str; 9] = [
    "AppKit",
    "QuartzCore",
    "CoreGraphics",
    "Carbon",
    "Metal",
    "IOSurface",
    "Foundation",
    "CoreFoundation",
    "Security",
];
const WARNINGS: [&str; 4] = ["-Wall", "-Wextra", "-Werror", "-O2"];

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|error| panic!("{command:?} did not start: {error}"));
    assert!(status.success(), "{command:?} failed");
}

/// Compiles one file into an object in `out`, and returns the object's path.
fn compile(source: &str, arc: bool, out: &Path) -> PathBuf {
    let stem = Path::new(source).file_stem().expect("a file name").to_string_lossy().into_owned();
    let object = out.join(format!("{stem}.o"));
    let mut command = Command::new("xcrun");
    command.arg("clang").args(WARNINGS).arg("-g").arg("-Ishim");
    if arc {
        command.arg("-fobjc-arc");
    }
    run(command.args(["-c", source, "-o"]).arg(&object));
    object
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for file in OBJECTIVE_C.iter().chain(&C).chain(&HEADERS) {
        println!("cargo:rerun-if-changed={file}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let mut objects: Vec<PathBuf> = OBJECTIVE_C.iter().map(|source| compile(source, true, &out)).collect();
    objects.extend(C.iter().map(|source| compile(source, false, &out)));
    let library = out.join("libpvg_host_shim.a");
    let _ = std::fs::remove_file(&library);
    run(Command::new("xcrun").args(["ar", "crs"]).arg(&library).args(&objects));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=pvg_host_shim");
    for framework in FRAMEWORKS {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
