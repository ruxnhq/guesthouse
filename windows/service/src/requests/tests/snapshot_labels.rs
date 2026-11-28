//! The service's rules for a snapshot's label as they are today, held before they move: which labels
//! it takes and the words of each refusal, which refusal comes first, and the most a machine has.

use super::*;

const LABEL: &str = "A snapshot's label is 1 to 60 characters on one line.";

fn refusal(service: &Arc<Service<Pretend>>, name: &str, label: &str) -> String {
    match ask(service, Request::Snapshot { name: name.into(), label: label.into() }).as_slice() {
        [Answer::Refused { reason }] => reason.clone(),
        other => panic!("{label:?}: {other:?}"),
    }
}

fn take(service: &Arc<Service<Pretend>>, label: &str) {
    assert_eq!(
        ask(service, Request::Snapshot { name: "win".into(), label: label.into() }),
        [Answer::Ok],
        "{label:?}"
    );
    wait_idle(service, "win");
}

#[test]
fn each_label_is_taken_or_refused_in_these_words() {
    let setup = setup("snapshot-labels", Pretend::default());
    let service = &setup.service;
    set_up(&setup, "win");
    for label in [
        "",
        " ",
        "\t",
        "\n",
        "\u{3000}",
        "a\nb",
        "a\tb",
        "a\rb",
        "a\u{7f}",
        "a\u{85}b",
        "a\u{0}",
        &"x".repeat(61),
        &"é".repeat(61),
        &format!(" {} ", "y".repeat(61)),
    ] {
        assert_eq!(refusal(service, "win", label), LABEL, "{label:?}");
    }
    let sixty = "x".repeat(60);
    let sixty_wide = "é".repeat(60);
    for label in [
        "a",
        " padded ",
        sixty.as_str(),
        sixty_wide.as_str(),
        "\t tabbed \t",
        "a",
        "Ünï \"q\" \\",
        "a\u{200b}b",
        "\u{85}nel",
    ] {
        take(service, label);
    }
    let labels: Vec<String> = status(service)[0].snapshots.iter().map(|shown| shown.label.clone()).collect();
    assert_eq!(
        labels,
        [
            "a",
            "padded",
            sixty.as_str(),
            sixty_wide.as_str(),
            "tabbed",
            "a",
            "Ünï \"q\" \\",
            "a\u{200b}b",
            "nel"
        ]
    );
    let (history, _) = service.machines().watch("win").unwrap();
    assert!(history.contains(&Event::Note("Taking the snapshot padded.".into())), "{history:?}");
    assert!(history.contains(&Event::Note("Took the snapshot padded.".into())), "{history:?}");
}

#[test]
fn the_label_is_judged_after_the_machine_is_found_and_before_the_rest() {
    let setup = setup("snapshot-label-order", Pretend::default());
    let service = &setup.service;
    assert_eq!(refusal(service, "nobody", ""), "There is no machine called nobody.");
    ask(service, Request::Create { machine: new_machine(&setup, "new") });
    assert_eq!(refusal(service, "new", ""), LABEL);
    assert_eq!(
        refusal(service, "new", "x"),
        "new isn't set up yet. A machine has snapshots once its first boot has set it up."
    );
    set_up(&setup, "win");
    service.machines().lock().get_mut("win").unwrap().busy = Some("Deleting the snapshot x".into());
    assert_eq!(refusal(service, "win", "\n"), LABEL);
    assert_eq!(refusal(service, "win", "y"), "win is deleting the snapshot x. Stop it first.");
}

/// An index of `count` snapshots, the first `hidden` of them deleted but still kept.
fn index(count: u64, hidden: u64) -> String {
    let entries: Vec<serde_json::Value> = (1..=count)
        .map(|id| serde_json::json!({"id": id, "label": "s", "taken": id, "parent": null, "hidden": id <= hidden}))
        .collect();
    serde_json::json!({"snapshots": entries, "current": null, "next_id": count + 1}).to_string()
}

#[test]
fn a_machine_has_32_snapshots_at_most_and_deleted_ones_kept_dont_count() {
    let setup = setup("snapshot-most", Pretend::default());
    let service = &setup.service;
    set_up(&setup, "win");
    let folder = service.machines().store.disk("win").parent().unwrap().join("snapshots");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("index.json"), index(32, 0)).unwrap();
    let most = "win has 32 snapshots, the most a machine has. Delete one first.";
    assert_eq!(refusal(service, "win", "one more"), most);
    assert_eq!(refusal(service, "win", ""), LABEL, "the label is judged first");
    fs::write(folder.join("index.json"), index(33, 2)).unwrap();
    take(service, "one more");
    assert_eq!(status(service)[0].snapshots.len(), 32);
    assert_eq!(refusal(service, "win", "and another"), most);
}
