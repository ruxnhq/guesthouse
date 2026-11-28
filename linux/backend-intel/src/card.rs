//! Everything this stage learns about the card: sysfs, and xe's read-only queries.
//!
//! Reading it asks the driver questions and nothing else. A query that fails is written down in
//! [`CardFacts::unanswered`] and the rest still go, so the probe can show what a kernel lacks.

use crate::hwconfig::{self, Hwconfig};
use crate::ioctl::RenderNode;
use crate::protocol::shape::Core;
use crate::sysfs::{self, Frequencies, PciIdentity};
use crate::xe_query::{self, Config, Engine, FirmwareVersion, Gt, MemoryRegion, TopologyMask, kind};
use guesthouse_backend::AdapterEngines;
use std::fs::{File, OpenOptions};
use std::io;

/// The smallest page a GPU maps, 4 KiB.
const SMALL_PAGE: u32 = 0x1000;

/// What the card and its driver say about themselves.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CardFacts {
    pub address: Option<String>,
    pub driver: String,
    pub kernel: Option<String>,
    pub xe_source: Option<String>,
    pub pci: PciIdentity,
    pub config: Config,
    pub regions: Vec<MemoryRegion>,
    pub engines: Vec<Engine>,
    pub gts: Vec<Gt>,
    pub topology: Vec<TopologyMask>,
    pub hwconfig: Hwconfig,
    pub guc: Option<FirmwareVersion>,
    pub huc: Option<FirmwareVersion>,
    pub frequencies: Frequencies,
    /// The queries the driver didn't answer, and what it said.
    pub unanswered: Vec<(&'static str, String)>,
}

impl CardFacts {
    /// Reads sysfs and asks xe every query this stage uses.
    pub fn read(node: &RenderNode) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(&node.device_file)?;
        let mut facts = Self {
            address: sysfs::pci_address(&node.sysfs),
            driver: node.driver.clone(),
            kernel: sysfs::kernel_release(),
            xe_source: sysfs::xe_source_version(),
            pci: PciIdentity::read(&node.sysfs),
            frequencies: Frequencies::read(&node.sysfs),
            ..Self::default()
        };
        if let Some(answer) = facts.ask(&file, "config", kind::CONFIG) {
            facts.config = Config::parse(&answer);
        }
        if let Some(answer) = facts.ask(&file, "memory regions", kind::MEMORY_REGIONS) {
            facts.regions = MemoryRegion::parse_all(&answer);
        }
        if let Some(answer) = facts.ask(&file, "engines", kind::ENGINES) {
            facts.engines = Engine::parse_all(&answer);
        }
        if let Some(answer) = facts.ask(&file, "GT list", kind::GT_LIST) {
            facts.gts = Gt::parse_all(&answer);
        }
        if let Some(answer) = facts.ask(&file, "GT topology", kind::GT_TOPOLOGY) {
            facts.topology = TopologyMask::parse_all(&answer);
        }
        if let Some(answer) = facts.ask(&file, "hwconfig", kind::HWCONFIG) {
            facts.hwconfig = Hwconfig::parse(&answer);
        }
        facts.guc = facts.firmware(&file, "GuC version", FirmwareVersion::GUC);
        facts.huc = facts.firmware(&file, "HuC version", FirmwareVersion::HUC);
        Ok(facts)
    }

    fn ask(&mut self, file: &File, what: &'static str, kind: u32) -> Option<Vec<u8>> {
        xe_query::ask(file, kind).map_err(|error| self.unanswered.push((what, error.to_string()))).ok()
    }

    fn firmware(&mut self, file: &File, what: &'static str, firmware: u16) -> Option<FirmwareVersion> {
        let mut buffer = FirmwareVersion::request(firmware);
        let asked = xe_query::ask_into(file, kind::FIRMWARE_VERSION, &mut buffer);
        asked.map_err(|error| self.unanswered.push((what, error.to_string()))).ok()?;
        Some(FirmwareVersion::parse(&buffer))
    }

    fn gt_of(&self, kind: u16) -> Option<&Gt> {
        self.gts.iter().find(|gt| gt.kind == kind)
    }

    /// The GT that runs graphics and compute.
    pub fn main_gt(&self) -> Option<&Gt> {
        self.gt_of(Gt::MAIN)
    }

    pub fn media_gt(&self) -> Option<&Gt> {
        self.gt_of(Gt::MEDIA)
    }

    /// All the system memory the card can reach, in bytes.
    pub fn system_memory(&self) -> u64 {
        let system = self.regions.iter().filter(|region| region.class == MemoryRegion::SYSTEM);
        system.map(|region| region.total).sum()
    }

    /// The memory on the card itself, in bytes. An integrated card has none.
    pub fn card_memory(&self) -> u64 {
        self.card_regions().map(|region| region.total).sum()
    }

    /// How much of the card's own memory the processor can reach, in bytes.
    pub fn reachable_card_memory(&self) -> u64 {
        self.card_regions().map(|region| region.cpu_visible).sum()
    }

    /// Whether memory on the card takes 4 KiB pages: xe gives every region of it that size as its
    /// least. A card with none has no such memory.
    pub fn small_pages_on_card(&self) -> bool {
        let mut regions = self.card_regions().peekable();
        regions.peek().is_some() && regions.all(|region| region.min_page_size <= SMALL_PAGE)
    }

    fn card_regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions.iter().filter(|region| region.class == MemoryRegion::VRAM)
    }

    /// The main GT's masks of these kinds, together.
    fn main_mask_union(&self, kinds: &[u16]) -> Vec<u8> {
        let Some(main) = self.main_gt() else { return Vec::new() };
        let masks = self.topology.iter().filter(|mask| mask.gt == main.id && kinds.contains(&mask.kind));
        let mut union = Vec::<u8>::new();
        for mask in masks {
            union.resize(union.len().max(mask.mask.len()), 0);
            union.iter_mut().zip(&mask.mask).for_each(|(into, byte)| *into |= byte);
        }
        union
    }

    /// The bits set in the main GT's masks of these kinds, together.
    fn main_mask_bits(&self, kinds: &[u16]) -> u32 {
        self.main_mask_union(kinds).iter().map(|byte| byte.count_ones()).sum()
    }

    /// Dual subslices the main GT has, for geometry or compute.
    pub fn dual_subslices(&self) -> u32 {
        self.main_mask_bits(&[TopologyMask::DSS_GEOMETRY, TopologyMask::DSS_COMPUTE])
    }

    /// EUs in each dual subslice, of whichever width the card has.
    pub fn eus_per_dual_subslice(&self) -> u32 {
        self.main_mask_bits(&[TopologyMask::EU_PER_DSS, TopologyMask::SIMD16_EU_PER_DSS])
    }

    pub fn eu_count(&self) -> u32 {
        self.dual_subslices().saturating_mul(self.eus_per_dual_subslice())
    }

    /// How many dual subslices a slice holds, when hwconfig says how many slices and dual subslices
    /// the platform has.
    pub fn dual_subslices_per_slice(&self) -> Option<u32> {
        let slices = self.hwconfig.value(hwconfig::MAX_SLICES_SUPPORTED).filter(|&count| count > 0)?;
        let most = self.hwconfig.most_cores()?;
        Some((most / slices).max(1))
    }

    /// The slices that hold the dual subslices there are, when hwconfig says how many fit in one.
    pub fn slices(&self) -> Option<u32> {
        Some(self.dual_subslices().div_ceil(self.dual_subslices_per_slice()?))
    }

    /// The main GT's Xe cores, each in the slice its bit falls in and at its place along that slice,
    /// with the EUs the card gives every core. Empty when hwconfig doesn't say how many fit in a slice.
    pub fn cores(&self) -> Vec<Core> {
        let Some(per_slice) = self.dual_subslices_per_slice() else { return Vec::new() };
        let cores = self.main_mask_union(&[TopologyMask::DSS_GEOMETRY, TopologyMask::DSS_COMPUTE]);
        let eus = self.main_mask_union(&[TopologyMask::EU_PER_DSS, TopologyMask::SIMD16_EU_PER_DSS]);
        let eu_count = eus.iter().map(|byte| byte.count_ones()).sum();
        let eu_mask = eus.iter().take(4).rev().fold(0u32, |mask, &byte| mask << 8 | u32::from(byte));
        let bits = cores.iter().enumerate().flat_map(|(at, &byte)| {
            (0..8u32).filter(move |bit| byte & (1 << bit) != 0).map(move |bit| at as u32 * 8 + bit)
        });
        bits.map(|bit| Core { slice: bit / per_slice, index: bit % per_slice, eu_count, eu_mask }).collect()
    }

    /// How many engines of a class the main GT has.
    pub fn engines_of(&self, class: u16) -> u32 {
        let main = self.main_gt().map_or(0, |gt| gt.id);
        self.engines.iter().filter(|engine| engine.class == class && engine.gt == main).count() as u32
    }

    /// The engines of a class on any GT, a bit for each by its instance. Video engines are on the
    /// media GT.
    pub fn instance_mask(&self, class: u16) -> u32 {
        let instances = self.engines.iter().filter(|engine| engine.class == class);
        instances.filter(|engine| engine.instance < 32).fold(0, |mask, engine| mask | 1 << engine.instance)
    }

    /// Which families of engine the card has.
    pub fn engine_families(&self) -> AdapterEngines {
        let has = |class| self.engines.iter().any(|engine| engine.class == class);
        AdapterEngines {
            graphics: has(Engine::RENDER),
            compute: has(Engine::COMPUTE),
            copy: has(Engine::COPY),
            video: has(Engine::VIDEO_DECODE),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hwconfig::tests::table;
    use crate::xe_query::IpVersion;

    /// A card shaped like an Arc 140V, with numbers chosen for the tests: eight dual subslices of
    /// eight EUs, one slice, and a 19.2 MHz timestamp.
    pub fn lunar_lake() -> CardFacts {
        let gt = |kind, id, ip| Gt {
            kind,
            tile: 0,
            id,
            reference_clock: 19_200_000,
            near_memory: 1,
            far_memory: 0,
            ip,
        };
        let engine = |class, gt| Engine { class, instance: 0, gt };
        let mask = |kind, mask: &[u8]| TopologyMask { gt: 0, kind, mask: mask.to_vec() };
        CardFacts {
            address: Some("0000:00:02.0".into()),
            driver: "xe".into(),
            pci: PciIdentity { vendor: 0x8086, device: 0x64A0, revision: 4, ..PciIdentity::default() },
            config: Config { device_id: 0x64A0, revision: 4, va_bits: 48, ..Config::default() },
            regions: vec![MemoryRegion {
                class: MemoryRegion::SYSTEM,
                instance: 0,
                min_page_size: 0x1000,
                total: 32 << 30,
                used: 0,
                cpu_visible: 0,
            }],
            engines: vec![
                engine(Engine::RENDER, 0),
                engine(Engine::COPY, 0),
                engine(Engine::COMPUTE, 0),
                engine(Engine::VIDEO_DECODE, 1),
            ],
            gts: vec![
                gt(Gt::MAIN, 0, IpVersion { major: 20, minor: 4, revision: 4 }),
                gt(Gt::MEDIA, 1, IpVersion { major: 20, minor: 0, revision: 4 }),
            ],
            topology: vec![
                mask(TopologyMask::DSS_GEOMETRY, &[0xFF, 0]),
                mask(TopologyMask::DSS_COMPUTE, &[0xFF, 0]),
                mask(TopologyMask::SIMD16_EU_PER_DSS, &[0xFF]),
            ],
            hwconfig: Hwconfig::parse(&table(&[
                (hwconfig::MAX_SLICES_SUPPORTED, 1),
                (hwconfig::MAX_DUAL_SUBSLICES_SUPPORTED, 8),
                (hwconfig::MAX_NUM_EU_PER_DSS, 8),
                (hwconfig::NUM_THREADS_PER_EU, 8),
            ])),
            frequencies: Frequencies { highest: Some(2050), lowest: Some(400), ..Frequencies::default() },
            ..CardFacts::default()
        }
    }

    /// An Arc B580 as intel-probe printed one: twenty Xe cores of eight EUs in five slices, 12 GiB
    /// of its own, and a hwconfig table that counts subslices and has no dual subslice keys.
    pub fn battlemage() -> CardFacts {
        let gt = |kind, id, ip| Gt {
            kind,
            tile: 0,
            id,
            reference_clock: 19_200_000,
            near_memory: 2,
            far_memory: 1,
            ip,
        };
        let engine = |class, instance, gt| Engine { class, instance, gt };
        let mask = |kind, mask: &[u8]| TopologyMask { gt: 0, kind, mask: mask.to_vec() };
        let region = |class, instance, total, cpu_visible| MemoryRegion {
            class,
            instance,
            min_page_size: 0x1000,
            total,
            used: 0,
            cpu_visible,
        };
        CardFacts {
            address: Some("0000:03:00.0".into()),
            driver: "xe".into(),
            pci: PciIdentity { vendor: 0x8086, device: 0xE20B, revision: 0, ..PciIdentity::default() },
            config: Config { device_id: 0xE20B, revision: 0, flags: 0x3F, va_bits: 48, ..Config::default() },
            regions: vec![
                region(MemoryRegion::SYSTEM, 0, 31735 << 20, 0),
                region(MemoryRegion::VRAM, 1, 12216 << 20, 12216 << 20),
            ],
            engines: vec![
                engine(Engine::RENDER, 0, 0),
                engine(Engine::COPY, 0, 0),
                engine(Engine::COMPUTE, 0, 0),
                engine(Engine::VIDEO_DECODE, 0, 1),
                engine(Engine::VIDEO_DECODE, 1, 1),
            ],
            gts: vec![
                gt(Gt::MAIN, 0, IpVersion { major: 20, minor: 1, revision: 0 }),
                gt(Gt::MEDIA, 1, IpVersion { major: 13, minor: 1, revision: 1 }),
            ],
            topology: vec![
                mask(TopologyMask::DSS_GEOMETRY, &[0xFF, 0xFF, 0x0F, 0]),
                mask(TopologyMask::DSS_COMPUTE, &[0xFF, 0xFF, 0x0F, 0]),
                mask(TopologyMask::SIMD16_EU_PER_DSS, &[0xFF, 0]),
            ],
            hwconfig: Hwconfig::parse(&table(&[
                (hwconfig::MAX_SLICES_SUPPORTED, 5),
                (hwconfig::MAX_SUBSLICE, 20),
                (hwconfig::MAX_EU_PER_SUBSLICE, 8),
                (hwconfig::NUM_THREADS_PER_EU, 8),
            ])),
            frequencies: Frequencies { highest: Some(2850), lowest: Some(400), ..Frequencies::default() },
            ..CardFacts::default()
        }
    }

    #[test]
    fn a_table_that_counts_subslices_still_places_the_cores() {
        let card = battlemage();
        assert_eq!((card.dual_subslices(), card.eu_count(), card.slices()), (20, 160, Some(5)));
        let cores = card.cores();
        assert_eq!(cores.len(), 20);
        assert_eq!((cores[19].slice, cores[19].index), (4, 3), "four to a slice");
        assert_eq!((card.card_memory(), card.system_memory()), (12216 << 20, 31735 << 20));
    }

    #[test]
    fn eus_are_the_dual_subslices_times_their_eus() {
        let card = lunar_lake();
        assert_eq!((card.dual_subslices(), card.eus_per_dual_subslice(), card.eu_count()), (8, 8, 64));
        assert_eq!(card.slices(), Some(1));
        assert_eq!(card.system_memory(), 32 << 30);
        assert_eq!(card.media_gt().map(|gt| gt.ip.major), Some(20));
    }

    #[test]
    fn the_cores_are_the_bits_of_the_mask_in_their_slices() {
        let card = CardFacts {
            hwconfig: Hwconfig::parse(&table(&[
                (hwconfig::MAX_SLICES_SUPPORTED, 2),
                (hwconfig::MAX_DUAL_SUBSLICES_SUPPORTED, 16),
            ])),
            topology: vec![
                TopologyMask { gt: 0, kind: TopologyMask::DSS_COMPUTE, mask: vec![0b0000_0101, 0b1000_0000] },
                TopologyMask { gt: 0, kind: TopologyMask::EU_PER_DSS, mask: vec![0b0111_1111] },
            ],
            ..lunar_lake()
        };
        let at: Vec<(u32, u32)> = card.cores().iter().map(|core| (core.slice, core.index)).collect();
        assert_eq!(at, [(0, 0), (0, 2), (1, 7)]);
        assert!(card.cores().iter().all(|core| (core.eu_count, core.eu_mask) == (7, 0x7F)));
        assert_eq!(card.engines_of(Engine::COMPUTE), 1);
        assert_eq!(CardFacts { hwconfig: Hwconfig::default(), ..card }.cores(), []);
    }

    #[test]
    fn with_no_hwconfig_the_slices_are_not_guessed() {
        let card = CardFacts { hwconfig: Hwconfig::default(), ..lunar_lake() };
        assert_eq!(card.slices(), None);
    }

    #[test]
    fn engine_families_follow_the_engines_there_are() {
        let card = lunar_lake();
        let families = card.engine_families();
        assert!(families.graphics && families.compute && families.copy && families.video);
        let bare = CardFacts { engines: vec![Engine { class: Engine::RENDER, instance: 0, gt: 0 }], ..card };
        assert_eq!(bare.engine_families(), AdapterEngines { graphics: true, ..AdapterEngines::default() });
    }
}
