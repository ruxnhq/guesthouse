//! A recipe's settings, fitted to what this PC can give. A recipe says what it would like, and the
//! machine gets that or the nearest the PC's limits allow, with a note for each that came out
//! different, so a recipe written on a big PC still makes a machine on a small one. What comes out is
//! checked again by the machine's own rules when it is made.

use super::format::SettingsAsked;
use crate::context::Context;
use crate::host::{RamField, physical_cores};
use crate::library::{MAX_DISK_GIB, MIN_PLAIN_DISK_GIB, MIN_WINDOWS_DISK_GIB};
use crate::machine::{Kind, MIN_CORES, MIN_RAM_GIB};

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

/// `vram_most` is what this machine's card allows, from `max_vram_gib_for`.
pub fn fit(context: &Context, kind: Kind, asked: &SettingsAsked, usual: Usual, vram_most: u32) -> Fitted {
    let windows = kind == Kind::Windows;
    let mut notes = Vec::new();
    let ram = if windows {
        RamField::for_gpu(context, context.slot, MIN_RAM_GIB.into(), usual.ram_gib.into())
    } else {
        RamField::for_plain(context, 1, usual.ram_gib.into())
    };
    let (least_cores, least_disk) =
        if windows { (MIN_CORES, MIN_WINDOWS_DISK_GIB) } else { (1, MIN_PLAIN_DISK_GIB) };
    let cores = within(
        "cores",
        asked.cores,
        usual.cores,
        least_cores,
        physical_cores(&context.cpus).max(1),
        &mut notes,
    );
    let ram_gib =
        within("GiB of RAM", asked.ram_gib, usual.ram_gib, ram.lower as u32, ram.upper as u32, &mut notes);
    let disk_gib =
        within("GiB of disk", asked.disk_gib, usual.disk_gib, least_disk, MAX_DISK_GIB, &mut notes);
    let vram_gib = within("GiB of video memory", asked.vram_gib, 0, 0, vram_most, &mut notes);
    Fitted { cores, ram_gib, disk_gib, vram_gib, notes }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
