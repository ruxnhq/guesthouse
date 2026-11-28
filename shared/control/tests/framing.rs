//! The Windows host's JSON lines on the pipe, held as they are: a line is read up to the limit and no
//! further, an end of input with nothing read is no line, a line that isn't the type is bad data with
//! serde_json's words, and a value is written as one compact line and flushed.

use guesthouse_control::{REQUEST_LIMIT, read_line, write_line};
use serde_json::{Value, json};
use std::io::{self, BufReader, Cursor, Write};

fn read(bytes: &[u8]) -> (io::Result<Option<Value>>, Cursor<Vec<u8>>) {
    let mut reader = Cursor::new(bytes.to_vec());
    let result = read_line::<Value>(&mut reader);
    (result, reader)
}

/// A JSON string whose text is `bytes` long, quotes included.
fn string_of(bytes: usize) -> String {
    format!("\"{}\"", "a".repeat(bytes - 2))
}

#[test]
fn the_limit_is_64_kib() {
    assert_eq!(REQUEST_LIMIT, 65_536);
}

#[test]
fn nothing_read_is_no_line() {
    assert!(read(b"").0.unwrap().is_none());
}

#[test]
fn a_line_is_read_with_or_without_its_end() {
    assert_eq!(read(b"{\"a\":1}\n").0.unwrap(), Some(json!({"a": 1})));
    assert_eq!(read(b"{\"a\":1}").0.unwrap(), Some(json!({"a": 1})));
    assert_eq!(read(b"{\"a\":1}\r\n").0.unwrap(), Some(json!({"a": 1})));
    assert_eq!(read(b"  {\"a\":1}  \n").0.unwrap(), Some(json!({"a": 1})));
    let (first, mut rest) = read(b"1\n2\n");
    assert_eq!(first.unwrap(), Some(json!(1)));
    assert_eq!(read_line::<Value>(&mut rest).unwrap(), Some(json!(2)));
    assert_eq!(read_line::<Value>(&mut rest).unwrap(), None);
}

#[test]
fn a_line_that_isnt_json_is_bad_data_in_serde_jsons_words() {
    let error = read(b"\n").0.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "EOF while parsing a value at line 1 column 0");
    let error = read(b"{\"a\":}\n").0.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "expected value at line 1 column 6");
}

#[test]
fn a_line_of_the_limit_is_read_and_its_end_is_left() {
    let line = string_of(REQUEST_LIMIT) + "\n";
    let (value, mut rest) = read(line.as_bytes());
    assert_eq!(value.unwrap(), Some(Value::String("a".repeat(REQUEST_LIMIT - 2))));
    assert_eq!(rest.position(), REQUEST_LIMIT as u64, "only the limit is read");
    assert!(read_line::<Value>(&mut rest).is_err(), "the line end left over reads as an empty line");
}

#[test]
fn a_line_past_the_limit_is_cut_and_refused() {
    let line = string_of(REQUEST_LIMIT + 1) + "\n";
    let (value, rest) = read(line.as_bytes());
    let error = value.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "EOF while parsing a string at line 1 column 65536");
    assert_eq!(rest.position(), REQUEST_LIMIT as u64);
}

#[test]
fn a_reader_with_a_small_buffer_reads_the_same() {
    let line = string_of(1000) + "\n";
    let mut reader = BufReader::with_capacity(7, line.as_bytes());
    assert_eq!(read_line::<Value>(&mut reader).unwrap(), Some(Value::String("a".repeat(998))));
}

/// Counts flushes, as the pipe's other end must have each line at once.
struct Flushes(Vec<u8>, usize);

impl Write for Flushes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.1 += 1;
        Ok(())
    }
}

#[test]
fn a_value_is_written_as_one_compact_line_and_flushed() {
    let mut out = Flushes(Vec::new(), 0);
    write_line(&mut out, &json!({"b": [1, 2], "a": "x\ny", "c": null})).unwrap();
    assert_eq!(String::from_utf8(out.0.clone()).unwrap(), "{\"a\":\"x\\ny\",\"b\":[1,2],\"c\":null}\n");
    assert_eq!(out.1, 1);
    write_line(&mut out, &"é").unwrap();
    assert!(out.0.ends_with("\"é\"\n".as_bytes()));
    assert_eq!(out.1, 2);
}
