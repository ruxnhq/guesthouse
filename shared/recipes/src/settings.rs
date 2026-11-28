//! A recipe's settings, fitted to what this PC can give. A recipe says what it would like, and the
//! machine gets that or the nearest the PC's limits allow, with a note for each that came out
//! different, so a recipe written on a big PC still makes a machine on a small one. What comes out is
//! checked again by the machine's own rules when it is made. The limits are the host's to find.

use super::format::SettingsAsked;

/// The settings a machine is made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fitted {
    pub cores: u32,
    pub ram_gib: u32,
    pub disk_gib: u32,
    pub vram_gib: u32,
    /// What the recipe asked for and could not have, each in a sentence.
    pub notes: Vec<String>,
}

/// The size a setting takes when the recipe doesn't ask for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usual {
    pub cores: u32,
    pub ram_gib: u32,
    pub disk_gib: u32,
}

fn within(
    unit: &str,
    asked: Option<u32>,
    usual: u32,
    lowest: u32,
    highest: u32,
    notes: &mut Vec<String>,
) -> u32 {
    let Some(wanted) = asked else { return usual.clamp(lowest, highest.max(lowest)) };
    let got = wanted.clamp(lowest, highest.max(lowest));
    if got != wanted {
        notes.push(format!(
            "The recipe asks for {wanted} {unit}, and this PC allows {got}, so the machine has {got}."
        ));
    }
    got
}

/// What this PC allows a machine of the kind being made, each setting's least and most. A most below
/// the least is taken as the least.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub least_cores: u32,
    pub most_cores: u32,
    pub least_ram_gib: u32,
    pub most_ram_gib: u32,
    pub least_disk_gib: u32,
    pub most_disk_gib: u32,
    /// What this machine's card allows. The least is always 0, which is no cap.
    pub most_vram_gib: u32,
}

/// The settings a recipe asks for, each the nearest `limits` allow, and `usual` for each it doesn't.
pub fn fit(asked: &SettingsAsked, usual: Usual, limits: Limits) -> Fitted {
    let mut notes = Vec::new();
    let cores = within("cores", asked.cores, usual.cores, limits.least_cores, limits.most_cores, &mut notes);
    let ram_gib = within(
        "GiB of RAM",
        asked.ram_gib,
        usual.ram_gib,
        limits.least_ram_gib,
        limits.most_ram_gib,
        &mut notes,
    );
    let disk_gib = within(
        "GiB of disk",
        asked.disk_gib,
        usual.disk_gib,
        limits.least_disk_gib,
        limits.most_disk_gib,
        &mut notes,
    );
    let vram_gib = within("GiB of video memory", asked.vram_gib, 0, 0, limits.most_vram_gib, &mut notes);
    Fitted { cores, ram_gib, disk_gib, vram_gib, notes }
}
