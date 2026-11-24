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
    prepared: Mutex<Vec<PrepareSettings>>,
    grown: Mutex<Vec<u64>>,
}

impl Work for Pretend {
    fn prepare(&self, settings: &PrepareSettings, sink: &dyn Sink) -> Result<(), String> {
        sink.note("making the disk");
        self.prepared.lock().unwrap().push(settings.clone());
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

    fn console_access(&self) -> Vec<String> {
        vec!["S-1-5-21-1-2-3-1001".into()]
    }

    fn grow(&self, _: &Path, size_gb: u64) -> Result<(), String> {
        self.grown.lock().unwrap().push(size_gb);
        Ok(())
    }

    fn disk_size_gb(&self, _: &Path) -> Option<u64> {
        Some(200)
    }

    fn disks(&self) -> &dyn Disks {
        &FileDisks
    }

    fn report_parts(&self) -> Vec<(String, Result<String, String>)> {
        let log = "started\nrefused: pid 4 asked for a thing\nstopped\n";
        vec![
            ("service.log".to_owned(), Ok(crate::report_parts::without_refusals(log))),
            ("windows.txt".to_owned(), Ok("ProductName: Windows 11 Pro\n".to_owned())),
            ("crashes.txt".to_owned(), Err("wevtutil isn't here.".to_owned())),
        ]
    }
}

/// Disks as small files that name their parent, which is enough for the service's part. The disk
/// work itself is tested in host-windows' snapshots.
struct FileDisks;

impl Disks for FileDisks {
    fn differencing(&self, child: &Path, parent: &Path) -> Result<(), String> {
        assert!(parent.is_file(), "made on a disk that's there");
        fs::write(child, format!("on {}", parent.display())).map_err(|error| error.to_string())
    }

    fn merge_into_parent(&self, _: &Path) -> Result<(), String> {
        Ok(())
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

    fn may_share(&mut self, path: &Path, _: bool) -> Result<Held, String> {
        self.may_read(path).map(|()| Box::new(()) as Held)
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
        edition: String::new(),
        user: String::new(),
        tpm: true,
        network: true,
        share: None,
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
    assert_eq!(service.work.booted.lock().unwrap()[0].console_access, ["S-1-5-21-1-2-3-1001"]);

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

#[test]
fn a_machine_is_ended_only_while_it_runs_and_has_a_screen_only_once_its_guest_says() {
    let setup = setup("end", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    assert!(matches!(ask(service, Request::End { name: "win".into() }).as_slice(), [Answer::Refused { .. }]));
    assert!(matches!(
        ask(service, Request::Screen { name: "win".into() }).as_slice(),
        [Answer::Refused { .. }]
    ));
    ask(service, Request::Start { name: "win".into() });
    wait_for(service, "win", State::Running);
    assert_eq!(ask(service, Request::End { name: "win".into() }), [Answer::Ok]);
    assert_eq!(*service.work.ended.lock().unwrap(), ["guesthouse-win"]);
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Stopped);
}

fn change(service: &Arc<Service<Pretend>>, settings: MachineSettings) -> Vec<Answer> {
    ask(service, Request::Change { name: "WIN".into(), settings })
}

#[test]
fn settings_change_while_off_are_kept_and_reach_the_next_boot() {
    let setup = setup("change", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let machine = &status(service)[0];
    assert_eq!((machine.chosen_card.clone(), machine.nvapi_wrapper), (Card::Default, true));
    let wanted = MachineSettings {
        memory_mb: 16384,
        processors: 1,
        card: Card::Named("Radeon Graphics".into()),
        nvapi_wrapper: false,
        tpm: true,
        network: true,
        share: None,
    };
    assert_eq!(change(service, wanted.clone()), [Answer::Ok]);
    let machine = &status(service)[0];
    assert_eq!((machine.memory_mb, machine.card.as_deref()), (16384, Some("Radeon Graphics")));
    assert_eq!((machine.chosen_card.clone(), machine.nvapi_wrapper), (wanted.card.clone(), false));
    let kept = service.machines().store.load();
    assert_eq!((kept[0].1.memory_mb, kept[0].1.nvapi_wrapper), (16384, false));

    assert_eq!(ask(service, Request::Start { name: "win".into() }), [Answer::Ok]);
    wait_for(service, "win", State::Running);
    let booted = service.work.booted.lock().unwrap()[0].clone();
    assert_eq!((booted.memory_mb, booted.nvapi_wrapper), (16384, false));
    assert_eq!(booted.gpu, GpuChoice::Matching("Radeon Graphics".into()));
    let refused = change(service, MachineSettings { nvapi_wrapper: true, ..wanted.clone() });
    assert!(matches!(refused.as_slice(), [Answer::Refused { reason }] if reason.contains("while it's off")));
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Stopped);
}

fn hardware(memory_mb: u64, card: Card) -> MachineSettings {
    MachineSettings {
        memory_mb,
        processors: 1,
        card,
        nvapi_wrapper: true,
        tpm: true,
        network: true,
        share: None,
    }
}

#[test]
fn a_stopped_machine_is_changed_and_boots_with_it_next() {
    let setup = setup("change-boots", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let change = |hardware| ask(service, Request::Change { name: "WIN".into(), settings: hardware });

    let card = Card::Named("Radeon Graphics".into());
    assert_eq!(change(hardware(16384, card.clone())), [Answer::Ok]);
    let machine = &status(service)[0];
    assert_eq!(
        (machine.memory_mb, machine.card.as_deref(), &machine.chosen_card),
        (16384, Some("Radeon Graphics"), &card)
    );
    let kept = &service.machines().store.load()[0].1;
    assert_eq!((kept.memory_mb, &kept.card), (16384, &card), "it's kept for the service's next start");

    ask(service, Request::Start { name: "win".into() });
    wait_for(service, "win", State::Running);
    let booted = service.work.booted.lock().unwrap()[0].clone();
    assert_eq!((booted.memory_mb, booted.gpu), (16384, GpuChoice::Matching("Radeon Graphics".into())));

    // Not while it runs, and its address, which the guest said, outlasts a change.
    assert!(matches!(change(hardware(8192, Card::None)).as_slice(), [Answer::Refused { .. }]));
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Stopped);
    assert_eq!(change(hardware(8192, Card::None)), [Answer::Ok]);
    let machine = &status(service)[0];
    assert_eq!((machine.card.as_deref(), machine.address.as_deref()), (None, Some("172.29.1.2")));
}

#[test]
fn a_change_is_kept_to_the_services_rules() {
    let setup = setup("change-rules", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let fine = hardware(8192, Card::None);
    for (wrong, words) in [
        (MachineSettings { memory_mb: 1024, ..fine.clone() }, "MB of memory"),
        (MachineSettings { processors: 0, ..fine.clone() }, "processors"),
        (MachineSettings { card: Card::Named("Voodoo".into()), ..fine.clone() }, "no card called Voodoo"),
    ] {
        let said = change(service, wrong);
        assert!(
            matches!(said.as_slice(), [Answer::Refused { reason }] if reason.contains(words)),
            "{said:?}"
        );
    }
    let nobody = ask(service, Request::Change { name: "nope".into(), settings: fine.clone() });
    assert!(matches!(nobody.as_slice(), [Answer::Refused { .. }]));
    assert_eq!(change(service, fine), [Answer::Ok]);
    assert_eq!(status(service)[0].card, None);
}

#[test]
fn a_change_is_kept_to_the_services_rules_for_share_and_size_too() {
    let setup = setup("change-rules-more", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let refused = |hardware| {
        matches!(
            ask(service, Request::Change { name: "win".into(), settings: hardware }).as_slice(),
            [Answer::Refused { .. }]
        )
    };
    assert!(refused(hardware(1, Card::Default)));
    assert!(refused(hardware(u64::MAX, Card::Default)));
    assert!(refused(MachineSettings { processors: 0, ..hardware(8192, Card::Default) }));
    assert!(refused(hardware(8192, Card::Named("A card this PC hasn't".into()))));
    assert!(matches!(
        ask(service, Request::Change { name: "ubuntu".into(), settings: hardware(8192, Card::Default) })
            .as_slice(),
        [Answer::Refused { .. }]
    ));
    assert_eq!(status(service)[0].memory_mb, 8192, "nothing refused was kept");
}

#[test]
fn an_ubuntu_guest_keeps_its_card_but_not_its_size() {
    let root = std::env::temp_dir().join(format!("gh-requests-change-iso-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let iso = root.join("ubuntu.iso");
    fs::write(&iso, b"ubuntu").unwrap();
    let setup = setup("change-linux", Pretend { linux_installer: Some(iso), ..Pretend::default() });
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "ubuntu") });
    let change = |hardware| ask(service, Request::Change { name: "ubuntu".into(), settings: hardware });

    assert!(matches!(change(hardware(8192, Card::None)).as_slice(), [Answer::Refused { .. }]));
    assert!(matches!(
        change(hardware(8192, Card::Named("Radeon Graphics".into()))).as_slice(),
        [Answer::Refused { .. }]
    ));
    // The default card by its name is the same card.
    let same = Card::Named("NVIDIA GeForce RTX 5090".into());
    assert_eq!(change(hardware(16384, same.clone())), [Answer::Ok]);
    assert_eq!(status(service)[0].chosen_card, same);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_card_thats_gone_from_the_pc_doesnt_stop_a_change_of_size() {
    let setup = setup("change-gone", Pretend::default());
    let service = &setup.service;
    let gone = Card::Named("A card taken out".into());
    ask(
        service,
        Request::Create { machine: NewMachine { card: gone.clone(), ..new_machine(&setup, "win") } },
    );
    assert_eq!(
        ask(service, Request::Change { name: "win".into(), settings: hardware(16384, gone) }),
        [Answer::Ok]
    );
}

#[test]
fn a_new_machine_is_made_with_its_user_and_edition_and_boots_with_its_switches() {
    let setup = setup("switches", Pretend::default());
    let service = &setup.service;
    let folder = setup.root.join("shared");
    fs::create_dir_all(&folder).unwrap();
    let share = Share { folder: folder.clone(), read_only: true };
    let machine = NewMachine {
        user: "pat".into(),
        edition: " Windows 11 Home ".into(),
        tpm: false,
        network: false,
        share: Some(share.clone()),
        ..new_machine(&setup, "win")
    };
    assert_eq!(ask(service, Request::Create { machine }).last(), Some(&Answer::Done { reason: None }));
    let prepared = service.work.prepared.lock().unwrap()[0].clone();
    assert_eq!((prepared.user.as_str(), prepared.edition.as_str()), ("pat", "Windows 11 Home"));
    let shown = &status(service)[0];
    assert_eq!(
        (shown.tpm, shown.network, &shown.share, shown.size_gb),
        (false, false, &Some(share), Some(128))
    );

    ask(service, Request::Start { name: "win".into() });
    wait_for(service, "win", State::Running);
    let booted = service.work.booted.lock().unwrap()[0].clone();
    assert_eq!((booted.tpm, booted.network), (false, false));
    assert_eq!(booted.share, Some(host_settings::Share { folder, read_only: true }));
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Stopped);
}

#[test]
fn with_no_user_named_the_guest_has_the_default_one() {
    let setup = setup("default-user", Pretend::default());
    ask(&setup.service, Request::Create { machine: new_machine(&setup, "win") });
    let prepared = setup.service.work.prepared.lock().unwrap()[0].clone();
    assert_eq!((prepared.user.as_str(), prepared.edition.as_str()), (DEFAULT_USER, ""));
}

#[test]
fn a_user_edition_or_share_the_rules_refuse_makes_no_machine() {
    let setup = setup("create-refused", Pretend::default());
    let refused = |machine: NewMachine| {
        matches!(ask(&setup.service, Request::Create { machine }).as_slice(), [Answer::Refused { .. }])
    };
    assert!(refused(NewMachine { user: "Administrator".into(), ..new_machine(&setup, "x") }));
    assert!(refused(NewMachine { edition: r"..\x".into(), ..new_machine(&setup, "x") }));
    let missing = Share { folder: setup.root.join("missing"), read_only: false };
    assert!(refused(NewMachine { share: Some(missing), ..new_machine(&setup, "x") }));
    let remote = Share { folder: PathBuf::from(r"\\server\share"), read_only: false };
    assert!(refused(NewMachine { share: Some(remote), ..new_machine(&setup, "x") }));
    assert!(status(&setup.service).is_empty());
}

#[test]
fn a_share_the_user_may_not_open_is_refused_on_a_change_but_an_unchanged_one_isnt_checked() {
    let setup = setup("change-share", Pretend::default());
    let service = &setup.service;
    let folder = setup.root.join("shared");
    fs::create_dir_all(&folder).unwrap();
    let share = Some(Share { folder, read_only: false });
    let machine = NewMachine { share: share.clone(), ..new_machine(&setup, "win") };
    ask(service, Request::Create { machine });
    let asked_by_another = |hardware: MachineSettings| {
        let (answers, received) = channel();
        let request = Request::Change { name: "win".into(), settings: hardware };
        service.answer(request, &mut Window { answers, readable: false, gone: false });
        received.try_iter().collect::<Vec<_>>()
    };
    let other = Some(Share { folder: setup.root.clone(), read_only: false });
    assert!(matches!(
        asked_by_another(MachineSettings { share: other, ..hardware(8192, Card::Default) }).as_slice(),
        [Answer::Refused { .. }]
    ));
    let kept = MachineSettings { share: share.clone(), network: false, ..hardware(16384, Card::Default) };
    assert_eq!(asked_by_another(kept), [Answer::Ok]);
    assert_eq!((status(service)[0].memory_mb, status(service)[0].network), (16384, false));
    assert_eq!(asked_by_another(hardware(16384, Card::Default)), [Answer::Ok], "a share can always go");
    assert_eq!(status(service)[0].share, None);
}

#[test]
fn a_stopped_machines_disk_only_grows() {
    let setup = setup("grow", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let grow = |size_gb| ask(service, Request::Grow { name: "win".into(), size_gb });
    assert!(matches!(grow(64).as_slice(), [Answer::Refused { .. }]), "smaller than its 128 GB");
    assert!(matches!(grow(u64::MAX).as_slice(), [Answer::Refused { .. }]));
    assert_eq!(grow(128), [Answer::Ok]);
    assert!(service.work.grown.lock().unwrap().is_empty(), "the same size asks nothing of the disk");
    assert_eq!(grow(256), [Answer::Ok]);
    assert_eq!(*service.work.grown.lock().unwrap(), [256]);
    assert_eq!(status(service)[0].size_gb, Some(256));
    assert_eq!(service.machines().store.load()[0].1.size_gb, Some(256));

    ask(service, Request::Start { name: "win".into() });
    wait_for(service, "win", State::Running);
    assert!(matches!(grow(512).as_slice(), [Answer::Refused { .. }]), "not while it runs");
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Stopped);
}

#[test]
fn a_machine_from_before_its_size_was_kept_has_it_read_from_its_disk() {
    let setup = setup("old-size", Pretend::default());
    let store = setup.service.machines().store.clone();
    store.make("old").unwrap();
    store.save("old", &Settings::basic(8192, 1, Card::Default)).unwrap();
    let again = Service::new(store, Pretend::default());
    assert_eq!(again.machines().lock()["old"].settings.size_gb, Some(200));
    assert_eq!(again.machines().store.load()[0].1.size_gb, Some(200));
}

#[test]
fn a_share_is_checked_again_as_whoever_starts_the_machine() {
    let setup = setup("start-share", Pretend::default());
    let service = &setup.service;
    let folder = setup.root.join("shared");
    fs::create_dir_all(&folder).unwrap();
    let machine =
        NewMachine { share: Some(Share { folder, read_only: false }), ..new_machine(&setup, "win") };
    ask(service, Request::Create { machine });
    let (answers, received) = channel();
    service
        .answer(Request::Start { name: "win".into() }, &mut Window { answers, readable: false, gone: false });
    assert_eq!(received.try_iter().collect::<Vec<_>>(), [Answer::Ok], "it still starts");
    wait_for(service, "win", State::Running);
    assert_eq!(service.work.booted.lock().unwrap()[0].share, None, "but shares nothing");
    let (history, _) = service.machines().watch("win").unwrap();
    assert!(
        history
            .iter()
            .any(|event| matches!(event, Event::Problem(words) if words.contains("shares no folder")))
    );
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Stopped);
}

#[test]
fn turning_the_network_off_forgets_the_guests_address() {
    let setup = setup("network-off", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    ask(service, Request::Start { name: "win".into() });
    wait_for(service, "win", State::Running);
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Stopped);
    assert!(status(service)[0].address.is_some());
    let off = MachineSettings { network: false, ..hardware(8192, Card::Default) };
    assert_eq!(ask(service, Request::Change { name: "win".into(), settings: off }), [Answer::Ok]);
    assert_eq!(status(service)[0].address, None);
}

/// A made machine, set up as its first boot would, with nothing going on.
fn set_up(setup: &Setup, name: &str) {
    ask(&setup.service, Request::Create { machine: new_machine(setup, name) });
    let disk = setup.service.machines().store.disk(name);
    Record { installed: true, ..Record::default() }.save(&disk).unwrap();
}

/// Waits for the service to finish what it's doing to a machine in the background.
fn wait_idle(service: &Arc<Service<Pretend>>, name: &str) {
    for _ in 0..400 {
        if service.machines().lock()[name].idle() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("{name} stayed busy");
}

fn shown_snapshots(service: &Arc<Service<Pretend>>) -> Vec<SnapshotInfo> {
    status(service)[0].snapshots.clone()
}

#[test]
fn a_set_up_machine_has_snapshots_taken_gone_back_to_and_deleted() {
    let setup = setup("snapshots", Pretend::default());
    let service = &setup.service;
    set_up(&setup, "win");
    let snapshot = |label: &str| ask(service, Request::Snapshot { name: "win".into(), label: label.into() });

    assert_eq!(snapshot(" clean "), [Answer::Ok]);
    wait_idle(service, "win");
    let shown = shown_snapshots(service);
    assert_eq!((shown.len(), shown[0].label.as_str(), shown[0].current), (1, "clean", true));
    assert!(
        matches!(
            ask(service, Request::Grow { name: "win".into(), size_gb: 512 }).as_slice(),
            [Answer::Refused { .. }]
        ),
        "a disk with snapshots doesn't grow"
    );

    snapshot("later");
    wait_idle(service, "win");
    let first = shown_snapshots(service)[0].id;
    assert_eq!(ask(service, Request::Revert { name: "win".into(), id: first }), [Answer::Ok]);
    wait_idle(service, "win");
    let shown = shown_snapshots(service);
    assert_eq!(shown.iter().map(|s| s.current).collect::<Vec<_>>(), [true, false]);

    for snapshot in shown {
        assert_eq!(
            ask(service, Request::DeleteSnapshot { name: "win".into(), id: snapshot.id }),
            [Answer::Ok]
        );
        wait_idle(service, "win");
    }
    assert!(shown_snapshots(service).is_empty());
    assert_eq!(
        ask(service, Request::Grow { name: "win".into(), size_gb: 512 }),
        [Answer::Ok],
        "it grows again"
    );
    let (history, _) = service.machines().watch("win").unwrap();
    assert!(history.contains(&Event::Note("Took the snapshot clean.".into())));
}

#[test]
fn snapshots_are_kept_to_the_services_rules() {
    let setup = setup("snapshot-rules", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "new") });
    let refused = |request| matches!(ask(service, request).as_slice(), [Answer::Refused { .. }]);
    assert!(refused(Request::Snapshot { name: "new".into(), label: "x".into() }), "not before it's set up");

    set_up(&setup, "win");
    assert!(refused(Request::Snapshot { name: "win".into(), label: "  ".into() }));
    assert!(refused(Request::Snapshot { name: "win".into(), label: "a\nb".into() }));
    assert!(refused(Request::Snapshot { name: "win".into(), label: "x".repeat(61) }));
    assert!(refused(Request::Revert { name: "win".into(), id: 7 }), "only an ID it has");
    assert!(refused(Request::DeleteSnapshot { name: "win".into(), id: 0 }));

    ask(service, Request::Start { name: "win".into() });
    wait_for(service, "win", State::Running);
    assert!(refused(Request::Snapshot { name: "win".into(), label: "x".into() }), "not while it runs");
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Stopped);
}

#[test]
fn a_busy_machine_says_so_and_does_nothing_else_meanwhile() {
    let setup = setup("busy", Pretend::default());
    let service = &setup.service;
    set_up(&setup, "win");
    service.machines().lock().get_mut("win").unwrap().busy = Some("Deleting the snapshot x".into());
    assert_eq!(status(service)[0].working.as_deref(), Some("Deleting the snapshot x"));
    for request in [
        Request::Start { name: "win".into() },
        Request::Delete { name: "win".into() },
        Request::Snapshot { name: "win".into(), label: "y".into() },
    ] {
        match ask(service, request).as_slice() {
            [Answer::Refused { reason }] => assert!(reason.contains("deleting the snapshot x"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
}

/// A report's answers: each file with its text, each file that was missing and why, and every answer.
type Report = (Vec<(String, String)>, Vec<(String, String)>, Vec<Answer>);

fn report(setup: &Setup, machine: Option<&str>) -> Report {
    let answers = ask(&setup.service, Request::ReportFiles { machine: machine.map(str::to_owned) });
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

fn made_machine(setup: &Setup, name: &str) -> PathBuf {
    ask(&setup.service, Request::Create { machine: new_machine(setup, name) });
    setup.service.machines().store.folder(name)
}

#[test]
fn a_report_without_a_machine_is_the_hosts_files_then_the_machines_then_done() {
    let setup = setup("report-host", Pretend::default());
    made_machine(&setup, "win");
    let (files, missing, answers) = report(&setup, None);
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["service.log", "windows.txt", "machines.txt"], "the work's parts, then the machines'");
    assert_eq!(missing, [("crashes.txt".to_owned(), "wevtutil isn't here.".to_owned())]);
    assert_eq!(files[0].1, "started\nstopped\n", "the lines about refused requests are left out");
    assert_eq!(files[2].1, "win: windows, stopped, not set up\n");
    assert_eq!(answers.last(), Some(&Answer::Done { reason: None }));
    assert!(answers.iter().all(|answer| !matches!(answer, Answer::Refused { .. })));
}

#[test]
fn a_report_for_a_machine_adds_only_its_allowed_files() {
    let setup = setup("report-machine", Pretend::default());
    let folder = made_machine(&setup, "win");
    fs::write(folder.join("win.account.txt"), "user: guesthouse\npassword: hunter2\n").unwrap();
    fs::write(folder.join("win.screen"), "PRIVATE KEY").unwrap();
    fs::write(folder.join("installer.iso"), "iso").unwrap();
    fs::write(folder.join("events.log"), "2026-01-01 00:00:00 UTC note one\n").unwrap();
    fs::create_dir(folder.join("snapshots")).unwrap();
    fs::write(folder.join("snapshots").join("index.json"), "{}").unwrap();
    fs::write(folder.join("snapshots").join("1.vhdx"), "disk").unwrap();
    let (files, missing, _) = report(&setup, Some("WIN"));
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["service.log", "windows.txt", "machines.txt", "machine.json", "snapshots/index.json", "events.log"]
    );
    assert!(files.iter().any(|(name, text)| name == "events.log" && text.contains("UTC note one")));
    let missing: Vec<&str> = missing.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(missing, ["crashes.txt", "win.guesthouse.json", "events.1.log"], "each is optional");
    let everything = format!("{files:?}");
    for secret in ["hunter2", "PRIVATE KEY", "account", "screen", "installer", "1.vhdx"] {
        assert!(!everything.contains(secret), "{secret} was handed over");
    }
}

#[test]
fn a_machines_record_and_older_events_come_when_they_are_there() {
    let iso = std::env::temp_dir().join(format!("gh-requests-report-iso-{}.iso", std::process::id()));
    fs::write(&iso, b"ubuntu").unwrap();
    let work = Pretend { linux_installer: Some(iso.clone()), ..Pretend::default() };
    let setup = setup("report-record", work);
    let folder = made_machine(&setup, "ubuntu");
    fs::write(folder.join("events.1.log"), "older\n").unwrap();
    let (files, missing, _) = report(&setup, Some("ubuntu"));
    assert!(files.iter().any(|(name, text)| name == "ubuntu.guesthouse.json" && text.contains("linux")));
    assert!(files.iter().any(|(name, text)| name == "events.1.log" && text == "older\n"));
    assert!(missing.iter().all(|(name, _)| name != "events.1.log"));
    let _ = fs::remove_file(&iso);
}

#[test]
fn a_large_file_comes_in_pieces_a_line_can_hold_and_only_its_end() {
    let setup = setup("report-large", Pretend::default());
    let folder = made_machine(&setup, "win");
    let line = "2026-01-01 00:00:00 UTC note something happened to the machine\n";
    let big = line.repeat(60_000);
    assert!(big.len() > 3 * 1024 * 1024);
    fs::write(folder.join("events.log"), &big).unwrap();
    let (files, _, answers) = report(&setup, Some("win"));
    let pieces = answers
        .iter()
        .filter(|answer| matches!(answer, Answer::ReportPart { name, .. } if name == "events.log"))
        .count();
    assert!(pieces > 60, "{pieces} pieces");
    for answer in &answers {
        let mut buffer = Vec::new();
        guesthouse_control::write_line(&mut buffer, answer).unwrap();
        assert!(buffer.len() <= 64 * 1024, "a line of {} bytes", buffer.len());
    }
    let (_, text) = files.iter().find(|(name, _)| name == "events.log").unwrap();
    assert!(text.starts_with('[') && text.contains("bytes left out at the start]\n2026-01-01"));
    assert!(text.ends_with(line));
    assert!(text.len() <= 2 * 1024 * 1024 + 64);
}

#[test]
fn a_machine_that_isnt_there_is_refused_before_anything_is_sent() {
    let setup = setup("report-none", Pretend::default());
    let answers = ask(&setup.service, Request::ReportFiles { machine: Some("nope".into()) });
    assert_eq!(answers, [Answer::Refused { reason: "There is no machine called nope.".into() }]);
}

#[test]
fn a_report_stops_when_the_window_has_gone() {
    let setup = setup("report-gone", Pretend::default());
    let (answers, received) = channel();
    drop(received);
    let mut window = Window { answers, readable: true, gone: true };
    setup.service.answer(Request::ReportFiles { machine: None }, &mut window);
}

#[cfg(unix)]
#[test]
fn a_file_that_is_a_link_is_not_followed() {
    let setup = setup("report-link", Pretend::default());
    let folder = made_machine(&setup, "win");
    let secret = setup.root.join("secret.txt");
    fs::write(&secret, "outside").unwrap();
    let _ = fs::remove_file(folder.join("events.log"));
    std::os::unix::fs::symlink(&secret, folder.join("events.log")).unwrap();
    let (files, missing, _) = report(&setup, Some("win"));
    assert!(!format!("{files:?}").contains("outside"));
    assert!(missing.iter().any(|(name, reason)| name == "events.log" && reason.contains("plain file")));
}
