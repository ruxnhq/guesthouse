use super::*;

#[test]
fn words_are_quoted_for_a_shell_only_when_they_need_it() {
    for (word, quoted) in [
        ("", "''"),
        ("a", "a"),
        ("_@%+=:,./-", "_@%+=:,./-"),
        ("Z9", "Z9"),
        ("a b", "'a b'"),
        ("$HOME", "'$HOME'"),
        ("*", "'*'"),
        ("~", "'~'"),
        ("'", r"''\'''"),
        ("it's", r"'it'\''s'"),
        ("é", "'é'"),
        ("a\nb", "'a\nb'"),
        ("a\"b", "'a\"b'"),
    ] {
        assert_eq!(shell_quote(word), quoted, "{word:?}");
    }
}

#[test]
fn base64_is_the_standard_alphabet_with_padding() {
    for (data, text) in [
        (&b""[..], ""),
        (b"\0", "AA=="),
        (b"\0\0", "AAA="),
        (b"\0\0\0", "AAAA"),
        (b"\xff\xff\xff", "////"),
        (b"\xfb\xef\xbe", "++++"),
        (b"Man", "TWFu"),
        (b"hello, guest\n", "aGVsbG8sIGd1ZXN0Cg=="),
    ] {
        assert_eq!(base64_encode(data), text, "{data:?}");
        assert_eq!(base64_decode(text).as_deref(), Some(data), "{text}");
    }
    let every: Vec<u8> = (0..=255).collect();
    assert_eq!(base64_decode(&base64_encode(&every)), Some(every));
}

#[test]
fn base64_reading_takes_what_it_always_took() {
    assert_eq!(base64_decode("Zg").as_deref(), Some(&b"f"[..]), "no padding");
    assert_eq!(base64_decode("Zg=====").as_deref(), Some(&b"f"[..]), "any padding");
    assert_eq!(base64_decode("====").as_deref(), Some(&b""[..]));
    assert_eq!(base64_decode("Z").as_deref(), Some(&b""[..]), "a lone letter is no byte");
    assert_eq!(base64_decode("Zm9v\n"), None, "a line end is not base64");
    assert_eq!(base64_decode("Zm=9v"), None, "padding inside is not");
    assert_eq!(base64_decode("Zm9v-_"), None, "nor is the URL alphabet");
}

#[test]
fn hidden_characters_are_shown_as_codes_and_a_long_text_is_cut() {
    assert_eq!(shown("a\x1b]0;x\x07b\u{202e}c", false), "a<1b>]0;x<07>b<202e>c");
    assert_eq!(shown("one\ntwo\r\tz", true), "one\ntwo<0d>\tz");
    assert_eq!(shown("one\ntwo", false), "one<0a>two");
    assert_eq!(shown_short("abcdef", 3), "abc...");
    assert_eq!(shown_short("abc", 3), "abc");
    assert!(is_hidden('\u{200b}') && !is_hidden('e'));
}

#[test]
fn a_comma_in_a_qemu_value_is_doubled() {
    for (value, written) in [
        ("", ""),
        ("/m/win.img", "/m/win.img"),
        ("a,b", "a,,b"),
        (",,", ",,,,"),
        ("Windows 11, ARM.iso", "Windows 11,, ARM.iso"),
    ] {
        assert_eq!(qemu_value(value), written, "{value:?}");
    }
}
