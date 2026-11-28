use super::*;

#[test]
fn a_tag_is_the_prefix_and_eight_lower_case_hex_digits() {
    assert!(is_snapshot_tag("guesthouse-0123abcd") && is_snapshot_tag("guesthouse-ffffffff"));
    for name in
        ["guesthouse-", "guesthouse-0123abc", "guesthouse-0123abcde", "guesthouse-0123ABCD", "by-hand"]
    {
        assert!(!is_snapshot_tag(name), "{name}");
    }
    assert_eq!(SNAPSHOT_HEX_DIGITS, TAG_RANDOM_BYTES * 2);
}

#[test]
fn a_name_is_trimmed_and_1_to_60_characters_of_any_kind() {
    let sixty = "é".repeat(60);
    for (name, kept) in
        [(" a ", "a"), ("a\nb", "a\nb"), ("\u{3000}x\t", "x"), (sixty.as_str(), sixty.as_str())]
    {
        assert_eq!(checked_name(name), Ok(kept));
    }
    for name in ["", " \t\n", &"x".repeat(61)] {
        assert_eq!(checked_name(name), Err("A snapshot needs a name of up to 60 characters.".to_owned()));
    }
}

#[test]
fn a_name_is_taken_only_as_it_is() {
    assert!(name_taken("a", ["b", "a"]));
    assert!(!name_taken("a", ["A", "a "]));
    assert!(!name_taken("a", []));
    assert_eq!(NAME_TAKEN, "There is already a snapshot with that name.");
}

#[test]
fn the_note_is_written_and_read_so() {
    assert_eq!(note_text("Ünï\t", true, false), r#"{"auto":true,"name":"Ünï\t","nvme":false}"#);
    assert_eq!(read_note(r#"{"name": "x"}"#), Some(Note { name: "x".into(), automatic: false }));
    assert_eq!(
        read_note(r#"{"name": 5, "auto": "yes"}"#),
        Some(Note { name: String::new(), automatic: false })
    );
    assert_eq!(read_note("[1]"), Some(Note { name: String::new(), automatic: false }));
    assert_eq!(read_note("not json"), None);
}
