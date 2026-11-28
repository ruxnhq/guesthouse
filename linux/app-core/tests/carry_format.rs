//! The export file's format, held byte for byte: `carry_format/export.ghm` was written by the
//! writer as it is today, from the made-up files below, and every export written since has to read
//! the same way. Nothing in it is real: the "token", "password", "key" and TPM state are test words.

#![cfg(target_os = "linux")]

use guesthouse_app_core::carry::{MAGIC, Member, Reader, Writer, safe_path};
use std::io::Cursor;
use std::path::PathBuf;

const FIXTURE: &[u8] = include_bytes!("carry_format/export.ghm");

fn files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("machine.json", b"{\n  \"name\": \"Made-up machine\",\n  \"kind\": \"windows\"\n}\n".to_vec()),
        ("guest.token", b"made-up-test-token-0000".to_vec()),
        ("windows-password", b"made-up test password\n".to_vec()),
        ("ssh/id_ed25519", b"made-up test key, not a key\n".to_vec()),
        ("tpm/tpm2-00.permall", (0..=255).collect()),
        ("empty", Vec::new()),
        ("a/b/c/d", b"four deep".to_vec()),
        ("x+y_z-1.2", b"marks".to_vec()),
    ]
}

fn written() -> Vec<u8> {
    let mut writer = Writer::new(Vec::new()).unwrap();
    for (path, data) in files() {
        writer.file(path, data.len() as u64, &mut &data[..]).unwrap();
    }
    writer.end().unwrap()
}

#[test]
fn an_export_is_written_byte_for_byte_as_before() {
    assert_eq!(MAGIC, b"GUESTHOUSE-MACHINE 1\n");
    assert_eq!(written(), FIXTURE);
}

#[test]
fn the_saved_export_reads_back_whole() {
    let mut reader = Reader::new(Cursor::new(FIXTURE)).unwrap();
    for (path, data) in files() {
        let member = reader.next_file().unwrap().unwrap();
        assert_eq!(member, Member { path: PathBuf::from(path), size: data.len() as u64 });
        let mut read = Vec::new();
        reader.copy_to(&member, &mut read).unwrap();
        assert_eq!(read, data, "{path}");
    }
    assert_eq!(reader.next_file().unwrap(), None);
}

#[test]
fn each_way_a_file_goes_wrong_says_what_it_said() {
    let refusal = |data: &[u8]| -> String {
        let mut reader = match Reader::new(Cursor::new(data.to_vec())) {
            Ok(reader) => reader,
            Err(error) => return error.0,
        };
        loop {
            match reader.next_file() {
                Ok(Some(member)) => {
                    if let Err(error) = reader.copy_to(&member, &mut Vec::new()) {
                        return error.0;
                    }
                }
                Ok(None) => return String::new(),
                Err(error) => return error.0,
            }
        }
    };
    let not_ours = "This is not a machine exported from Guesthouse, or a newer Guesthouse wrote it.";
    let cut = "The file ends early. It may not have finished copying.";
    assert_eq!(refusal(FIXTURE), "");
    assert_eq!(refusal(b""), not_ours);
    assert_eq!(refusal(b"GUESTHOUSE-MACHINE 2\n"), not_ours);
    assert_eq!(refusal(&FIXTURE[..FIXTURE.len() - 1]), cut);
    assert_eq!(refusal(&FIXTURE[..MAGIC.len()]), cut);
    let mut damaged = FIXTURE.to_vec();
    let at = damaged.windows(9).position(|part| part == b"four deep").unwrap();
    damaged[at] = b'F';
    assert_eq!(refusal(&damaged), "a/b/c/d is damaged: it isn't what was exported.");
    let mut unsafe_name = MAGIC.to_vec();
    unsafe_name.extend_from_slice(b"{\"path\":\"../x\",\"size\":0}\n");
    assert_eq!(refusal(&unsafe_name), "The export names a file that can't be made safely: \"../x\"");
    let mut unknown = MAGIC.to_vec();
    unknown.extend_from_slice(b"{\"path\":\"a\",\"size\":0,\"mode\":1}\n");
    assert_eq!(refusal(&unknown), not_ours);
    let mut both = MAGIC.to_vec();
    both.extend_from_slice(b"{\"path\":\"a\",\"size\":0,\"end\":true}\n");
    assert_eq!(refusal(&both), not_ours);
    let mut fits = MAGIC.to_vec();
    fits.extend_from_slice(&[b' '; 4095]);
    fits.extend_from_slice(b"\n");
    assert_eq!(refusal(&fits), not_ours, "a line of 4096 bytes is read, and isn't ours");
    let mut long = MAGIC.to_vec();
    long.extend_from_slice(&[b' '; 4096]);
    long.extend_from_slice(b"\n");
    assert_eq!(refusal(&long), cut, "a line past 4096 bytes is cut short");
}

#[test]
fn a_name_that_cant_go_in_an_export_is_refused_by_the_writer() {
    let mut writer = Writer::new(Vec::new()).unwrap();
    let error = writer.file("../x", 0, &mut &b""[..]).unwrap_err();
    assert_eq!(error.0, "../x can't go in an export.");
    let error = writer.file("short", 5, &mut &b"abc"[..]).unwrap_err();
    assert_eq!(error.0, "short got shorter while it was written.");
}

#[test]
fn safe_names_at_their_edges() {
    assert!(safe_path(&"x".repeat(200)).is_some());
    assert!(safe_path(&"x".repeat(201)).is_none());
    assert!(safe_path("a/b/c/d").is_some());
    assert!(safe_path("a/b/c/d/e").is_none());
    assert!(safe_path("a.").is_some());
    assert!(safe_path(".a").is_none());
    assert!(safe_path("a b").is_none());
}
