use super::*;
use serde_json::{Value, json};
use std::io::Cursor;

#[test]
fn a_line_is_read_up_to_its_limit() {
    let mut reader = Cursor::new(b"\"abcd\"\n\"abcde\"\n".to_vec());
    assert_eq!(read_line_within::<Value>(&mut reader, 6).unwrap(), Some(json!("abcd")));
    assert_eq!(reader.position(), 6, "the line end is past the limit, and stays");
    assert!(read_line_within::<Value>(&mut reader, 6).is_err(), "the end read alone is an empty line");
    let error = read_line_within::<Value>(&mut reader, 6).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData, "a line past the limit is cut");
    assert_eq!(read_line_within::<Value>(&mut Cursor::new(Vec::new()), 6).unwrap(), None);
}

#[test]
fn a_value_is_one_compact_line() {
    let mut out = Vec::new();
    write_line(&mut out, &json!({"b": 1, "a": [true]})).unwrap();
    assert_eq!(out, b"{\"a\":[true],\"b\":1}\n");
    let mut reader = Cursor::new(out);
    assert_eq!(read_line_within::<Value>(&mut reader, 64).unwrap(), Some(json!({"a": [true], "b": 1})));
}
