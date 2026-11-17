//! The few facts that go with a report outside its bundle, so it can be sorted before anyone opens it:
//! `summary.json` inside, and the same in the `X-Report-Summary` header.
//!
//! The server takes a flat object of strings, at most 20 of them, none longer than 2000 characters,
//! and a header of at most 6000 characters once it's base64url.

use crate::text::base64_encode;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const VERSION: &str = "version";
pub const CARD: &str = "card";
pub const DRIVER: &str = "driver";
pub const DISTRO: &str = "distro";
pub const KERNEL: &str = "kernel";
pub const SESSION: &str = "session";
/// The guest's kind, never the machine's name, which is the user's.
pub const MACHINE: &str = "machine";
pub const KIND: &str = "kind";
pub const DESCRIPTION: &str = "description";
pub const CONTACT: &str = "contact";

const MOST_KEYS: usize = 20;
const MOST_VALUE_CHARS: usize = 2000;
const MOST_HEADER_CHARS: usize = 6000;
/// The start of the description goes in the summary. The whole of it is in the bundle.
const DESCRIPTION_CHARS: usize = 1500;
/// How much shorter the description gets each time the header is still too long.
const DESCRIPTION_STEP: usize = 100;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary(BTreeMap<String, String>);

impl Summary {
    /// Sets a fact, cut to what the server takes. An empty one is left out, as it says nothing.
    pub fn set(&mut self, key: &str, value: &str) {
        let value = value.trim();
        let is_new = !self.0.contains_key(key);
        if value.is_empty() || (is_new && self.0.len() >= MOST_KEYS) {
            return;
        }
        let most = if key == DESCRIPTION { DESCRIPTION_CHARS } else { MOST_VALUE_CHARS };
        self.0.insert(key.to_owned(), first_chars(value, most).to_owned());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn facts(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub fn to_json(&self) -> String {
        let fields: Map<String, Value> =
            self.0.iter().map(|(key, value)| (key.clone(), Value::String(value.clone()))).collect();
        serde_json::to_string_pretty(&Value::Object(fields)).unwrap_or_default() + "\n"
    }

    /// The header's value. Text in other scripts takes more bytes for each character, so a long
    /// description is cut until the whole fits, and as a last resort left out.
    pub fn header(&self) -> String {
        let mut summary = self.clone();
        loop {
            let compact = serde_json::to_string(&Value::Object(
                summary.0.iter().map(|(key, value)| (key.clone(), Value::String(value.clone()))).collect(),
            ))
            .unwrap_or_default();
            let encoded = base64url(compact.as_bytes());
            if encoded.len() <= MOST_HEADER_CHARS {
                return encoded;
            }
            let Some(description) = summary.0.get_mut(DESCRIPTION) else {
                // Only the description can be long enough for this, so with none it's a few facts
                // that can't be cut, and the server is better off with no summary than a wrong one.
                return String::new();
            };
            let shorter = description.chars().count().saturating_sub(DESCRIPTION_STEP);
            if shorter == 0 {
                summary.0.remove(DESCRIPTION);
            } else {
                *description = first_chars(description, shorter).to_owned();
            }
        }
    }
}

fn first_chars(text: &str, most: usize) -> &str {
    text.char_indices().nth(most).map_or(text, |(at, _)| &text[..at])
}

/// base64 with the two letters that mean something in a URL changed, and no padding.
fn base64url(data: &[u8]) -> String {
    base64_encode(data).replace('+', "-").replace('/', "_").trim_end_matches('=').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::base64_decode;

    fn decoded(header: &str) -> Value {
        let standard = header.replace('-', "+").replace('_', "/");
        serde_json::from_slice(&base64_decode(&standard).unwrap()).unwrap()
    }

    fn sample() -> Summary {
        let mut summary = Summary::default();
        summary.set(VERSION, "0.6.1");
        summary.set(CARD, "NVIDIA GeForce RTX 4080");
        summary.set(KIND, "crashed");
        summary
    }

    #[test]
    fn the_header_is_a_flat_object_of_strings() {
        let Value::Object(fields) = decoded(&sample().header()) else { panic!("not an object") };
        assert_eq!(fields.len(), 3);
        assert!(fields.values().all(Value::is_string));
        assert_eq!(fields["card"], "NVIDIA GeForce RTX 4080");
    }

    #[test]
    fn the_header_is_url_safe_with_no_padding() {
        let mut summary = sample();
        summary.set(DESCRIPTION, "??>>~~ a description with bytes that make + and / in base64 ???");
        let header = summary.header();
        assert!(header.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
    }

    #[test]
    fn empty_facts_are_left_out() {
        let mut summary = sample();
        summary.set(CONTACT, "  ");
        assert_eq!(summary.get(CONTACT), None);
    }

    #[test]
    fn values_are_cut_to_what_the_server_takes() {
        let mut summary = sample();
        summary.set(DISTRO, &"d".repeat(5000));
        summary.set(DESCRIPTION, &"w".repeat(5000));
        assert_eq!(summary.get(DISTRO).unwrap().len(), MOST_VALUE_CHARS);
        assert_eq!(summary.get(DESCRIPTION).unwrap().len(), DESCRIPTION_CHARS);
    }

    #[test]
    fn no_more_than_twenty_keys() {
        let mut summary = Summary::default();
        for number in 0..30 {
            summary.set(&format!("key{number}"), "x");
        }
        assert_eq!(summary.facts().count(), MOST_KEYS);
    }

    #[test]
    fn a_long_description_in_another_script_still_fits_the_header() {
        let mut summary = sample();
        summary.set(DESCRIPTION, &"画面が真っ黒になりました。".repeat(200));
        summary.set(CONTACT, "someone on the Discord");
        let header = summary.header();
        assert!(header.len() <= MOST_HEADER_CHARS, "{} characters", header.len());
        let fields = decoded(&header);
        assert!(fields["description"].as_str().unwrap().starts_with("画面が"));
        assert_eq!(fields["contact"], "someone on the Discord");
    }

    #[test]
    fn the_json_file_has_the_whole_summary() {
        let json: Value = serde_json::from_str(&sample().to_json()).unwrap();
        assert_eq!(json["version"], "0.6.1");
        assert_eq!(json["kind"], "crashed");
    }
}
