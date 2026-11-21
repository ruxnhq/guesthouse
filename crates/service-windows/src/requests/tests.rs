use super::*;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};

/// Work that makes a disk as a file, and runs a machine until it's asked to stop.
#[derive(Default)]
struct Pretend {
    fail_prepare: bool,
    linux_installer: Option<PathBuf>,
    ended: Mutex<Vec<String>>,
    booted: Mutex<Vec<BootSettings>>,
}

impl Work for Pretend {
    fn prepare(&self, settings: &PrepareSettings, sink: &dyn Sink) -> Result<(), String> {
        sink.note("making the disk");
        if self.fail_prepare {
            return Err("the image is broken".into());
        }
        fs::write(&settings.disk, b"disk").unwrap();
        fs::write(settings.disk.with_extension("account.txt"), "user: guesthouse\npassword: secret\n")
            .unwrap();
        if let Some(installer) = &self.linux_installer {
            let record =
                Record { guest: Guest::Linux, installer: Some(installer.clone()), ..Record::default() };
            record.save(&settings.disk).unwrap();
        }
        Ok(())
    }

    fn boot(
        &self,
        settings: &BootSettings,
        sink: Arc<dyn Sink>,
        stop: Arc<AtomicBool>,
    ) -> Result<(), String> {
        self.booted.lock().unwrap().push(settings.clone());
        sink.state(State::Running);
        sink.event(Event::Guest("guesthouse: address: 172.29.1.2".into()));
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
        sink.state(State::Stopped);
        Ok(())
    }

    fn cards(&self) -> Vec<CardInfo> {
        vec![
            CardInfo { name: "Radeon Graphics".into(), memory_mb: Some(512), default: false },
            CardInfo { name: "NVIDIA GeForce RTX 5090".into(), memory_mb: Some(32 * 1024), default: true },
        ]
    }

    fn end(&self, id: &str) {
        self.ended.lock().unwrap().push(id.to_owned());
    }
}

/// A window that keeps every answer, may read every file unless told otherwise, and may close.
struct Window {
    answers: Sender<Answer>,
    readable: bool,
    gone: bool,
}

impl Connection for Window {
    fn send(&mut self, answer: &Answer) -> io::Result<()> {
        self.answers.send(answer.clone()).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn gone(&mut self) -> bool {
        self.gone
    }

    fn may_read(&mut self, path: &Path) -> Result<(), String> {
        if self.readable { Ok(()) } else { Err(format!("You may not read {}.", path.display())) }
    }
}

struct Setup {
    service: Arc<Service<Pretend>>,
    root: PathBuf,
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn setup(test: &str, work: Pretend) -> Setup {
    let root = std::env::temp_dir().join(format!("gh-requests-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("machines")).unwrap();
    fs::write(root.join("image.iso"), b"iso").unwrap();
    Setup { service: Service::new(Store::new(root.join("machines")), work), root }
}

fn ask(service: &Arc<Service<Pretend>>, request: Request) -> Vec<Answer> {
    let (answers, received) = channel();
    service.answer(request, &mut Window { answers, readable: true, gone: false });
    received.try_iter().collect()
}

fn new_machine(setup: &Setup, name: &str) -> NewMachine {
    NewMachine {
        name: name.into(),
        image: setup.root.join("image.iso"),
        memory_mb: 8192,
        processors: 1,
        size_gb: 128,
        card: Card::Default,
    }
}

fn status(service: &Arc<Service<Pretend>>) -> Vec<Machine> {
    match ask(service, Request::Status).pop() {
        Some(Answer::Status { machines, .. }) => machines,
        other => panic!("{other:?}"),
    }
}

fn wait_for(service: &Arc<Service<Pretend>>, name: &str, state: State) {
    for _ in 0..400 {
        if service
            .machines()
            .lock()
            .get(name)
            .is_some_and(|entry| entry.state == state && (state != State::Stopped || entry.idle()))
        {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("{name} never became {state:?}");
}

#[test]
fn a_new_machine_is_made_with_its_events_then_done() {
    let setup = setup("create", Pretend::default());
    let answers = ask(&setup.service, Request::Create { machine: new_machine(&setup, "win") });
    assert_eq!(answers.first(), Some(&Answer::Event { event: Event::State(State::Preparing) }));
    assert!(answers.contains(&Answer::Event { event: Event::Note("making the disk".into()) }));
    assert_eq!(answers.last(), Some(&Answer::Done { reason: None }));
    let machines = status(&setup.service);
    assert_eq!(machines.len(), 1);
    assert_eq!(
        (machines[0].state, machines[0].card.as_deref()),
        (State::Stopped, Some("NVIDIA GeForce RTX 5090"))
    );
    assert_eq!(setup.service.machines().store.load().len(), 1, "its settings are kept");
}

#[test]
fn a_machine_that_couldnt_be_made_is_gone_and_its_name_free() {
    let setup = setup("failed", Pretend { fail_prepare: true, ..Pretend::default() });
    let answers = ask(&setup.service, Request::Create { machine: new_machine(&setup, "win") });
    assert_eq!(answers.last(), Some(&Answer::Done { reason: Some("the image is broken".into()) }));
    assert!(status(&setup.service).is_empty());
    assert!(!setup.service.machines().store.folder("win").exists());
}

#[test]
fn what_the_app_asks_for_is_kept_to_the_services_rules() {
    let setup = setup("rules", Pretend::default());
    let refused = |machine: NewMachine| {
        matches!(ask(&setup.service, Request::Create { machine }).as_slice(), [Answer::Refused { .. }])
    };
    assert!(refused(NewMachine { name: "con".into(), ..new_machine(&setup, "x") }));
    assert!(refused(NewMachine { memory_mb: 1, ..new_machine(&setup, "x") }));
    assert!(refused(NewMachine { memory_mb: u64::MAX, ..new_machine(&setup, "x") }));
    assert!(refused(NewMachine { processors: 0, ..new_machine(&setup, "x") }));
    assert!(refused(NewMachine { size_gb: u64::MAX, ..new_machine(&setup, "x") }));
    assert!(refused(NewMachine { image: PathBuf::from("relative.iso"), ..new_machine(&setup, "x") }));
    assert!(refused(NewMachine {
        image: PathBuf::from(r"\\server\share\x.iso"),
        ..new_machine(&setup, "x")
    }));
    assert!(refused(NewMachine { image: setup.root.join("missing.iso"), ..new_machine(&setup, "x") }));
    assert!(status(&setup.service).is_empty());

    ask(&setup.service, Request::Create { machine: new_machine(&setup, "win") });
    assert!(refused(new_machine(&setup, "WIN")), "names are the same whatever their case");
}

#[test]
fn an_image_the_user_may_not_read_is_refused() {
    let setup = setup("unreadable", Pretend::default());
    let (answers, received) = channel();
    let machine = new_machine(&setup, "win");
    setup.service.answer(Request::Create { machine }, &mut Window { answers, readable: false, gone: false });
    assert!(matches!(received.try_iter().collect::<Vec<_>>().as_slice(), [Answer::Refused { .. }]));
    assert!(!setup.service.machines().store.folder("win").exists());
}

#[test]
fn a_machine_starts_stops_and_only_then_is_deleted() {
    let setup = setup("lifecycle", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    assert_eq!(ask(service, Request::Start { name: "win".into() }), [Answer::Ok]);
    wait_for(service, "win", State::Running);
    assert!(matches!(
        ask(service, Request::Start { name: "win".into() }).as_slice(),
        [Answer::Refused { .. }]
    ));
    assert!(matches!(
        ask(service, Request::Delete { name: "win".into() }).as_slice(),
        [Answer::Refused { .. }]
    ));
    assert_eq!(service.work.booted.lock().unwrap()[0].name, "guesthouse-win");

    assert_eq!(ask(service, Request::Stop { name: "win".into() }), [Answer::Ok]);
    wait_for(service, "win", State::Stopped);
    assert_eq!(status(service)[0].address.as_deref(), Some("172.29.1.2"));
    assert!(matches!(
        ask(service, Request::Stop { name: "win".into() }).as_slice(),
        [Answer::Refused { .. }]
    ));
    assert_eq!(
        ask(service, Request::Account { name: "win".into() }),
        [Answer::Account { user: "guesthouse".into(), password: "secret".into() }]
    );
    assert_eq!(ask(service, Request::Delete { name: "win".into() }), [Answer::Ok]);
    assert!(status(service).is_empty());
    assert!(!service.machines().store.folder("win").exists());
}

#[test]
fn a_watcher_gets_what_happened_and_what_happens_until_it_goes() {
    let setup = setup("watch", Pretend::default());
    let service = Arc::clone(&setup.service);
    ask(&service, Request::Create { machine: new_machine(&setup, "win") });
    let (answers, received) = channel();
    let watching = Arc::clone(&service);
    let watcher = thread::spawn(move || {
        watching.answer(
            Request::Watch { name: "win".into() },
            &mut Window { answers, readable: true, gone: false },
        );
    });
    assert_eq!(received.recv().unwrap(), Answer::Event { event: Event::State(State::Preparing) });
    ask(&service, Request::Start { name: "win".into() });
    let running = Answer::Event { event: Event::State(State::Running) };
    while received.recv_timeout(Duration::from_secs(2)).unwrap() != running {}
    // The window closes its end: the watch ends with the next event.
    drop(received);
    ask(&service, Request::Stop { name: "win".into() });
    watcher.join().unwrap();
}

#[test]
fn a_linux_guest_is_given_a_copy_of_its_installer() {
    let root = std::env::temp_dir().join(format!("gh-requests-iso-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let iso = root.join("ubuntu.iso");
    fs::write(&iso, b"ubuntu").unwrap();
    let setup = setup("installer", Pretend { linux_installer: Some(iso.clone()), ..Pretend::default() });
    ask(&setup.service, Request::Create { machine: new_machine(&setup, "ubuntu") });
    let store = &setup.service.machines().store;
    let record = Record::load(&store.disk("ubuntu")).unwrap();
    assert_eq!(record.installer.as_deref(), Some(store.installer("ubuntu").as_path()));
    assert_eq!(fs::read(store.installer("ubuntu")).unwrap(), b"ubuntu");
    assert_eq!(status(&setup.service)[0].system, System::Linux);

    // Once it has installed, the copy goes at the next start.
    Record { installed: true, ..record }.save(&store.disk("ubuntu")).unwrap();
    ask(&setup.service, Request::Start { name: "ubuntu".into() });
    assert!(!store.installer("ubuntu").exists());
    ask(&setup.service, Request::Stop { name: "ubuntu".into() });
    wait_for(&setup.service, "ubuntu", State::Stopped);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn machines_left_running_are_ended_when_the_service_starts_and_stops() {
    let setup = setup("leftover", Pretend::default());
    ask(&setup.service, Request::Create { machine: new_machine(&setup, "win") });
    let again = Service::new(setup.service.machines().store.clone(), Pretend::default());
    assert_eq!(*again.work.ended.lock().unwrap(), ["guesthouse-win"]);

    ask(&again, Request::Start { name: "win".into() });
    wait_for(&again, "win", State::Running);
    again.stop_all(Duration::from_secs(2));
    assert!(again.machines().lock()["win"].idle(), "asked to shut down, it did");
}
