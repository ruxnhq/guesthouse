use super::*;

#[test]
fn no_file_is_off_with_no_port() {
    assert_eq!(Switch::read(None), Switch { allow_remote_view: false, remote_port: None });
    assert_eq!(Switch::read(None).text(), None);
}

#[test]
fn a_file_that_cannot_be_read_is_off() {
    for text in
        ["", "{", "[]", "null", r#"{"allow_remote_view":"yes"}"#, r#"{"allow_remote_view":true,"extra":1}"#]
    {
        assert_eq!(Switch::read(Some(text)), Switch::default(), "{text}");
    }
}

#[test]
fn what_it_holds_is_read_and_written_by_its_names() {
    let on = Switch::read(Some(r#"{"allow_remote_view":true,"remote_port":7800}"#));
    assert_eq!(on, Switch { allow_remote_view: true, remote_port: Some(7800) });
    let text = on.text().unwrap();
    assert!(text.contains("\"allow_remote_view\": true") && text.contains("\"remote_port\": 7800"));
    assert!(text.ends_with('\n'));
    assert_eq!(Switch::read(Some(&text)), on);
    // A field that is left out is as it is by default.
    assert_eq!(
        Switch::read(Some(r#"{"remote_port":7801}"#)),
        Switch { allow_remote_view: false, remote_port: Some(7801) }
    );
    assert_eq!(Switch::read(Some("{}")), Switch::default());
}

#[test]
fn a_port_outside_the_range_is_not_kept() {
    for port in [0, 7791, 7900, 80, 65535] {
        let text = format!(r#"{{"allow_remote_view":true,"remote_port":{port}}}"#);
        assert_eq!(
            Switch::read(Some(&text)),
            Switch { allow_remote_view: true, remote_port: None },
            "{port}"
        );
    }
    assert_eq!(Switch::read(Some(r#"{"remote_port":7792}"#)).remote_port, Some(7792));
    assert_eq!(Switch::read(Some(r#"{"remote_port":7899}"#)).remote_port, Some(7899));
}

#[test]
fn turning_it_on_chooses_a_port_and_keeps_it_when_it_goes_off_and_on() {
    let on = Switch::default().turned(true, &[], |_| true).unwrap();
    assert_eq!(on, Switch { allow_remote_view: true, remote_port: Some(7792) });
    let off = on.turned(false, &[], |_| true).unwrap();
    assert_eq!(off, Switch { allow_remote_view: false, remote_port: Some(7792) });
    assert!(off.text().is_some(), "the port is kept in the file");
    // The others have taken more by now, and this one still has its own.
    assert_eq!(off.turned(true, &[7792, 7793, 7794], |_| true).unwrap().remote_port, Some(7792));
}

#[test]
fn turning_it_off_with_no_port_leaves_no_file() {
    assert_eq!(Switch::default().turned(false, &[], |_| true).unwrap().text(), None);
}

#[test]
fn ports_taken_or_not_free_are_skipped() {
    assert_eq!(choose_port(&[], |_| true), Some(7792));
    assert_eq!(choose_port(&[7792, 7793], |_| true), Some(7794));
    assert_eq!(choose_port(&[7793], |port| port != 7792), Some(7794));
    assert_eq!(choose_port(&[], |port| port == 7899), Some(7899));
}

#[test]
fn running_out_of_ports_is_said() {
    let all: Vec<u16> = (7792..=7899).collect();
    assert_eq!(all.len(), 108);
    assert_eq!(choose_port(&all, |_| true), None);
    assert_eq!(choose_port(&[], |_| false), None);
    assert_eq!(
        Switch::default().turned(true, &all, |_| true).unwrap_err(),
        "No port is free for sharing the screen (7792 to 7899 are all in use)."
    );
    let nearly: Vec<u16> = (7792..7899).collect();
    assert_eq!(choose_port(&nearly, |_| true), Some(7899));
}
