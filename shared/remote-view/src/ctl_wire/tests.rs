use super::samples::{REFUSED, TAKEN};
use super::*;
use crate::ctl;

const FP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn w(line: &str) -> Words {
    line.split(' ').map(str::to_string).collect()
}

/// Every answer line, read back.
fn parsed(lines: &[String]) -> Vec<Line> {
    lines.iter().map(|line| parse_line(line).unwrap()).collect()
}

fn reassemble(id: Id, lines: &[Line]) -> Progress {
    let mut whole = Reassembly::new(id);
    let mut last = Progress::Ignored;
    for line in lines {
        last = whole.feed(line);
    }
    last
}

#[test]
fn every_command_this_host_makes_is_in_the_grammar() {
    let mut made = vec![
        ctl::status(),
        ctl::pair_open(),
        ctl::pair_close(),
        ctl::pair_accept(FP).unwrap(),
        ctl::pair_decline(FP).unwrap(),
        ctl::updates_off(),
        ctl::share(true),
        ctl::device_revoke(FP).unwrap(),
        ctl::device_revoke(&FP.to_ascii_uppercase()).unwrap(),
    ];
    for on in [true, false] {
        made.push(ctl::host_allow("clipboard", on).unwrap());
        made.push(ctl::host_allow("files", on).unwrap());
        for what in PERMISSIONS {
            made.push(ctl::device_allow(FP, what, on).unwrap());
        }
    }
    for words in made {
        assert_eq!(allowed(&words), Ok(()), "{words:?}");
    }
}

#[test]
fn the_commands_that_cut_guesthouses_own_viewer_are_not() {
    for words in [ctl::share(false), ctl::device_drop()] {
        assert!(allowed(&words).is_err(), "{words:?}");
    }
}

#[test]
fn the_grammar_takes_what_it_says_and_nothing_else() {
    for line in TAKEN {
        assert_eq!(allowed(&w(line)), Ok(()), "{line:?}");
    }
    for line in REFUSED {
        assert!(allowed(&w(line)).is_err(), "{line:?}");
    }
    assert!(allowed(&[]).is_err());
}

#[test]
fn a_request_is_one_line_the_guest_reads_back() {
    let line = request_line(7, &ctl::pair_open()).unwrap();
    assert_eq!(line, "guesthouse: ctl ask 7 pair on");
    assert_eq!(read_request(&line), Some(Ok((7, ctl::pair_open()))));
    assert_eq!(read_request(&format!("{line}\r\n")), Some(Ok((7, ctl::pair_open()))));
    assert!(request_line(0, &ctl::status()).is_err());
    assert!(request_line(1, &ctl::share(false)).is_err());
    // The longest command is far inside remoteman's own limit of a line.
    let longest = request_line(Id::MAX, &ctl::device_allow(FP, "clipboard", false).unwrap()).unwrap();
    assert!(longest.len() < 200, "{}", longest.len());
}

#[test]
fn the_guest_refuses_a_request_it_can_number_and_ignores_one_it_cant() {
    assert_eq!(read_request("guesthouse: ctl ask 9 sharing off"), Some(Err(9)));
    assert_eq!(read_request("guesthouse: ctl ask 9 status; calc"), Some(Err(9)));
    assert_eq!(read_request("guesthouse: ctl ask 9"), Some(Err(9)));
    for line in [
        "guesthouse: ctl ask 0 status",
        "guesthouse: ctl ask 09 status",
        "guesthouse: ctl ask -1 status",
        "guesthouse: ctl ask 4294967296 status",
        "guesthouse: ctl ask x status",
        "guesthouse: ctl ask  status",
        "guesthouse: shutdown",
        "guesthouse: ctl answer 1 0 1/1 -",
    ] {
        assert_eq!(read_request(line), None, "{line:?}");
    }
}

#[test]
fn a_long_answer_comes_back_whole_in_lines_far_inside_the_limit() {
    let said: String = (0..5000).map(|at| char::from(b'a' + (at % 26) as u8)).collect();
    let said = format!("{{\"devices\":[{{\"name\":\"Zo\u{eb}'s phone\",\"code\":\"123456\"}}]}}{said}");
    let lines = answer_lines(42, 0, said.as_bytes()).unwrap();
    assert!(lines.len() > 1);
    for line in &lines {
        assert!(line.len() < 1200, "{}", line.len());
        assert!(line.is_ascii() && is_channel(line));
    }
    assert_eq!(reassemble(42, &parsed(&lines)), Progress::Done(Answer { code: 0, text: said }));
}

#[test]
fn an_empty_answer_is_one_part_and_keeps_its_code() {
    let lines = answer_lines(3, -1, b"").unwrap();
    assert_eq!(lines, ["guesthouse: ctl answer 3 -1 1/1 -"]);
    assert_eq!(reassemble(3, &parsed(&lines)), Progress::Done(Answer { code: -1, text: String::new() }));
}

#[test]
fn parts_in_any_order_make_the_same_answer() {
    let said = vec![b'x'; CHUNK * 3];
    let mut lines = parsed(&answer_lines(5, 1, &said).unwrap());
    lines.reverse();
    lines.swap(0, 1);
    assert_eq!(reassemble(5, &lines), Progress::Done(Answer { code: 1, text: "x".repeat(CHUNK * 3) }));
}

#[test]
fn a_missing_part_leaves_it_unfinished() {
    let mut lines = parsed(&answer_lines(5, 0, &vec![b'x'; CHUNK * 2]).unwrap());
    lines.remove(1);
    assert_eq!(reassemble(5, &lines), Progress::Partial);
}

#[test]
fn a_part_sent_twice_counts_once_and_a_different_one_garbles_it() {
    let lines = parsed(&answer_lines(5, 0, &vec![b'x'; CHUNK * 2]).unwrap());
    let twice = [lines[0].clone(), lines[0].clone(), lines[1].clone(), lines[2].clone()];
    assert!(matches!(reassemble(5, &twice), Progress::Done(_)));
    let Line::Part { id, code, index, total, .. } = lines[0].clone() else { panic!() };
    let other = Line::Part { id, code, index, total, chunk: "QUJD".into() };
    assert!(matches!(reassemble(5, &[lines[0].clone(), other]), Progress::Garbled(_)));
}

#[test]
fn parts_that_disagree_on_the_code_or_total_garble_it() {
    let one = Line::Part { id: 1, code: 0, index: 1, total: 2, chunk: "QUJD".into() };
    let code = Line::Part { id: 1, code: 1, index: 2, total: 2, chunk: "QUJD".into() };
    let total = Line::Part { id: 1, code: 0, index: 2, total: 3, chunk: "QUJD".into() };
    assert!(matches!(reassemble(1, &[one.clone(), code]), Progress::Garbled(_)));
    assert!(matches!(reassemble(1, &[one, total]), Progress::Garbled(_)));
}

#[test]
fn another_requests_lines_are_ignored() {
    let old = parsed(&answer_lines(4, 0, b"old").unwrap());
    let new = parsed(&answer_lines(5, 0, b"new").unwrap());
    let mut whole = Reassembly::new(5);
    assert_eq!(whole.feed(&old[0]), Progress::Ignored);
    assert_eq!(whole.feed(&Line::Failed { id: 4, failure: Failure::Missing }), Progress::Ignored);
    assert_eq!(whole.feed(&Line::Ask(5)), Progress::Ignored);
    assert_eq!(whole.feed(&Line::Garbage), Progress::Ignored);
    assert_eq!(whole.feed(&new[0]), Progress::Done(Answer { code: 0, text: "new".into() }));
}

#[test]
fn the_guest_says_why_it_ran_nothing() {
    for failure in Failure::ALL {
        let line = failed_line(8, failure);
        assert_eq!(parse_line(&line), Some(Line::Failed { id: 8, failure }));
        assert_eq!(reassemble(8, &parsed(&[line])), Progress::Failed(failure));
    }
    assert_eq!(parse_line("guesthouse: ctl failed 8 bored"), Some(Line::Garbage));
}

#[test]
fn an_answer_over_the_cap_is_refused_at_both_ends() {
    assert_eq!(answer_lines(1, 0, &vec![b'x'; MAX_ANSWER + 1]), Err(Failure::TooLong));
    let most = answer_lines(1, 0, &vec![b'x'; MAX_ANSWER]).unwrap();
    assert_eq!(most.len() as u32, MAX_PARTS);
    // A guest that claims more parts than any answer may have is not believed.
    let line = format!("{ANSWER}1 0 1/{} QUJD", MAX_PARTS + 1);
    assert_eq!(parse_line(&line), Some(Line::Garbage));
    let line = format!("{ANSWER}1 0 1/{} QUJD", u32::MAX);
    assert_eq!(parse_line(&line), Some(Line::Garbage));
}

#[test]
fn garbage_that_begins_as_the_channel_is_still_the_channels() {
    for line in [
        "guesthouse: ctl ",
        "guesthouse: ctl answer",
        "guesthouse: ctl answer 1 0 1/1",
        "guesthouse: ctl answer 1 0 1/1 ",
        "guesthouse: ctl answer 1 0 0/1 QUJD",
        "guesthouse: ctl answer 1 0 2/1 QUJD",
        "guesthouse: ctl answer 1 0 1/0 QUJD",
        "guesthouse: ctl answer 1 x 1/1 QUJD",
        "guesthouse: ctl answer 1 +1 1/1 QUJD",
        "guesthouse: ctl answer 1 99999999999 1/1 QUJD",
        "guesthouse: ctl answer 0 0 1/1 QUJD",
        "guesthouse: ctl answer 1 0 1/1 QU*D",
        "guesthouse: ctl answer 1 0 1/1 QUJD extra",
        "guesthouse: ctl answer 1 0 1-1 QUJD",
        "guesthouse: ctl hello",
        "guesthouse: ctl ask",
    ] {
        assert_eq!(parse_line(line), Some(Line::Garbage), "{line:?}");
        assert!(is_channel(line));
    }
    let long = format!("{ANSWER}1 0 1/1 {}", "A".repeat(CHUNK + 4));
    assert_eq!(parse_line(&long), Some(Line::Garbage));
}

#[test]
fn bad_base64_in_whole_parts_garbles_the_answer() {
    for chunk in ["QUJ", "Q===", "QQ=A", "QR==", "QUJD=AAA"] {
        let line = Line::Part { id: 1, code: 0, index: 1, total: 1, chunk: chunk.into() };
        assert!(matches!(reassemble(1, &[line]), Progress::Garbled(_)), "{chunk}");
    }
}

#[test]
fn the_guests_ordinary_lines_are_not_the_channels() {
    for line in [
        "guesthouse: screen: fingerprint 0123",
        "guesthouse: address: 172.29.1.2",
        "guesthouse: ctlx",
        "guesthouse:ctl answer 1 0 1/1 -",
        " guesthouse: ctl answer 1 0 1/1 -",
        "card: ok",
    ] {
        assert!(!is_channel(line), "{line:?}");
        assert_eq!(parse_line(line), None);
    }
}

#[test]
fn base64_is_the_standard_padded_kind() {
    for (bytes, text) in [
        (&b""[..], ""),
        (b"f", "Zg=="),
        (b"fo", "Zm8="),
        (b"foo", "Zm9v"),
        (b"foob", "Zm9vYg=="),
        (b"\xff\xfe\x00", "//4A"),
    ] {
        assert_eq!(base64_encode(bytes), text);
        assert_eq!(base64_decode(text).as_deref(), Some(bytes));
    }
    let every: Vec<u8> = (0..=255).collect();
    assert_eq!(base64_decode(&base64_encode(&every)), Some(every));
    for bad in ["Zg", "Zg=", "Z===", "Zh==", "Zm9v=Zm9", "Zm9v\n", "Zm-v"] {
        assert_eq!(base64_decode(bad), None, "{bad}");
    }
}
