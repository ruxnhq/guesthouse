//! Turning Windows' Hyper-V feature on, against the double's DISM, which is all that's ever run here.

use super::network::{FakeNet, NetState, net, start_and_run, stop};
use super::*;
use guesthouse_host_windows::hyper_v::Enabled;
use guesthouse_host_windows::network::bridging_from;

fn off() -> FakeNet {
    FakeNet::with(|net: &mut NetState| net.bridging = bridging_from(HyperV::Off, Ok(Vec::new())))
}

fn hyper_v(service: &Arc<Service<Pretend>>) -> HyperV {
    match ask(service, Request::Status).pop() {
        Some(Answer::Status { bridging, .. }) => bridging.hyper_v,
        other => panic!("{other:?}"),
    }
}

fn enables(service: &Arc<Service<Pretend>>) -> usize {
    net(service).calls.iter().filter(|call| *call == "enable_hyper_v").count()
}

#[test]
fn hyper_v_is_turned_on_with_its_progress_and_then_waits_for_a_restart() {
    let setup = setup("hyper-v-on", Pretend { net: off(), ..Pretend::default() });
    let service = &setup.service;
    assert_eq!(hyper_v(service), HyperV::Off);
    let answers = ask(service, Request::EnableHyperV);
    assert_eq!(
        answers,
        [
            Answer::Event { event: Event::Note("Turning on Hyper-V: 30%".into()) },
            Answer::Event { event: Event::Note("Turning on Hyper-V: 100%".into()) },
            Answer::Done { reason: None },
        ]
    );
    // Read again at once, not after what was kept had run out.
    assert_eq!(hyper_v(service), HyperV::RestartPending);
    // Asked again, nothing more is run: it waits for the restart.
    let again = ask(service, Request::EnableHyperV);
    assert!(matches!(again.last(), Some(Answer::Done { reason: None })), "{again:?}");
    assert_eq!(enables(service), 1);
    assert!(service.work.logged.lock().unwrap().iter().any(|line| line.contains("Hyper-V is turned on")));
}

#[test]
fn hyper_v_on_already_runs_nothing() {
    let fake = FakeNet::with(|net| net.bridging = bridging_from(HyperV::On, Ok(Vec::new())));
    let setup = setup("hyper-v-already", Pretend { net: fake, ..Pretend::default() });
    let answers = ask(&setup.service, Request::EnableHyperV);
    assert_eq!(
        answers,
        [
            Answer::Event { event: Event::Note("Hyper-V is on already.".into()) },
            Answer::Done { reason: None }
        ]
    );
    assert_eq!(enables(&setup.service), 0);
}

#[test]
fn an_edition_without_it_or_a_pc_without_virtualization_is_refused_and_nothing_runs() {
    for (hyper_v, words) in [(HyperV::NotInEdition, "Windows Home"), (HyperV::NoVirtualization, "firmware")] {
        let fake = FakeNet::with(|net| net.bridging = bridging_from(hyper_v.clone(), Ok(Vec::new())));
        let setup = setup("hyper-v-refused", Pretend { net: fake, ..Pretend::default() });
        let answers = ask(&setup.service, Request::EnableHyperV);
        assert!(
            matches!(answers.as_slice(), [Answer::Refused { reason }] if reason.contains(words)),
            "{answers:?}"
        );
        assert_eq!(enables(&setup.service), 0);
    }
}

#[test]
fn a_failure_says_why_with_the_log_and_status_says_so_until_it_works() {
    let fake = off();
    fake.state.lock().unwrap().enable =
        Some(Enabled::Failed { reason: "No room.".into(), log: r"C:\Windows\Logs\DISM\dism.log".into() });
    let setup = setup("hyper-v-failed", Pretend { net: fake, ..Pretend::default() });
    let service = &setup.service;
    let answers = ask(service, Request::EnableHyperV);
    assert_eq!(
        answers.last(),
        Some(&Answer::Done { reason: Some(r"No room. DISM's log is C:\Windows\Logs\DISM\dism.log.".into()) })
    );
    assert_eq!(
        hyper_v(service),
        HyperV::Failed { reason: "No room.".into(), log: Some(r"C:\Windows\Logs\DISM\dism.log".into()) }
    );
    // It can be tried again, which clears the failure once it works.
    net(service).enable = None;
    assert_eq!(ask(service, Request::EnableHyperV).last(), Some(&Answer::Done { reason: None }));
    assert_eq!(hyper_v(service), HyperV::RestartPending);
    assert_eq!(enables(service), 2);
}

#[test]
fn it_waits_for_every_machine_to_stop_and_none_starts_or_is_made_meanwhile() {
    let setup = setup("hyper-v-machines", Pretend { net: off(), ..Pretend::default() });
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    ask(service, Request::Create { machine: new_machine(&setup, "ubuntu") });
    start_and_run(service, "win");
    let refused = ask(service, Request::EnableHyperV);
    assert!(
        matches!(refused.as_slice(), [Answer::Refused { reason }] if reason.starts_with("win is running") && reason.contains("Stop it first")),
        "{refused:?}"
    );
    start_and_run(service, "ubuntu");
    let refused = ask(service, Request::EnableHyperV);
    assert!(
        matches!(refused.as_slice(), [Answer::Refused { reason }] if reason.contains("ubuntu and win are") && reason.contains("Stop them")),
        "{refused:?}"
    );
    assert_eq!(enables(service), 0);
    stop(service, "win");
    stop(service, "ubuntu");

    // While it's turned on, the status says how far, and nothing starts or is made.
    net(service).enable_held = true;
    let turning = Arc::clone(service);
    let asked = thread::spawn(move || ask(&turning, Request::EnableHyperV));
    for _ in 0..400 {
        if hyper_v(service) == (HyperV::TurningOn { percent: Some(30) }) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(hyper_v(service), HyperV::TurningOn { percent: Some(30) });
    let start = ask(service, Request::Start { name: "win".into() });
    assert!(
        matches!(start.as_slice(), [Answer::Refused { reason }] if reason.contains("being turned on")),
        "{start:?}"
    );
    let made = ask(service, Request::Create { machine: new_machine(&setup, "third") });
    assert!(
        matches!(made.as_slice(), [Answer::Refused { reason }] if reason.contains("being turned on")),
        "{made:?}"
    );
    let twice = ask(service, Request::EnableHyperV);
    assert!(
        matches!(twice.as_slice(), [Answer::Refused { reason }] if reason.contains("already")),
        "{twice:?}"
    );
    net(service).enable_held = false;
    assert_eq!(asked.join().unwrap().last(), Some(&Answer::Done { reason: None }));
    assert_eq!(enables(service), 1);
    // And once it's done, they do.
    start_and_run(service, "win");
    stop(service, "win");
}

#[test]
fn a_machine_being_made_holds_it_up() {
    let setup = setup("hyper-v-making", Pretend { net: off(), ..Pretend::default() });
    *setup.service.making.lock().unwrap() = Some("studio".into());
    let refused = ask(&setup.service, Request::EnableHyperV);
    assert!(
        matches!(refused.as_slice(), [Answer::Refused { reason }] if reason.starts_with("studio is being made")),
        "{refused:?}"
    );
}
