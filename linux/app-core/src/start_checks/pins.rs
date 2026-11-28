//! The start checks as they are today, held before they move: every sentence, word for word, and the
//! order a start's problems are said in, as the first of them is what a refused start says.

use super::*;
use crate::host::stand_in::fake_cpus;
use crate::network::{Forward, Protocol};
use crate::processes::stand_in::fake_process;
use crate::slots;
use crate::testing::{Scratch, scratch_context};
use std::fs;

const RAM: &str = "This PC's RAM cannot hold this machine beside the ones running.";
const ALL_VIDEO: &str = "The running machine has all the video memory. Give it a cap in its settings first.";
const VCACHE_TAKEN: &str = "The V-Cache cores are taken by the machines running.";
const NO_VCACHE: &str = "This CPU has no V-Cache cores to pin to.";

fn machine(id: &str) -> Machine {
    Machine::new(id, id, &format!("/{id}"), "/v")
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
fn the_sentences_kept_today() {
    assert_eq!(
        SHARE_REFUSAL,
        "This folder can't be shared. Choose a folder inside your home that isn't hidden, and isn't one \
         Guesthouse keeps its own files in."
    );
    assert_eq!(
        MEMORY_TAKEN,
        "Another VM on this PC has just taken this slot's memory. Start the machine again."
    );
    assert_eq!(SECOND_INSTANCE_REFUSAL, "A second instance leaves GPU-PV alone.");
    assert_eq!(
        NO_SETUP,
        "A GPU machine needs setup first, and it hasn't been run. Run Setup Again from the menu."
    );
    assert_eq!(
        NO_TOKEN,
        "This machine has no guest token, so the app can't reach Windows to finish starting it. Choose the token \
         file it was installed with in its settings, under Guest Token."
    );
}

#[test]
fn what_several_machines_share_is_said_in_this_order() {
    let others = [Machine { vram_gib: 10, vcache_pin: true, cores: 8, ..machine("first") }];
    let second = Machine { vram_gib: 20, vcache_pin: true, cores: 9, ram_gib: 31, ..machine("second") };
    let small = SharedFacts { ram_limit: 30, ..facts() };
    assert_eq!(
        shared_problems(&second, &others, &small),
        [RAM, "Only 18 GiB of video memory is left beside the machines running.", VCACHE_TAKEN]
    );
    let uncapped = [Machine { vcache_pin: true, cores: 8, ..machine("first") }];
    assert_eq!(shared_problems(&second, &uncapped, &small), [RAM, ALL_VIDEO, VCACHE_TAKEN]);
    assert_eq!(
        shared_problems(&second, &[], &SharedFacts { ram_limit: 0, vcache_cpus: 0, ..facts() }),
        [""; 0]
    );
}

#[test]
fn each_shared_limit_holds_at_its_edge() {
    let others = [Machine { vram_gib: 10, vcache_pin: true, cores: 8, ram_gib: 6, ..machine("first") }];
    // 4 GiB the first hasn't touched yet, beside this one's 26, is the limit exactly.
    let at = Machine { ram_gib: 26, vram_gib: 18, vcache_pin: true, cores: 8, ..machine("second") };
    let facts = SharedFacts { ram_limit: 30, touched_gib: 2, vcache_cpus: 16, ..facts() };
    assert_eq!(shared_problems(&at, &others, &facts), [""; 0]);
    let over = Machine { ram_gib: 27, vram_gib: 19, cores: 9, ..at.clone() };
    assert_eq!(
        shared_problems(&over, &others, &facts),
        [RAM, "Only 18 GiB of video memory is left beside the machines running.", VCACHE_TAKEN]
    );
    let touched_more = SharedFacts { touched_gib: 9, ..facts.clone() };
    assert_eq!(shared_problems(&Machine { ram_gib: 30, ..at.clone() }, &others, &touched_more), [""; 0]);
    // A machine with no GPU counts only its V-Cache cores.
    let plain = Machine { kind: Kind::Plain, ..over.clone() };
    assert_eq!(shared_problems(&plain, &others, &facts), [VCACHE_TAKEN]);
    let not_pinned = Machine { vcache_pin: false, ..over };
    assert_eq!(shared_problems(&not_pinned, &others, &facts).len(), 2);
}

#[test]
fn the_cards_memory_is_counted_by_card() {
    let capped = Machine { vram_gib: 10, ..machine("capped") };
    let uncapped = machine("uncapped");
    let plain = Machine { kind: Kind::Plain, vram_gib: 5, ..machine("plain") };
    let installing = Machine { installing: true, vram_gib: 7, ..machine("installing") };
    assert_eq!(video_memory_left(&[capped.clone(), plain.clone(), installing], 28), 18);
    assert_eq!(video_memory_left(&[capped.clone(), uncapped.clone()], 28), 0);
    assert_eq!(video_memory_left(&[], 28), 28);
    assert_eq!(video_cap(&machine("new"), std::slice::from_ref(&capped), 28), 18);
    assert_eq!(video_cap(&machine("new"), &[], 28), 0);
    assert_eq!(video_cap(&Machine { vram_gib: 4, ..machine("new") }, std::slice::from_ref(&capped), 28), 4);
    let mut elsewhere = machine("elsewhere");
    elsewhere.use_windows_card(crate::gpu_maker::GpuMaker::Nvidia, "0000:02:00.0");
    let mut by_address = machine("by-address");
    by_address.use_windows_card(crate::gpu_maker::GpuMaker::Nvidia, "0000:01:00.0");
    let running = [capped.clone(), plain, elsewhere.clone(), by_address.clone()];
    let ids = |found: Vec<Machine>| found.into_iter().map(|found| found.id).collect::<Vec<_>>();
    assert_eq!(ids(card_mates(&uncapped, &running, "0000:01:00.0")), ["capped", "by-address"]);
    assert_eq!(ids(card_mates(&elsewhere, &running, "0000:01:00.0")), ["elsewhere"]);
    assert_eq!(ids(card_mates(&uncapped, &running, "")), ["capped"], "setup's card unknown");
}

fn startable(scratch: &Scratch, sizes: &[u32]) -> (Context, Machine) {
    let mut context = scratch_context(scratch);
    context.cpus = fake_cpus(scratch, "cpus", sizes);
    context.meminfo = scratch.write("meminfo", "MemTotal: 100000000 kB\nMemAvailable: 100000000 kB\n");
    let disk = scratch.write("disk", "").display().to_string();
    let vars = scratch.write("vars", "").display().to_string();
    let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", &disk, &vars) };
    (context, machine)
}

#[test]
fn a_plain_machine_with_everything_wrong_says_so_in_this_order() {
    let scratch = Scratch::new();
    let (context, machine) = startable(&scratch, &[32768; 16]);
    let gone = |name: &str| scratch.path(name).display().to_string();
    let wrong = Machine {
        disk: gone("gone.qcow2"),
        vars: gone("gone.fd"),
        iso: gone("gone.iso"),
        share: gone("gone-folder"),
        cores: 9,
        ram_gib: 1000,
        vcache_pin: true,
        ..machine.clone()
    };
    assert_eq!(
        start_problems(&context, &wrong).unwrap(),
        [
            format!("{} is missing.", gone("gone.qcow2")),
            format!("{} is missing.", gone("gone.fd")),
            format!("{} is missing. Eject it in the settings to boot from the disk.", gone("gone.iso")),
            format!("{} is missing. Choose another shared folder in the settings.", gone("gone-folder")),
            "This PC has 8 cores, so a machine can have that many at most.".to_owned(),
            "This PC can give a machine 79 GiB at most right now.".to_owned(),
            NO_VCACHE.to_owned(),
        ]
    );
    // The settings' own problems come first.
    let unset = Machine { cores: 0, ram_gib: 0, name: " ".into(), ..wrong };
    assert_eq!(
        start_problems(&context, &unset).unwrap()[..3],
        [
            "The machine needs a name.",
            "Cores must be a whole number, at least 1.",
            "RAM must be a whole number of GiB, at least 1."
        ]
    );
    assert_eq!(start_problems(&context, &unset).unwrap()[3], format!("{} is missing.", gone("gone.qcow2")));
    // An empty path is never missing.
    let none = Machine { disk: String::new(), vars: String::new(), ..machine };
    assert_eq!(start_problems(&context, &none).unwrap(), [""; 0]);
}

#[test]
fn too_few_vcache_threads_are_said_with_their_numbers() {
    let scratch = Scratch::new();
    let mut sizes = vec![98304; 8];
    sizes.extend([32768; 8]);
    let (context, machine) = startable(&scratch, &sizes);
    let pinned = Machine { vcache_pin: true, cores: 9, ..machine };
    assert_eq!(
        start_problems(&context, &pinned).unwrap(),
        [
            "This PC has 8 cores, so a machine can have that many at most.",
            "The V-Cache cores run 8 threads, too few for 9 cores."
        ]
    );
}

#[test]
fn a_shared_folder_that_cant_be_shared_says_so_where_it_would_be_missing() {
    let scratch = Scratch::new();
    let (mut context, machine) = startable(&scratch, &[32768; 16]);
    context.paths.home = scratch.path("home");
    fs::create_dir_all(scratch.path("home/.hidden")).unwrap();
    let home = context.paths.home.display().to_string();
    let hidden = scratch.path("home/.hidden").display().to_string();
    for share in [home, hidden] {
        let sharing = Machine { share, cores: 9, ..machine.clone() };
        assert_eq!(
            start_problems(&context, &sharing).unwrap(),
            [SHARE_REFUSAL, "This PC has 8 cores, so a machine can have that many at most."]
        );
    }
}

#[test]
fn a_windows_machine_without_the_gpu_part_says_only_that_of_its_card() {
    let scratch = Scratch::new();
    let (mut context, machine) = startable(&scratch, &[32768; 16]);
    let _ = fs::remove_dir_all(context.paths.guest_files());
    context.paths.own_run = Some(scratch.path("second"));
    let gpu = Machine { kind: Kind::Windows, ..machine.clone() };
    assert_eq!(
        start_problems(&context, &gpu).unwrap(),
        [crate::gpu_support::NO_GPU_PART, SECOND_INSTANCE_REFUSAL]
    );
    let installing = Machine { installing: true, ..gpu };
    assert_eq!(start_problems(&context, &installing).unwrap(), [crate::gpu_support::NO_GPU_PART]);
}

#[test]
fn a_gpu_machine_says_its_card_its_token_a_second_instance_and_taken_memory_in_this_order() {
    let scratch = Scratch::new();
    let (mut context, machine) = startable(&scratch, &[32768; 16]);
    context.paths.own_run = Some(scratch.path("second"));
    let memory = context.paths.shared_file("ram", slots::first());
    fs::write(&memory, "").unwrap();
    fake_process(&context, 200, &["/other/guesthouse-daemon", "/other"], &[], &memory.display().to_string());
    let gpu = Machine { kind: Kind::Windows, ..machine.clone() };
    assert_eq!(
        start_problems(&context, &gpu).unwrap(),
        [NO_SETUP, NO_TOKEN, SECOND_INSTANCE_REFUSAL, MEMORY_TAKEN]
    );
    // A machine without a GPU uses none of them.
    assert_eq!(start_problems(&context, &machine).unwrap(), [""; 0]);
}

#[test]
fn what_runs_beside_it_is_said_last_in_this_order() {
    let scratch = Scratch::new();
    let (context, machine) = startable(&scratch, &[32768; 16]);
    let qemu = context.paths.qemu.display().to_string();
    let run = |slot| format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slot).display());
    let drive = format!("file={},format=qcow2", machine.disk);
    // Another machine runs on the same disk in the second slot, and a VM has this slot.
    let other = Machine { id: "other".into(), name: "Other".into(), vcache_pin: true, ..machine.clone() };
    other.save(&context).unwrap();
    fake_process(&context, 61, &[&qemu, "-qmp", &run(slots::number(1)), "-drive", &drive], &[], "");
    fake_process(&context, 62, &[&qemu, "-qmp", &run(slots::first())], &[], "");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let wanting = Machine {
        vcache_pin: true,
        drives: vec![crate::machine::Drive { id: "ata-Gone_1".into() }],
        forwards: vec![Forward::new(Protocol::Tcp, port, 80)],
        ..machine
    };
    assert_eq!(
        start_problems(&context, &wanting).unwrap(),
        [
            NO_VCACHE.to_owned(),
            "This machine is running.".to_owned(),
            "The drive ata-Gone_1 is missing. Plug it in again, or remove it in the settings.".to_owned(),
            "All 4 slots are taken, 2 of them by another user or install on this PC.".to_owned(),
            VCACHE_TAKEN.to_owned(),
            format!("TCP port {port} on this PC is in use by another program."),
        ]
    );
}
