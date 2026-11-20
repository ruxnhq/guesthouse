//! The adapter record: the answer to the private adapter query, KMTQAITYPE_UMDRIVERPRIVATE.
//!
//! A four-byte ask gets the number of linked adapters, which is one, and an ask of [`RECORD_SIZE`]
//! bytes gets the record. Any other size is refused, as the vendor's kernel driver wouldn't take it.
//!
//! The record is built once, from the card and its generation: the platform and core families,
//! the card's ids, clocks and shape, the feature table, and the GPU address partition. The shape of
//! the GPU includes its slice arrays, [`super::shape`]. For a card Intel's kernel driver knows, the
//! feature table, the platform words and the hwconfig fields are that driver's,
//! [`super::card_data`], with this backend's own flags on top. Compression is off, as the host can't
//! set up compressed memory yet, and the byte that marks the adapter as a virtual machine's is set.
//! The workaround table is the one Intel's kernel driver would give the card,
//! [`super::workarounds`]. What is left at zero is listed in [`super::gaps::RECORD`].

use super::bytes::{dword, put_dword, put_qword, put_word};
use super::card_data;
use super::features::{self, Feature};
use super::partition;
use super::shape::{self, Core};
use super::workarounds::{self, Outcome};
use guesthouse_backend::{Status, VendorQuery};

/// KMTQAITYPE_UMDRIVERPRIVATE.
pub const QUERY_TYPE: u32 = 0;
/// One adapter's record.
pub const RECORD_SIZE: usize = 0x2574;
/// The ask for how many linked adapters there are.
pub const COUNT_SIZE: usize = 4;
/// A paravirtual adapter is never linked to another.
const LINKED_ADAPTERS: u32 = 1;

/// The registry key the record names as the device's, under HKEY_LOCAL_MACHINE. Intel's OpenCL and
/// Level Zero drivers read it from the record and look in the guest's own registry for the OpenCL
/// driver's name there, and offer no device when it has none. On a real machine the kernel driver
/// names the adapter's key under the display class; in the guest that key belongs to Windows' virtual
/// render driver and its number isn't known here, so the key is this backend's choice, and the app
/// writes [`DEVICE_KEY_VALUES`] into it at start.
pub const DEVICE_KEY: &str = r"SOFTWARE\Guesthouse\Intel adapter";
/// The values the drivers look for in [`DEVICE_KEY`], as text.
pub const DEVICE_KEY_VALUES: [(&str, &str); 1] = [("OpenCLDriverName", "igdrcl64.dll")];
/// The bytes of the adapter's name field, its ending zero included.
const ADAPTER_NAME_SIZE: usize = 250;
/// How the kernel driver's path starts, which Intel's drivers cut off before opening the key.
const MACHINE_ROOT: &str = r"\REGISTRY\MACHINE\";

/// Where the record's fields are. The names are those of gmmlib's ADAPTER_INFO and the structures
/// in it, though the record's order is not that header's.
mod at {
    /// A word the guest writes in its ask, which goes back as it came.
    pub const BUILD_STAMP: usize = 0x004;
    /// The device's registry path, a string of bytes ending in zero.
    pub const DEVICE_REGISTRY_PATH: usize = 0x008;
    pub const PRODUCT_FAMILY: usize = 0x208;
    pub const PCH_FAMILY: usize = 0x20C;
    pub const DISPLAY_CORE: usize = 0x210;
    pub const RENDER_CORE: usize = 0x214;
    pub const PLATFORM_TYPE: usize = 0x218;
    pub const DEVICE_ID: usize = 0x21C;
    pub const REVISION: usize = 0x21E;
    /// The PCI subsystem vendor and subsystem, a word each.
    pub const SUBSYSTEM_VENDOR: usize = 0x220;
    pub const SUBSYSTEM: usize = 0x222;
    pub const GT_TYPE: usize = 0x228;
    pub const MEDIA_CORE: usize = 0x22C;
    /// The display, render and media IP versions, each an [`super::IpWord`].
    pub const DISPLAY_IP: usize = 0x230;
    pub const RENDER_IP: usize = 0x238;
    pub const MEDIA_IP: usize = 0x23C;
    /// The PCI revision again, a word.
    pub const REVISION_AGAIN: usize = 0x240;
    /// The core family of each IP block: the display's, then four dwords later the render's four
    /// times, then the media's four times.
    pub const DISPLAY_IP_CORE: usize = 0x244;
    pub const RENDER_IP_CORES: usize = 0x254;
    pub const MEDIA_IP_CORES: usize = 0x264;
    /// The dword of the feature table that holds the Direct3D 11 interface versions.
    pub const DDI_VERSIONS: usize = 0x364;
    pub const TIMESTAMP_FREQUENCY: usize = 0x998;
    pub const MIN_RENDER_FREQUENCY: usize = 0x9AC;
    pub const MAX_RENDER_FREQUENCY: usize = 0x9B0;
    pub const NUMBER_OF_EUS: usize = 0x9BC;
    pub const CACHE_LINE_SIZE: usize = 0x9E4;
    pub const DEDICATED_VIDEO_MEMORY: usize = 0x9F4;
    pub const SYSTEM_SHARED_MEMORY: usize = 0x9FC;
    pub const SYSTEM_VIDEO_MEMORY: usize = 0xA04;
    /// A string of [`ADAPTER_NAME_SIZE`] bytes, ending in zero. Intel's Vulkan driver copies it into
    /// its physical device's name.
    pub const ADAPTER_NAME: usize = 0xA1C;
    /// GT_SYSTEM_INFO.
    pub const EU_COUNT: usize = 0xB34;
    pub const THREAD_COUNT: usize = 0xB38;
    pub const SLICE_COUNT: usize = 0xB3C;
    pub const SUBSLICE_COUNT: usize = 0xB40;
    pub const DUAL_SUBSLICE_COUNT: usize = 0xB44;
    /// TotalVsThreads, then the hull, domain, geometry and pixel (windower range) stages' and
    /// TotalVsThreads_Pocs, a dword each.
    pub const STAGE_THREADS: usize = 0xB78;
    /// The most render, compute, video, video-enhancement and copy engines, a dword each. Only the
    /// fourth has an open name, MaxVECS. The order is the hwconfig keys' for the same five, which
    /// puts MaxVECS fourth too, and Intel's Vulkan driver reads the first for its graphics queues
    /// and the last for its transfer queues.
    pub const ENGINE_MAXIMA: usize = 0x21A0;
    pub const MAX_EU_PER_SUBSLICE: usize = 0xB94;
    pub const MAX_SLICES: usize = 0xB98;
    pub const MAX_SUBSLICES: usize = 0xB9C;
    pub const MAX_DUAL_SUBSLICES: usize = 0xBA0;
    /// The compute engines' enable mask and count, and the flag that says they are filled in.
    pub const COMPUTE_ENGINE_MASK: usize = 0x2078;
    pub const COMPUTE_ENGINE_COUNT: usize = 0x207C;
    pub const COMPUTE_ENGINES_VALID: usize = 0x2090;
    /// GMM_GFX_PARTITIONING, [`super::partition`].
    pub const PARTITION: usize = 0x23B8;
    /// NumThreadsPerEu, inside GT_SYSTEM_INFO.
    pub const THREADS_PER_EU: usize = 0x2110;
    /// The URB's size for the whole GPU in KB, inside GT_SYSTEM_INFO. A 3D driver divides it by
    /// [`SLICE_COUNT`] for a slice's share.
    pub const URB_SIZE: usize = 0x211C;
    /// The least and most URB entries of each stage, a dword each, least first.
    pub const VS_URB_ENTRIES: usize = 0x2120;
    pub const HS_URB_ENTRIES: usize = 0x2130;
    pub const GS_URB_ENTRIES: usize = 0x2138;
    pub const DS_URB_ENTRIES: usize = 0x2140;
    /// The task and mesh stages', in the order of the hwconfig keys.
    pub const TASK_URB_ENTRIES: usize = 0x2150;
    pub const MESH_URB_ENTRIES: usize = 0x2158;
    /// The highest URB starting address a stage may be given.
    pub const MAX_URB_STARTING_ADDRESS: usize = 0x2174;
    /// KMD_CAPS_INFO, the kernel driver's capability bits.
    pub const KERNEL_DRIVER_CAPS: usize = 0xB18;
    /// GT_VDBOX_INFO and GT_VEBOX_INFO, inside GT_SYSTEM_INFO and again in the record's own copy of
    /// them that Intel's Vulkan driver reads.
    pub const VDBOX_INFO: [usize; 2] = [0x209C, 0x2324];
    pub const VEBOX_INFO: [usize; 2] = [0x20C0, 0x2348];
    /// A byte that marks the adapter as a virtual machine's.
    pub const FROM_VIRTUAL_MACHINE: usize = 0x2468;
    /// A byte that marks that machine as a secure one.
    pub const SECURE_VIRTUAL_MACHINE: usize = 0x2469;
}

/// Inside GT_VDBOX_INFO: the enable mask, then the number enabled and the valid flag. The SFC
/// support bits between stay zero, as nothing here knows them.
const VDBOX_COUNT: usize = 0x1C;
const VDBOX_VALID: usize = 0x20;
/// The same inside GT_VEBOX_INFO.
const VEBOX_COUNT: usize = 0x14;
const VEBOX_VALID: usize = 0x18;

/// The workaround entry that is set for an adapter that is a virtual machine's, and its value.
/// Intel's kernel driver sets it only when asked for a virtual machine's record; this adapter always
/// is one, so the entry is set whatever the table gives. On every card the table covers it gives 1.
const VIRTUAL_MACHINE_WORKAROUND: (usize, u8) = (968, 1);
/// An unnamed entry, the top nibble of the record's dword at 0x4A8, that Intel's Direct3D 11 driver
/// reads as tiled resources at tier 2 or above. Direct3D 11 refuses a device at 12_0 or above from a
/// driver that says less, which is every device, the desktop compositor's included, so it is set
/// whatever the table gives. On every card the table covers it gives 1 too; a card it doesn't cover
/// still needs it.
const TILED_RESOURCES_WORKAROUND: (usize, u8) = (535, 1);
/// An unnamed entry, the top nibble of the record's byte at 0x817. With it set, Intel's Direct3D 11
/// driver takes the render colour cache to be keyed by binding table and index, as Intel's kernel
/// driver keys it on cards whose table sets it, and leaves out the render target cache flush the
/// hardware otherwise needs whenever a binding comes to name another surface. xe keys the cache by
/// index alone unless a queue is made keyed by binding table, so the entry is kept as the table
/// gives it only when every render queue is made so ([`RecordFacts::render_cache_keyed`]), and
/// cleared otherwise, so the driver flushes. Set over a cache keyed by index alone, draws into
/// several surfaces one after another can land in the wrong one, and the display driver stamps its
/// slots that way.
pub const RENDER_CACHE_KEYING_WORKAROUND: usize = 2287;
/// KMD_CAPS_INFO's DriverStoreEnabled: the driver runs from the driver store, as one from a DCH INF
/// always does, and the guest's copy is in its host driver store. Intel's Direct3D 12 driver loads
/// its shader compiler from its own folder only when this is set, and by bare name, from nowhere it
/// is, when it isn't: then every device fails.
const DRIVER_STORE_ENABLED: u32 = 1 << 10;
/// The compute engines the record's mask has room for.
const MAX_COMPUTE_ENGINES: u32 = 4;
/// The GPU address bits from which the card has five levels of page tables.
const FIVE_LEVEL_ADDRESS_BITS: u64 = 57;

/// GTTYPE_GT1, which Intel's kernel driver gives every card it knows.
const KNOWN_CARD_GT_TYPE: u32 = 0;

/// Feature bits the driver's table sets that this record keeps clear on every card.
const KEPT_CLEAR: [Feature; 9] = [
    // The guest's tiled resources: xe never turns on the hardware's translation table for them,
    // whether the kernel driver or the user-mode driver keeps it (see `work::choices`); the core has
    // no request that maps a range to nothing, which null pages are; and tiles mapped through the
    // page tables need remapping the core has not been shown to give.
    features::TILE_MAPPED_RESOURCE,
    features::TRANSLATION_TABLE,
    features::USER_MODE_TRANSLATION_TABLE,
    features::NULL_PAGES,
    // Compression, which stays off as [`features::COMPRESSION`] does: render compression for what
    // the display shows, and two unnamed bits that the table sets on an Arrow Lake card only when
    // it sets compression too.
    features::RENDER_COMPRESSION,
    features::bit("unnamed bit 684, which goes with compression", 684),
    features::bit("unnamed bit 686, which goes with compression", 686),
    // A service of the kernel driver rather than of the card: resetting the function, which the
    // host does.
    features::DRIVER_FLR,
    // A capability the Direct3D 12 driver offers on this bit's word. Offered on an adapter whose
    // display is another device's, as the guest's display always is, Windows then wants resources
    // shared across adapters, which the guest is never offered, and refuses to make a Direct3D 12
    // device there: on every adapter but the render-only one, the default adapter among them.
    CROSS_ADAPTER_CAPABILITY,
];

/// The hwconfig key for how many bits a bindless surface offset has. The record leaves its place
/// at zero on every card, where a real card says 26 or so. With it set Intel's Direct3D 12 driver
/// fills a heap of surface states in memory the guest's program was never given, and the program
/// dies; with zero it makes no such heap.
const BINDLESS_SURFACE_OFFSET_BITS_KEY: u32 = 60;

/// The unnamed bit [`KEPT_CLEAR`] keeps for Direct3D 12.
const CROSS_ADAPTER_CAPABILITY: Feature = features::bit("unnamed bit 505, a Direct3D 12 capability", 505);

/// The inputs to the driver's tables that are its defaults rather than the card's, as the log
/// names them.
const PCI_CAPABILITIES_DEFAULT: &str = "PCI capabilities as the platform's cards have them";
const DISPLAY_FUSES_DEFAULT: &str = "every display pipe present";
const CPU_DEFAULT: &str = "a CPU that is not a Xeon";
const FUSE_WORD_DEFAULT: &str = "a fuse word xe doesn't report, as zero";
const SUBSYSTEM_DEFAULT: &str = "a subsystem the driver doesn't single out";
/// The product family from which the driver reads that fuse word, Battlemage's.
const FUSE_WORD_FROM_FAMILY: u32 = 1274;
/// The devices for which the driver singles out some subsystems.
const SUBSYSTEM_DEVICES: [u16; 2] = [0x7D55, 0x7D51];

/// The bytes a CPU cache line takes on the hosts this runs on.
const CACHE_LINE_BYTES: u32 = 64;

/// What the record says the platform is. Families and types are gmmlib's enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Platform {
    pub product_family: u32,
    pub pch_family: u32,
    pub display_core: u32,
    pub render_core: u32,
    pub platform_type: u32,
    pub device_id: u16,
    pub revision: u16,
    pub gt_type: u32,
    pub render_ip: IpWord,
}

/// An IP version as the record packs it: architecture in bits 22 to 31, release in 14 to 21 and
/// revision in 0 to 5, as compute-runtime's HardwareIpVersion lays it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IpWord(pub u32);

impl IpWord {
    const ARCHITECTURE_SHIFT: u32 = 22;
    const RELEASE_SHIFT: u32 = 14;
    const RELEASE_MASK: u32 = 0xFF;
    const REVISION_MASK: u32 = 0x3F;

    pub fn new(architecture: u16, release: u16, revision: u16) -> Self {
        Self(
            u32::from(architecture) << Self::ARCHITECTURE_SHIFT
                | (u32::from(release) & Self::RELEASE_MASK) << Self::RELEASE_SHIFT
                | u32::from(revision) & Self::REVISION_MASK,
        )
    }

    pub fn architecture(self) -> u32 {
        self.0 >> Self::ARCHITECTURE_SHIFT
    }

    pub fn release(self) -> u32 {
        self.0 >> Self::RELEASE_SHIFT & Self::RELEASE_MASK
    }

    pub fn revision(self) -> u32 {
        self.0 & Self::REVISION_MASK
    }

    /// As the workaround table reads it, or none for a word that is all zero.
    fn workaround_ip(self) -> Option<workarounds::Ip> {
        let part = |value: u32| value as u16;
        (self.0 != 0).then(|| {
            workarounds::Ip::new(part(self.architecture()), part(self.release()), part(self.revision()))
        })
    }
}

/// The GPU's shape, as GT_SYSTEM_INFO counts it. Zero where the card didn't say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GtSystem {
    pub eu_count: u32,
    pub thread_count: u32,
    pub slice_count: u32,
    pub subslice_count: u32,
    pub dual_subslice_count: u32,
    pub max_eu_per_subslice: u32,
    pub max_slices: u32,
    pub max_subslices: u32,
    pub max_dual_subslices: u32,
    pub threads_per_eu: u32,
    /// The threads each 3D stage may have across the GPU, as the card's hwconfig gives them.
    pub stage_threads: StageThreads,
    pub engine_maxima: EngineMaxima,
}

/// The most engines of each kind the card can have, near GT_SYSTEM_INFO's end. Intel's Vulkan driver
/// counts its graphics queues from the render count, and with it at zero offered compute and video
/// queues alone. It reads the copy count for its transfer queues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EngineMaxima {
    pub render: u32,
    pub compute: u32,
    pub video: u32,
    pub video_enhance: u32,
    pub copy: u32,
}

/// GT_SYSTEM_INFO's thread counts for the 3D stages. A 3D driver sizes its pipeline from these, and
/// Intel's Vulkan driver offers no graphics queue with them all zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageThreads {
    pub vertex: u32,
    pub hull: u32,
    pub domain: u32,
    pub geometry: u32,
    pub pixel: u32,
    pub vertex_pocs: u32,
}

/// The URB, the on-chip memory the 3D stages pass vertices through, as GT_SYSTEM_INFO describes
/// it. A 3D driver lays its stages out in it from these, so they have to be the card's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Urb {
    /// For the whole GPU, in KB.
    pub size_kb: u32,
    pub vertex: UrbEntries,
    pub hull: UrbEntries,
    pub geometry: UrbEntries,
    pub domain: UrbEntries,
    pub task: UrbEntries,
    pub mesh: UrbEntries,
    pub max_starting_address: u32,
}

/// The least and most URB entries a stage may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UrbEntries {
    pub min: u32,
    pub max: u32,
}

impl Urb {
    fn stages(&self) -> [(usize, UrbEntries); 6] {
        [
            (at::VS_URB_ENTRIES, self.vertex),
            (at::HS_URB_ENTRIES, self.hull),
            (at::GS_URB_ENTRIES, self.geometry),
            (at::DS_URB_ENTRIES, self.domain),
            (at::TASK_URB_ENTRIES, self.task),
            (at::MESH_URB_ENTRIES, self.mesh),
        ]
    }
}

impl std::fmt::Display for Urb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let range = |entries: UrbEntries| format!("{}-{}", entries.min, entries.max);
        write!(
            f,
            "size_kb={} vs={} hs={} gs={} ds={} task={} mesh={} max_start={}",
            self.size_kb,
            range(self.vertex),
            range(self.hull),
            range(self.geometry),
            range(self.domain),
            range(self.task),
            range(self.mesh),
            self.max_starting_address
        )
    }
}

fn write_urb(record: &mut [u8], urb: &Urb) {
    put_dword(record, at::URB_SIZE, urb.size_kb);
    for (field, entries) in urb.stages() {
        put_dword(record, field, entries.min);
        put_dword(record, field + 4, entries.max);
    }
    put_dword(record, at::MAX_URB_STARTING_ADDRESS, urb.max_starting_address);
}

/// The URB as the record gives it.
pub fn urb(record: &[u8]) -> Urb {
    let read = |field| dword(record, field).unwrap_or(0);
    let entries = |field| UrbEntries { min: read(field), max: read(field + 4) };
    Urb {
        size_kb: read(at::URB_SIZE),
        vertex: entries(at::VS_URB_ENTRIES),
        hull: entries(at::HS_URB_ENTRIES),
        geometry: entries(at::GS_URB_ENTRIES),
        domain: entries(at::DS_URB_ENTRIES),
        task: entries(at::TASK_URB_ENTRIES),
        mesh: entries(at::MESH_URB_ENTRIES),
        max_starting_address: read(at::MAX_URB_STARTING_ADDRESS),
    }
}

/// Memory as the record describes it, in bytes. An integrated card has none of its own and shares
/// the system's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Memory {
    pub dedicated: u64,
    pub system_shared: u64,
    pub system_video: u64,
}

/// Everything the record is built from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecordFacts {
    pub platform: Platform,
    pub gt: GtSystem,
    /// The GPU timestamp's frequency, in Hz.
    pub timestamp_hz: u32,
    /// The adapter's name, which Intel's Vulkan driver copies for its device name.
    pub adapter_name: String,
    /// The render clock's least and most, in MHz.
    pub render_mhz: (u32, u32),
    pub memory: Memory,
    pub local_memory: bool,
    pub urb: Urb,
    /// The Xe cores, as [`shape::representable`] leaves them. The counts in [`GtSystem`] are of these.
    pub cores: Vec<Core>,
    /// The card's compute engines, and whether it has a copy engine.
    pub compute_engines: u32,
    pub copy_engine: bool,
    /// The video decode and video enhancement engines the card has, a bit for each by its instance.
    pub video_engines: u32,
    pub video_enhance_engines: u32,
    /// The bits of GPU address the card has.
    pub address_bits: u64,
    /// Whether the card tiles as Xe2 and later do.
    pub xe2_tiling: bool,
    /// The media GT's IP version, which the workaround table is built from. A card with no media
    /// GT has none.
    pub media_ip: Option<IpWord>,
    /// The PCI subsystem vendor and subsystem, and the PCI class.
    pub subsystem: (u16, u16),
    pub pci_class: u32,
    /// The card's hwconfig, each key with its first value.
    pub hwconfig: Vec<(u32, u32)>,
    /// Whether memory on the card may be mapped in 4 KiB pages.
    pub local_memory_small_pages: bool,
    /// Whether every queue on a render engine has its colour cache keyed by binding table and
    /// index: see [`RENDER_CACHE_KEYING_WORKAROUND`].
    pub render_cache_keyed: bool,
}

/// How the parts of the record that come from data were built, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telling {
    pub workarounds: Outcome,
    pub card_data: CardData,
}

/// Whether the feature table, platform words and hwconfig fields are the kernel driver's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardData {
    Built(DataBuilt),
    /// The data has no table for this device: those parts are this backend's alone.
    Uncovered {
        device: u16,
        revision: u8,
        feature_bits: u32,
    },
}

/// What the kernel driver's data gave the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataBuilt {
    pub platform: &'static str,
    pub device: u16,
    pub revision: u8,
    /// The feature table's set bits, this backend's own included.
    pub feature_bits: u32,
    /// The hwconfig keys copied into the record.
    pub hwconfig_keys: usize,
    /// The inputs that are defaults rather than the card's.
    pub defaults: Vec<&'static str>,
    /// What the data has no value for, left at zero.
    pub unknown: Vec<String>,
}

impl std::fmt::Display for DataBuilt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "platform=\"{}\" device={:#06x} pci_rev={:#04x} bits={} hwconfig_keys={} defaults=\"{}\"",
            self.platform,
            self.device,
            self.revision,
            self.feature_bits,
            self.hwconfig_keys,
            self.defaults.join(", ")
        )?;
        if !self.unknown.is_empty() {
            write!(f, " unknown=\"{}\"", self.unknown.join(", "))?;
        }
        Ok(())
    }
}

/// The feature table's set bits.
pub fn feature_bits(record: &[u8]) -> u32 {
    record[features::TABLE..features::TABLE + card_data::TABLE_BYTES]
        .iter()
        .map(|byte| byte.count_ones())
        .sum()
}

/// Builds the record.
pub fn build(facts: &RecordFacts) -> Vec<u8> {
    build_telling(facts).0
}

/// Builds the record, and says how its parts that come from data were built.
pub fn build_telling(facts: &RecordFacts) -> (Vec<u8>, Telling) {
    let mut record = vec![0u8; RECORD_SIZE];
    let platform = &facts.platform;
    for (field, value) in [
        (at::PRODUCT_FAMILY, platform.product_family),
        (at::PCH_FAMILY, platform.pch_family),
        (at::DISPLAY_CORE, platform.display_core),
        (at::RENDER_CORE, platform.render_core),
        (at::PLATFORM_TYPE, platform.platform_type),
        (at::GT_TYPE, platform.gt_type),
        (at::RENDER_IP, platform.render_ip.0),
    ] {
        put_dword(&mut record, field, value);
    }
    put_word(&mut record, at::DEVICE_ID, platform.device_id);
    put_word(&mut record, at::REVISION, platform.revision);

    let path = format!("{MACHINE_ROOT}{DEVICE_KEY}");
    record[at::DEVICE_REGISTRY_PATH..at::DEVICE_REGISTRY_PATH + path.len()].copy_from_slice(path.as_bytes());
    put_dword(&mut record, at::TIMESTAMP_FREQUENCY, facts.timestamp_hz);
    let name = facts.adapter_name.as_bytes();
    let name = &name[..name.len().min(ADAPTER_NAME_SIZE - 1)];
    record[at::ADAPTER_NAME..at::ADAPTER_NAME + name.len()].copy_from_slice(name);
    put_dword(&mut record, at::MIN_RENDER_FREQUENCY, facts.render_mhz.0);
    put_dword(&mut record, at::MAX_RENDER_FREQUENCY, facts.render_mhz.1);
    put_dword(&mut record, at::NUMBER_OF_EUS, facts.gt.eu_count);
    put_dword(&mut record, at::CACHE_LINE_SIZE, CACHE_LINE_BYTES);
    put_qword(&mut record, at::DEDICATED_VIDEO_MEMORY, facts.memory.dedicated);
    put_qword(&mut record, at::SYSTEM_SHARED_MEMORY, facts.memory.system_shared);
    put_qword(&mut record, at::SYSTEM_VIDEO_MEMORY, facts.memory.system_video);

    let gt = &facts.gt;
    for (field, value) in [
        (at::EU_COUNT, gt.eu_count),
        (at::THREAD_COUNT, gt.thread_count),
        (at::SLICE_COUNT, gt.slice_count),
        (at::SUBSLICE_COUNT, gt.subslice_count),
        (at::DUAL_SUBSLICE_COUNT, gt.dual_subslice_count),
        (at::MAX_EU_PER_SUBSLICE, gt.max_eu_per_subslice),
        (at::MAX_SLICES, gt.max_slices),
        (at::MAX_SUBSLICES, gt.max_subslices),
        (at::MAX_DUAL_SUBSLICES, gt.max_dual_subslices),
        (at::THREADS_PER_EU, gt.threads_per_eu),
    ] {
        put_dword(&mut record, field, value);
    }
    let most = &gt.engine_maxima;
    for (at, value) in
        [most.render, most.compute, most.video, most.video_enhance, most.copy].into_iter().enumerate()
    {
        put_dword(&mut record, at::ENGINE_MAXIMA + at * 4, value);
    }
    let stages = &gt.stage_threads;
    for (at, value) in
        [stages.vertex, stages.hull, stages.domain, stages.geometry, stages.pixel, stages.vertex_pocs]
            .into_iter()
            .enumerate()
    {
        put_dword(&mut record, at::STAGE_THREADS + at * 4, value);
    }

    write_urb(&mut record, &facts.urb);

    shape::write(&mut record, &facts.cores);
    if facts.compute_engines > 0 {
        let engines = facts.compute_engines.min(MAX_COMPUTE_ENGINES);
        put_dword(&mut record, at::COMPUTE_ENGINE_MASK, (1 << engines) - 1);
        put_dword(&mut record, at::COMPUTE_ENGINE_COUNT, engines);
        record[at::COMPUTE_ENGINES_VALID] = 1;
    }

    put_dword(&mut record, at::KERNEL_DRIVER_CAPS, DRIVER_STORE_ENABLED);
    for (blocks, mask, count, valid) in [
        (at::VDBOX_INFO, facts.video_engines, VDBOX_COUNT, VDBOX_VALID),
        (at::VEBOX_INFO, facts.video_enhance_engines, VEBOX_COUNT, VEBOX_VALID),
    ] {
        for block in blocks {
            put_dword(&mut record, block, mask);
            put_dword(&mut record, block + count, mask.count_ones());
            record[block + valid] = 1;
        }
    }
    // The workaround table reads the feature table, so it comes after it, and the entries this
    // backend insists on after the table.
    let revision = platform.revision as u8;
    let card_data = match card_data::table(platform.product_family, platform.device_id, revision) {
        Some(table) => CardData::Built(fill_from_data(&mut record, facts, &table)),
        None => {
            set_features(&mut record, facts);
            CardData::Uncovered { device: platform.device_id, revision, feature_bits: feature_bits(&record) }
        }
    };
    let card = workarounds::Card {
        product_family: platform.product_family,
        device_id: platform.device_id,
        pci_revision: platform.revision as u8,
        render: platform.render_ip.workaround_ip(),
        media: facts.media_ip.and_then(IpWord::workaround_ip),
    };
    let outcome = workarounds::fill(&mut record, &card);
    for (entry, value) in [VIRTUAL_MACHINE_WORKAROUND, TILED_RESOURCES_WORKAROUND] {
        features::set_workaround(&mut record, entry, value);
    }
    if !facts.render_cache_keyed {
        features::set_workaround(&mut record, RENDER_CACHE_KEYING_WORKAROUND, 0);
    }
    let block = &mut record[at::PARTITION..at::PARTITION + partition::SIZE];
    partition::write(block, &partition::ours());
    record[at::FROM_VIRTUAL_MACHINE] = 1;
    record[at::SECURE_VIRTUAL_MACHINE] = 0;
    (record, Telling { workarounds: outcome, card_data })
}

/// The parts of the record the kernel driver's data gives a card it knows: the platform words, the
/// hwconfig fields and the feature table, with this backend's flags on top of the table.
fn fill_from_data(record: &mut [u8], facts: &RecordFacts, table: &card_data::Table) -> DataBuilt {
    let platform = &facts.platform;
    let data = table.platform;
    let mut built = DataBuilt {
        platform: data.name,
        device: platform.device_id,
        revision: platform.revision as u8,
        feature_bits: 0,
        hwconfig_keys: 0,
        defaults: Vec::new(),
        unknown: Vec::new(),
    };

    // The platform words.
    put_dword(record, at::PLATFORM_TYPE, data.platform_type);
    put_dword(record, at::DISPLAY_CORE, data.display_core);
    put_dword(record, at::GT_TYPE, KNOWN_CARD_GT_TYPE);
    put_word(record, at::SUBSYSTEM_VENDOR, facts.subsystem.0);
    put_word(record, at::SUBSYSTEM, facts.subsystem.1);
    put_word(record, at::REVISION_AGAIN, platform.revision);
    let media_core = facts.media_ip.and_then(|ip| {
        let core = card_data::media_core(ip.architecture() as u16, ip.release() as u16);
        if core.is_none() {
            built.unknown.push(format!("the media core of media {}.{:02}", ip.architecture(), ip.release()));
        }
        core
    });
    if let Some(ip) = facts.media_ip {
        put_dword(record, at::MEDIA_IP, ip.0);
    }
    put_dword(record, at::MEDIA_CORE, media_core.unwrap_or(0));
    // xe doesn't report the display IP: it is the platform's, at revision 0, as the workaround
    // table takes it.
    if let Some((architecture, release)) =
        workarounds::platform_display_ip(platform.product_family, platform.device_id)
    {
        put_dword(record, at::DISPLAY_IP, IpWord::new(architecture, release, 0).0);
        built.defaults.extend([workarounds::DISPLAY_IP_DEFAULT, workarounds::DISPLAY_REVISION_DEFAULT]);
    }
    if data.per_ip_cores {
        put_dword(record, at::DISPLAY_IP_CORE, data.display_core);
        for slot in 0..4 {
            put_dword(record, at::RENDER_IP_CORES + slot * 4, platform.render_core);
            put_dword(record, at::MEDIA_IP_CORES + slot * 4, media_core.unwrap_or(0));
        }
    }

    // Every hwconfig key the card gives, where the driver puts it. These come after this backend's
    // own fields, so a key the card gives wins over what was worked out without it.
    for &(key, value) in &facts.hwconfig {
        if key == BINDLESS_SURFACE_OFFSET_BITS_KEY {
            continue;
        }
        let places = card_data::hwconfig_places(key);
        for &place in places {
            put_dword(record, place, value);
        }
        built.hwconfig_keys += usize::from(!places.is_empty());
    }

    // The feature table: the driver's known bits, but the Direct3D 11 interface versions, which
    // stay this backend's; then the bits this record keeps clear; then this backend's flags.
    let ddi_word = (at::DDI_VERSIONS - features::TABLE) / 4;
    for (word, (&value, &known)) in table.features.iter().zip(&table.known).enumerate() {
        let known = if word == ddi_word { 0 } else { known };
        let place = features::TABLE + word * 4;
        let kept = dword(record, place).unwrap_or(0) & !known;
        put_dword(record, place, kept | value & known);
    }
    if card_data::is_not_vga(facts.pci_class) {
        for &bit in card_data::not_vga_clears() {
            features::set(record, features::bit("VGA-compatible", usize::from(bit)), false);
        }
    }
    for feature in KEPT_CLEAR {
        features::set(record, feature, false);
    }
    set_features(record, facts);
    features::set(
        record,
        features::LOCAL_MEMORY_ALLOWS_4KB,
        facts.local_memory && facts.local_memory_small_pages,
    );

    built.defaults.extend([
        workarounds::GUEST_SETTINGS_DEFAULT,
        PCI_CAPABILITIES_DEFAULT,
        DISPLAY_FUSES_DEFAULT,
        CPU_DEFAULT,
    ]);
    if platform.product_family >= FUSE_WORD_FROM_FAMILY {
        built.defaults.push(FUSE_WORD_DEFAULT);
    }
    if SUBSYSTEM_DEVICES.contains(&platform.device_id) {
        built.defaults.push(SUBSYSTEM_DEFAULT);
    }
    built.feature_bits = feature_bits(record);
    built
}

/// The flags this backend sets: the memory model the core offers, hardware scheduling, Xe2's
/// tiling where the card has it, the engines the card has and the preemption Xe2 has. Compression
/// is always off. On a card the kernel driver's data covers these go on top of its table, and where
/// they differ from it the comment says so.
///
/// Five-level page tables are on only when the card has the address bits for them. The core's
/// addresses stop at 1 TiB, so a guest told it has five levels still never gets an address that
/// needs more than the first four; the card's own bits decide whether to say so.
fn set_features(record: &mut [u8], facts: &RecordFacts) {
    let compute = facts.compute_engines > 0;
    let flags: [(Feature, bool); 19] = [
        (features::PPGTT, true),
        (features::WDDM2_GPU_MMU, true),
        (features::WDDM2_64KB_PAGES, true),
        // The core's GPU addresses stop at 1 TiB, below where a process's own pointers live, so the
        // GPU can't share them. A real card says both.
        (features::SVM, false),
        (features::WDDM2_SVM, false),
        (features::LOCAL_MEMORY, facts.local_memory),
        (features::TILE_Y, false),
        (features::HW_SEMAPHORE_64, false),
        // The kernel driver's own table leaves this clear; Windows sets it later for a card that
        // schedules in hardware, as every card here is offered to the guest.
        (features::HW_SCHEDULING, true),
        (features::XE2_TILING, facts.xe2_tiling),
        // A real Xe2 or Xe3 card says this whatever xe gives.
        (features::PML5, facts.address_bits >= FIVE_LEVEL_ADDRESS_BITS),
        (features::WALKER_MTP, compute),
        // The engines are the ones xe gives. Alchemist G10's table has no compute ring, and every
        // Alchemist table says more than one compute engine.
        (features::CCS_RING, compute),
        (features::CCS_NODE, compute),
        (features::CCS_MULTI_INSTANCE, facts.compute_engines > 1),
        (features::BLITTER_RING, facts.copy_engine),
        (features::GPGPU_MID_BATCH_PREEMPT, compute),
        (features::GPGPU_THREAD_GROUP_PREEMPT, compute),
        // Compute-runtime takes mid-thread preemption to be the walker's. The kernel driver's own
        // table leaves this clear.
        (features::GPGPU_MID_THREAD_PREEMPT, compute),
    ];
    for (feature, on) in flags {
        features::set(record, feature, on);
    }
    for feature in features::PIPELINE_LEVELS.into_iter().chain(offered_ddi_versions(&facts.urb)) {
        features::set(record, feature, true);
    }
    // A real card says compression on every Xe2 and Xe3 card and most others.
    for feature in features::COMPRESSION {
        features::set(record, feature, false);
    }
}

/// The Direct3D 11 interface versions the record offers: every one a real card offers, when the
/// record has the stage limits Intel's OpenGL driver needs, and otherwise the newest alone.
///
/// Intel's OpenGL driver takes part only when the older versions are offered too. Offered the
/// newest alone it stays out, and a program gets Microsoft's OpenGL 1.1 in its place, which has no
/// pixel formats to choose by attribute, multisampled ones among them, so a program that asks for
/// one stops. Making a context, Intel's driver divides by the most URB entries of the vertex, hull,
/// domain and geometry stages, so on a record without them it would fail every program that uses
/// OpenGL rather than leave them Microsoft's.
fn offered_ddi_versions(urb: &Urb) -> Vec<Feature> {
    let stages = [urb.vertex, urb.hull, urb.domain, urb.geometry];
    if stages.iter().all(|stage| stage.max > 0) {
        features::every_ddi_version().collect()
    } else {
        vec![features::NEWEST_DDI_VERSION]
    }
}

/// What a record query asked for, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Asked {
    Count,
    Record { build_stamp: u32 },
    Refused(&'static str),
}

/// Answers a record query with `record`, keeping the guest's build stamp.
pub fn answer(record: &[u8], query: &VendorQuery<'_>, out: &mut Vec<u8>) -> (Status, Asked) {
    if query.size == COUNT_SIZE {
        out.extend_from_slice(&LINKED_ADAPTERS.to_le_bytes());
        return (Status::SUCCESS, Asked::Count);
    }
    if query.size < RECORD_SIZE {
        return (Status::INVALID_PARAMETER, Asked::Refused("shorter than one record"));
    }
    if query.size != RECORD_SIZE * LINKED_ADAPTERS as usize {
        return (Status::INVALID_PARAMETER, Asked::Refused("not the size of one record"));
    }
    let start = out.len();
    out.extend_from_slice(record);
    let build_stamp = dword(query.data, at::BUILD_STAMP).unwrap_or(0);
    put_dword(&mut out[start..], at::BUILD_STAMP, build_stamp);
    (Status::SUCCESS, Asked::Record { build_stamp })
}

/// The record's fields a log line names.
pub fn summary(record: &[u8]) -> RecordSummary {
    let field = |at| dword(record, at).unwrap_or(0);
    RecordSummary {
        product_family: field(at::PRODUCT_FAMILY),
        render_core: field(at::RENDER_CORE),
        device_id: field(at::DEVICE_ID) & 0xFFFF,
        render_ip: field(at::RENDER_IP),
        eu_count: field(at::EU_COUNT),
        virtual_machine: record.get(at::FROM_VIRTUAL_MACHINE).copied().unwrap_or(0),
        compression: features::COMPRESSION.iter().any(|&feature| features::is_set(record, feature)),
    }
}

/// The few fields of a record a person checks first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordSummary {
    pub product_family: u32,
    pub render_core: u32,
    pub device_id: u32,
    pub render_ip: u32,
    pub eu_count: u32,
    pub virtual_machine: u8,
    pub compression: bool,
}

impl std::fmt::Display for RecordSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "product_family={} render_core={:#x} device={:#06x} render_ip={:#x} eus={} vm={} compression={}",
            self.product_family,
            self.render_core,
            self.device_id,
            self.render_ip,
            self.eu_count,
            self.virtual_machine,
            if self.compression { "on" } else { "off" }
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol::bytes::{qword, word};
    use std::collections::BTreeMap;

    pub fn facts() -> RecordFacts {
        RecordFacts {
            platform: Platform {
                product_family: 1275,
                render_core: 0x0C09,
                display_core: 0x0C09,
                device_id: 0x64A0,
                revision: 4,
                gt_type: 18,
                render_ip: IpWord::new(20, 4, 4),
                ..Platform::default()
            },
            gt: GtSystem {
                eu_count: 64,
                thread_count: 512,
                dual_subslice_count: 8,
                threads_per_eu: 8,
                engine_maxima: EngineMaxima { render: 1, compute: 2, video: 2, video_enhance: 2, copy: 1 },
                ..GtSystem::default()
            },
            timestamp_hz: 19_200_000,
            adapter_name: "Intel GPU 0x64a0".to_owned(),
            render_mhz: (400, 2050),
            memory: Memory { system_shared: 16 << 30, ..Memory::default() },
            local_memory: false,
            urb: Urb::default(),
            cores: (0..8).map(|index| Core { slice: 0, index, eu_count: 8, eu_mask: 0xFF }).collect(),
            compute_engines: 1,
            copy_engine: true,
            video_engines: 0b11,
            video_enhance_engines: 0b11,
            address_bits: 48,
            xe2_tiling: true,
            media_ip: Some(IpWord::new(20, 0, 0)),
            subsystem: (0, 0),
            pci_class: 0x03_0000,
            hwconfig: Vec::new(),
            local_memory_small_pages: false,
            render_cache_keyed: false,
        }
    }

    fn query(size: usize, data: &[u8]) -> VendorQuery<'_> {
        VendorQuery { kind: QUERY_TYPE, size, data, process_is_32_bit: false, program: "" }
    }

    #[test]
    fn the_record_is_one_size_and_names_the_platform_the_guest_picks_its_module_by() {
        let record = build(&facts());
        assert_eq!(record.len(), RECORD_SIZE);
        assert_eq!(dword(&record, at::PRODUCT_FAMILY), Some(1275));
        assert_eq!(dword(&record, at::RENDER_CORE), Some(0x0C09));
        assert_eq!(word(&record, at::DEVICE_ID), Some(0x64A0));
        assert_eq!(word(&record, at::REVISION), Some(4));
        assert_eq!(dword(&record, at::GT_TYPE), Some(0), "GTTYPE_GT1, as the kernel driver gives it");
    }

    #[test]
    fn ip_versions_pack_as_architecture_release_and_revision() {
        assert_eq!(IpWord::new(20, 4, 4).0, 20 << 22 | 4 << 14 | 4);
        assert_eq!(IpWord::new(20, 4, 0x41).0 & 0x3F, 1, "the revision has six bits");
        let record = build(&facts());
        assert_eq!(dword(&record, at::RENDER_IP), Some(IpWord::new(20, 4, 4).0));
    }

    #[test]
    fn the_record_says_virtual_machine_and_not_secure() {
        let record = build(&facts());
        assert_eq!(record[at::FROM_VIRTUAL_MACHINE], 1);
        assert_eq!(record[at::SECURE_VIRTUAL_MACHINE], 0);
    }

    #[test]
    fn the_video_engines_are_in_both_copies_of_their_blocks() {
        let record = build(&facts());
        for block in at::VDBOX_INFO.into_iter().chain(at::VEBOX_INFO) {
            assert_eq!(dword(&record, block), Some(0b11), "{block:#x}");
        }
        assert_eq!(dword(&record, 0x2340), Some(2), "NumberOfVDBoxEnabled");
        assert_eq!(dword(&record, 0x235C), Some(2), "NumberOfVEBoxEnabled");
        assert_eq!((record[0x2344], record[0x2360]), (1, 1), "both valid");
    }

    #[test]
    fn the_3d_stages_threads_go_where_gt_system_info_has_them() {
        let threads =
            StageThreads { vertex: 546, hull: 336, domain: 546, geometry: 312, pixel: 96, vertex_pocs: 0 };
        let mut facts = facts();
        facts.gt.stage_threads = threads;
        let record = build(&facts);
        let read: Vec<u32> = (0..6).map(|at| dword(&record, 0xB78 + at * 4).unwrap()).collect();
        assert_eq!(read, [546, 336, 546, 312, 96, 0]);
    }

    #[test]
    fn the_kernel_driver_runs_from_the_driver_store_and_says_nothing_else() {
        let record = build(&facts());
        assert_eq!(dword(&record, at::KERNEL_DRIVER_CAPS), Some(1 << 10));
    }

    #[test]
    fn compression_is_off_whatever_else_is_on() {
        let record = build(&facts());
        for feature in features::COMPRESSION {
            assert!(!features::is_set(&record, feature), "{}", feature.name);
        }
        assert!(!summary(&record).compression);
    }

    #[test]
    fn direct3d_11_sees_every_pipeline_level_from_10_0_to_12_1_with_no_gap() {
        let record = build(&facts());
        assert_eq!(record[0x305] & 0x1B, 0x1B, "10_0, 10_1, 11_0 and 11_1");
        assert!(features::is_set(&record, features::WDDM2_GPU_MMU), "12_0 and 12_1");
    }

    #[test]
    fn the_memory_model_is_the_cores() {
        let record = build(&facts());
        for feature in [features::PPGTT, features::WDDM2_GPU_MMU, features::WDDM2_64KB_PAGES] {
            assert!(features::is_set(&record, feature), "{}", feature.name);
        }
        for feature in [features::SVM, features::WDDM2_SVM, features::LOCAL_MEMORY, features::TILE_Y] {
            assert!(!features::is_set(&record, feature), "{}", feature.name);
        }
        let discrete = build(&RecordFacts { local_memory: true, ..facts() });
        assert!(features::is_set(&discrete, features::LOCAL_MEMORY));
    }

    #[test]
    fn xe2_tiling_is_said_only_of_a_card_that_has_it() {
        let older = build(&RecordFacts { xe2_tiling: false, ..facts() });
        assert!(!features::is_set(&older, features::XE2_TILING));
        assert!(!features::is_set(&older, features::TILE_Y), "nor the tiling before it");
    }

    #[test]
    fn hardware_scheduling_engines_and_preemption_are_set_for_a_card_with_them() {
        let record = build(&facts());
        for feature in [
            features::HW_SCHEDULING,
            features::XE2_TILING,
            features::WALKER_MTP,
            features::CCS_RING,
            features::CCS_NODE,
            features::BLITTER_RING,
            features::GPGPU_MID_BATCH_PREEMPT,
            features::GPGPU_THREAD_GROUP_PREEMPT,
            features::GPGPU_MID_THREAD_PREEMPT,
        ] {
            assert!(features::is_set(&record, feature), "{}", feature.name);
        }
        for feature in [features::CCS_MULTI_INSTANCE, features::PML5] {
            assert!(!features::is_set(&record, feature), "{}", feature.name);
        }
        assert_eq!(dword(&record, at::COMPUTE_ENGINE_MASK), Some(1));
        assert_eq!(dword(&record, at::COMPUTE_ENGINE_COUNT), Some(1));
        assert_eq!(record[at::COMPUTE_ENGINES_VALID], 1);
    }

    #[test]
    fn the_engine_flags_follow_the_engines_the_card_has() {
        let bare = build(&RecordFacts { compute_engines: 0, copy_engine: false, ..facts() });
        for feature in [features::CCS_RING, features::BLITTER_RING, features::WALKER_MTP] {
            assert!(!features::is_set(&bare, feature), "{}", feature.name);
        }
        assert_eq!(bare[at::COMPUTE_ENGINES_VALID], 0);
        let two = build(&RecordFacts { compute_engines: 2, ..facts() });
        assert!(features::is_set(&two, features::CCS_MULTI_INSTANCE));
        assert_eq!(dword(&two, at::COMPUTE_ENGINE_MASK), Some(0b11));
    }

    #[test]
    fn five_level_page_tables_need_the_address_bits() {
        let wide = build(&RecordFacts { address_bits: 57, ..facts() });
        assert!(features::is_set(&wide, features::PML5));
    }

    #[test]
    fn direct3d_11_reads_tiled_resources_from_the_top_of_dword_0x4a8() {
        let record = build(&facts());
        assert_eq!(features::workaround(&record, 535), Some(1));
        assert_eq!(record[0x4AB], 0x10);
        assert_ne!(dword(&record, 0x4A8).unwrap() & 0xF000_0000, 0);
        assert_eq!(features::workaround(&record, 534), Some(0));
    }

    /// The table Intel's kernel driver gives an Arc B580, as `entry:value` with the value in hex,
    /// on a guest whose feature table is a real card's.
    const B580_REAL_CARD: &str = "\
        0:1 91:1 427:1 535:1 705:1 830:1 876:1 925:1 926:1 957:1 968:1 982:1 984:1 1002:1 1093:1 \
        1234:1 1249:1 1261:1 1316:1 1385:1 1398:1 1401:1 1402:1 1403:1 1405:1 1407:1 1408:1 1410:1 \
        1413:1 1414:1 1432:1 1433:1 1468:1 1527:1 1528:1 2159:8 2163:2 2287:1 2350:8 2368:8 2411:8 \
        2414:8 2456:8 2457:1 2479:8 2480:8 2500:8 2523:8 2525:4 2532:2 2533:2 2534:2 2535:1 2540:2 \
        2541:2 2542:4 2543:4 2544:2 2554:2 2559:2 2563:2 2569:2 2571:8 2572:8 2574:8 2581:2 2588:8 \
        2595:2 2596:2 2607:2 2613:2 2627:4 2628:2 2631:2 2632:2 2635:2 2644:2 2648:4 2650:2 2651:8 \
        2654:4 2655:4 2661:2 2662:2 2663:2 2665:2 2667:2 2668:2 2669:2 2671:2 2672:4 2676:2 2678:6 \
        2681:4 2682:2 2683:2 2687:2 2690:4 2695:2 2697:2 2698:2 2699:4 2705:4 2709:8 2710:2 2711:4 \
        2714:2 2719:2 2721:2 2722:6 2725:2 2734:2 2735:2 2740:8 2741:4 2742:8 2743:8 2744:2 2747:2 \
        2750:8 2752:2 2754:8 2756:2 2759:8 2760:2 2763:2 2765:2 2770:2 2774:8 2775:2 2776:4 2777:2 \
        2781:2 2782:2 2783:2 2784:2 2787:8 2789:2 2791:8 2792:2 2794:2 2795:2 2796:8 2799:8 2802:2 \
        2803:2 2807:2 2811:2 2815:4 2822:8 2827:2 2834:2 2835:1 2836:1 2838:1 2839:1 2847:1 2851:2 \
        2854:2 2860:2 2864:1 2865:1 2866:8 2867:2 2869:2 2871:2 2874:6 2876:2 2877:2 2878:2 2882:4 \
        2883:2 2888:2 2893:2 2896:8 2913:2 2920:2 2922:4 2924:8 2925:2 2927:2 2928:8 2930:8 2935:6 \
        2943:6 2948:4 2949:2 2950:8 2964:2 3024:1 3039:3 3040:5 3041:9";
    // What Intel's kernel driver gives the other cards in the tests below for the same inputs,
    // with the feature table this record carries (the newest Direct3D 11 interface version alone).
    // Lunar Lake's and Panther Lake's entry 2849 and Alchemist's 2835 go with feature bits of their
    // tables.

    /// 211 entries.
    const LUNAR_LAKE: &str = "\
        91:1 427:1 535:1 705:1 830:1 876:1 925:1 926:1 957:1 968:1 982:1 984:1 1002:1 1022:1 1093:1 \
        1234:1 1249:1 1261:1 1316:1 1385:1 1398:1 1401:1 1402:1 1403:1 1404:1 1406:1 1407:1 1409:1 \
        1410:1 1412:1 1413:1 1414:1 1432:1 1433:1 1468:1 1527:1 1528:1 1552:1 1982:1 2163:2 2287:1 \
        2457:1 2485:1 2500:8 2501:8 2503:8 2523:8 2525:4 2532:2 2533:2 2534:2 2535:1 2539:2 2540:2 \
        2541:6 2542:4 2543:4 2544:2 2554:2 2559:2 2563:2 2569:2 2571:8 2572:8 2574:8 2581:2 2585:8 \
        2588:8 2595:2 2596:2 2597:4 2599:8 2601:8 2604:4 2607:2 2613:2 2615:6 2616:6 2619:8 2627:4 \
        2631:2 2633:2 2635:2 2644:2 2645:2 2648:4 2651:8 2654:4 2655:4 2657:8 2658:8 2661:2 2662:2 \
        2663:2 2664:8 2665:2 2667:2 2668:2 2671:6 2672:4 2676:2 2678:6 2682:2 2683:2 2687:2 2693:8 \
        2695:2 2697:2 2698:2 2699:4 2700:8 2702:8 2705:4 2706:8 2710:2 2714:2 2719:2 2720:8 2721:2 \
        2722:6 2723:8 2725:2 2734:2 2735:2 2741:4 2742:8 2743:8 2744:2 2748:2 2749:4 2751:4 2752:2 \
        2755:8 2756:6 2757:2 2759:8 2760:4 2763:2 2764:6 2765:2 2768:4 2769:4 2771:2 2773:8 2774:8 \
        2776:4 2777:2 2779:8 2781:2 2782:2 2783:2 2784:2 2785:8 2788:8 2789:2 2790:4 2791:8 2792:2 \
        2793:2 2794:2 2796:8 2799:8 2802:2 2803:2 2811:2 2822:8 2825:8 2826:8 2827:2 2836:1 2838:1 \
        2843:1 2847:1 2849:1 2853:8 2854:2 2856:6 2858:8 2864:1 2865:1 2867:2 2869:2 2871:2 2873:8 2874:6 \
        2876:2 2877:2 2878:2 2882:4 2888:2 2893:2 2896:8 2899:4 2913:2 2922:4 2924:8 2927:2 2928:8 \
        2930:8 2935:6 2938:4 2943:6 2948:6 2949:2 2964:2 3014:2 3023:1 3024:1 3039:3 3040:5 3041:9";
    /// 248 entries.
    const METEOR_LAKE: &str = "\
        91:1 399:1 415:1 427:1 535:1 692:1 705:1 706:1 771:1 876:1 925:1 926:1 957:1 968:1 982:1 \
        983:1 1002:1 1021:1 1093:1 1138:1 1234:1 1249:1 1261:1 1316:1 1385:1 1398:1 1401:1 1402:1 \
        1403:1 1404:1 1406:1 1407:1 1409:1 1410:1 1412:1 1413:1 1414:1 1432:1 1433:1 1439:1 1442:1 \
        1468:1 1527:1 1528:1 1552:1 1726:2 1917:2 1982:1 2132:2 2147:8 2159:8 2172:2 2191:2 2197:2 \
        2198:2 2201:6 2223:2 2225:2 2229:2 2232:2 2238:6 2240:2 2241:2 2245:2 2246:2 2250:2 2251:2 \
        2252:2 2258:8 2261:8 2265:1 2266:2 2267:2 2268:2 2270:2 2273:2 2275:8 2277:2 2278:8 2280:2 \
        2283:2 2284:2 2286:2 2288:2 2289:2 2295:2 2300:2 2301:2 2302:2 2303:2 2340:2 2343:6 2344:2 \
        2349:8 2350:8 2352:4 2354:2 2356:2 2357:2 2358:2 2360:2 2363:2 2365:2 2367:8 2368:8 2376:8 \
        2379:8 2380:2 2385:2 2386:2 2387:2 2388:2 2389:2 2390:2 2391:2 2394:2 2397:4 2399:2 2400:2 \
        2401:2 2403:4 2404:2 2411:8 2413:2 2416:2 2421:4 2436:2 2437:8 2438:1 2440:2 2442:2 2443:2 \
        2444:2 2446:2 2447:2 2449:2 2451:2 2452:2 2455:2 2456:8 2457:1 2462:4 2467:2 2468:2 2470:8 \
        2471:2 2473:4 2475:2 2476:2 2477:8 2480:8 2484:2 2485:1 2487:2 2491:2 2498:2 2499:2 2500:8 \
        2502:2 2503:8 2507:2 2513:2 2517:2 2518:4 2520:2 2521:2 2522:2 2523:8 2524:2 2526:2 2535:1 \
        2538:2 2543:4 2560:2 2571:8 2572:8 2574:8 2575:8 2577:6 2582:4 2586:2 2587:2 2588:8 2593:2 \
        2594:6 2596:2 2603:2 2604:6 2614:8 2621:2 2623:2 2629:2 2630:2 2639:4 2642:2 2645:4 2646:4 \
        2649:2 2651:8 2656:8 2659:2 2672:4 2683:2 2691:8 2704:2 2705:4 2708:2 2709:8 2712:4 2716:6 \
        2721:2 2733:8 2741:4 2742:8 2743:8 2753:8 2759:8 2774:8 2779:8 2791:8 2796:8 2799:8 2810:2 \
        2822:8 2827:2 2836:1 2839:1 2841:1 2842:1 2843:1 2857:2 2863:2 2865:1 2866:8 2874:6 2880:2 \
        2882:4 2896:8 2921:8 2922:4 2924:8 2928:8 2930:8 2968:8 3024:1 3039:3 3040:5 3041:9";
    /// 148 entries.
    const PANTHER_LAKE: &str = "\
        91:1 427:1 535:1 705:1 830:1 876:1 925:1 926:1 957:1 968:1 982:1 1002:1 1093:1 1234:1 1249:1 \
        1261:1 1316:1 1385:1 1398:1 1401:1 1402:1 1403:1 1404:1 1406:1 1407:1 1409:1 1410:1 1412:1 \
        1413:1 1414:1 1432:1 1433:1 1468:1 1527:1 1528:1 1552:1 2287:1 2457:1 2534:2 2535:1 2559:2 \
        2662:2 2671:4 2705:4 2717:4 2718:8 2720:8 2732:8 2734:2 2741:4 2742:8 2743:8 2748:2 2749:4 \
        2751:4 2752:2 2755:8 2756:6 2760:4 2761:4 2763:2 2764:6 2768:4 2769:4 2772:8 2773:8 2774:8 \
        2777:2 2779:8 2781:2 2782:2 2783:2 2785:8 2788:8 2789:2 2792:2 2796:8 2797:4 2799:8 2801:8 \
        2802:2 2803:2 2804:2 2812:8 2819:2 2822:8 2825:8 2826:8 2828:4 2831:2 2832:4 2836:1 2838:1 \
        2843:1 2847:1 2849:1 2852:2 2853:8 2854:2 2858:8 2859:8 2862:8 2865:1 2869:2 2870:4 2871:2 2874:6 \
        2876:2 2877:2 2878:2 2879:8 2883:2 2885:4 2890:2 2891:4 2893:2 2896:8 2903:4 2907:4 2910:8 \
        2911:8 2913:2 2922:4 2924:8 2926:8 2927:2 2928:8 2930:8 2931:4 2935:6 2938:4 2940:4 2943:6 \
        2948:6 2949:2 2952:8 2964:2 2973:2 2977:8 2984:2 3005:8 3013:8 3014:2 3023:1 3030:1 3039:3 \
        3040:5 3041:9";
    /// 297 entries.
    const ALCHEMIST_G11_REV_0: &str = "\
        0:1 1:1 91:1 92:1 96:1 97:1 310:1 318:1 399:1 415:1 427:1 535:1 675:1 692:1 705:1 706:1 \
        707:1 771:1 876:1 881:1 883:1 925:1 926:1 957:1 968:1 970:1 981:1 986:1 997:1 1016:1 1020:1 \
        1022:1 1093:1 1138:1 1234:1 1249:1 1250:1 1257:1 1261:1 1316:1 1328:1 1337:1 1385:1 1398:1 \
        1401:1 1402:1 1403:1 1404:1 1406:1 1407:1 1409:1 1410:1 1412:1 1413:1 1414:1 1438:1 1439:1 \
        1468:1 1471:1 1475:1 1500:1 1527:1 1528:1 1552:1 1582:1 1594:1 1624:1 1684:1 1726:1 1728:1 \
        1793:1 1816:1 1819:1 1830:1 1844:1 1847:1 1854:1 1871:1 1893:1 1917:1 1952:1 1953:1 1960:1 \
        1967:1 1981:1 1982:1 1999:1 2009:1 2011:1 2020:1 2024:1 2025:1 2028:1 2031:1 2034:1 2062:1 \
        2070:1 2071:1 2072:1 2078:1 2083:1 2086:1 2093:1 2094:1 2095:1 2097:1 2102:1 2103:1 2107:1 \
        2109:1 2112:1 2119:1 2122:1 2123:1 2125:1 2129:1 2130:1 2131:1 2132:1 2140:1 2142:1 2147:1 \
        2148:1 2149:1 2152:1 2153:1 2154:1 2157:1 2159:1 2160:1 2161:1 2162:1 2165:1 2166:1 2169:1 \
        2171:1 2172:1 2173:1 2178:1 2179:1 2184:1 2185:1 2186:1 2187:1 2188:1 2189:1 2191:1 2194:1 \
        2195:1 2196:1 2197:1 2198:1 2199:1 2201:1 2222:1 2223:1 2225:1 2229:1 2230:1 2232:1 2236:1 \
        2238:1 2240:1 2243:1 2245:1 2246:1 2247:1 2248:1 2250:1 2251:1 2252:1 2253:1 2256:1 2260:1 \
        2261:1 2266:1 2267:1 2268:1 2270:1 2271:1 2272:1 2273:1 2276:1 2277:1 2279:1 2280:1 2283:1 \
        2284:1 2286:1 2288:1 2289:1 2295:1 2300:1 2302:1 2306:1 2317:1 2340:1 2342:1 2343:1 2344:1 \
        2349:1 2350:1 2352:1 2354:1 2356:1 2357:1 2358:1 2363:1 2364:1 2365:1 2366:1 2368:1 2369:1 \
        2376:1 2379:1 2380:1 2382:1 2386:1 2387:1 2388:1 2389:1 2390:1 2391:1 2394:1 2395:1 2396:1 \
        2400:1 2401:1 2402:1 2412:1 2417:1 2421:1 2437:1 2439:1 2440:1 2441:1 2442:1 2443:1 2444:1 \
        2446:1 2449:1 2452:1 2455:1 2457:1 2467:1 2468:1 2469:1 2474:1 2488:1 2494:1 2500:1 2502:1 \
        2503:1 2513:1 2522:1 2523:1 2526:1 2535:1 2553:1 2560:1 2561:1 2572:1 2575:1 2576:1 2579:1 \
        2587:1 2588:1 2596:1 2604:1 2611:1 2621:1 2623:1 2649:1 2683:1 2691:1 2696:1 2716:1 2721:1 \
        2742:1 2743:1 2774:1 2791:1 2796:1 2798:1 2810:1 2822:1 2827:1 2835:1 2836:1 2839:1 2865:1 2866:1 \
        2874:1 3024:1 3039:1 3040:1 3041:1";
    /// 284 entries.
    const ALCHEMIST_G11_REV_8: &str = "\
        0:1 1:1 91:1 92:1 96:1 97:1 310:1 318:1 399:1 415:1 427:1 535:1 675:1 692:1 705:1 706:1 \
        707:1 771:1 876:1 881:1 883:1 925:1 926:1 957:1 968:1 981:1 986:1 997:1 1016:1 1020:1 1022:1 \
        1093:1 1138:1 1234:1 1249:1 1250:1 1257:1 1261:1 1316:1 1328:1 1337:1 1385:1 1398:1 1401:1 \
        1402:1 1403:1 1404:1 1406:1 1407:1 1409:1 1410:1 1412:1 1413:1 1414:1 1438:1 1439:1 1449:1 \
        1468:1 1471:1 1475:1 1500:1 1527:1 1528:1 1552:1 1582:1 1594:1 1624:1 1684:1 1726:1 1728:1 \
        1793:1 1816:1 1819:1 1830:1 1844:1 1847:1 1854:1 1871:1 1893:1 1917:1 1952:1 1953:1 1960:1 \
        1967:1 1981:1 1982:1 1999:1 2009:1 2024:1 2025:1 2028:1 2031:1 2034:1 2078:1 2083:1 2095:1 \
        2102:1 2107:1 2112:1 2119:1 2125:1 2129:1 2132:1 2142:1 2147:1 2148:1 2152:1 2154:1 2157:1 \
        2159:1 2160:1 2161:1 2165:1 2166:1 2172:1 2178:1 2179:1 2184:1 2185:1 2191:1 2196:1 2197:1 \
        2198:1 2199:1 2201:1 2222:1 2223:1 2224:1 2225:1 2229:1 2230:1 2232:1 2236:1 2238:1 2240:1 \
        2243:1 2245:1 2246:1 2247:1 2248:1 2250:1 2251:1 2252:1 2253:1 2256:1 2261:1 2266:1 2267:1 \
        2268:1 2270:1 2271:1 2272:1 2273:1 2276:1 2277:1 2279:1 2280:1 2283:1 2284:1 2286:1 2288:1 \
        2289:1 2295:1 2300:1 2302:1 2303:1 2306:1 2317:1 2340:1 2342:1 2343:1 2344:1 2349:1 2350:1 \
        2352:1 2354:1 2355:1 2356:1 2357:1 2358:1 2363:1 2364:1 2365:1 2366:1 2368:1 2369:1 2370:1 \
        2376:1 2379:1 2380:1 2382:1 2386:1 2387:1 2388:1 2389:1 2390:1 2391:1 2394:1 2395:1 2396:1 \
        2400:1 2401:1 2402:1 2404:1 2412:1 2413:1 2417:1 2421:1 2424:1 2437:1 2439:1 2440:1 2441:1 \
        2442:1 2443:1 2444:1 2446:1 2449:1 2452:1 2455:1 2457:1 2467:1 2468:1 2469:1 2471:1 2474:1 \
        2481:1 2482:1 2487:1 2488:1 2494:1 2498:1 2499:1 2500:1 2502:1 2503:1 2506:1 2507:1 2513:1 \
        2519:1 2520:1 2522:1 2523:1 2526:1 2535:1 2553:1 2560:1 2561:1 2572:1 2575:1 2576:1 2579:1 \
        2587:1 2588:1 2596:1 2604:1 2611:1 2621:1 2623:1 2649:1 2683:1 2691:1 2696:1 2716:1 2721:1 \
        2742:1 2743:1 2774:1 2791:1 2796:1 2798:1 2810:1 2822:1 2827:1 2835:1 2836:1 2839:1 2865:1 2866:1 \
        2874:1 3024:1 3039:1 3040:1 3041:1";
    /// 295 entries.
    const ALCHEMIST_A770: &str = "\
        0:1 1:1 91:1 92:1 96:1 97:1 310:1 318:1 399:1 415:1 427:1 535:1 675:1 692:1 705:1 706:1 \
        709:1 771:1 876:1 881:1 925:1 926:1 957:1 968:1 981:1 986:1 997:1 1016:1 1020:1 1022:1 \
        1093:1 1138:1 1234:1 1249:1 1250:1 1257:1 1261:1 1316:1 1328:1 1337:1 1385:1 1398:1 1401:1 \
        1402:1 1403:1 1404:1 1406:1 1407:1 1409:1 1410:1 1412:1 1413:1 1414:1 1438:1 1439:1 1449:1 \
        1468:1 1471:1 1475:1 1481:1 1500:1 1527:1 1528:1 1552:1 1582:1 1624:1 1684:1 1726:1 1728:1 \
        1793:1 1816:1 1819:1 1830:1 1844:1 1847:1 1854:1 1871:1 1887:1 1893:1 1910:1 1917:1 1952:1 \
        1953:1 1960:1 1967:1 1981:1 1982:1 1999:1 2009:1 2024:1 2025:1 2028:1 2031:1 2034:1 2078:1 \
        2083:1 2095:1 2102:1 2107:1 2110:1 2112:1 2119:1 2125:1 2129:1 2132:1 2142:1 2147:1 2148:1 \
        2152:1 2154:1 2159:1 2160:1 2161:1 2165:1 2166:1 2172:1 2177:1 2178:1 2179:1 2184:1 2185:1 \
        2191:1 2196:1 2197:1 2198:1 2201:1 2222:1 2223:1 2227:1 2229:1 2238:1 2240:1 2243:1 2245:1 \
        2246:1 2248:1 2250:1 2251:1 2252:1 2256:1 2259:1 2261:1 2266:1 2267:1 2268:1 2270:1 2273:1 \
        2276:1 2277:1 2279:1 2280:1 2283:1 2284:1 2286:1 2288:1 2289:1 2295:1 2300:1 2302:1 2303:1 \
        2306:1 2317:1 2340:1 2341:1 2342:1 2343:1 2344:1 2346:1 2349:1 2350:1 2352:1 2354:1 2355:1 \
        2356:1 2357:1 2358:1 2359:1 2363:1 2364:1 2365:1 2366:1 2368:1 2369:1 2370:1 2376:1 2379:1 \
        2380:1 2381:1 2382:1 2385:1 2386:1 2387:1 2388:1 2389:1 2390:1 2391:1 2394:1 2395:1 2396:1 \
        2399:1 2400:1 2401:1 2402:1 2404:1 2412:1 2413:1 2417:1 2421:1 2423:1 2424:1 2437:1 2440:1 \
        2441:1 2443:1 2444:1 2446:1 2449:1 2451:1 2452:1 2455:1 2457:1 2467:1 2468:1 2469:1 2471:1 \
        2474:1 2476:1 2481:1 2482:1 2487:1 2488:1 2494:1 2498:1 2499:1 2500:1 2502:1 2503:1 2506:1 \
        2507:1 2513:1 2519:1 2520:1 2522:1 2523:1 2524:1 2526:1 2528:1 2535:1 2538:1 2553:1 2560:1 \
        2561:1 2572:1 2575:1 2576:1 2579:1 2586:1 2587:1 2588:1 2589:1 2596:1 2604:1 2611:1 2621:1 \
        2623:1 2629:1 2642:1 2645:1 2649:1 2659:1 2683:1 2691:1 2703:1 2716:1 2721:1 2731:1 2742:1 \
        2743:1 2774:1 2791:1 2796:1 2798:1 2810:1 2822:1 2827:1 2835:1 2836:1 2839:1 2865:1 2866:1 2874:1 \
        3024:1 3039:1 3040:1 3041:1";

    fn expected(table: &str) -> BTreeMap<usize, u8> {
        table
            .split_whitespace()
            .map(|pair| {
                let (entry, value) = pair.split_once(':').unwrap();
                (entry.parse().unwrap(), u8::from_str_radix(value, 16).unwrap())
            })
            .collect()
    }

    fn table_of(record: &[u8]) -> BTreeMap<usize, u8> {
        (0..0x5F4 * 2)
            .filter_map(|entry| Some((entry, features::workaround(record, entry)?)))
            .filter(|&(_, value)| value != 0)
            .collect()
    }

    /// A card with GMD_ID versions: its product family and device, and its render and media IPs.
    fn card(product_family: u32, device_id: u16, render_ip: IpWord, media_ip: Option<IpWord>) -> RecordFacts {
        let base = facts();
        let platform = Platform { product_family, device_id, render_ip, ..base.platform };
        RecordFacts { platform, media_ip, ..base }
    }

    fn alchemist(device_id: u16, revision: u16) -> RecordFacts {
        let mut facts = card(1270, device_id, IpWord::new(12, 55, 8), None);
        facts.platform.revision = revision;
        facts
    }

    /// 8086:5690/rev0, its known bits and which are known.
    const ALCHEMIST_G10_REV_0_DATA: DataTable = DataTable {
        values: "\
            a5d505000000020130405760a0310000000000001520f0075080c40d30e4008205080400200000000500000001000000 \
            30f7c2ea0950693fc0c51088917780029601000000000000010000002000000000df23ec2e2400000600000005000000 \
            f300193f00380038437cbb9154e31300bd1b5e609fb7c10722ef8ca43b84087803805803640040000cb7defcff0fee3b \
            e400c7329f716fe0ffdf059e5f46046ce317fe3f3004800200000000000000001f800000000000002900000000000000 \
            0100000006000000000000000000000000000000000000000000000000000000050000000000000001000000",
        known: "\
            a5dd05000001f70138575760a0310800000000001d20f0075080cc0d34f4418245a81400200100000500000001000000 \
            30ffe6ea0b79797fc2c51088997780029601000200000000010000002000000000ff33ec3f2710000700000007000000 \
            f301193f0038c03a57fcfbf157e31300bd1b5f60bff7c10722ef8de4bb8408f827905803640040004cb7deffff0fee3b \
            e400c7f69f716feafffff59f5f66046cef17fe3f3004800200000000000000001f800100000000002900000000000000 \
            01000000060000000600000000000000000000000000000000000000000000009d0000000000000001000000",
    };

    /// 8086:56a0/rev8, its known bits and which are known.
    const ARC_A770_DATA: DataTable = DataTable {
        values: "\
            a5d505000000020130404760a0310000000000001520f0075080c40d30e4008205280400200000000500000001000000 \
            30f7c2ea0950793fc2451088917780029601000000000000010000002000000000df23ec3f2700000700000005000000 \
            f300193f00380038437cbb9154e31300bd1b5e609fb7c10722ef8ca43b84087803805803640040000cb7defcff0fee3b \
            e400c7329f716fe0ffdf059e5f46046ce317fe3f3004800200000000000000001f800000000000002800000000000000 \
            01000000060000000000000000000000000000000000000000000000000000009d0000000000000001000000",
        known: "\
            a5dd05000001f70138575760a0310800000000001d20f0075080cc0d34f4418245a81400200100000500000001000000 \
            30ffe6ea0b79797fc2451088997780029601000200000000010000002000000000ff33ec3f2710000700000007000000 \
            f301193f0038c03a57fcfbf157e31300bd1b5f60bff7c10722ef8de4bb8408f827905803640040004cb7deffff0fee3b \
            e400c7f69f716feafffff59f5f66046cef17fe3f3004800200000000000000001f800100000000002900000000000000 \
            01000000060000000600000000000000000000000000000000000000000000009d0000000000000001000000",
    };

    /// 8086:7d55/rev0, its known bits and which are known.
    const METEOR_LAKE_7D55_DATA: DataTable = DataTable {
        values: "\
            34d50500000002412011cf60a0710000000000001d20800f5080ec1d37a4458aa4e8062a200000000100000001000000 \
            30f7c2ea0950419bc005128891378006900a002000000000010000002000000000df23acdf2300000500000001000000 \
            f30119fb073cd23a517cb9f17fe31300bd1bde608fb7c19f36ef8da43b84027827a66807a00040000c37c0bcff0fee3b \
            e400c7323f716fe0ffdf059e5f46042ce317febf3000400200000000000000001f800000000000002900000000000000 \
            0100000000000000000000000000000000000000000000000000000000000000e30300000000000000000000",
        known: "\
            35dd05000001f7413011df60a0710000000000001d20800f5080ec1d37f4458ae4e816aa200100000100000001000000 \
            30ffe6ea0b7951dbc205128891378006900a002200000000010000002000000000ff33acff2310000500000003000000 \
            f30119fb073cd23a57fcf9f17fe31300bd1bdf60bff7c19f36ef8de43b8402f827b67807a00040004c37c0bfff0fee3b \
            e400c7f63f716feafffff5bf5f66042cef17febf3000400200000000000000001f800100000000002900000000000000 \
            0100000000000000060000000000000000000000000000000000000000000000e30300000000000000000000",
    };

    /// 8086:7d51, its known bits and which are known.
    const ARROW_LAKE_H_7D51_DATA: DataTable = DataTable {
        values: "\
            34d50500000002412011cf60a0710000000000001d20801f5080ec1d37a4418aa4e8062a200000000100000001000000 \
            30f7c2ea0950519bc005128891378006900a002000000000010000002000000000df23acdf7300000100000001000000 \
            f30119fb072eca3a517cb9f17fe31700bd1b5e608fb7c19f36ef8da43b8402f827a66807a80840000c37c0bcff0fee3b \
            e400c7323f716fe0ffdf059e5f46042ce317febf3000400600000000000000001f800000000000002900000000000000 \
            0100000000000000000000000000000000000000000000000000000000000000e30300000000000000000000",
        known: "\
            35dd05000001f7413011df60a0710000000000001d20801f5080ec1d37f4418aa4e816aa200100000100000001000000 \
            30ffe6ea0b7951dbc005128891378006900a002200000000010000002000000000ff33acff7310000500000003000000 \
            f30119fb072eca3a57fcf9f17fe31700bd1b5f60aff7c19f36ef8de43b8402f827b67807a8fe41004c37c0bfff0fee3b \
            e400c7f63f716feafffff5bf5f66042cef17febf3000400600000000000000001f800100000000002900000000000000 \
            0100000000000000060000000000000000000000000000000000000000000000e30300000000000000000000",
    };

    /// 8086:64a0, its known bits and which are known.
    const LUNAR_LAKE_64A0_DATA: DataTable = DataTable {
        values: "\
            34d50500000002012031efe0ae774000000000001d20c0075080ec9d3dac7e8ae6e80426600000000100000001000000 \
            30f7daea0950719bc705138891178066d10a003001000000010000002000000000df23c01f3304040800000001000000 \
            f323191b2724ea3a517cb9f147e31700bd1bdee08fb1c11f72eb8da43b4c02f8e71f29eea10840000c37c0bcff0fee3b \
            ec00c7321f716fe0ffdf059ebf4604ace1b7ff3f3060510600000000000000001f800000000000002900000000000000 \
            0100000000000000000000000000000000000000000000000000000000000000e30300000000000000000000",
        known: "\
            b5dd05000001f7013871ffe0ae774800000000001d20d0075080ec9d3ffc7f8ae6e81626600100000100000001000000 \
            30fffeea1b7971dbc707138891178066d10a003201000000010000002000000000ff33e07f3314040900000003000000 \
            f32319fb2724ea3a57fcf9f14fe31700bd1bdfe0aff1c19f76ef8de43b4c02f8e71f39eea1fe41004cb7debfff0fee3b \
            fc00c7f61f716feafffff5bfbf6604acedb7ff3f3060510600000000000000001f800100000000002900000000000000 \
            0100000000000000060000000000000000000000000000000000000000000000e30300000000000000000000",
    };

    /// 8086:b080, its known bits and which are known.
    const PANTHER_LAKE_B080_DATA: DataTable = DataTable {
        values: "\
            34d50500000002012031efe0ae774000000000001d20c01f50806c9d33ac7e8ae6e80626601100000100000001000000 \
            30f7daea0950711bc7051388911f8066d10a003001000000010000002000000000df23c01f1304041800000001000000 \
            f327191ba725ea3a517cb9f147e30f00bd1bdee08fb1c19f72eb8da43b4c02f8e71f21eea90850000c3fc0bcff0feefb \
            ec00c7321f716fe0ffdf059ebf4004ace1b7ff3ff060514e00000000000000001f800000000000002900000000000000 \
            0100000000000000000000000000000000000000000000000000000000000000e30200000000000000000000",
        known: "\
            b5dd05000001f7013871ffe0ae774880000000001d20d01f50806c9d33fc7f8ae6e81626601100000100000001000000 \
            30fffeea1b79715bc7071388911f8066d10a00b201000000010000002000000000ff33e07f13140c1900000003000000 \
            f32719fba725ea3a57fcf9f14fe30f00bd1bdfe0aff1c19f76ef8de43b4c12f8e79f35eea9fe51004cbfdebfff0feefb \
            fc00c7f61f716feafffff5bfbf6404acedb7ff3ff060514e00000000000000001f800100000000002900000000000000 \
            0100000000000000060000000000000000000000000000000000000000000000e30200000000000000000000",
    };

    /// 8086:fd80, its known bits and which are known.
    const WILDCAT_LAKE_FD80_DATA: DataTable = DataTable {
        values: "\
            34d505000000c2002031efe0ae774000000000001d20c00750806c9d33ac668ac6a80626611100000100000001000000 \
            30f7daea0950711bc7051388911f8066d10e003001000000010000002000000000df23c01d1304001800000001000000 \
            f327191ba725ea3a517cb9f147e30f00bd1bde608fb1c19f72eb8da43b4c02e0e71f21eea90850000c3fc0bcff0feefb \
            e400c7321f716fe0ffdf059ebe40042ce1b7ff3ff060510600000000000000001f800000000000002900000000000000 \
            0100000000000000000000000000000000000000000000000000000000000000e30200000000000000000000",
        known: "\
            b5dd05000001f7013871ffe0ae774880000000001d20d00750806c9d33fc678ac6e81626611100000100000001000000 \
            30fffeea1b79715bc7071388911f8066d10e003201000000010000002000000000ff33e07f1314081900000003000000 \
            f32719fba725ea3a57fcf9f14fe30f00bd1bdf60aff1c19f76ef8de43b4c02f8e79f35eea9fe51004cbfdebfff0feefb \
            fc00c7f61f716feafffff5bfbf64042cedb7ff3ff060510600000000000000001f800100000000002900000000000000 \
            0100000000000000060000000000000000000000000000000000000000000000e30200000000000000000000",
    };

    /// A card's feature table as Intel's kernel driver gives it: the values of its known bits, and
    /// which bits are known, in hex, the table's first byte first.
    struct DataTable {
        values: &'static str,
        known: &'static str,
    }

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// Every known bit where the record's table is not the driver's: (the driver's value, the
    /// record's), by the bit's place in the table.
    fn departures_from(record: &[u8], data: &DataTable) -> BTreeMap<usize, (bool, bool)> {
        let (values, known) = (hex(data.values), hex(data.known));
        assert_eq!((values.len(), known.len()), (card_data::TABLE_BYTES, card_data::TABLE_BYTES));
        let is = |bytes: &[u8], at: usize| bytes[at / 8] >> (at % 8) & 1 == 1;
        let table = &record[features::TABLE..];
        (0..card_data::TABLE_BYTES * 8)
            .filter(|&at| is(&known, at) && is(&values, at) != is(table, at))
            .map(|at| (at, (is(&values, at), is(table, at))))
            .collect()
    }

    fn place(feature: Feature) -> usize {
        (feature.byte - features::TABLE) * 8 + usize::from(feature.bit)
    }

    /// A flag where the record departs from the driver's table: the flag, the driver's value and
    /// the record's.
    type Departure = (Feature, bool, bool);

    /// The departures named: each flag, with the driver's value and the record's.
    fn named(departures: &[Departure]) -> BTreeMap<usize, (bool, bool)> {
        departures.iter().map(|&(feature, data, ours)| (place(feature), (data, ours))).collect()
    }

    /// The Direct3D 11 interface versions below 0x000B0023, the dword's five low bits, which a real
    /// card offers and a record without the URB stage limits does not, as it offers the newest alone.
    fn older_ddi_versions() -> Vec<Departure> {
        features::OLDER_DDI_VERSIONS.iter().map(|&version| (version, true, false)).collect()
    }

    /// The URB an Arc B580 gives through its hwconfig, as a tester's card logged it.
    fn b580_urb() -> Urb {
        Urb {
            size_kb: 3840,
            vertex: UrbEntries { min: 64, max: 5888 },
            hull: UrbEntries { min: 0, max: 1548 },
            geometry: UrbEntries { min: 2, max: 1952 },
            domain: UrbEntries { min: 50, max: 4480 },
            task: UrbEntries { min: 0, max: 1548 },
            mesh: UrbEntries { min: 4, max: 1952 },
            max_starting_address: 127,
        }
    }

    /// The Direct3D 11 interface versions a record offers, as the dword's low sixteen bits.
    fn ddi_versions(record: &[u8]) -> u32 {
        dword(record, at::DDI_VERSIONS).unwrap() & 0xFFFF
    }

    /// What every card's record departs from the driver's table in, where the driver says it.
    fn always_departs() -> Vec<Departure> {
        let mut departures = vec![
            (features::SVM, true, false),
            (features::COMPRESSION[1], true, false),
            (features::TILE_MAPPED_RESOURCE, true, false),
            (features::TRANSLATION_TABLE, true, false),
            (features::USER_MODE_TRANSLATION_TABLE, true, false),
            (features::NULL_PAGES, true, false),
            (features::RENDER_COMPRESSION, true, false),
            (features::DRIVER_FLR, true, false),
            (CROSS_ADAPTER_CAPABILITY, true, false),
            (features::GPGPU_MID_THREAD_PREEMPT, false, true),
        ];
        departures.extend(older_ddi_versions());
        departures
    }

    fn unnamed(index: usize) -> Feature {
        features::bit("unnamed", index)
    }

    /// The flags the driver's table doesn't know that this backend sets, by name.
    fn assert_backends_own(record: &[u8], name: &str) {
        for feature in [features::HW_SCHEDULING, features::WDDM2_64KB_PAGES, features::NEWEST_DDI_VERSION] {
            assert!(features::is_set(record, feature), "{name}: {}", feature.name);
        }
        for feature in [features::WDDM2_SVM, features::HW_SEMAPHORE_64] {
            assert!(!features::is_set(record, feature), "{name}: {}", feature.name);
        }
    }

    fn with(mut facts: RecordFacts, change: impl FnOnce(&mut RecordFacts)) -> RecordFacts {
        change(&mut facts);
        facts
    }

    /// A discrete card: memory of its own, which takes 4 KiB pages only on Battlemage.
    fn discrete(facts: RecordFacts, small_pages: bool) -> RecordFacts {
        with(facts, |facts| {
            facts.local_memory = true;
            facts.local_memory_small_pages = small_pages;
        })
    }

    fn older(facts: RecordFacts) -> RecordFacts {
        with(facts, |facts| facts.xe2_tiling = false)
    }

    #[test]
    fn every_family_gets_the_kernel_drivers_feature_table_but_where_this_record_departs() {
        let xe2_departures = || {
            let mut departures = always_departs();
            departures.extend([
                (features::COMPRESSION[0], true, false),
                (features::COMPRESSION[2], true, false),
                (features::COMPRESSION[3], true, false),
                (features::PML5, true, false),
                (unnamed(684), true, false),
            ]);
            departures
        };
        let cards: [(&str, RecordFacts, &DataTable, Vec<Departure>); 7] = [
            (
                "Alchemist G10 at revision 0",
                discrete(older(alchemist(0x5690, 0)), false),
                &ALCHEMIST_G10_REV_0_DATA,
                {
                    let mut departures = always_departs();
                    departures.extend([
                        // Its compute engine is xe's to give, and xe gives one.
                        (features::CCS_RING, false, true),
                        (features::CCS_NODE, false, true),
                        (features::CCS_MULTI_INSTANCE, true, false),
                    ]);
                    departures
                },
            ),
            ("Arc A770 at revision 8", discrete(older(alchemist(0x56A0, 8)), false), &ARC_A770_DATA, {
                let mut departures = always_departs();
                departures.extend([
                    (features::COMPRESSION[0], true, false),
                    (features::COMPRESSION[2], true, false),
                    // G10's table has no compute ring at any revision; xe gives this card one.
                    (features::CCS_RING, false, true),
                    (features::CCS_NODE, false, true),
                    (features::CCS_MULTI_INSTANCE, true, false),
                ]);
                departures
            }),
            (
                "Meteor Lake at revision 0",
                with(
                    older(card(1272, 0x7D55, IpWord::new(12, 70, 0), Some(IpWord::new(13, 0, 0)))),
                    |facts| {
                        facts.platform.revision = 0;
                    },
                ),
                &METEOR_LAKE_7D55_DATA,
                always_departs(),
            ),
            (
                "Arrow Lake H",
                older(card(1273, 0x7D51, IpWord::new(12, 74, 0), Some(IpWord::new(13, 0, 0)))),
                &ARROW_LAKE_H_7D51_DATA,
                {
                    let mut departures = always_departs();
                    departures.extend([
                        (features::COMPRESSION[0], true, false),
                        (unnamed(684), true, false),
                        (unnamed(686), true, false),
                    ]);
                    departures
                },
            ),
            (
                "Lunar Lake",
                card(1275, 0x64A0, IpWord::new(20, 4, 4), Some(IpWord::new(20, 0, 0))),
                &LUNAR_LAKE_64A0_DATA,
                xe2_departures(),
            ),
            (
                "Panther Lake",
                card(1300, 0xB080, IpWord::new(30, 0, 4), Some(IpWord::new(30, 0, 0))),
                &PANTHER_LAKE_B080_DATA,
                xe2_departures(),
            ),
            (
                "Wildcat Lake",
                card(1300, 0xFD80, IpWord::new(30, 3, 0), Some(IpWord::new(30, 2, 0))),
                &WILDCAT_LAKE_FD80_DATA,
                xe2_departures(),
            ),
        ];
        for (name, facts, data, departures) in cards {
            let (record, telling) = build_telling(&facts);
            assert_eq!(departures_from(&record, data), named(&departures), "{name}");
            assert_backends_own(&record, name);
            let CardData::Built(built) = telling.card_data else { panic!("{name} is covered") };
            assert_eq!(built.feature_bits, feature_bits(&record), "{name}");
        }
    }

    #[test]
    fn a_card_with_a_class_other_than_vga_loses_the_bit_that_says_it() {
        let lunar_lake = card(1275, 0x64A0, IpWord::new(20, 4, 4), Some(IpWord::new(20, 0, 0)));
        let vga = build(&lunar_lake);
        let other = build(&with(lunar_lake, |facts| facts.pci_class = 0x03_8000));
        let vga_bit = features::bit("VGA-compatible", 8);
        assert!(features::is_set(&vga, vga_bit) && !features::is_set(&other, vga_bit));
        assert_eq!(feature_bits(&vga), feature_bits(&other) + 1);
    }

    #[test]
    fn the_platform_words_are_the_kernel_drivers() {
        let read = |record: &[u8], at| dword(record, at).unwrap();
        let lunar_lake =
            with(card(1275, 0x64A0, IpWord::new(20, 4, 4), Some(IpWord::new(20, 0, 0))), |facts| {
                facts.subsystem = (0x1028, 0x0C3B);
            });
        let record = build(&lunar_lake);
        assert_eq!(read(&record, at::PLATFORM_TYPE), 2, "mobile");
        assert_eq!(read(&record, at::DISPLAY_CORE), 0xF00, "display 20.00's");
        assert_eq!(read(&record, at::RENDER_CORE), 0xC09);
        assert_eq!(read(&record, at::MEDIA_CORE), 0x1000, "media 20.00's");
        assert_eq!(read(&record, at::GT_TYPE), 0);
        assert_eq!(read(&record, at::PCH_FAMILY), 0, "left as it was");
        assert_eq!(
            (word(&record, at::SUBSYSTEM_VENDOR), word(&record, at::SUBSYSTEM)),
            (Some(0x1028), Some(0x0C3B))
        );
        assert_eq!(read(&record, at::DISPLAY_IP), IpWord::new(20, 0, 0).0, "the platform's, at revision 0");
        assert_eq!(read(&record, at::MEDIA_IP), IpWord::new(20, 0, 0).0);
        assert_eq!(word(&record, at::REVISION_AGAIN), Some(4));
        assert_eq!(read(&record, at::DISPLAY_IP_CORE), 0xF00);
        for slot in 0..4 {
            assert_eq!(read(&record, at::RENDER_IP_CORES + slot * 4), 0xC09);
            assert_eq!(read(&record, at::MEDIA_IP_CORES + slot * 4), 0x1000);
        }
        let a770 = build(&discrete(older(alchemist(0x56A0, 8)), false));
        assert_eq!((read(&a770, at::PLATFORM_TYPE), read(&a770, at::DISPLAY_CORE)), (1, 0x11), "desktop");
        assert_eq!(read(&a770, at::DISPLAY_IP), 0, "Alchemist has no IP versions");
        assert_eq!(read(&a770, at::DISPLAY_IP_CORE), 0, "nor per-IP cores");
        // Platform type, display core, media core and GT type.
        let words = |record: &[u8]| {
            [at::PLATFORM_TYPE, at::DISPLAY_CORE, at::MEDIA_CORE, at::GT_TYPE].map(|at| read(record, at))
        };
        for (name, facts, expected) in [
            (
                "Meteor Lake",
                older(card(1272, 0x7D55, IpWord::new(12, 70, 0), Some(IpWord::new(13, 0, 0)))),
                [2, 0xE00, 0xD00, 0],
            ),
            (
                "Arrow Lake",
                older(card(1273, 0x7D51, IpWord::new(12, 74, 0), Some(IpWord::new(13, 0, 0)))),
                [2, 0xE00, 0xD00, 0],
            ),
            (
                "Panther Lake",
                card(1300, 0xB080, IpWord::new(30, 0, 4), Some(IpWord::new(30, 0, 0))),
                [2, 0x1E00, 0x1E00, 0],
            ),
            (
                "Wildcat Lake",
                card(1300, 0xFD80, IpWord::new(30, 3, 0), Some(IpWord::new(30, 2, 0))),
                [2, 0x1E00, 0x1E00, 0],
            ),
        ] {
            assert_eq!(words(&build(&facts)), expected, "{name}");
        }
    }

    #[test]
    fn a_media_ip_the_data_has_no_core_for_leaves_it_at_zero_and_says_so() {
        let facts = card(1275, 0x64A0, IpWord::new(20, 4, 4), Some(IpWord::new(20, 9, 0)));
        let (record, telling) = build_telling(&facts);
        assert_eq!(dword(&record, at::MEDIA_CORE), Some(0));
        assert_eq!(dword(&record, at::MEDIA_IP), Some(IpWord::new(20, 9, 0).0));
        let CardData::Built(built) = telling.card_data else { panic!("covered") };
        assert_eq!(built.unknown, ["the media core of media 20.09"]);
        assert!(built.to_string().ends_with(" unknown=\"the media core of media 20.09\""));
    }

    #[test]
    fn every_hwconfig_key_goes_where_the_kernel_driver_puts_it_and_wins() {
        let facts = with(facts(), |facts| {
            facts.gt.max_eu_per_subslice = 16;
            facts.hwconfig = vec![(46, 2048), (60, 26), (11, 4), (25, 2), (71, 8), (68, 768), (999, 7)];
        });
        let (record, telling) = build_telling(&facts);
        let read = |at| dword(&record, at).unwrap();
        assert_eq!(read(0x2180), 2048, "ray tracing stacks per core");
        assert_eq!(read(0x22FC), 0, "bindless surface offset bits, kept at zero");
        assert_eq!(read(0x219C), 4, "memory type");
        assert_eq!((read(0x21A8), read(0x239C)), (2, 2), "the most video engines, in both places");
        assert_eq!(read(at::MAX_EU_PER_SUBSLICE), 8, "the key, over what was worked out without it");
        let CardData::Built(built) = telling.card_data else { panic!("covered") };
        assert_eq!(built.hwconfig_keys, 4, "two keys have no place in the record and one is kept out");
    }

    #[test]
    fn the_log_line_names_the_platform_the_bits_and_the_defaults() {
        let (record, telling) = build_telling(&facts());
        let CardData::Built(built) = telling.card_data else { panic!("covered") };
        assert_eq!(
            built.to_string(),
            format!(
                "platform=\"Lunar Lake\" device=0x64a0 pci_rev=0x04 bits={} hwconfig_keys=0 defaults=\"display IP by \
                 platform, display revision 0, the guest's Windows settings, PCI capabilities as the platform's cards \
                 have them, every display pipe present, a CPU that is not a Xeon, a fuse word xe doesn't report, as \
                 zero\"",
                feature_bits(&record)
            )
        );
        let arrow_lake = older(card(1273, 0x7D51, IpWord::new(12, 74, 0), Some(IpWord::new(13, 0, 0))));
        let CardData::Built(built) = build_telling(&arrow_lake).1.card_data else { panic!("covered") };
        assert!(built.defaults.contains(&SUBSYSTEM_DEFAULT) && !built.defaults.contains(&FUSE_WORD_DEFAULT));
    }

    #[test]
    fn a_device_the_data_does_not_list_keeps_this_backends_own_feature_table_and_platform_words() {
        // A Battlemage device the data has no table for, with a hwconfig key it would copy.
        let facts = with(card(1274, 0xE20D, IpWord::new(20, 1, 0), Some(IpWord::new(13, 1, 1))), |facts| {
            facts.subsystem = (0x8086, 0x1100);
            facts.hwconfig = vec![(46, 2048)];
            facts.local_memory = true;
            facts.local_memory_small_pages = true;
        });
        let (record, telling) = build_telling(&facts);
        assert_eq!(
            telling.card_data,
            CardData::Uncovered { device: 0xE20D, revision: 4, feature_bits: feature_bits(&record) }
        );
        // The memory model's three, hardware scheduling, Xe2's tiling, the engines' and preemption's
        // seven, memory on the card, the four pipeline levels and the one interface version.
        assert_eq!(feature_bits(&record), 18, "this backend's flags alone");
        assert!(!features::is_set(&record, features::LOCAL_MEMORY_ALLOWS_4KB));
        assert_eq!(dword(&record, at::GT_TYPE), Some(18));
        assert_eq!(dword(&record, at::DISPLAY_CORE), Some(0x0C09), "the generation's");
        for at in
            [at::SUBSYSTEM_VENDOR, at::MEDIA_CORE, at::DISPLAY_IP, at::MEDIA_IP, at::DISPLAY_IP_CORE, 0x2180]
        {
            assert_eq!(dword(&record, at), Some(0), "{at:#x}");
        }
    }

    #[test]
    fn the_arc_b580_gets_the_table_intels_kernel_driver_gives_it() {
        let b580 = RecordFacts {
            urb: b580_urb(),
            ..card(1274, 0xE20B, IpWord::new(20, 1, 0), Some(IpWord::new(13, 1, 1)))
        };
        let (record, outcome) = build_telling(&b580);
        let mut ours = table_of(&record);
        let mut real = expected(B580_REAL_CARD);
        assert_eq!(real.len(), 193);
        // Where this backend's record departs from a real card's, each entry by name: (entry, the
        // real card's value, this record's).
        let departures = [
            // This backend's own two, the virtual machine's and tiled resources: the same as a
            // real card's.
            (VIRTUAL_MACHINE_WORKAROUND.0, 1, 1),
            (TILED_RESOURCES_WORKAROUND.0, 1, 1),
            // The render cache keyed by binding table, which xe's queues aren't unless made so.
            (RENDER_CACHE_KEYING_WORKAROUND, 1, 0),
        ];
        for (entry, real_value, our_value) in departures {
            assert_eq!(real.remove(&entry).unwrap_or(0), real_value, "{entry}");
            assert_eq!(ours.remove(&entry).unwrap_or(0), our_value, "{entry}");
        }
        assert_eq!(ours, real);
        assert_eq!(features::workaround(&record, 2835), Some(1), "with the table's unnamed bit 445");
        assert_eq!(features::workaround(&record, 2964), Some(2), "the compositor follows it");
        assert_eq!(record[0x96A] & 0xF, 0x02, "the low half of the byte Direct3D 11 tests");
        let Outcome::Built(built) = outcome.workarounds else { panic!("the B580 is covered") };
        assert_eq!(built.entries, 193, "as many as a real card's");
    }

    #[test]
    fn the_arc_b580_keeps_its_render_cache_entry_when_every_render_queue_is_keyed_by_binding_table() {
        let b580 = card(1274, 0xE20B, IpWord::new(20, 1, 0), Some(IpWord::new(13, 1, 1)));
        let keyed = build(&RecordFacts { render_cache_keyed: true, ..b580.clone() });
        let plain = build(&b580);
        assert_eq!(features::workaround(&keyed, RENDER_CACHE_KEYING_WORKAROUND), Some(1), "the table's");
        assert_eq!(features::workaround(&plain, RENDER_CACHE_KEYING_WORKAROUND), Some(0));
        let (mut keyed_table, mut plain_table) = (table_of(&keyed), table_of(&plain));
        keyed_table.remove(&RENDER_CACHE_KEYING_WORKAROUND);
        plain_table.remove(&RENDER_CACHE_KEYING_WORKAROUND);
        assert_eq!(keyed_table, plain_table, "nothing else differs");
        let differ = keyed.iter().zip(&plain).filter(|(a, b)| a != b).count();
        assert_eq!(differ, 1, "one byte, the entry's");
    }

    #[test]
    fn every_other_family_gets_its_table() {
        let cards = [
            (
                "Lunar Lake",
                card(1275, 0x64A0, IpWord::new(20, 4, 4), Some(IpWord::new(20, 0, 0))),
                LUNAR_LAKE,
            ),
            (
                "Meteor Lake",
                card(1272, 0x7D55, IpWord::new(12, 70, 0), Some(IpWord::new(13, 0, 0))),
                METEOR_LAKE,
            ),
            (
                "Panther Lake",
                card(1300, 0xB080, IpWord::new(30, 0, 4), Some(IpWord::new(30, 0, 0))),
                PANTHER_LAKE,
            ),
            ("Alchemist G11 rev 0", alchemist(0x5693, 0), ALCHEMIST_G11_REV_0),
            ("Alchemist G11 rev 8", alchemist(0x5693, 8), ALCHEMIST_G11_REV_8),
            ("Arc A770", alchemist(0x56A0, 8), ALCHEMIST_A770),
        ];
        for (name, facts, table) in cards {
            let record = build(&facts);
            let mut real = expected(table);
            // Kept clear on every card, as on the B580, unless every render queue is keyed so.
            assert_eq!(table_of(&build(&RecordFacts { render_cache_keyed: true, ..facts })), real, "{name}");
            real.remove(&RENDER_CACHE_KEYING_WORKAROUND);
            assert_eq!(table_of(&record), real, "{name}");
        }
        assert_eq!(features::workaround(&build(&alchemist(0x56A0, 8)), 2964), Some(0));
    }

    #[test]
    fn a_card_whose_ip_the_data_does_not_cover_keeps_only_this_backends_entries() {
        let unknown = card(1274, 0xE20B, IpWord::new(20, 9, 0), Some(IpWord::new(13, 1, 1)));
        let (record, outcome) = build_telling(&unknown);
        assert_eq!(outcome.workarounds, Outcome::Uncovered("render 20.09".into()));
        assert_eq!(table_of(&record), BTreeMap::from([(535, 1), (968, 1)]));
    }

    #[test]
    fn the_virtual_machine_workaround_entry_is_set_and_its_neighbours_are_not() {
        let record = build(&facts());
        assert_eq!(features::workaround(&record, 968), Some(1));
        assert_eq!(record[0x584], 1);
        assert_eq!(features::workaround(&record, 969), Some(0));
        assert_eq!(features::workaround(&record, 967), Some(0));
    }

    #[test]
    fn clocks_counts_and_memory_land_where_they_belong() {
        let record = build(&facts());
        assert_eq!(dword(&record, at::TIMESTAMP_FREQUENCY), Some(19_200_000));
        assert_eq!(dword(&record, at::TIMESTAMP_FREQUENCY + 4), Some(0), "a 32-bit field");
        assert_eq!(dword(&record, at::THREADS_PER_EU), Some(8));
        let name = &record[at::ADAPTER_NAME..at::ADAPTER_NAME + ADAPTER_NAME_SIZE];
        assert_eq!(&name[..17], b"Intel GPU 0x64a0\0");
        assert_eq!(dword(&record, at::ENGINE_MAXIMA), Some(1), "render");
        assert_eq!(dword(&record, 0x21AC), Some(2), "MaxVECS, where compute-runtime's tokens put it");
        assert_eq!(dword(&record, at::ENGINE_MAXIMA + 16), Some(1), "copy");
        assert_eq!(dword(&record, at::MIN_RENDER_FREQUENCY), Some(400));
        assert_eq!(dword(&record, at::MAX_RENDER_FREQUENCY), Some(2050));
        assert_eq!(dword(&record, at::NUMBER_OF_EUS), Some(64));
        assert_eq!(dword(&record, at::EU_COUNT), Some(64));
        assert_eq!(dword(&record, at::THREAD_COUNT), Some(512));
        assert_eq!(dword(&record, at::DUAL_SUBSLICE_COUNT), Some(8));
        assert_eq!(qword(&record, at::SYSTEM_SHARED_MEMORY), Some(16 << 30));
        assert_eq!(qword(&record, at::DEDICATED_VIDEO_MEMORY), Some(0));
    }

    #[test]
    fn the_urb_goes_where_gt_system_info_has_it_and_reads_back() {
        let urb = b580_urb();
        let record = build(&RecordFacts { urb, ..facts() });
        let read = |at| dword(&record, at).unwrap();
        assert_eq!(read(0x211C), 3840);
        assert_eq!([read(0x2120), read(0x2124)], [64, 5888], "vertex");
        assert_eq!([read(0x2130), read(0x2134)], [0, 1548], "hull");
        assert_eq!([read(0x2138), read(0x213C)], [2, 1952], "geometry");
        assert_eq!([read(0x2140), read(0x2144)], [50, 4480], "domain");
        assert_eq!([read(0x2150), read(0x2154), read(0x2158), read(0x215C)], [0, 1548, 4, 1952]);
        assert_eq!(read(0x2174), 127);
        assert_eq!(super::urb(&record), urb);
        assert_eq!(
            urb.to_string(),
            "size_kb=3840 vs=64-5888 hs=0-1548 gs=2-1952 ds=50-4480 task=0-1548 mesh=4-1952 max_start=127"
        );
    }

    #[test]
    fn the_timestamp_frequency_is_not_written_over_the_vertex_stages_most_entries() {
        let record = build(&facts());
        assert_eq!(dword(&record, at::TIMESTAMP_FREQUENCY), Some(19_200_000));
        assert_eq!(dword(&record, at::VS_URB_ENTRIES + 4), Some(0));
    }

    #[test]
    fn the_kernel_drivers_interface_version_is_a_gap_left_at_zero() {
        assert_eq!(dword(&build(&facts()), 0), Some(0));
    }

    #[test]
    fn the_registry_path_names_the_device_key_and_ends_in_zero() {
        let record = build(&facts());
        let field = &record[at::DEVICE_REGISTRY_PATH..at::PRODUCT_FAMILY];
        let end = field.iter().position(|&byte| byte == 0).unwrap();
        assert_eq!(&field[..end], br"\REGISTRY\MACHINE\SOFTWARE\Guesthouse\Intel adapter");
        assert!(field[end..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn the_record_carries_the_partition_the_escape_gives() {
        let record = build(&facts());
        assert_eq!(
            partition::read(&record[at::PARTITION..at::PARTITION + partition::SIZE]),
            partition::ours()
        );
        assert_eq!(
            at::PARTITION + partition::SIZE,
            at::FROM_VIRTUAL_MACHINE,
            "it ends where the next field starts"
        );
    }

    #[test]
    fn a_record_with_the_urb_stage_limits_offers_every_direct3d_11_interface_version() {
        let b580 = card(1274, 0xE20B, IpWord::new(20, 1, 0), Some(IpWord::new(13, 1, 1)));
        for (name, facts) in [("a card the data lists", b580), ("a card it doesn't", facts())] {
            let record = build(&RecordFacts { urb: b580_urb(), ..facts });
            assert_eq!(ddi_versions(&record), 0xFFFF, "{name}");
        }
    }

    #[test]
    fn a_record_short_of_a_stage_limit_offers_the_newest_direct3d_11_interface_version_alone() {
        assert_eq!(ddi_versions(&build(&facts())), 1 << 15, "0x000B002D, and none of the older ones");
        let short_of: [fn(&mut Urb); 4] = [
            |urb| urb.vertex.max = 0,
            |urb| urb.hull.max = 0,
            |urb| urb.domain.max = 0,
            |urb| urb.geometry.max = 0,
        ];
        for (stage, clear) in short_of.into_iter().enumerate() {
            let mut urb = b580_urb();
            clear(&mut urb);
            assert_eq!(ddi_versions(&build(&RecordFacts { urb, ..facts() })), 1 << 15, "stage {stage}");
        }
        let mut urb = b580_urb();
        (urb.task.max, urb.mesh.max) = (0, 0);
        assert_eq!(
            ddi_versions(&build(&RecordFacts { urb, ..facts() })),
            0xFFFF,
            "task and mesh don't count"
        );
    }

    #[test]
    fn the_arc_b580_without_the_stage_limits_departs_from_a_real_cards_table_in_six_entries() {
        let b580 = card(1274, 0xE20B, IpWord::new(20, 1, 0), Some(IpWord::new(13, 1, 1)));
        let with_limits = table_of(&build(&RecordFacts { urb: b580_urb(), ..b580.clone() }));
        let without = table_of(&build(&b580));
        let entries: BTreeMap<_, _> = (1400..1416)
            .filter(|entry| with_limits.get(entry) != without.get(entry))
            .map(|entry| (entry, (with_limits.get(&entry).copied(), without.get(&entry).copied())))
            .collect();
        let one = Some(1);
        assert_eq!(
            entries,
            BTreeMap::from([
                (1404, (None, one)),
                (1405, (one, None)),
                (1406, (None, one)),
                (1408, (one, None)),
                (1409, (None, one)),
                (1412, (None, one)),
            ])
        );
    }

    #[test]
    fn with_the_stage_limits_the_older_versions_are_offered_as_the_drivers_table_offers_them() {
        let lunar_lake = card(1275, 0x64A0, IpWord::new(20, 4, 4), Some(IpWord::new(20, 0, 0)));
        let without = departures_from(&build(&lunar_lake), &LUNAR_LAKE_64A0_DATA);
        let with_limits =
            departures_from(&build(&RecordFacts { urb: b580_urb(), ..lunar_lake }), &LUNAR_LAKE_64A0_DATA);
        for version in features::OLDER_DDI_VERSIONS {
            assert_eq!(without.get(&place(version)), Some(&(true, false)), "{}", version.name);
            assert_eq!(with_limits.get(&place(version)), None, "{}", version.name);
        }
        assert_eq!(without.len(), with_limits.len() + features::OLDER_DDI_VERSIONS.len());
    }

    #[test]
    fn the_first_ask_is_for_the_adapter_count() {
        let mut out = Vec::new();
        let (status, asked) = answer(&build(&facts()), &query(COUNT_SIZE, &[0; 4]), &mut out);
        assert_eq!((status, asked), (Status::SUCCESS, Asked::Count));
        assert_eq!(out, 1u32.to_le_bytes());
    }

    #[test]
    fn the_record_goes_back_with_the_guests_build_stamp() {
        let mut ask = vec![0u8; RECORD_SIZE];
        put_dword(&mut ask, at::BUILD_STAMP, 0x000F_0001);
        let mut out = vec![0xAA; 4];
        let (status, asked) = answer(&build(&facts()), &query(RECORD_SIZE, &ask), &mut out);
        assert_eq!(status, Status::SUCCESS);
        assert_eq!(asked, Asked::Record { build_stamp: 0x000F_0001 });
        assert_eq!(out.len(), 4 + RECORD_SIZE);
        assert_eq!(dword(&out[4..], at::BUILD_STAMP), Some(0x000F_0001));
        assert_eq!(dword(&out[4..], at::RENDER_CORE), Some(0x0C09));
    }

    #[test]
    fn any_other_size_is_refused_as_the_kernel_driver_refuses_it() {
        for size in [0, 8, RECORD_SIZE - 1, RECORD_SIZE + 1, 2 * RECORD_SIZE] {
            let mut out = Vec::new();
            let (status, asked) = answer(&build(&facts()), &query(size, &[]), &mut out);
            assert_eq!(status, Status::INVALID_PARAMETER, "{size}");
            assert!(matches!(asked, Asked::Refused(_)), "{size}");
            assert!(out.is_empty());
        }
    }

    #[test]
    fn the_summary_reads_back_what_was_built() {
        let text = summary(&build(&facts())).to_string();
        assert_eq!(
            text,
            "product_family=1275 render_core=0xc09 device=0x64a0 render_ip=0x5010004 eus=64 vm=1 compression=off"
        );
    }
}
