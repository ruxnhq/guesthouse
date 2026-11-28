//! Why a machine can't start on this PC now. The judging is plain data in and words out, so a test
//! can state any host it likes.

use crate::context::Context;
use crate::failure::Result;
use crate::gpu_maker::GpuMaker;
use crate::gpu_support::{NO_GPU_PART, gpu_ready};
use crate::held_memory::used_by_others;
use crate::host::{self, GIB, physical_cores, ram_limit_for, vcache_cpus};
use crate::host_drives::drive_problems;
use crate::machine::{Kind, Machine, load_all};
use crate::network_settings::{network_start_problems, other_machines};
use crate::processes::{memory_taken, qemu_pid};
use crate::screen_gpu::setup_card;
use crate::settings::max_vram_gib_for;
use crate::setup::amd::amd_start_problems;
use crate::setup::cards::{card_problem, nvidia_card_problem};
use crate::setup::intel::intel_start_problems;
use crate::setup::setup_done;
use crate::tasks::no_slot_text;
use guesthouse_start_checks_core::linux_host::{self, MachineFacts, StartFacts, judge};
pub use guesthouse_start_checks_core::linux_host::{
    MEMORY_TAKEN, NO_SETUP, NO_TOKEN, SECOND_INSTANCE_REFUSAL, SHARE_REFUSAL, SharedFacts,
};
use std::path::{Path, PathBuf};

/// [`SharedFacts`] read from the PC.
pub trait FromThisPc: Sized {
    fn read(context: &Context, machine: &Machine) -> Result<Self>;
}

impl FromThisPc for SharedFacts {
    fn read(context: &Context, machine: &Machine) -> Result<Self> {
        Ok(Self {
            ram_limit: ram_limit_for(context, machine)?,
            touched_gib: used_by_others(context, context.slot) / GIB,
            max_vram_gib: max_vram_gib_for(context, machine),
            vcache_cpus: vcache_cpus(&context.cpus).len() as u32,
            setup_card: setup_card(context),
        })
    }
}

/// What the checks need of a machine's settings.
pub fn facts_of(machine: &Machine) -> MachineFacts {
    MachineFacts {
        has_gpu: machine.has_gpu(),
        cores: machine.cores,
        ram_gib: machine.ram_gib,
        vram_gib: machine.vram_gib,
        vcache_pin: machine.vcache_pin,
        own_card: machine.amd_gpu.clone(),
    }
}

fn all_facts(machines: &[Machine]) -> Vec<MachineFacts> {
    machines.iter().map(facts_of).collect()
}

/// The machines in `others` with a GPU on the same card as this one, which share that card's memory.
/// `setup_card` is where setup's NVIDIA card is.
pub fn card_mates(machine: &Machine, others: &[Machine], setup_card: &str) -> Vec<Machine> {
    let this = facts_of(machine);
    let same_card = |other: &&Machine| linux_host::on_same_card(&this, &facts_of(other), setup_card);
    others.iter().filter(same_card).cloned().collect()
}

/// What the machines running now leave of the card. One with no cap has all of it.
pub fn video_memory_left(others: &[Machine], max_vram_gib: u32) -> u32 {
    linux_host::video_memory_left(&all_facts(others), max_vram_gib)
}

/// What several machines share: see [`linux_host::shared_problems`].
pub fn shared_problems(machine: &Machine, others: &[Machine], facts: &SharedFacts) -> Vec<String> {
    linux_host::shared_problems(&facts_of(machine), &all_facts(others), facts)
}

/// The cap a machine starts with: see [`linux_host::video_cap`].
pub fn video_cap(machine: &Machine, others: &[Machine], max_vram_gib: u32) -> u32 {
    linux_host::video_cap(&facts_of(machine), &all_facts(others), max_vram_gib)
}

/// Why this folder can't be shared. A guest can write to what it's given, so it can't be given the
/// user's home as a whole, the hidden folders in it, or what this app runs from.
pub fn share_problem(context: &Context, folder: &str) -> Option<String> {
    let folder = Path::new(folder).canonicalize().ok()?;
    let refused = place_problem(context, &folder);
    refused.or_else(|| machine_file_in(&folder, &load_all(context).0))
}

/// As the file system finds a path, so neither a `..` nor a link in it hides where it is.
fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

/// The files a machine's QEMU opens: its disk and the files it's built on, its disc, its firmware
/// settings, and OpenCore's and Android's disks.
fn files_qemu_opens(machine: &Machine) -> impl Iterator<Item = &str> {
    let own = [&machine.disk, &machine.iso, &machine.vars, &machine.opencore, &machine.android_data];
    let backing = machine.backing_files.iter().map(|file| &file.path);
    own.into_iter().chain(backing).map(String::as_str).filter(|path| !path.is_empty())
}

/// The first of these machines' files in the folder, which is found as the file system has it.
fn file_in<'a>(folder: &Path, machines: &'a [Machine]) -> Option<(&'a Machine, &'a str)> {
    machines.iter().find_map(|machine| {
        let file = files_qemu_opens(machine).find(|file| real(Path::new(file)).starts_with(folder))?;
        Some((machine, file))
    })
}

/// Says which of these machines' files is in the folder, if one is.
fn machine_file_in(folder: &Path, machines: &[Machine]) -> Option<String> {
    file_in(folder, machines).map(|(machine, file)| {
        format!(
            "The shared folder holds {file}, which {} starts from, and the guest can change anything in it. \
             Share a folder that holds no machine's disk, disc or firmware settings.",
            machine.name
        )
    })
}

/// Whether the folder is the home folder, holds it or the app's own, or is a hidden one in home.
fn place_problem(context: &Context, folder: &Path) -> Option<String> {
    let paths = &context.paths;
    let home = real(&paths.home);
    let ours = [&paths.store, &paths.config, &paths.state].map(|path| real(path));
    let holds_ours = ours.iter().any(|path| path.starts_with(folder) || folder.starts_with(path));
    let hidden = folder.strip_prefix(&home).is_ok_and(|inside| {
        inside.components().next().is_some_and(|first| first.as_os_str().to_string_lossy().starts_with('.'))
    });
    (folder == home || home.starts_with(folder) || holds_ours || hidden).then(|| SHARE_REFUSAL.to_owned())
}

/// Says why a GPU machine can't start on its card, by the maker whose setup and card it needs.
fn card_start_problems(context: &Context, machine: &Machine) -> Vec<String> {
    match machine.windows_maker() {
        Some(GpuMaker::Nvidia) if !setup_done(context) => vec![NO_SETUP.to_owned()],
        Some(GpuMaker::Nvidia) => match machine.windows_card_address() {
            Some(pci) => nvidia_card_problem(context, pci).into_iter().collect(),
            None => card_problem(context).into_iter().collect(),
        },
        Some(GpuMaker::Amd) => amd_start_problems(context, &machine.amd_gpu),
        Some(GpuMaker::Intel) => intel_start_problems(context, &machine.amd_gpu),
        None => Vec::new(),
    }
}

/// What this machine's start is judged on, read from this PC in the order the checks were made.
fn start_facts(context: &Context, machine: &Machine) -> Result<StartFacts> {
    let settings_problems = machine.problems(max_vram_gib_for(context, machine));
    let missing = |path: &String| !path.is_empty() && !Path::new(path).exists();
    let missing_files =
        [&machine.disk, &machine.vars].into_iter().filter(|path| missing(path)).cloned().collect();
    let missing_disc = missing(&machine.iso).then(|| machine.iso.clone());
    let missing_share =
        (!machine.share.is_empty() && !Path::new(&machine.share).is_dir()).then(|| machine.share.clone());
    // Its own share holds no machine's files, and no other machine's share holds its own.
    let in_another_share = || {
        let others = load_all(context).0.into_iter().filter(|other| other.id != machine.id);
        let shares = others.map(|other| other.share).filter(|share| !share.is_empty());
        shares.filter_map(|share| Path::new(&share).canonicalize().ok()).find_map(|share| {
            file_in(&share, std::slice::from_ref(machine)).map(|(_, file)| {
                format!(
                    "{file} is in another machine's shared folder, {}, and its guest can change anything in \
                     it. Move it out, or share another folder there.",
                    share.display()
                )
            })
        })
    };
    let share_problem = share_problem(context, &machine.share).or_else(in_another_share);
    let no_gpu_part = (machine.kind == Kind::Windows && !gpu_ready(context)).then(|| NO_GPU_PART.to_owned());
    let reads_card = no_gpu_part.is_none() && machine.has_gpu();
    let card_problems = if reads_card { card_start_problems(context, machine) } else { Vec::new() };
    let no_token = reads_card && !machine.token_file(context).exists();
    let second_instance = machine.has_gpu() && context.second_instance();
    let pc_cores = physical_cores(&context.cpus);
    let ram_limit = ram_limit_for(context, machine)?;
    let vcache_threads = if machine.vcache_pin { host::vcache_cpus(&context.cpus).len() } else { 0 };
    let others = other_machines(context);
    let disk_in_use = others.iter().any(|other| other.disk == machine.disk);
    let drive_problems = drive_problems(context, machine, &others);
    let slot_taken = qemu_pid(context, None).is_some().then(|| no_slot_text(context));
    let memory_taken = slot_taken.is_none() && machine.has_gpu() && memory_taken(context, None);
    let shared = SharedFacts::read(context, machine)?;
    let network_problems = network_start_problems(context, machine, &others);
    Ok(StartFacts {
        machine: facts_of(machine),
        settings_problems,
        missing_files,
        missing_disc,
        missing_share,
        share_problem,
        no_gpu_part,
        card_problems,
        no_token,
        second_instance,
        pc_cores,
        ram_limit,
        vcache_threads,
        disk_in_use,
        drive_problems,
        slot_taken,
        memory_taken,
        others: all_facts(&others),
        shared,
        network_problems,
    })
}

/// Says why this machine can't start on this host now.
pub fn start_problems(context: &Context, machine: &Machine) -> Result<Vec<String>> {
    Ok(judge(&start_facts(context, machine)?))
}

#[cfg(test)]
mod pins;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::stand_in::fake_cpus;
    use crate::machine::Kind;
    use crate::processes::stand_in::fake_process;
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};
    use std::fs;

    fn machine(id: &str) -> Machine {
        Machine::new(id, id, &format!("/{id}"), "/v")
    }

    fn facts() -> SharedFacts {
        SharedFacts {
            ram_limit: 36,
            touched_gib: 0,
            max_vram_gib: 28,
            vcache_cpus: 0,
            setup_card: String::new(),
        }
    }

    fn says(found: &[String], text: &str) -> bool {
        found.iter().any(|problem| problem.contains(text))
    }

    #[test]
    fn the_home_folder_and_whats_above_it_cant_be_shared() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.home = scratch.path("home");
        fs::create_dir_all(&context.paths.home).unwrap();
        let home = context.paths.home.display().to_string();
        assert!(share_problem(&context, &home).is_some());
        assert!(share_problem(&context, &scratch.path("").display().to_string()).is_some());
    }

    #[test]
    fn hidden_folders_and_the_apps_own_cant_be_shared() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.home = scratch.path("home");
        let hidden = scratch.path("home/.config/autostart");
        fs::create_dir_all(&hidden).unwrap();
        fs::create_dir_all(context.paths.machines.join("one")).unwrap();
        assert!(share_problem(&context, &hidden.display().to_string()).is_some());
        let machine = context.paths.machines.join("one").display().to_string();
        assert!(share_problem(&context, &machine).is_some());
    }

    /// A machine kept under `place`, with its disk, firmware settings and disc there.
    fn kept_in(context: &Context, scratch: &Scratch, id: &str, place: &str) -> Machine {
        let file = |name: &str| scratch.write(&format!("{place}/{name}"), "x").display().to_string();
        let machine = Machine {
            kind: Kind::Plain,
            iso: file(&format!("{id}.iso")),
            ..Machine::new(id, id, &file(&format!("{id}.qcow2")), &file(&format!("{id}-vars.fd")))
        };
        fs::create_dir_all(machine.folder(context)).unwrap();
        machine.save(context).unwrap();
        machine
    }

    #[test]
    fn a_folder_that_holds_a_machines_files_cant_be_shared() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.home = scratch.path("home");
        kept_in(&context, &scratch, "lab", "home/VMs/lab");
        let vms = scratch.path("home/VMs").display().to_string();
        assert!(share_problem(&context, &vms).unwrap().contains("lab.qcow2"), "the disk");
        // The same folder by a link to it.
        std::os::unix::fs::symlink(scratch.path("home/VMs"), scratch.path("home/Linked")).unwrap();
        assert!(share_problem(&context, &scratch.path("home/Linked").display().to_string()).is_some());
        // A disk named through a link into the share.
        fs::create_dir_all(scratch.path("home/Shared/real")).unwrap();
        std::os::unix::fs::symlink(scratch.path("home/Shared/real"), scratch.path("home/Disks")).unwrap();
        kept_in(&context, &scratch, "other", "home/Disks");
        let shared = scratch.path("home/Shared").display().to_string();
        assert!(share_problem(&context, &shared).unwrap().contains("other.qcow2"));
        let backing = scratch.write("home/Bases/base.qcow2", "x");
        let mut on_base = kept_in(&context, &scratch, "layered", "home/Layered");
        on_base.backing_files = vec![crate::disk_image::backing::BackingFile {
            path: backing.display().to_string(),
            format: "qcow2".into(),
        }];
        on_base.save(&context).unwrap();
        let bases = scratch.path("home/Bases").display().to_string();
        assert!(share_problem(&context, &bases).unwrap().contains("base.qcow2"), "a backing file");
    }

    #[test]
    fn a_start_is_refused_while_any_machines_share_holds_its_files() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.home = scratch.path("home");
        let lab = kept_in(&context, &scratch, "lab", "home/Downloads");
        let mut own = lab.clone();
        own.share = scratch.path("home/Downloads").display().to_string();
        let found = start_problems(&context, &own).unwrap();
        assert!(says(&found, "The shared folder holds"), "its own share holds its files: {found:?}");
        // A disc alone in the share is refused too.
        let disc_only = Machine {
            disk: scratch.write("home/Other/lab2.qcow2", "x").display().to_string(),
            ..lab.clone()
        };
        let disc_only = Machine { vars: String::new(), ..disc_only };
        disc_only.save(&context).unwrap();
        let found =
            start_problems(&context, &Machine { share: own.share.clone(), ..disc_only.clone() }).unwrap();
        assert!(says(&found, "lab.iso"), "{found:?}");
        lab.save(&context).unwrap();
        let mut other = kept_in(&context, &scratch, "other", "home/Other");
        other.share = scratch.path("home/Downloads").display().to_string();
        other.save(&context).unwrap();
        assert!(
            says(&start_problems(&context, &lab).unwrap(), "lab.qcow2"),
            "another's share holds its disk"
        );
    }

    #[test]
    fn a_folder_of_its_own_can_be_shared() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.home = scratch.path("home");
        let documents = scratch.path("home/Documents/For the guest");
        fs::create_dir_all(&documents).unwrap();
        assert_eq!(share_problem(&context, &documents.display().to_string()), None);
    }

    #[test]
    fn one_with_no_cap_has_all_the_card() {
        let (first, second) = (machine("first"), machine("second"));
        let others = [first.clone()];
        assert_eq!(video_memory_left(&others, 28), 0);
        assert!(says(&shared_problems(&second, &others, &facts()), "all the video memory"));
    }

    #[test]
    fn machines_on_different_cards_do_not_share_video_memory() {
        let first = machine("first");
        let mut second = machine("second");
        second.use_windows_card(GpuMaker::Nvidia, "0000:02:00.0");
        let others = [first.clone()];
        let facts = SharedFacts { setup_card: "0000:01:00.0".to_owned(), ..facts() };
        assert!(card_mates(&second, &others, &facts.setup_card).is_empty());
        assert!(shared_problems(&second, &others, &facts).is_empty(), "the second card is its own");
        assert_eq!(video_cap(&second, &card_mates(&second, &others, &facts.setup_card), 28), 0);
        let mut on_first_by_address = machine("third");
        on_first_by_address.use_windows_card(GpuMaker::Nvidia, "0000:01:00.0");
        assert_eq!(card_mates(&on_first_by_address, &others, &facts.setup_card), others);
        assert!(says(&shared_problems(&on_first_by_address, &others, &facts), "all the video memory"));
        let running_second = [second.clone()];
        assert_eq!(card_mates(&first, &running_second, &facts.setup_card), []);
    }

    #[test]
    fn a_cap_leaves_the_rest_for_the_next_machine() {
        let others = [Machine { vram_gib: 10, ..machine("first") }];
        let mut second = machine("second");
        assert_eq!(video_memory_left(&others, 28), 18);
        assert!(shared_problems(&second, &others, &facts()).is_empty());
        assert_eq!(video_cap(&second, &others, 28), 18, "what the first leaves");
        second.vram_gib = 20;
        assert!(says(&shared_problems(&second, &others, &facts()), "18 GiB"));
    }

    #[test]
    fn each_machine_counts_in_full_against_the_pcs_ram() {
        let others = [Machine { vram_gib: 10, ..machine("first") }];
        let small = SharedFacts { ram_limit: 30, ..facts() };
        assert!(says(&shared_problems(&machine("second"), &others, &small), "PC's RAM cannot hold"));
        assert!(shared_problems(&machine("second"), &others, &facts()).is_empty());
        let touched = SharedFacts { touched_gib: 16, ..small };
        assert!(
            shared_problems(&machine("second"), &others, &touched).is_empty(),
            "what is touched is in the limit"
        );
    }

    #[test]
    fn the_vcache_cores_count_together() {
        let others = [Machine { vcache_pin: true, vram_gib: 10, ..machine("first") }];
        let second = Machine { vcache_pin: true, cores: 9, ..machine("second") };
        let with_cores = SharedFacts { vcache_cpus: 16, ..facts() };
        assert!(says(&shared_problems(&second, &others, &with_cores), "V-Cache"));
    }

    #[test]
    fn alone_nothing_is_shared() {
        assert!(shared_problems(&machine("second"), &[], &facts()).is_empty());
        assert_eq!(video_cap(&machine("second"), &[], 28), 0);
    }

    fn startable(scratch: &Scratch) -> (Context, Machine) {
        let mut context = scratch_context(scratch);
        context.cpus = fake_cpus(scratch, "cpus", &[32768; 16]);
        context.meminfo =
            scratch.write("meminfo", "MemTotal:       100000000 kB\nMemAvailable:   100000000 kB\n");
        let disk = scratch.write("disk", "").display().to_string();
        let vars = scratch.write("vars", "").display().to_string();
        let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", &disk, &vars) };
        (context, machine)
    }

    #[test]
    fn a_ready_plain_machine_has_no_problems() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        assert_eq!(start_problems(&context, &machine).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn missing_things_are_named() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let gone = scratch.path("gone").display().to_string();
        let problems = |machine: &Machine| start_problems(&context, machine).unwrap();
        assert!(says(&problems(&Machine { disk: gone.clone(), ..machine.clone() }), "is missing"));
        assert!(says(&problems(&Machine { iso: gone.clone(), ..machine.clone() }), "Eject it"));
        assert!(says(&problems(&Machine { share: gone, ..machine.clone() }), "shared folder"));
    }

    #[test]
    fn more_than_the_pc_has_is_refused() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let found = start_problems(&context, &Machine { cores: 9, ram_gib: 1000, ..machine }).unwrap();
        assert!(says(&found, "8 cores") && says(&found, "GiB at most"));
    }

    #[test]
    fn pinning_needs_vcache_cores() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let found = start_problems(&context, &Machine { vcache_pin: true, ..machine }).unwrap();
        assert!(says(&found, "no V-Cache"));
    }

    #[test]
    fn a_second_instance_leaves_the_gpu_alone() {
        let scratch = Scratch::new();
        let (mut context, machine) = startable(&scratch);
        context.paths.own_run = Some(scratch.path("second"));
        let gpu = Machine { kind: Kind::Windows, ..machine };
        assert!(says(&start_problems(&context, &gpu).unwrap(), "leaves GPU-PV alone"));
    }

    fn finish_setup(context: &Context) {
        fs::create_dir_all(crate::settings::driver_folder(context)).unwrap();
        let card = serde_json::json!({"card": {"device": 1}});
        crate::settings::write_setup(context, card.as_object().cloned().unwrap()).unwrap();
    }

    #[test]
    fn a_gpu_machine_needs_setup_first() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let gpu = Machine { kind: Kind::Windows, ..machine };
        let found = start_problems(&context, &gpu).unwrap();
        assert!(says(&found, NO_SETUP), "{found:?}");
        assert!(found[0].contains("Run Setup Again"), "it comes first, so it's what the row says");
        finish_setup(&context);
        assert!(!says(&start_problems(&context, &gpu).unwrap(), NO_SETUP));
    }

    #[test]
    fn a_windows_machine_on_an_amd_card_needs_amds_driver_and_not_nvidias_setup() {
        use crate::setup::amd::NO_AMD_SETUP;
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let on_amd = Machine { kind: Kind::Windows, amd_gpu: "0000:7c:00.0".into(), ..machine };
        let found = start_problems(&context, &on_amd).unwrap();
        assert!(says(&found, NO_AMD_SETUP), "{found:?}");
        assert!(says(&found, "at 0000:7c:00.0, isn't in this PC now"), "{found:?}");
        assert!(!says(&found, NO_SETUP), "NVIDIA's setup is not this machine's");
    }

    #[test]
    fn a_windows_machine_on_an_intel_card_needs_intels_own_setup_and_card() {
        use crate::setup::intel::NO_INTEL_SETUP;
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        finish_setup(&context);
        let mut on_intel = Machine { kind: Kind::Windows, ..machine };
        on_intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
        fs::create_dir_all(on_intel.folder(&context)).unwrap();
        crate::agent::make_token(&context, &on_intel).unwrap();
        let found = start_problems(&context, &on_intel).unwrap();
        assert!(says(&found, NO_INTEL_SETUP), "{found:?}");
        assert!(says(&found, "at 0000:00:02.0, isn't in this PC now"), "{found:?}");
        assert!(!says(&found, NO_SETUP), "NVIDIA's setup is not this machine's");
    }

    #[test]
    fn on_two_cards_a_gpu_machine_needs_setup_to_have_named_one() {
        use crate::setup::cards::stand_in::{NAMES, fake_pci};
        let scratch = Scratch::new();
        let (mut context, machine) = startable(&scratch);
        context.pci_devices = scratch.path("pci");
        context.pci_names = scratch.write("pci.ids", NAMES);
        fake_pci(&scratch, "0000:01:00.0", 0x2704, "0x10de", "0x030000");
        let gpu = Machine { kind: Kind::Windows, ..machine };
        finish_setup(&context);
        assert!(!says(&start_problems(&context, &gpu).unwrap(), "Run Setup Again"), "one card, an old setup");
        fake_pci(&scratch, "0000:02:00.0", 0x21C4, "0x10de", "0x030000");
        assert!(says(&start_problems(&context, &gpu).unwrap(), "more than one NVIDIA card"));
        let card = serde_json::json!({"card": {"device": 0x21C4, "pci": "0000:02:00.0"}});
        crate::settings::write_setup(&context, card.as_object().cloned().unwrap()).unwrap();
        assert!(!says(&start_problems(&context, &gpu).unwrap(), "Run Setup Again"));
    }

    #[test]
    fn a_machine_given_the_second_nvidia_card_starts_while_it_is_here_and_is_refused_once_it_is_gone() {
        use crate::setup::cards::stand_in::{NAMES, fake_pci};
        let scratch = Scratch::new();
        let (mut context, machine) = startable(&scratch);
        context.pci_names = scratch.write("pci.ids", NAMES);
        fake_pci(&scratch, "0000:01:00.0", 0x2704, "0x10de", "0x030000");
        let card = serde_json::json!({"card": {"device": 0x2704, "pci": "0000:01:00.0"}});
        fs::create_dir_all(crate::settings::driver_folder(&context)).unwrap();
        crate::settings::write_setup(&context, card.as_object().cloned().unwrap()).unwrap();
        let mut gpu = Machine { kind: Kind::Windows, ..machine };
        gpu.use_windows_card(GpuMaker::Nvidia, "0000:02:00.0");
        fs::create_dir_all(gpu.folder(&context)).unwrap();
        crate::agent::make_token(&context, &gpu).unwrap();
        let found = start_problems(&context, &gpu).unwrap();
        assert!(says(&found, "The NVIDIA card this machine runs on, at 0000:02:00.0, isn't in this PC now"));
        assert!(says(&found, "Choose another NVIDIA card in its settings"), "{found:?}");
        fake_pci(&scratch, "0000:02:00.0", 0x21C4, "0x10de", "0x030000");
        assert_eq!(start_problems(&context, &gpu).unwrap(), Vec::<String>::new());
        fs::remove_dir_all(scratch.path("pci/0000:01:00.0")).unwrap();
        assert!(start_problems(&context, &gpu).unwrap().is_empty(), "setup's card isn't this machine's");
    }

    #[test]
    fn only_a_gpu_machine_needs_setup() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        assert!(!says(&start_problems(&context, &machine).unwrap(), NO_SETUP), "a plain VM");
        let installing = Machine { kind: Kind::Windows, installing: true, ..machine };
        assert!(!says(&start_problems(&context, &installing).unwrap(), NO_SETUP), "an install");
    }

    #[test]
    fn a_gpu_machine_with_no_token_says_where_to_give_it_one() {
        let scratch = Scratch::new();
        let (mut context, machine) = startable(&scratch);
        context.pci_names = scratch.write("pci.ids", crate::setup::cards::stand_in::NAMES);
        crate::setup::cards::stand_in::fake_pci(&scratch, "0000:01:00.0", 0x2704, "0x10de", "0x030000");
        finish_setup(&context);
        let gpu = Machine { kind: Kind::Windows, ..machine };
        fs::create_dir_all(gpu.folder(&context)).unwrap();
        assert_eq!(start_problems(&context, &gpu).unwrap(), [NO_TOKEN]);
        assert!(NO_TOKEN.contains("Guest Token"), "the settings group the Token row is in");
        crate::agent::make_token(&context, &gpu).unwrap();
        assert!(start_problems(&context, &gpu).unwrap().is_empty());
    }

    #[test]
    fn only_a_gpu_machine_needs_a_token_to_start() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        finish_setup(&context);
        assert!(!says(&start_problems(&context, &machine).unwrap(), NO_TOKEN), "a plain VM");
        let installing = Machine { kind: Kind::Windows, installing: true, ..machine };
        assert!(!says(&start_problems(&context, &installing).unwrap(), NO_TOKEN), "an install makes it");
    }

    #[test]
    fn memory_another_vm_took_says_so_only_for_a_gpu_machine() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let memory = context.paths.shared_file("ram", slots::first());
        fs::write(&memory, "").unwrap();
        fake_process(
            &context,
            200,
            &["/other/guesthouse-daemon", "/other"],
            &[],
            &memory.display().to_string(),
        );
        let gpu = Machine { kind: Kind::Windows, ..machine.clone() };
        // The fake meminfo says nothing about the fake shared memory's real size, so only the words count.
        assert!(says(&start_problems(&context, &gpu).unwrap(), MEMORY_TAKEN));
        assert!(
            !says(&start_problems(&context, &machine).unwrap(), MEMORY_TAKEN),
            "a plain VM does not use it"
        );
    }

    /// A disk of the PC under a name, with a file standing in for its node.
    fn plug_in_a_disk(scratch: &Scratch) -> Context {
        let (context, _) = startable(scratch);
        scratch.write("block/sdb/dev", "8:16\n");
        let node = scratch.write("dev/sdb", "");
        fs::create_dir_all(scratch.path("drive-ids")).unwrap();
        std::os::unix::fs::symlink(node, scratch.path("drive-ids/ata-Disk_1")).unwrap();
        scratch.write("block/sdb/size", "100\n");
        context
    }

    fn with_drive(machine: Machine, id: &str) -> Machine {
        Machine { drives: vec![crate::machine::Drive { id: id.into() }], ..machine }
    }

    #[test]
    fn a_free_drive_is_no_problem_and_a_mounted_one_says_where() {
        let scratch = Scratch::new();
        let context = plug_in_a_disk(&scratch);
        let (_, machine) = startable(&scratch);
        let machine = with_drive(machine, "ata-Disk_1");
        assert_eq!(start_problems(&context, &machine).unwrap(), Vec::<String>::new());
        scratch.write("mountinfo", "30 1 8:16 / /mnt/data rw - ext4 /dev/sdb rw\n");
        let found = start_problems(&context, &machine).unwrap();
        assert!(says(&found, "is mounted at /mnt/data"), "{found:?}");
    }

    #[test]
    fn a_drive_that_is_not_plugged_in_is_missing() {
        let scratch = Scratch::new();
        let context = plug_in_a_disk(&scratch);
        let (_, machine) = startable(&scratch);
        let found = start_problems(&context, &with_drive(machine, "ata-Other_2")).unwrap();
        assert!(says(&found, "The drive ata-Other_2 is missing"), "{found:?}");
    }
}
