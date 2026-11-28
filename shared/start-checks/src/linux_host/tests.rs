use super::*;

fn gpu(id: &str) -> MachineFacts {
    MachineFacts { has_gpu: true, cores: 8, ram_gib: 16, own_card: id.to_owned(), ..MachineFacts::default() }
}

fn facts() -> SharedFacts {
    SharedFacts {
        ram_limit: 36,
        touched_gib: 0,
        max_vram_gib: 28,
        vcache_cpus: 16,
        setup_card: String::new(),
    }
}

#[test]
fn what_several_machines_share_is_said_in_this_order() {
    let others = [MachineFacts { vram_gib: 10, vcache_pin: true, ..gpu("") }];
    let second = MachineFacts { vram_gib: 20, vcache_pin: true, cores: 9, ram_gib: 31, ..gpu("") };
    let small = SharedFacts { ram_limit: 30, ..facts() };
    assert_eq!(
        shared_problems(&second, &others, &small),
        [
            "This PC's RAM cannot hold this machine beside the ones running.",
            "Only 18 GiB of video memory is left beside the machines running.",
            "The V-Cache cores are taken by the machines running."
        ]
    );
    let uncapped = [MachineFacts { vcache_pin: true, ..gpu("") }];
    assert!(shared_problems(&second, &uncapped, &small)[1].starts_with("The running machine has all"));
    assert!(shared_problems(&second, &[], &SharedFacts::default()).is_empty(), "alone nothing is shared");
}

#[test]
fn the_card_a_machine_runs_on_is_its_own_or_setups() {
    assert_eq!(card_of("", "0000:01:00.0"), "0000:01:00.0");
    assert_eq!(card_of("0000:02:00.0", "0000:01:00.0"), "0000:02:00.0");
    let running = [gpu(""), gpu("0000:02:00.0"), gpu("0000:01:00.0"), MachineFacts::default()];
    assert_eq!(card_mates(&gpu(""), &running, "0000:01:00.0"), [running[0].clone(), running[2].clone()]);
    assert_eq!(video_memory_left(&[MachineFacts { vram_gib: 10, ..gpu("") }], 28), 18);
    assert_eq!(video_cap(&gpu(""), &[MachineFacts { vram_gib: 10, ..gpu("") }], 28), 18);
    assert_eq!(video_cap(&gpu(""), &[], 28), 0);
}

#[test]
fn a_start_is_judged_in_this_order() {
    let machine = MachineFacts { vcache_pin: true, cores: 9, ram_gib: 100, ..gpu("") };
    let facts = StartFacts {
        machine: machine.clone(),
        settings_problems: vec!["settings".into()],
        missing_files: vec!["/disk".into(), "/vars".into()],
        missing_disc: Some("/disc".into()),
        missing_share: Some("/share".into()),
        share_problem: Some(SHARE_REFUSAL.into()),
        card_problems: vec!["card".into()],
        no_token: true,
        second_instance: true,
        pc_cores: 8,
        ram_limit: 79,
        vcache_threads: 8,
        disk_in_use: true,
        drive_problems: vec!["drive".into()],
        slot_taken: Some("slot".into()),
        memory_taken: true,
        others: vec![MachineFacts { vcache_pin: true, ..MachineFacts::default() }],
        shared: SharedFacts { ram_limit: 200, vcache_cpus: 8, ..SharedFacts::default() },
        network_problems: vec!["network".into()],
        ..StartFacts::default()
    };
    assert_eq!(
        judge(&facts),
        [
            "settings",
            "/disk is missing.",
            "/vars is missing.",
            "/disc is missing. Eject it in the settings to boot from the disk.",
            "/share is missing. Choose another shared folder in the settings.",
            SHARE_REFUSAL,
            "card",
            NO_TOKEN,
            SECOND_INSTANCE_REFUSAL,
            "This PC has 8 cores, so a machine can have that many at most.",
            "This PC can give a machine 79 GiB at most right now.",
            "The V-Cache cores run 8 threads, too few for 9 cores.",
            ALREADY_RUNNING,
            "drive",
            "slot",
            "The V-Cache cores are taken by the machines running.",
            "network",
        ]
    );
    let no_part = StartFacts { no_gpu_part: Some("no part".into()), slot_taken: None, ..facts.clone() };
    let said = judge(&no_part);
    assert!(said.contains(&"no part".to_owned()) && !said.contains(&NO_TOKEN.to_owned()));
    assert!(said.contains(&MEMORY_TAKEN.to_owned()));
    let plain =
        StartFacts { machine: MachineFacts { has_gpu: false, vcache_pin: false, ..machine }, ..facts };
    let said = judge(&plain);
    assert!(
        !said.iter().any(|problem| ["card", NO_TOKEN, SECOND_INSTANCE_REFUSAL].contains(&problem.as_str()))
    );
    assert!(!said.contains(&"This CPU has no V-Cache cores to pin to.".to_owned()));
}

#[test]
fn nothing_wrong_is_nothing_said() {
    let facts = StartFacts { machine: gpu(""), pc_cores: 8, ram_limit: 16, ..StartFacts::default() };
    assert_eq!(judge(&facts), [""; 0]);
}
