use super::*;

#[test]
fn a_label_is_trimmed_and_1_to_60_characters_on_one_line() {
    let sixty = "é".repeat(60);
    for (label, kept) in
        [(" a ", "a"), ("\u{85}nel", "nel"), ("a\u{200b}b", "a\u{200b}b"), (sixty.as_str(), sixty.as_str())]
    {
        assert_eq!(checked_label(label), Ok(kept));
    }
    for label in ["", " ", "a\nb", "a\tb", "a\u{7f}", "a\u{85}b", &"x".repeat(61)] {
        assert_eq!(
            checked_label(label),
            Err("A snapshot's label is 1 to 60 characters on one line.".to_owned()),
            "{label:?}"
        );
    }
}

#[test]
fn a_machine_has_32_snapshots_at_most() {
    assert_eq!(count_refusal("win", 31), None);
    for count in [32, 33] {
        assert_eq!(
            count_refusal("win", count).as_deref(),
            Some("win has 32 snapshots, the most a machine has. Delete one first.")
        );
    }
}

#[test]
fn the_index_is_read_and_written_so() {
    let older = r#"{"snapshots": [{"id": 1, "label": "old", "taken": 5, "parent": null}], "current": 1, "future": 1}"#;
    let index = read_index(older).unwrap();
    assert_eq!(shown(&index), [Snapshot { id: 1, label: "old".into(), taken: 5, current: true }]);
    assert_eq!(index.next_id, 0);
    assert_eq!(
        index_text(&index),
        "{\n  \"snapshots\": [\n    {\n      \"id\": 1,\n      \"label\": \"old\",\n      \"taken\": 5,\n      \
         \"parent\": null,\n      \"hidden\": false\n    }\n  ],\n  \"current\": 1,\n  \"next_id\": 0\n}\n"
    );
    assert_eq!(read_index("{}").unwrap(), Index::default());
    assert_eq!(read_index("not json").unwrap_err().to_string(), "expected ident at line 1 column 2");
}

#[test]
fn the_chain_and_links_are_read_from_the_index() {
    let disk = Path::new("machine").join("win.vhdx");
    let entry = |id, parent, hidden| Entry { id, label: String::new(), taken: 0, parent, hidden };
    let index = Index {
        snapshots: vec![entry(1, None, true), entry(2, Some(1), false), entry(3, Some(1), false)],
        current: Some(3),
        next_id: 4,
        taking: None,
    };
    let in_folder = |id: u64| Path::new("machine").join("snapshots").join(format!("{id}.vhdx"));
    assert_eq!(chain_in(&index, &disk).unwrap(), [in_folder(3), in_folder(1)]);
    assert_eq!(
        links_in(&index, &disk),
        [(in_folder(2), in_folder(1)), (in_folder(3), in_folder(1)), (disk.clone(), in_folder(3))]
    );
    assert_eq!(shown(&index).len(), 2);
    let lost = Index { current: Some(9), ..Index::default() };
    assert_eq!(
        chain_in(&lost, &disk),
        Err("The snapshot index names a snapshot 9 it doesn't have.".to_owned())
    );
}
