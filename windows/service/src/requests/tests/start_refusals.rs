//! What the service says when a machine can't start, as it is today: it judges no checks before a
//! start, only these refusals, in this order, and a share left out of one boot. A machine whose disks
//! can't be used is refused as every request is, in `places`.

use super::*;

fn refusal(service: &Arc<Service<Pretend>>, name: &str) -> String {
    match ask(service, Request::Start { name: name.into() }).as_slice() {
        [Answer::Refused { reason }] => reason.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_start_is_refused_in_these_words_in_this_order() {
    let setup = setup("start-refusals", Pretend::default());
    let service = &setup.service;
    assert_eq!(refusal(service, "nobody"), "There is no machine called nobody.");
    set_up(&setup, "win");
    service.machines().lock().get_mut("win").unwrap().busy = Some("Deleting the snapshot x".into());
    assert_eq!(refusal(service, "win"), "win is deleting the snapshot x.");
    service.machines().lock().get_mut("win").unwrap().busy = None;
    ask(service, Request::Start { name: "win".into() });
    wait_for(service, "win", State::Running);
    assert_eq!(refusal(service, "win"), "win is running.");
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Off);
}

#[test]
fn a_share_left_out_of_a_boot_is_said_so() {
    let setup = setup("start-share-words", Pretend::default());
    let service = &setup.service;
    let folder = setup.root.join("gone");
    let machine = NewMachine {
        shares: vec![Share { folder: folder.clone(), read_only: false }],
        ..new_machine(&setup, "win")
    };
    fs::create_dir_all(&folder).unwrap();
    ask(service, Request::Create { machine });
    fs::remove_dir_all(&folder).unwrap();
    assert_eq!(ask(service, Request::Start { name: "win".into() }), [Answer::Ok]);
    wait_for(service, "win", State::Running);
    let (history, _) = service.machines().watch("win").unwrap();
    let said: Vec<&String> = history
        .iter()
        .filter_map(|event| if let Event::Problem(words) = event { Some(words) } else { None })
        .collect();
    let shown = folder.display();
    assert_eq!(said, [&format!("This boot doesn't share {shown}: {shown} isn't a folder.")]);
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Off);
}
