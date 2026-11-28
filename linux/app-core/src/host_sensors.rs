//! The host card's readings, as the daemon's sensor thread publishes them. The viewer shows the
//! same figures a guest is told, and nothing in the app has to load the vendor's library.

use crate::context::Context;
use crate::gpu_maker::GpuMaker;
use crate::gpu_sample::{CardReader, Sample};
use crate::machine::Machine;
use crate::settings::read_setup;
use crate::setup::cards::normal_address;
use guesthouse_sensors::{SharedSnapshot, Snapshot, SnapshotReader, snapshot_path_for};
use serde_json::Value;
use std::path::{Path, PathBuf};

const BYTES_PER_MIB: u64 = 1 << 20;
const MILLIWATTS_PER_WATT: f64 = 1000.0;

pub struct HostSensors(SharedSnapshot);

impl HostSensors {
    /// Opens the shared file at `path`, which [`sensors_file`] names. `None` means nothing can be
    /// read, which the overlay says as much.
    pub fn open_at(path: &Path) -> Option<Box<dyn CardReader>> {
        let shared = SharedSnapshot::open(path).ok()?;
        Some(Box::new(Self(shared)))
    }
}

/// Where the daemon of this machine publishes its card's readings. They go by the address of the
/// NVIDIA card it opens: the one the machine was given, or else setup's, as the daemon reads it.
pub fn sensors_file(context: &Context, machine: &Machine) -> PathBuf {
    let given = machine.windows_card_address().filter(|_| machine.windows_maker() == Some(GpuMaker::Nvidia));
    let setup = read_setup(context);
    let setups = setup.get("card").and_then(|card| card.get("pci")).and_then(Value::as_str);
    let card = given.or(setups).filter(|pci| !pci.trim().is_empty()).map(normal_address);
    snapshot_path_for(card.as_deref())
}

/// The figures the overlay shows, out of everything the host read.
pub fn sample_of(snapshot: &Snapshot) -> Sample {
    Sample {
        taken: std::time::Duration::ZERO,
        load_percent: snapshot.load_percent,
        power_watts: snapshot.power_milliwatts.map(|milliwatts| f64::from(milliwatts) / MILLIWATTS_PER_WATT),
        clock_mhz: snapshot.graphics_megahertz,
        memory_used_mib: snapshot.memory_used_bytes.map(|bytes| bytes / BYTES_PER_MIB),
        memory_total_mib: snapshot.memory_total_bytes.map(|bytes| bytes / BYTES_PER_MIB),
    }
}

impl CardReader for HostSensors {
    /// A snapshot too old to be true reads as nothing, as the sampler's own check does.
    fn sample(&mut self) -> Sample {
        self.0.latest().as_ref().map_or_else(Sample::default, sample_of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlays_units_come_from_the_snapshots() {
        let snapshot = Snapshot {
            load_percent: Some(42),
            power_milliwatts: Some(129_600),
            graphics_megahertz: Some(2400),
            memory_used_bytes: Some(5 << 30),
            memory_total_bytes: Some(32 << 30),
            ..Snapshot::default()
        };
        let sample = sample_of(&snapshot);
        assert_eq!(sample.load_percent, Some(42));
        assert_eq!(sample.power_watts, Some(129.6));
        assert_eq!(sample.clock_mhz, Some(2400));
        assert_eq!((sample.memory_used_mib, sample.memory_total_mib), (Some(5120), Some(32768)));
    }

    #[test]
    fn a_machine_reads_the_file_of_the_card_its_daemon_opens() {
        use crate::testing::{Scratch, scratch_context};
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let on_setups = Machine::new("w", "W", "/d", "/v");
        assert_eq!(
            sensors_file(&context, &on_setups),
            snapshot_path_for(None),
            "before setup kept an address"
        );
        let card = serde_json::json!({"card": {"pci": "0000:01:00.0"}});
        crate::settings::write_setup(&context, card.as_object().cloned().unwrap()).unwrap();
        assert_eq!(sensors_file(&context, &on_setups), snapshot_path_for(Some("0000:01:00.0")));
        let mut on_second = on_setups.clone();
        on_second.use_windows_card(GpuMaker::Nvidia, "0000:02:00.0");
        assert_eq!(sensors_file(&context, &on_second), snapshot_path_for(Some("0000:02:00.0")));
        let mut on_amd = on_setups;
        on_amd.use_windows_card(GpuMaker::Amd, "0000:7c:00.0");
        assert_eq!(sensors_file(&context, &on_amd), snapshot_path_for(Some("0000:01:00.0")), "as before");
    }

    #[test]
    fn a_reading_the_host_has_not_got_stays_unknown() {
        let sample = sample_of(&Snapshot::default());
        assert_eq!(sample, Sample::default());
    }
}
