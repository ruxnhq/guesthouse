//! The small JSON files: what setup left behind, and what the app remembers between runs.

use crate::context::Context;
use crate::failure::Result;
use crate::gpu_maker::GpuMaker;
use crate::machine::Machine;
use serde::Serialize;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Where the driver's files go until setup has found them.
const DRIVER_FOLDER: &str = "setup/driver";
const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;
/// The card's memory in MiB, in `setup.json`.
pub const CARD_MEMORY_FIELD: &str = "card_memory_mib";
/// Every NVIDIA card's memory in MiB, by its address, in `setup.json`. A setup from before machines
/// had their own NVIDIA card has none.
pub const CARD_MEMORIES_FIELD: &str = "card_memories";
/// The host keeps an eighth of the card, and at least 2 GiB, for its desktop.
const HOST_SHARE: u64 = 8;
const HOST_MINIMUM_GIB: u64 = 2;

pub type Fields = Map<String, Value>;

pub use guesthouse_basics::json_file::with_suffix;

/// Replaces the file in one step, so a crash can't leave half of it. Every host shares how, in
/// `guesthouse_basics::json_file`; an error's words are the same as they were here.
pub fn write_json(path: &Path, fields: &impl Serialize) -> Result<()> {
    Ok(guesthouse_basics::json_file::write_json(path, fields)?)
}

/// A file of fields, which is empty when missing or bad. None of what these hold can stop the app.
fn read_fields(path: &Path) -> Fields {
    let text = fs::read_to_string(path).unwrap_or_default();
    match serde_json::from_str(&text) {
        Ok(Value::Object(fields)) => fields,
        _ => Fields::new(),
    }
}

/// What setup left behind, which is empty until it has run. The daemon reads this file too.
pub fn read_setup(context: &Context) -> Fields {
    read_fields(&context.paths.setup_file())
}

pub fn write_setup(context: &Context, changes: Fields) -> Result<()> {
    let mut fields = read_setup(context);
    fields.insert("version".into(), 1.into());
    fields.extend(changes);
    write_json(&context.paths.setup_file(), &fields)
}

pub fn read_prefs(context: &Context) -> Fields {
    read_fields(&context.paths.prefs_file())
}

pub fn write_prefs(context: &Context, changes: Fields) -> Result<()> {
    let mut fields = read_prefs(context);
    fields.extend(changes);
    write_json(&context.paths.prefs_file(), &fields)
}

/// The Windows driver's files, which setup took from the installer.
pub fn driver_folder(context: &Context) -> PathBuf {
    let setup = read_setup(context);
    let folder = setup.get("driver_folder").and_then(Value::as_str).map(PathBuf::from);
    folder.unwrap_or_else(|| context.paths.store.join(DRIVER_FOLDER))
}

/// The card's size in whole GiB, less what the host keeps.
fn video_budget_gib(dedicated: u64) -> u64 {
    let card = dedicated.div_ceil(GIB);
    card.saturating_sub((card / HOST_SHARE).max(HOST_MINIMUM_GIB))
}

/// The same for a card of this many MiB, as a Linux guest's device has to be told.
pub(crate) fn video_budget_gib_of(card_mib: u64) -> u32 {
    video_budget_gib(card_mib * MIB) as u32
}

/// What the daemon offers with no cap: the host's desktop needs the rest. 0 until setup knows the card.
pub fn max_vram_gib(context: &Context) -> u32 {
    let mib = read_setup(context).get(CARD_MEMORY_FIELD).and_then(Value::as_u64).unwrap_or(0);
    video_budget_gib(mib * MIB) as u32
}

/// The same for this machine's card. A Windows machine given an NVIDIA card by its address has that
/// card's, and 0 until setup knows it. Every other machine has setup's card's, as it always had.
pub fn max_vram_gib_for(context: &Context, machine: &Machine) -> u32 {
    match (machine.windows_maker(), machine.windows_card_address()) {
        (Some(GpuMaker::Nvidia), Some(pci)) => video_budget_gib(card_memory_mib(context, pci) * MIB) as u32,
        _ => max_vram_gib(context),
    }
}

/// The memory in MiB setup wrote down for the NVIDIA card at `pci`. A setup from before it wrote down
/// every card's knows only its own card's.
fn card_memory_mib(context: &Context, pci: &str) -> u64 {
    let setup = read_setup(context);
    let own_card = setup.get("card").and_then(|card| card.get("pci")).and_then(Value::as_str) == Some(pci);
    let memories = setup.get(CARD_MEMORIES_FIELD).and_then(|memories| memories.get(pci));
    let memory = memories.or_else(|| setup.get(CARD_MEMORY_FIELD).filter(|_| own_card));
    memory.and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};
    use serde_json::json;

    fn fields(value: Value) -> Fields {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn what_is_remembered_never_stops_the_app() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert!(read_prefs(&context).is_empty());
        write_prefs(&context, fields(json!({"width": 900, "monitor": "DP-1"}))).unwrap();
        write_prefs(&context, fields(json!({"width": 800}))).unwrap();
        assert_eq!(Value::Object(read_prefs(&context)), json!({"width": 800, "monitor": "DP-1"}));
        for bad in ["[1]", "{"] {
            fs::write(context.paths.prefs_file(), bad).unwrap();
            assert!(read_prefs(&context).is_empty());
        }
    }

    #[test]
    fn the_cap_follows_the_card_setup_found() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(max_vram_gib(&context), 0, "no card known yet");
        write_setup(&context, fields(json!({CARD_MEMORY_FIELD: 16 * 1024}))).unwrap();
        assert_eq!(max_vram_gib(&context), 14);
        write_setup(&context, fields(json!({CARD_MEMORY_FIELD: 32607}))).unwrap();
        assert_eq!(max_vram_gib(&context), 28, "28 GiB on a 32 GiB card");
    }

    #[test]
    fn a_machine_on_another_nvidia_card_has_that_cards_cap() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let card = json!({"pci": "0000:01:00.0"});
        write_setup(&context, fields(json!({"card": card, CARD_MEMORY_FIELD: 16376}))).unwrap();
        let on_setups = Machine::new("w", "W", "/d", "/v");
        let mut on_second = on_setups.clone();
        on_second.use_windows_card(GpuMaker::Nvidia, "0000:02:00.0");
        let mut on_first = on_setups.clone();
        on_first.use_windows_card(GpuMaker::Nvidia, "0000:01:00.0");
        assert_eq!(max_vram_gib_for(&context, &on_setups), 14);
        assert_eq!(max_vram_gib_for(&context, &on_first), 14, "setup's card by its address");
        assert_eq!(max_vram_gib_for(&context, &on_second), 0, "an old setup doesn't know the card");
        let memories = json!({"0000:01:00.0": 16376, "0000:02:00.0": 6144});
        write_setup(&context, fields(json!({CARD_MEMORIES_FIELD: memories}))).unwrap();
        assert_eq!(max_vram_gib_for(&context, &on_second), 4, "a 6 GiB card keeps 2 for the host");
        assert_eq!(max_vram_gib_for(&context, &on_setups), 14);
        let mut on_amd = on_setups;
        on_amd.use_windows_card(GpuMaker::Amd, "0000:02:00.0");
        assert_eq!(max_vram_gib_for(&context, &on_amd), 14, "AMD's is as it was");
    }
}
