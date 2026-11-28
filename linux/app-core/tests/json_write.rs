//! What `settings::write_json` does today, held so that moving it can't change a byte: the text a
//! machine file, a task file or `setup.json` gets, and how each way of failing is reported. The
//! golden files in `json_write/` were written by this function before it moved.

#![cfg(target_os = "linux")]

use guesthouse_app_core::Failure;
use guesthouse_app_core::settings::{Fields, with_suffix, write_json};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A record with each kind of value a machine or task file holds.
#[derive(Serialize)]
struct Sample {
    name: String,
    cores: u32,
    ram_gib: u64,
    share: f64,
    on: bool,
    nothing: Option<String>,
    empty: Vec<u8>,
    none_of_them: BTreeMap<String, u8>,
    forwards: Vec<Forward>,
    note: String,
}

#[derive(Serialize)]
struct Forward {
    host: u16,
    guest: u16,
}

fn sample() -> Sample {
    Sample {
        name: "Windows 11 \u{e9}t\u{e9}".into(),
        cores: 8,
        ram_gib: 16,
        share: 0.5,
        on: true,
        nothing: None,
        empty: Vec::new(),
        none_of_them: BTreeMap::new(),
        forwards: vec![Forward { host: 2222, guest: 22 }, Forward { host: 8080, guest: 80 }],
        note: "a \"quote\", a back\\slash, a tab\there and a line\nend, and \u{1}".into(),
    }
}

/// Fields put in out of order, as `write_setup` and `write_prefs` gather them.
fn fields() -> Fields {
    let mut fields = Fields::new();
    fields.insert("version".into(), 1.into());
    fields.insert("card_memory_mib".into(), 24576.into());
    fields.insert("driver_folder".into(), "/somewhere/driver".into());
    fields.insert("card_memories".into(), serde_json::json!({"0000:01:00.0": 24576, "0000:02:00.0": 8192}));
    fields.insert("big".into(), Value::from(u64::MAX));
    fields.insert("negative".into(), Value::from(-3));
    fields.insert("nothing".into(), Value::Null);
    fields
}

fn folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("guesthouse-json-write-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    folder
}

fn new_beside(path: &Path) -> PathBuf {
    with_suffix(path, ".new")
}

#[test]
fn a_record_is_written_as_it_always_was() {
    let folder = folder("record");
    let path = folder.join("deeper").join("machine.json");
    write_json(&path, &sample()).unwrap();
    assert_eq!(fs::read(&path).unwrap(), include_bytes!("json_write/record.json"));
    assert!(!new_beside(&path).exists());
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn fields_are_written_in_order_of_name() {
    let folder = folder("fields");
    let path = folder.join("setup.json");
    write_json(&path, &fields()).unwrap();
    assert_eq!(fs::read(&path).unwrap(), include_bytes!("json_write/fields.json"));
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn small_values_are_written_as_they_always_were() {
    let folder = folder("small");
    for (name, value, text) in [
        ("empty-object", serde_json::json!({}), "{}\n"),
        ("empty-list", serde_json::json!([]), "[]\n"),
        ("number", serde_json::json!(7), "7\n"),
        ("text", serde_json::json!("x"), "\"x\"\n"),
        ("one", serde_json::json!({"on": true}), "{\n  \"on\": true\n}\n"),
    ] {
        let path = folder.join(name);
        write_json(&path, &value).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "{name}");
    }
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_file_there_is_replaced_whole() {
    let folder = folder("replace");
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join("prefs.json");
    fs::write(&path, "a much longer file than the one that replaces it, with an end to lose\n").unwrap();
    write_json(&path, &serde_json::json!({"a": 1})).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\n  \"a\": 1\n}\n");
    assert!(!new_beside(&path).exists());
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_new_file_left_by_a_crash_is_written_over() {
    let folder = folder("left");
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join("task.json");
    fs::write(new_beside(&path), "half a fi").unwrap();
    write_json(&path, &serde_json::json!({"b": 2})).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\n  \"b\": 2\n}\n");
    assert!(!new_beside(&path).exists());
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn the_new_file_is_named_after_the_whole_name() {
    assert_eq!(new_beside(Path::new("/a/machine.json")), Path::new("/a/machine.json.new"));
    assert_eq!(with_suffix(Path::new("/a/disc.iso"), ".part"), Path::new("/a/disc.iso.part"));
    assert_eq!(with_suffix(Path::new("noextension"), ".new"), Path::new("noextension.new"));
    assert_eq!(with_suffix(Path::new("/a/b/"), ".new"), Path::new("/a/b/.new"));
}

#[test]
fn what_json_cannot_hold_is_refused_before_anything_is_made() {
    let folder = folder("unwritable");
    let path = folder.join("machine.json");
    let not_json: BTreeMap<(u8, u8), u8> = [((1, 2), 3)].into();
    assert_eq!(write_json(&path, &not_json), Err(Failure("key must be a string".into())));
    assert!(!folder.exists(), "the folder isn't made for a file that can't be written");
}

#[test]
fn a_file_where_the_folder_should_be_is_reported() {
    let folder = folder("file-for-folder");
    fs::create_dir_all(&folder).unwrap();
    let file = folder.join("machines");
    fs::write(&file, "").unwrap();
    let error = write_json(&file.join("machine.json"), &serde_json::json!({})).unwrap_err();
    assert_eq!(error, Failure("File exists (os error 17)".into()));
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_folder_where_the_file_should_be_is_reported_and_the_new_file_stays() {
    let folder = folder("folder-for-file");
    let path = folder.join("machine.json");
    fs::create_dir_all(path.join("inside")).unwrap();
    let error = write_json(&path, &serde_json::json!({"c": 3})).unwrap_err();
    assert_eq!(error, Failure("Is a directory (os error 21)".into()));
    assert_eq!(fs::read_to_string(new_beside(&path)).unwrap(), "{\n  \"c\": 3\n}\n");
    assert!(path.join("inside").is_dir(), "the folder is left as it was");
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_folder_where_the_new_file_should_be_is_reported() {
    let folder = folder("folder-for-new");
    let path = folder.join("machine.json");
    fs::create_dir_all(new_beside(&path)).unwrap();
    fs::write(&path, "kept\n").unwrap();
    let error = write_json(&path, &serde_json::json!({})).unwrap_err();
    assert_eq!(error, Failure("Is a directory (os error 21)".into()));
    assert_eq!(fs::read_to_string(&path).unwrap(), "kept\n", "the old file is untouched");
    fs::remove_dir_all(folder).unwrap();
}
