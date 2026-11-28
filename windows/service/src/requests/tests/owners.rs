//! Whose each machine is: the user who made it, and administrators, see it and act on it, and
//! nobody else does.

use super::*;
use guesthouse_control::Forward;

const SOMEONE_ELSE: &str = "S-1-5-21-1-2-3-1002";
const AN_ADMINISTRATOR: &str = "S-1-5-21-1-2-3-500";

/// A window of someone's in particular, which keeps every answer.
struct As {
    asker: Asker,
    answers: Sender<Answer>,
}

impl Connection for As {
    fn send(&mut self, answer: &Answer) -> io::Result<()> {
        self.answers.send(answer.clone()).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn gone(&mut self) -> bool {
        true
    }

    fn may_read(&mut self, path: &Path) -> Result<(PathBuf, Held), String> {
        Ok((path.to_owned(), Box::new(())))
    }

    fn may_share(&mut self, _: &Path, _: bool) -> Result<Held, String> {
        Ok(Box::new(()))
    }

    fn may_place(&mut self, _: &Path) -> Result<Held, String> {
        Ok(Box::new(()))
    }

    fn asker(&self) -> Asker {
        self.asker.clone()
    }
}

fn ask_as(service: &Arc<Service<Pretend>>, user: &str, administrator: bool, request: Request) -> Vec<Answer> {
    let (answers, received) = channel();
    let asker = Asker { user: user.into(), administrator };
    service.answer(request, &mut As { asker, answers });
    received.try_iter().collect()
}

fn names_listed(service: &Arc<Service<Pretend>>, user: &str, administrator: bool) -> Vec<String> {
    match ask_as(service, user, administrator, Request::Status).as_slice() {
        [Answer::Status { machines, .. }] => machines.iter().map(|machine| machine.name.clone()).collect(),
        other => panic!("not a status: {other:?}"),
    }
}

/// What each request naming `name` asks of a machine, its settings left as they are.
fn every_request_naming(name: &str, settings: MachineSettings) -> Vec<Request> {
    let name = name.to_owned();
    vec![
        Request::Start { name: name.clone() },
        Request::Stop { name: name.clone() },
        Request::End { name: name.clone() },
        Request::Screen { name: name.clone() },
        Request::Grow { name: name.clone(), size_gb: 200 },
        Request::Snapshot { name: name.clone(), label: "x".into() },
        Request::Revert { name: name.clone(), id: 1 },
        Request::DeleteSnapshot { name: name.clone(), id: 1 },
        Request::Account { name: name.clone() },
        Request::Watch { name: name.clone() },
        Request::Change { name: name.clone(), settings },
        Request::Move { name: name.clone(), folder: PathBuf::from(r"C:\elsewhere") },
        Request::ReportFiles { machine: Some(name.clone()) },
        Request::Share { name: name.clone(), on: Some(true), pairing: None, pairing_seconds: None },
        Request::RemoteView { name: name.clone() },
        Request::PairClose { name: name.clone() },
        Request::PairAccept { name: name.clone() },
        Request::PairDecline { name: name.clone() },
        Request::DeviceAllow { name: name.clone(), fingerprint: "ab".into(), what: "input".into(), on: true },
        Request::DeviceRevoke { name: name.clone(), fingerprint: "ab".into() },
        Request::Delete { name },
    ]
}

#[test]
fn a_machine_is_its_makers_and_another_user_finds_nothing_there() {
    let setup = setup("owners-other", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    assert_eq!(service.machines().store.load()[0].1.owner.as_deref(), Some(USER), "kept on disk");
    assert_eq!(names_listed(service, USER, false), ["win"]);
    assert!(names_listed(service, SOMEONE_ELSE, false).is_empty());

    for request in every_request_naming("WIN", hardware(4096, Card::Default)) {
        let said = format!("{request:?}");
        assert_eq!(
            ask_as(service, SOMEONE_ELSE, false, request),
            [Answer::Refused { reason: "There is no machine called WIN.".into() }],
            "{said}"
        );
    }
    // Nothing they asked did anything to it.
    let kept = &service.machines().store.load()[0].1;
    assert_eq!((kept.memory_mb, kept.owner.as_deref()), (8192, Some(USER)));
    assert_eq!(names_listed(service, USER, false), ["win"]);
    let (files, _, _) = report_as(&setup, SOMEONE_ELSE);
    let (_, machines) = files.iter().find(|(name, _)| name == "machines.txt").unwrap();
    assert_eq!(machines, "", "a report lists only the asker's machines");
}

#[test]
fn an_administrator_sees_and_acts_on_every_machine_and_takes_none() {
    let setup = setup("owners-admin", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    assert_eq!(names_listed(service, AN_ADMINISTRATOR, true), ["win"]);
    let change = Request::Change { name: "win".into(), settings: hardware(4096, Card::Default) };
    assert_eq!(ask_as(service, AN_ADMINISTRATOR, true, change), [Answer::Ok]);
    let kept = &service.machines().store.load()[0].1;
    assert_eq!((kept.memory_mb, kept.owner.as_deref()), (4096, Some(USER)), "still its maker's");
    assert_eq!(ask_as(service, AN_ADMINISTRATOR, true, Request::Delete { name: "win".into() }), [Answer::Ok]);
}

#[test]
fn a_machine_from_before_owners_is_the_first_users_who_asks_and_no_one_elses() {
    let setup = setup("owners-old", Pretend::default());
    let store = setup.service.machines().store.clone();
    store.make("old").unwrap();
    store.save("old", &Settings::basic(8192, 1, Card::Default)).unwrap();
    let service = Service::new(store.clone(), Pretend::default());

    // An administrator sees it, and asking doesn't make it theirs.
    assert_eq!(names_listed(&service, AN_ADMINISTRATOR, true), ["old"]);
    assert_eq!(store.load()[0].1.owner, None);
    // The first user who asks anything takes it, and that's kept before they're answered.
    assert_eq!(names_listed(&service, USER, false), ["old"]);
    assert_eq!(store.load()[0].1.owner.as_deref(), Some(USER));
    assert!(names_listed(&service, SOMEONE_ELSE, false).is_empty());
    let account = Request::Account { name: "old".into() };
    assert_eq!(
        ask_as(&service, SOMEONE_ELSE, false, account),
        [Answer::Refused { reason: "There is no machine called old.".into() }]
    );
    // And it stays theirs when the service starts again.
    let again = Service::new(store, Pretend::default());
    assert!(names_listed(&again, SOMEONE_ELSE, false).is_empty());
    assert_eq!(names_listed(&again, USER, false), ["old"]);
}

#[test]
fn a_machines_console_is_opened_by_its_owner_and_a_change_keeps_its_owner() {
    let setup = setup("owners-console", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    assert_eq!(change(service, hardware(4096, Card::Default)), [Answer::Ok]);
    assert_eq!(service.machines().store.load()[0].1.owner.as_deref(), Some(USER));
    assert_eq!(ask_as(service, AN_ADMINISTRATOR, true, Request::Start { name: "win".into() }), [Answer::Ok]);
    wait_for(service, "win", State::Running);
    assert_eq!(
        service.work.booted.lock().unwrap()[0].console_access,
        [USER],
        "its owner's, not the starter's"
    );
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Off);
}

#[test]
fn another_users_machine_is_not_named_in_a_refusal() {
    let setup = setup("owners-words", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let forwards = vec![Forward { host_port: 8080, guest_port: 80 }];
    assert_eq!(
        change(service, MachineSettings { forwards: forwards.clone(), ..hardware(8192, Card::Default) }),
        [Answer::Ok]
    );
    let mut theirs = new_machine(&setup, "theirs");
    theirs.image = setup.root.join("image.iso");
    ask_as(service, SOMEONE_ELSE, false, Request::Create { machine: theirs });
    let wanted = MachineSettings { forwards, ..hardware(8192, Card::Default) };
    let answers =
        ask_as(service, SOMEONE_ELSE, false, Request::Change { name: "theirs".into(), settings: wanted });
    assert_eq!(
        answers,
        [Answer::Refused { reason: "This PC's port 8080 is forwarded to another machine already.".into() }]
    );
}

fn report_as(setup: &Setup, user: &str) -> Report {
    let answers = ask_as(&setup.service, user, false, Request::ReportFiles { machine: None });
    let mut parts = guesthouse_control::report::Parts::default();
    for answer in &answers {
        match answer {
            Answer::ReportPart { name, text } => parts.piece(name, text),
            Answer::ReportMissing { name, reason } => parts.miss(name, reason),
            _ => {}
        }
    }
    (parts.files, parts.missing, answers)
}
