use super::*;

#[test]
fn nested_virtualization_is_off_until_asked_for_and_reaches_the_next_boot() {
    let setup = setup("nested", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    assert!(!status(service)[0].nested_virtualization);
    let on = MachineSettings { nested_virtualization: true, ..hardware(8192, Card::Default) };
    assert_eq!(change(service, on), [Answer::Ok]);
    assert!(status(service)[0].nested_virtualization);
    assert!(service.machines().store.load()[0].1.nested_virtualization, "kept on disk");
    assert_eq!(ask(service, Request::Start { name: "win".into() }), [Answer::Ok]);
    wait_for(service, "win", State::Running);
    assert!(service.work.booted.lock().unwrap()[0].nested_virtualization);
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Off);
}
