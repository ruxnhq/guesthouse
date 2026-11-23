use super::*;
use crate::testing::{Scratch, scratch_context};

const USUAL: Usual = Usual { cores: 4, ram_gib: 8, disk_gib: 64 };

fn asked(
    cores: Option<u32>,
    ram_gib: Option<u32>,
    disk_gib: Option<u32>,
    vram_gib: Option<u32>,
) -> SettingsAsked {
    SettingsAsked { cores, ram_gib, disk_gib, vram_gib }
}

#[test]
fn what_the_recipe_does_not_ask_stays_as_it_was() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    // A PC with eight cores, whatever the one the test runs on has.
    context.cpus = crate::host::stand_in::fake_cpus(&scratch, "cpus", &[32768; 16]);
    let fitted = fit(&context, Kind::Windows, &SettingsAsked::default(), USUAL, 8);
    assert_eq!((fitted.cores, fitted.ram_gib, fitted.disk_gib, fitted.vram_gib), (4, 8, 64, 0));
    assert!(fitted.notes.is_empty());
}

#[test]
fn what_it_asks_for_and_this_pc_has_is_what_the_machine_gets() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.ram_limit = Some(32);
    let fitted = fit(&context, Kind::Windows, &asked(Some(2), Some(16), Some(200), Some(4)), USUAL, 8);
    assert_eq!((fitted.cores, fitted.ram_gib, fitted.disk_gib, fitted.vram_gib), (2, 16, 200, 4));
    assert!(fitted.notes.is_empty());
}

#[test]
fn what_this_pc_cannot_give_comes_down_to_what_it_can_and_says_so() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.ram_limit = Some(12);
    let fitted = fit(&context, Kind::Windows, &asked(Some(1), Some(64), Some(100_000), Some(32)), USUAL, 8);
    assert_eq!(fitted.cores, MIN_CORES, "Windows needs its two");
    assert_eq!(fitted.ram_gib, 12);
    assert_eq!(fitted.disk_gib, MAX_DISK_GIB);
    assert_eq!(fitted.vram_gib, 8);
    assert_eq!(fitted.notes.len(), 4, "{:?}", fitted.notes);
    assert!(fitted.notes[1].contains("asks for 64 GiB of RAM") && fitted.notes[1].contains("allows 12"));
}

#[test]
fn a_disk_below_windows_least_comes_up_to_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let fitted = fit(&context, Kind::Windows, &asked(None, None, Some(10), None), USUAL, 0);
    assert_eq!(fitted.disk_gib, MIN_WINDOWS_DISK_GIB);
    let ubuntu = fit(&context, Kind::Linux, &asked(None, None, Some(10), None), USUAL, 0);
    assert_eq!(ubuntu.disk_gib, 10, "Ubuntu has no such least");
}
