//! The Linux host's start checks: what several machines running at once share ([`shared_problems`]),
//! and every reason a machine can't start on this PC now ([`judge`]), from facts the host reads.

pub const SHARE_REFUSAL: &str = "This folder can't be shared. Choose a folder inside your home that isn't hidden, and \
                             isn't one Guesthouse keeps its own files in.";
pub const ALREADY_RUNNING: &str = "This machine is running.";
pub const MEMORY_TAKEN: &str =
    "Another VM on this PC has just taken this slot's memory. Start the machine again.";
pub const SECOND_INSTANCE_REFUSAL: &str = "A second instance leaves GPU-PV alone.";
/// The daemon can't start a GPU machine without what setup takes from NVIDIA's installer.
pub const NO_SETUP: &str =
    "A GPU machine needs setup first, and it hasn't been run. Run Setup Again from the menu.";
/// Without it the app can't reach the guest's agent, so the display driver and the rest of what a
/// start does in Windows would never happen. Only an install makes one, so a machine from before
/// each had its own must be given the old one.
pub const NO_TOKEN: &str = "This machine has no guest token, so the app can't reach Windows to finish starting it. \
                            Choose the token file it was installed with in its settings, under Guest Token.";

/// What several machines share, read once from the PC.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedFacts {
    /// The most RAM this machine can have on this PC now.
    pub ram_limit: u64,
    /// How much of the PC's RAM the other VMs' memory already takes, in GiB.
    pub touched_gib: u64,
    /// What this machine's card can give it.
    pub max_vram_gib: u32,
    pub vcache_cpus: u32,
    /// Where the NVIDIA card setup chose is, which a machine given no card runs on. Empty if it
    /// can't be told.
    pub setup_card: String,
}

/// What the checks need of a machine's settings, this one's or one running.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineFacts {
    /// Whether it runs with a GPU: a Windows machine that isn't installing.
    pub has_gpu: bool,
    pub cores: u32,
    pub ram_gib: u32,
    /// Its video memory cap in GiB, or 0 for none.
    pub vram_gib: u32,
    pub vcache_pin: bool,
    /// Where the card it was given is, or empty for setup's NVIDIA card.
    pub own_card: String,
}

/// Where a machine given `own_card`, or none, runs: on that card, or else on `setup_card`.
pub fn card_of(own_card: &str, setup_card: &str) -> String {
    if own_card.is_empty() { setup_card.to_owned() } else { own_card.to_owned() }
}

/// Whether `other` has a GPU on the same card as `machine`, so shares that card's memory.
/// `setup_card` is where setup's NVIDIA card is.
pub fn on_same_card(machine: &MachineFacts, other: &MachineFacts, setup_card: &str) -> bool {
    let card = card_of(&machine.own_card, setup_card);
    other.has_gpu && card_of(&other.own_card, setup_card) == card
}

/// The machines in `others` with a GPU on the same card as this one, which share that card's memory.
/// `setup_card` is where setup's NVIDIA card is.
pub fn card_mates(machine: &MachineFacts, others: &[MachineFacts], setup_card: &str) -> Vec<MachineFacts> {
    let same_card = |other: &&MachineFacts| on_same_card(machine, other, setup_card);
    others.iter().filter(same_card).cloned().collect()
}

/// What the machines running now leave of the card. One with no cap has all of it.
pub fn video_memory_left(others: &[MachineFacts], max_vram_gib: u32) -> u32 {
    let gpu = others.iter().filter(|other| other.has_gpu);
    let used: u32 = gpu.map(|other| if other.vram_gib > 0 { other.vram_gib } else { max_vram_gib }).sum();
    max_vram_gib.saturating_sub(used)
}

/// What several machines share: the memory that holds them, and the card's for those on the same
/// card. None of it counts if this one runs alone, so a single VM is told what it always was.
pub fn shared_problems(machine: &MachineFacts, others: &[MachineFacts], facts: &SharedFacts) -> Vec<String> {
    if others.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    // The limit already leaves out what they hold, which is all of their RAM once claimed. A machine
    // still starting may not have claimed its own yet, and that will come out of the same RAM.
    let reserved: u64 =
        others.iter().filter(|other| other.has_gpu).map(|other| u64::from(other.ram_gib)).sum();
    let untouched = reserved.saturating_sub(facts.touched_gib);
    if machine.has_gpu && u64::from(machine.ram_gib) + untouched > facts.ram_limit {
        found.push("This PC's RAM cannot hold this machine beside the ones running.".to_owned());
    }
    let mates = if machine.has_gpu { card_mates(machine, others, &facts.setup_card) } else { Vec::new() };
    if !mates.is_empty() {
        let left = video_memory_left(&mates, facts.max_vram_gib);
        if left == 0 {
            found.push(
                "The running machine has all the video memory. Give it a cap in its settings first."
                    .to_owned(),
            );
        } else if machine.vram_gib > left {
            found.push(format!("Only {left} GiB of video memory is left beside the machines running."));
        }
    }
    let pinned: u32 = others.iter().filter(|other| other.vcache_pin).map(|other| other.cores).sum();
    if machine.vcache_pin && pinned + machine.cores > facts.vcache_cpus {
        found.push("The V-Cache cores are taken by the machines running.".to_owned());
    }
    found
}

/// The cap a machine starts with: its own, or what `others`, the machines running on its card, leave.
/// Alone on its card it needs none.
pub fn video_cap(machine: &MachineFacts, others: &[MachineFacts], max_vram_gib: u32) -> u32 {
    match (machine.vram_gib, others.is_empty()) {
        (0, false) => video_memory_left(others, max_vram_gib),
        (cap, _) => cap,
    }
}

/// What a start is judged on, as the host reads it from the PC. A fact that only counts under another
/// is read only then, and is empty or `false` otherwise, as each field says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartFacts {
    /// The machine's own settings.
    pub machine: MachineFacts,
    /// What is wrong with its settings whatever the host, in the host's sentences.
    pub settings_problems: Vec<String>,
    /// Its disk and firmware file, each if it's named and isn't there, in that order.
    pub missing_files: Vec<String>,
    /// Its disc, if one is named and isn't there.
    pub missing_disc: Option<String>,
    /// Its shared folder, if one is named and isn't a folder.
    pub missing_share: Option<String>,
    /// Why its shared folder can't be shared, if it can't.
    pub share_problem: Option<String>,
    /// What this build says of a Windows machine when it has no GPU part, when it hasn't.
    pub no_gpu_part: Option<String>,
    /// Why its card can't run it, read for a machine with a GPU when the build has its part.
    pub card_problems: Vec<String>,
    /// Whether it lacks its guest token, read when its card is.
    pub no_token: bool,
    /// Whether this is a second instance of the app.
    pub second_instance: bool,
    /// The PC's cores.
    pub pc_cores: u32,
    /// The most RAM it can have on this PC now.
    pub ram_limit: u64,
    /// The PC's V-Cache threads, read for a machine pinned to them.
    pub vcache_threads: usize,
    /// Whether a machine running now runs on its disk.
    pub disk_in_use: bool,
    /// What is wrong with the drives it's given.
    pub drive_problems: Vec<String>,
    /// What to say when a VM already has its slot, if one has.
    pub slot_taken: Option<String>,
    /// Whether another VM has taken its slot's memory, read for a machine with a GPU when no VM has
    /// the slot.
    pub memory_taken: bool,
    /// The machines running now.
    pub others: Vec<MachineFacts>,
    /// What several machines share.
    pub shared: SharedFacts,
    /// What stops its network now.
    pub network_problems: Vec<String>,
}

/// Says why this machine can't start on this host now, in the order the host says it.
pub fn judge(facts: &StartFacts) -> Vec<String> {
    let machine = &facts.machine;
    let mut found = facts.settings_problems.clone();
    for path in &facts.missing_files {
        found.push(format!("{path} is missing."));
    }
    if let Some(disc) = &facts.missing_disc {
        found.push(format!("{disc} is missing. Eject it in the settings to boot from the disk."));
    }
    if let Some(share) = &facts.missing_share {
        found.push(format!("{share} is missing. Choose another shared folder in the settings."));
    }
    found.extend(facts.share_problem.clone());
    if let Some(no_gpu_part) = &facts.no_gpu_part {
        found.push(no_gpu_part.clone());
    } else if machine.has_gpu {
        found.extend(facts.card_problems.iter().cloned());
        if facts.no_token {
            found.push(NO_TOKEN.to_owned());
        }
    }
    if machine.has_gpu && facts.second_instance {
        found.push(SECOND_INSTANCE_REFUSAL.to_owned());
    }
    let cores = facts.pc_cores;
    if machine.cores > cores {
        found.push(format!("This PC has {cores} cores, so a machine can have that many at most."));
    }
    let limit = facts.ram_limit;
    if u64::from(machine.ram_gib) > limit {
        found.push(format!("This PC can give a machine {limit} GiB at most right now."));
    }
    if machine.vcache_pin {
        let cpus = facts.vcache_threads;
        if cpus == 0 {
            found.push("This CPU has no V-Cache cores to pin to.".to_owned());
        } else if machine.cores as usize > cpus {
            found.push(format!(
                "The V-Cache cores run {} threads, too few for {} cores.",
                cpus, machine.cores
            ));
        }
    }
    if facts.disk_in_use {
        found.push(ALREADY_RUNNING.to_owned());
    }
    found.extend(facts.drive_problems.iter().cloned());
    if let Some(taken) = &facts.slot_taken {
        found.push(taken.clone());
    } else if machine.has_gpu && facts.memory_taken {
        found.push(MEMORY_TAKEN.to_owned());
    }
    found.extend(shared_problems(machine, &facts.others, &facts.shared));
    found.extend(facts.network_problems.iter().cloned());
    found
}

#[cfg(test)]
mod tests;
