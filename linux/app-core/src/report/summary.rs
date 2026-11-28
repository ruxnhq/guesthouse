//! The few facts that go with a report outside its bundle, so it can be sorted before anyone opens it:
//! `summary.json` inside, and the same in the `X-Report-Summary` header. Every host keeps to the
//! server's rules for them, so they're `guesthouse-report-core`'s.
//!
//! The server takes a flat object of strings, at most 20 of them, none longer than 2000 characters,
//! and a header of at most 6000 characters once it's base64url.

pub use guesthouse_report_core::summary::{
    CARD, CONTACT, DESCRIPTION, DISTRO, DRIVER, KERNEL, KIND, MACHINE, SESSION, Summary, VERSION,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::base64_decode;
    use guesthouse_report_core::summary::{
        DESCRIPTION_CHARS, MOST_HEADER_CHARS, MOST_KEYS, MOST_VALUE_CHARS,
    };
    use serde_json::Value;

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
