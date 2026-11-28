//! The host card's readings, as the card's sensors publisher writes them. The viewer shows the same
//! figures a guest is told, and nothing in the app has to load the vendor's library. The app only
//! reads them.

use crate::context::Context;
use crate::gpu_maker::GpuMaker;
use crate::gpu_sample::{CardReader, Sample};
use crate::machine::Machine;
use crate::settings::read_setup;
use crate::setup::cards::normal_address;
use guesthouse_sensors::{SharedSnapshot, Snapshot, SnapshotReader, snapshot_path_for};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const BYTES_PER_MIB: u64 = 1 << 20;
const MILLIWATTS_PER_WATT: f64 = 1000.0;
const XE_ENGINES: [&str; 5] = ["rcs", "ccs", "bcs", "vcs", "vecs"];

pub struct HostSensors {
    shared: SharedSnapshot,
    pci: Option<String>,
    prev_fdinfo: Option<EngineTotals>,
}

impl HostSensors {
    /// Opens the shared file at `path`, which [`sensors_file`] names. `None` means nothing can be
    /// read, which the overlay says as much.
    pub fn open_at(path: &Path) -> Option<Box<dyn CardReader>> {
        let shared = SharedSnapshot::open_to_read(path).ok()?;
        let pci = pci_from_snapshot_path(path);
        Some(Box::new(Self { shared, pci, prev_fdinfo: None }))
    }
}

fn pci_from_snapshot_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    // `gpupv-sensors-<uid>-<slot>-<pci>`
    let candidate = name.rsplit('-').next()?;
    candidate.contains(':').then(|| candidate.to_owned())
}

/// Where this machine's card's readings are published. They go by the address of the
/// NVIDIA card it opens: the one the machine was given, or else setup's, as the daemon reads it.
pub fn sensors_file(context: &Context, machine: &Machine) -> PathBuf {
    let given = machine
        .windows_card_address()
        .filter(|_| matches!(machine.windows_maker(), Some(GpuMaker::Nvidia | GpuMaker::Intel)));
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
        let mut sample = self.shared.latest().as_ref().map_or_else(Sample::default, sample_of);
        if sample.load_percent.is_none()
            && (sample.power_watts.is_some()
                || sample.clock_mhz.is_some()
                || sample.memory_total_mib.is_some())
        {
            let curr = read_xe_fdinfo(self.pci.as_deref());
            if let Some(prev) = self.prev_fdinfo.as_ref() {
                sample.load_percent = compute_fdinfo_load(prev, &curr);
            }
            self.prev_fdinfo = Some(curr);
        }
        sample
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct EngineSample {
    cycles: u64,
    total_cycles: u64,
    capacity: u64,
}

type EngineTotals = HashMap<&'static str, EngineSample>;

fn read_xe_fdinfo(pci: Option<&str>) -> EngineTotals {
    let mut totals: EngineTotals = HashMap::new();
    let mut seen_clients = HashSet::new();
    let Ok(procs) = fs::read_dir("/proc") else { return totals };
    for proc_entry in procs.flatten() {
        let name = proc_entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        if !name_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = fs::read_dir(proc_entry.path().join("fdinfo")) else { continue };
        for fd_entry in fds.flatten() {
            let Ok(text) = fs::read_to_string(fd_entry.path()) else { continue };
            if !text.contains("drm-driver:") || !text.contains("xe") {
                continue;
            }
            if let Some(parsed) = parse_xe_fdinfo(&text, pci)
                && seen_clients.insert(parsed.client_id)
            {
                for (engine, sample) in parsed.engines {
                    let entry = totals.entry(engine).or_default();
                    entry.cycles = entry.cycles.saturating_add(sample.cycles);
                    entry.total_cycles = entry.total_cycles.max(sample.total_cycles);
                    entry.capacity = entry.capacity.max(sample.capacity);
                }
            }
        }
    }
    totals
}

struct ParsedFdinfo {
    client_id: u64,
    engines: EngineTotals,
}

fn parse_xe_fdinfo(text: &str, expected_pci: Option<&str>) -> Option<ParsedFdinfo> {
    let mut is_xe = false;
    let mut client_id = None;
    let mut pdev_ok = expected_pci.is_none();
    let mut engines: EngineTotals = HashMap::new();

    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else { continue };
        let val = val.trim();
        match key {
            "drm-driver" => is_xe = val == "xe",
            "drm-client-id" => client_id = val.parse().ok(),
            "drm-pdev" => {
                if let Some(pci) = expected_pci {
                    pdev_ok = val.eq_ignore_ascii_case(pci);
                }
            }
            _ => {
                for &engine in &XE_ENGINES {
                    if key.strip_prefix("drm-cycles-") == Some(engine) {
                        if let Ok(v) = val.parse() {
                            engines.entry(engine).or_default().cycles = v;
                        }
                    } else if key.strip_prefix("drm-total-cycles-") == Some(engine) {
                        if let Ok(v) = val.parse() {
                            engines.entry(engine).or_default().total_cycles = v;
                        }
                    } else if key.strip_prefix("drm-engine-capacity-") == Some(engine)
                        && let Ok(v) = val.parse()
                    {
                        engines.entry(engine).or_default().capacity = v;
                    }
                }
            }
        }
    }

    if !is_xe || !pdev_ok {
        return None;
    }
    Some(ParsedFdinfo { client_id: client_id?, engines })
}

fn compute_fdinfo_load(prev: &EngineTotals, curr: &EngineTotals) -> Option<u32> {
    let mut max_pct: Option<u32> = None;
    for &engine in &XE_ENGINES {
        let (Some(p), Some(c)) = (prev.get(engine), curr.get(engine)) else { continue };
        if c.total_cycles <= p.total_cycles || c.cycles < p.cycles {
            continue;
        }
        let dt = c.total_cycles - p.total_cycles;
        let dc = c.cycles - p.cycles;
        let cap = c.capacity.max(1);
        let denom = dt.saturating_mul(cap);
        if denom == 0 {
            continue;
        }
        let pct = ((dc.saturating_mul(100)) / denom).min(100) as u32;
        max_pct = Some(max_pct.map_or(pct, |m| m.max(pct)));
    }
    max_pct
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
        let mut on_intel = on_setups.clone();
        on_intel.use_windows_card(GpuMaker::Intel, "0000:03:00.0");
        assert_eq!(sensors_file(&context, &on_intel), snapshot_path_for(Some("0000:03:00.0")));
        let mut on_amd = on_setups;
        on_amd.use_windows_card(GpuMaker::Amd, "0000:7c:00.0");
        assert_eq!(sensors_file(&context, &on_amd), snapshot_path_for(Some("0000:01:00.0")), "as before");
    }

    #[test]
    fn a_reading_the_host_has_not_got_stays_unknown() {
        let sample = sample_of(&Snapshot::default());
        assert_eq!(sample, Sample::default());
    }

    #[test]
    fn xe_fdinfo_computes_host_engine_load() {
        let sample_a = "\
drm-driver:\txe
drm-client-id:\t6
drm-pdev:\t0000:03:00.0
drm-cycles-rcs:\t1000
drm-total-cycles-rcs:\t10000
drm-cycles-vcs:\t0
drm-total-cycles-vcs:\t10000
drm-engine-capacity-vcs:\t2
";
        let sample_b = "\
drm-driver:\txe
drm-client-id:\t6
drm-pdev:\t0000:03:00.0
drm-cycles-rcs:\t3500
drm-total-cycles-rcs:\t20000
drm-cycles-vcs:\t0
drm-total-cycles-vcs:\t20000
drm-engine-capacity-vcs:\t2
";
        let prev = parse_xe_fdinfo(sample_a, Some("0000:03:00.0")).unwrap().engines;
        let curr = parse_xe_fdinfo(sample_b, Some("0000:03:00.0")).unwrap().engines;
        assert_eq!(compute_fdinfo_load(&prev, &curr), Some(25));
        assert!(parse_xe_fdinfo(sample_a, Some("0000:04:00.0")).is_none());
        assert_eq!(
            pci_from_snapshot_path(Path::new("/dev/shm/gpupv-sensors-1000-2-0000:03:00.0")).as_deref(),
            Some("0000:03:00.0")
        );
    }
}
