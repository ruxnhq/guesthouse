use super::*;
use std::collections::BTreeMap;

fn folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("guesthouse-basics-json-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    folder
}

#[test]
fn the_text_is_pretty_json_and_a_line_end() {
    let folder = folder("text");
    let path = folder.join("deeper").join("machine.json");
    let value = serde_json::json!({"name": "a \"b\"\n", "cores": 4, "shares": [], "on": true});
    write_json(&path, &value).unwrap();
    let text = "{\n  \"cores\": 4,\n  \"name\": \"a \\\"b\\\"\\n\",\n  \"on\": true,\n  \"shares\": []\n}\n";
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert!(!with_suffix(&path, ".new").exists());
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_file_there_is_replaced_whole() {
    let folder = folder("replace");
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join("prefs.json");
    fs::write(&path, "a much longer file than the one that replaces it\n").unwrap();
    fs::write(with_suffix(&path, ".new"), "left by a crash").unwrap();
    write_json(&path, &serde_json::json!([1])).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "[\n  1\n]\n");
    assert!(!with_suffix(&path, ".new").exists());
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn what_json_cannot_hold_is_refused_before_anything_is_made() {
    let folder = folder("refused");
    let not_json: BTreeMap<(u8, u8), u8> = [((1, 2), 3)].into();
    let error = write_json(&folder.join("machine.json"), &not_json).unwrap_err();
    assert_eq!(error.to_string(), "key must be a string");
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert!(!folder.exists());
}

#[test]
fn a_folder_where_the_file_goes_leaves_the_new_file() {
    let folder = folder("folder-for-file");
    let path = folder.join("machine.json");
    fs::create_dir_all(path.join("inside")).unwrap();
    assert!(write_json(&path, &serde_json::json!({})).is_err());
    assert_eq!(fs::read_to_string(with_suffix(&path, ".new")).unwrap(), "{}\n");
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn the_suffix_goes_after_the_whole_name() {
    assert_eq!(with_suffix(Path::new("a/machine.json"), ".new"), Path::new("a/machine.json.new"));
    assert_eq!(with_suffix(Path::new("disc.iso"), ".part"), Path::new("disc.iso.part"));
    assert_eq!(with_suffix(Path::new("plain"), ".new"), Path::new("plain.new"));
}
