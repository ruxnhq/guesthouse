//! A recipe's settings, fitted to what this PC can give, by `guesthouse_recipes_core`'s rule. What stays
//! here is finding this PC's limits for a kind of machine.

use super::format::SettingsAsked;
use crate::context::Context;
use crate::host::{RamField, physical_cores};
use crate::library::{MAX_DISK_GIB, MIN_PLAIN_DISK_GIB, MIN_WINDOWS_DISK_GIB};
use crate::machine::{Kind, MIN_CORES, MIN_RAM_GIB};
use guesthouse_recipes_core::Limits;
pub use guesthouse_recipes_core::{Fitted, Usual};

/// `vram_most` is what this machine's card allows, from `max_vram_gib_for`.
pub fn fit(context: &Context, kind: Kind, asked: &SettingsAsked, usual: Usual, vram_most: u32) -> Fitted {
    let windows = kind == Kind::Windows;
    let ram = if windows {
        RamField::for_gpu(context, context.slot, MIN_RAM_GIB.into(), usual.ram_gib.into())
    } else {
        RamField::for_plain(context, 1, usual.ram_gib.into())
    };
    let (least_cores, least_disk) =
        if windows { (MIN_CORES, MIN_WINDOWS_DISK_GIB) } else { (1, MIN_PLAIN_DISK_GIB) };
    let limits = Limits {
        least_cores,
        most_cores: physical_cores(&context.cpus).max(1),
        least_ram_gib: ram.lower as u32,
        most_ram_gib: ram.upper as u32,
        least_disk_gib: least_disk,
        most_disk_gib: MAX_DISK_GIB,
        most_vram_gib: vram_most,
    };
    guesthouse_recipes_core::fit(asked, usual, limits)
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
