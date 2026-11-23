//! The whole card's figures, which count the host's desktop and every guest together. They are only
//! read while a machine runs on the card, so a sleeping card isn't woken each second to be asked.

use crate::usb::sysfs_text;
use guesthouse_sensors::Snapshot;
use std::path::Path;

/// amdgpu's files in the card's PCI folder: how busy the graphics engine and the video engine were
/// over the last moment, and its video memory.
const AMD_BUSY: &str = "gpu_busy_percent";
const AMD_VIDEO_BUSY: &str = "vcn_busy_percent";
const AMD_MEMORY_USED: &str = "mem_info_vram_used";
const AMD_MEMORY_TOTAL: &str = "mem_info_vram_total";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WholeCard {
    /// How much of the last moment the card spent busy, as its driver counts it.
    pub load_percent: Option<u32>,
    /// The same for its video engine, which encodes and decodes.
    pub video_percent: Option<u32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
}

fn number<T: std::str::FromStr>(file: &Path) -> Option<T> {
    sysfs_text(file).trim().parse().ok()
}

/// An AMD card, from its PCI folder in sysfs. A file an older kernel lacks is a reading it lacks.
pub fn amd(device: &Path) -> WholeCard {
    WholeCard {
        load_percent: number(&device.join(AMD_BUSY)),
        video_percent: number(&device.join(AMD_VIDEO_BUSY)),
        memory_used_bytes: number(&device.join(AMD_MEMORY_USED)),
        memory_total_bytes: number(&device.join(AMD_MEMORY_TOTAL)),
    }
}

/// An NVIDIA card, from the snapshot its daemons publish, which the overlay reads too. The snapshot
/// has no figure for the video engines.
pub fn nvidia(snapshot: Option<&Snapshot>) -> WholeCard {
    let Some(snapshot) = snapshot else { return WholeCard::default() };
    WholeCard {
        load_percent: snapshot.load_percent,
        video_percent: None,
        memory_used_bytes: snapshot.memory_used_bytes,
        memory_total_bytes: snapshot.memory_total_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn an_amd_card_is_read_from_its_files() {
        let scratch = Scratch::new();
        // As amdgpu wrote them for a Radeon 610M with a guest on it.
        scratch.write("card/gpu_busy_percent", "99\n");
        scratch.write("card/vcn_busy_percent", "0\n");
        scratch.write("card/mem_info_vram_used", "292540416\n");
        scratch.write("card/mem_info_vram_total", "2147483648\n");
        let card = amd(&scratch.path("card"));
        assert_eq!(
            card,
            WholeCard {
                load_percent: Some(99),
                video_percent: Some(0),
                memory_used_bytes: Some(292_540_416),
                memory_total_bytes: Some(2 << 30),
            }
        );
    }

    #[test]
    fn a_file_the_kernel_lacks_is_a_reading_it_lacks() {
        let scratch = Scratch::new();
        scratch.write("card/gpu_busy_percent", "12\n");
        let card = amd(&scratch.path("card"));
        assert_eq!(card, WholeCard { load_percent: Some(12), ..WholeCard::default() });
    }

    #[test]
    fn an_nvidia_card_is_the_snapshots_figures() {
        let snapshot =
            Snapshot { load_percent: Some(42), memory_used_bytes: Some(5 << 30), ..Snapshot::default() };
        let card = nvidia(Some(&snapshot));
        assert_eq!(
            (card.load_percent, card.memory_used_bytes, card.memory_total_bytes),
            (Some(42), Some(5 << 30), None)
        );
        assert_eq!(nvidia(None), WholeCard::default());
    }
}
