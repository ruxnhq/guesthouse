//! Whose each machine is: the user who made it, and administrators, see it and act on it, and
//! nobody else does.

use super::*;
use guesthouse_control::Forward;

const ADMINISTRATORS: &str = "S-1-5-32-544";

/// A standard user. `USER`'s account is an administrator's, whose app runs unelevated, as on a
/// PC with one user.
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
    let asker = Asker { user: user.into(), administrator, administrators_account: user != SOMEONE_ELSE };
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

/// A machine from before owners, as a store has it, and the service started on it with `work`.
fn old_machine(setup: &Setup, work: impl FnOnce(&Store) -> Pretend) -> (Store, Arc<Service<Pretend>>) {
    let store = setup.service.machines().store.clone();
    store.make("old").unwrap();
    store.save("old", &Settings::basic(8192, 1, Card::Default)).unwrap();
    let service = Service::new(store.clone(), work(&store));
    (store, service)
}

#[test]
fn a_machine_from_before_owners_is_the_user_its_files_belong_to() {
    let setup = setup("owners-files", Pretend::default());
    let (store, service) = old_machine(&setup, |store| Pretend {
        owners: vec![(store.folder("old"), "S-1-5-18".into()), (store.disk("old"), SOMEONE_ELSE.into())],
        ..Pretend::default()
    });
    assert_eq!(store.load()[0].1.owner.as_deref(), Some(SOMEONE_ELSE), "kept as the service starts");
    // The PC's administrator asking first, unelevated, takes nothing that's someone's.
    assert!(names_listed(&service, USER, false).is_empty());
    assert_eq!(names_listed(&service, SOMEONE_ELSE, false), ["old"]);
}

#[test]
fn a_machine_whose_files_name_no_user_waits_for_an_administrators_account() {
    let setup = setup("owners-old", Pretend::default());
    let (store, service) = old_machine(&setup, |store| Pretend {
        owners: vec![(store.folder("old"), "S-1-5-18".into()), (store.disk("old"), ADMINISTRATORS.into())],
        ..Pretend::default()
    });
    // A standard user neither sees it nor takes it.
    assert!(names_listed(&service, SOMEONE_ELSE, false).is_empty());
    let account = Request::Account { name: "old".into() };
    assert_eq!(
        ask_as(&service, SOMEONE_ELSE, false, account),
        [Answer::Refused { reason: "There is no machine called old.".into() }]
    );
    // An elevated administrator sees it without taking it.
    assert_eq!(names_listed(&service, AN_ADMINISTRATOR, true), ["old"]);
    assert_eq!(store.load()[0].1.owner, None);
    // The administrator's account asking as itself, as the app does on a PC with one user, takes it,
    // and that's kept before it's answered and through the service starting again.
    assert_eq!(names_listed(&service, USER, false), ["old"]);
    assert_eq!(store.load()[0].1.owner.as_deref(), Some(USER));
    let again = Service::new(store, Pretend::default());
    assert_eq!(names_listed(&again, USER, false), ["old"]);
    assert!(names_listed(&again, SOMEONE_ELSE, false).is_empty());
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

/// Windows' own profiles, which every PC keeps: the system's and two services'.
fn windows_own() -> Vec<String> {
    ["S-1-5-18", "S-1-5-19", "S-1-5-20"].map(String::from).to_vec()
}

#[test]
fn a_pcs_one_user_takes_a_waiting_machine_and_with_two_users_it_waits() {
    let one = setup("owners-one-user", Pretend::default());
    let profiles = [windows_own(), vec![SOMEONE_ELSE.to_owned()]].concat();
    let (store, service) = old_machine(&one, |_| Pretend { profiles, ..Pretend::default() });
    assert_eq!(names_listed(&service, SOMEONE_ELSE, false), ["old"], "a standard user, the PC's one");
    assert_eq!(store.load()[0].1.owner.as_deref(), Some(SOMEONE_ELSE));

    let two = setup("owners-two-users", Pretend::default());
    let profiles = [windows_own(), vec![SOMEONE_ELSE.to_owned(), "S-1-5-21-1-2-3-1003".to_owned()]].concat();
    let (store, service) = old_machine(&two, |_| Pretend { profiles, ..Pretend::default() });
    assert!(names_listed(&service, SOMEONE_ELSE, false).is_empty(), "one of two standard users");
    assert_eq!(store.load()[0].1.owner, None);
}
