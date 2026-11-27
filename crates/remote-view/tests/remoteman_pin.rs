//! The remoteman release the Windows installer carries, held byte for byte as it was pinned: the file
//! `scripts/windows-setup.ps1` reads its version, address and SHA-256s from is found the way the script
//! finds it, and has to be this text, so moving the pin can't change what the installer fetches and
//! checks, and can't leave the script looking where the pin no longer is.

use std::fs;
use std::path::{Path, PathBuf};

/// The pin as it was when this test was written, the file's whole text.
const PINNED: &str = r#"{
  "version": "0.1.10",
  "base": "https://files.ruxn.com/p/remoteman/",
  "host_setup": {
    "file": "remoteman-host-setup-0.1.10.exe",
    "sha256": "9e163215ed120d237bbbc30940360f273a61684a01ad1e23ca33a9872b3477f4"
  },
  "viewer_zip": {
    "file": "remoteman-viewer-windows-0.1.10.zip",
    "sha256": "9db08a962c45850553ad4440ece7f25d3d534d7d8959de6d85d5c6f8ad70c9cf"
  }
}
"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The one file the script's `Read-Pin` reads: `Join-Path $root 'FOLDER\remoteman.json'`.
fn read_by_the_script() -> PathBuf {
    let script = fs::read_to_string(repo().join("scripts").join("windows-setup.ps1")).unwrap();
    let start = script.find("function Read-Pin {").expect("windows-setup.ps1 has Read-Pin");
    let body = &script[start..];
    let body = &body[..body.find("\n}").unwrap()];
    let found: Vec<&str> = body
        .lines()
        .filter_map(|line| line.split("Join-Path $root '").nth(1))
        .filter_map(|rest| rest.split('\'').next())
        .collect();
    assert_eq!(found.len(), 1, "Read-Pin reads one file: {found:?}");
    assert!(found[0].ends_with("remoteman.json"), "{found:?}");
    found[0].split('\\').fold(repo(), |path, part| path.join(part))
}

#[test]
fn the_installer_script_reads_the_pin_it_always_had() {
    let path = read_by_the_script();
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    // A checkout may turn line ends into Windows' own; PowerShell reads either the same.
    assert_eq!(text.replace("\r\n", "\n"), PINNED);
}
