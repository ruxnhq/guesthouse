//! Live readings from the host's Intel card (`xe` sysfs, hwmon, and memory queries), published to
//! `guesthouse-sensors` for the performance overlay by the card's sensors publisher, a process of its
//! own that serves no guest and runs in no jail. Whole-host engine load is read by the app from
//! `/proc/*/fdinfo`.

use crate::ioctl::intel_render_node;
use crate::xe_query::{self, MemoryRegion};
use guesthouse_backend::PciAddress;
use guesthouse_sensors::words::wait_for_writers_turn;
use guesthouse_sensors::{SharedSnapshot, Snapshot, milliseconds_since_epoch, snapshot_path_for};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// The publisher for the Intel card at `card`, or with none the one setup chose: waits for the
/// writer's turn, then reads the card and publishes for as long as this process runs. Returns only
/// if the card can't be found.
pub fn publish(card: Option<&PciAddress>) -> io::Result<()> {
    let address = card.map(PciAddress::to_string);
    let node = intel_render_node(address.as_deref())
        .ok_or_else(|| io::Error::other("no Intel card on the xe driver at that address"))?;
    let path = snapshot_path_for(address.as_deref());
    let _turn = wait_for_writers_turn(&path)?;
    read_forever(&SharedSnapshot::open(&path)?, &node.sysfs, &node.device_file);
    Ok(())
}

fn read_forever(snapshot: &SharedSnapshot, sysfs: &Path, device_file: &Path) {
    let drm = File::open(device_file).ok();
    let energy_path = find_card_energy(sysfs);
    let act_freq_path = find_gt_freq(sysfs, "act_freq");
    let cur_freq_path = find_gt_freq(sysfs, "cur_freq");

    let mut prev_energy = energy_path.as_deref().and_then(read_u64).map(|v| (Instant::now(), v));

    loop {
        thread::sleep(SAMPLE_INTERVAL);
        let now = Instant::now();

        let power_milliwatts = energy_path.as_deref().and_then(read_u64).and_then(|uj| {
            let prev = prev_energy.replace((now, uj))?;
            compute_power_mw(prev, (now, uj))
        });

        // `act_freq` reads 0 when GT0 is in RC6; fall back to `cur_freq` so idle stays at min_freq.
        let graphics_megahertz = act_freq_path
            .as_deref()
            .and_then(read_u32)
            .filter(|&mhz| mhz > 0)
            .or_else(|| cur_freq_path.as_deref().and_then(read_u32));

        let (memory_used_bytes, memory_total_bytes) =
            drm.as_ref().and_then(query_vram).map_or((None, None), |(used, total)| (Some(used), Some(total)));

        let reading = Snapshot {
            taken_at: milliseconds_since_epoch(),
            power_milliwatts,
            graphics_megahertz,
            memory_used_bytes,
            memory_total_bytes,
            ..Snapshot::default()
        };
        snapshot.publish(&reading);
    }
}

fn find_gt_freq(sysfs: &Path, name: &str) -> Option<PathBuf> {
    let candidate = sysfs.join("tile0/gt0/freq0").join(name);
    candidate.exists().then_some(candidate)
}

/// Finds `hwmon/hwmon*/energy*_input` whose paired `energy*_label` reads `"card"`.
fn find_card_energy(sysfs: &Path) -> Option<PathBuf> {
    let hwmon_dir = fs::read_dir(sysfs.join("hwmon")).ok()?;
    for hwmon in hwmon_dir.flatten() {
        let dir = hwmon.path();
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else { continue };
            if let Some(stem) = name_str.strip_suffix("_label")
                && fs::read_to_string(entry.path()).ok().is_some_and(|s| s.trim() == "card")
            {
                let input = dir.join(format!("{stem}_input"));
                if input.exists() {
                    return Some(input);
                }
            }
        }
    }
    None
}

fn query_vram(file: &File) -> Option<(u64, u64)> {
    let answer = xe_query::ask(file, xe_query::kind::MEMORY_REGIONS).ok()?;
    vram_usage(&MemoryRegion::parse_all(&answer))
}

fn vram_usage(regions: &[MemoryRegion]) -> Option<(u64, u64)> {
    let mut used = 0u64;
    let mut total = 0u64;
    for region in regions {
        if region.class == MemoryRegion::VRAM {
            used = used.saturating_add(region.used);
            total = total.saturating_add(region.total);
        }
    }
    (total > 0).then_some((used, total))
}

fn compute_power_mw(prev: (Instant, u64), curr: (Instant, u64)) -> Option<u32> {
    let dt_ms = curr.0.saturating_duration_since(prev.0).as_millis() as u64;
    if dt_ms == 0 || curr.1 < prev.1 {
        return None;
    }
    let delta_uj = curr.1 - prev.1;
    // microjoules / milliseconds == milliwatts.
    u32::try_from(delta_uj / dt_ms).ok()
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_u32(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_mw_from_energy_uj() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(compute_power_mw((t0, 10_000_000), (t1, 75_000_000)), Some(130_000));
        assert_eq!(compute_power_mw((t0, 10_000_000), (t1, 5_000_000)), None);
    }

    #[test]
    fn vram_usage_sums_only_vram_regions() {
        let regions = [
            MemoryRegion {
                class: MemoryRegion::SYSTEM,
                instance: 0,
                min_page_size: 4096,
                total: 32 << 30,
                used: 1 << 30,
                cpu_visible: 0,
            },
            MemoryRegion {
                class: MemoryRegion::VRAM,
                instance: 0,
                min_page_size: 4096,
                total: 12 << 30,
                used: 2 << 30,
                cpu_visible: 12 << 30,
            },
        ];
        assert_eq!(vram_usage(&regions), Some((2 << 30, 12 << 30)));
        assert_eq!(vram_usage(&regions[..1]), None);
    }
}
