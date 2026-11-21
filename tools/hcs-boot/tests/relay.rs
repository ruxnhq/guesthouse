//! The serial relay in its real jail, fed what a guest might send.

#![cfg(windows)]

use guesthouse_jail::windows::{Handles, Jail};
use std::io::{self, Read, Write};
use std::os::windows::io::OwnedHandle;
use std::path::Path;

#[test]
fn the_jailed_relay_passes_lines_on_and_shows_escapes_as_codes() {
    let (relay_input, mut guest) = io::pipe().unwrap();
    let (mut lines, relay_output) = io::pipe().unwrap();
    let handles = Handles {
        stdin: Some(OwnedHandle::from(relay_input)),
        stderr: Some(OwnedHandle::from(relay_output.try_clone().unwrap())),
        stdout: Some(OwnedHandle::from(relay_output)),
    };
    let program = Path::new(env!("CARGO_BIN_EXE_guesthouse-serial-relay"));
    let jailed = Jail::new("guesthouse.serial-relay-test").spawn(program, &[], handles).unwrap();

    guest.write_all(b"guesthouse: card: ok\r\n\x1b]52;c;bad\x07title\nno end").unwrap();
    drop(guest);
    let mut said = String::new();
    lines.read_to_string(&mut said).unwrap();

    assert_eq!(jailed.wait().unwrap(), 0, "the relay said: {said}");
    assert_eq!(said.replace("\r\n", "\n"), "guesthouse: card: ok\n<1b>]52;c;bad<07>title\nno end\n");
}
