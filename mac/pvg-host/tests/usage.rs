// SPDX-License-Identifier: GPL-2.0-or-later
//! The program's usage, byte for byte what the Mac app expects: the app tells a build that
//! publishes frames by its `--frames`, and a daemon that takes its place compares it line for
//! line. Nothing here reaches the device.
#![cfg(target_os = "macos")]

use std::process::Command;

const USAGE: &str = "pvg-host: usage: guesthouse-pvg-host --socket PATH [--control PATH] [--modes WxH@R,...] [--name NAME] [--window] [--frames NAME]\n       guesthouse-pvg-host --option-rom\n";

fn run(arguments: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_guesthouse-pvg-host"))
        .args(arguments)
        .output()
        .expect("the program starts");
    let text = |bytes: Vec<u8>| String::from_utf8(bytes).expect("text");
    (output.status.code(), text(output.stdout), text(output.stderr))
}

#[test]
fn the_usage_is_what_the_app_expects_and_goes_to_standard_error() {
    for arguments in [&[][..], &["--help"], &["--bogus"], &["--control", "c.sock"]] {
        assert_eq!(run(arguments), (Some(1), String::new(), USAGE.to_owned()), "{arguments:?}");
    }
    let (code, out, error) = run(&["--socket"]);
    assert_eq!((code, out), (Some(1), String::new()));
    assert_eq!(error, format!("pvg-host: --socket wants a value\n{}", &USAGE["pvg-host: ".len()..]));
}
