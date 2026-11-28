use super::*;
use crate::card_share::look::{CardShare, GpuTime, MachineShare, OnCard};
use crate::card_share::whole_card::WholeCard;
use crate::clock::stand_in::Ticking;
use crate::gpu_maker::GpuMaker;
use guesthouse_sensors::guest_use::GuestUse;
use std::collections::BTreeMap;
use std::sync::Arc;

#[test]
fn with_nothing_running_no_card_is_shared() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Windows);
    assert_eq!(
        said(&context, "share"),
        ("No machine with a GPU is running, so no card is being shared.\n".to_owned(), Ok(()))
    );
    assert!(usage_of("share work").contains("share takes no words, only options."));
}

#[test]
fn a_running_machine_with_nothing_read_says_so_under_its_card() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.clock = Arc::new(Ticking::default());
    let mut machine = make(&context, "work", "Office PC", Kind::Windows);
    machine.use_windows_card(GpuMaker::Amd, "0000:14:00.0");
    machine.save(&context).unwrap();
    running(&context, &machine, 40);
    let (out, result) = said(&context, "share");
    assert_eq!(result, Ok(()));
    assert_eq!(
        out,
        "AMD card\n  Whole card: Load not available, video memory not available\n  Office PC: GPU time not \
         available. Video memory not available\n"
    );
}

#[test]
fn an_nvidia_card_reads_as_the_panel_does() {
    let books = GuestUse { daemon: 1, video_bytes: 778_764_288, spilled_bytes: 0, cap_bytes: Some(4 << 30) };
    let engines = BTreeMap::from([("sm".to_owned(), 12), ("enc".to_owned(), 0), ("dec".to_owned(), 0)]);
    let card = CardShare {
        card: OnCard { maker: GpuMaker::Nvidia, pci: Some("0000:01:00.0".to_owned()) },
        name: "NVIDIA GeForce RTX 5090".to_owned(),
        whole: WholeCard { load_percent: Some(14), ..WholeCard::default() },
        per_process: true,
        machines: vec![MachineShare {
            name: "Games".to_owned(),
            time: GpuTime::Engines(engines),
            video: Some(books),
            card_bytes: Some(1848 << 20),
        }],
    };
    let lines = share::card_text(&[card]);
    assert_eq!(lines[0], "NVIDIA GeForce RTX 5090");
    assert!(lines[1].starts_with("Each machine's GPU time is the driver's last one-second sample"));
    assert_eq!(lines[2], "  Whole card: Load 14%, video memory not available");
    assert_eq!(
        lines[3],
        "  Games: Graphics and compute 12%, decode 0%, encode 0%. Video memory 0.7 GiB of its 4.0 GiB cap, \
         1.8 GiB by the driver's count"
    );
}
