//! The encoder a machine's card is given and the ports other machines hold, as they are today, held
//! before they move.

use super::machine::{encoder, port, set_allowed};
use crate::context::Context;
use crate::gpu_maker::GpuMaker;
use crate::machine::{Kind, Machine};
use crate::testing::{Scratch, scratch_context};
use guesthouse_remote_view_core::switch::{SWITCH_FILE, choose_port};
use std::fs;

#[test]
fn each_cards_maker_has_its_encoder() {
    let windows = Machine { kind: Kind::Windows, ..Machine::new("w", "w", "/d", "/v") };
    assert_eq!(encoder(&windows), "auto", "no card named is setup's NVIDIA card");
    let on = |maker, pci: &str| {
        let mut machine = windows.clone();
        machine.use_windows_card(maker, pci);
        encoder(&machine)
    };
    assert_eq!(on(GpuMaker::Nvidia, "0000:02:00.0"), "auto");
    assert_eq!(on(GpuMaker::Amd, "0000:7c:00.0"), "d3d12");
    assert_eq!(on(GpuMaker::Intel, "0000:00:02.0"), "d3d12");
    let amd_by_address = Machine { amd_gpu: "0000:7c:00.0".into(), ..windows.clone() };
    assert_eq!(encoder(&amd_by_address), "d3d12");
    for kind in [Kind::Linux, Kind::Plain] {
        assert_eq!(encoder(&Machine { kind, ..windows.clone() }), "d3d12", "{kind:?} has no maker");
    }
}

fn machine(context: &Context, id: &str, kind: Kind) -> Machine {
    let machine = Machine { kind, ..Machine::new(id, id, "/d", "/v") };
    fs::create_dir_all(machine.folder(context)).unwrap();
    machine.save(context).unwrap();
    machine
}

#[test]
fn every_other_machines_port_is_taken_and_its_own_is_not() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    // A machine that isn't Windows can't be turned on, but a port in its file still counts.
    let linux = machine(&context, "lin", Kind::Linux);
    assert_eq!(
        set_allowed(&context, &linux, true).unwrap_err().0,
        "Only a Windows machine can be viewed from other devices."
    );
    // The port is a real one, which another test may hold for a moment, so only how they differ is held.
    let first_free = choose_port(&[], guesthouse_relay::udp_port_free).unwrap();
    fs::write(linux.folder(&context).join(SWITCH_FILE), format!("{{\"remote_port\": {first_free}}}"))
        .unwrap();
    assert_eq!(port(&context, &linux), Some(first_free));
    let one = machine(&context, "one", Kind::Windows);
    let two = machine(&context, "two", Kind::Windows);
    let one_port = set_allowed(&context, &one, true).unwrap().remote_port.unwrap();
    assert_ne!(one_port, first_free, "a machine that isn't Windows holds its port too");
    let two_port = set_allowed(&context, &two, true).unwrap().remote_port.unwrap();
    assert!(![first_free, one_port].contains(&two_port), "{two_port}");
    // Off and on again, a machine keeps its own port, which isn't taken from it.
    set_allowed(&context, &one, false).unwrap();
    assert_eq!(set_allowed(&context, &one, true).unwrap().remote_port, Some(one_port));
    // With its file gone, it is given a port no other machine has.
    fs::remove_file(one.folder(&context).join(SWITCH_FILE)).unwrap();
    let again = set_allowed(&context, &one, true).unwrap().remote_port.unwrap();
    assert!(![first_free, two_port].contains(&again), "{again}");
}
