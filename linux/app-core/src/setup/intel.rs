//! What a Windows guest on an Intel card needs from setup: the user-mode half of Intel's Windows
//! driver, taken out of Intel's installer, and its version. The daemon reads them from `intel.json`
//! in the setup folder. The card itself is the machine's own choice, so the file names none.
//!
//! The Intel daemon tells Windows where each driver is by its file name in one folder, as AMD's
//! does, so every file it names has to be in the folder the guest gets.

use crate::context::Context;
use crate::failure::Result;
use crate::gpu_maker::GpuMaker;
use crate::render_cards::{RenderCard, render_card, render_cards};
use crate::settings::write_json;
use guesthouse_backend_intel::generation::{self, Generation};
use guesthouse_backend_intel::protocol::guest_driver::{CardDrivers, OPENCL_32_BIT};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// The list, beside the driver's files, of what goes to the guest's system folders: folder, file and
/// name, split by tabs, as for NVIDIA.
pub const SYSTEM_LIST: &str = "system-files.txt";
const INTEL_SETUP_FILE: &str = "intel.json";
const DRIVER_FOLDER_FIELD: &str = "driver_folder";
const DRIVER_VERSION_FIELD: &str = "driver_version";
pub const NO_INTEL_SETUP: &str = "A Windows machine on an Intel card needs Intel's Windows driver unpacked first, \
                                  and it hasn't been.";

/// The files the Intel daemon names to Windows for every card: the Direct3D drivers for 64-bit and
/// 32-bit programs, OpenCL's and Level Zero's for 64-bit programs, and the Vulkan manifests. The rest
/// differ by the kind of card: see [`card_kinds`]. A test holds the daemon to these.
pub const NAMED_FILES: [&str; 10] = [
    "igd9trinity64.dll",
    "igd10iumd64.dll",
    "igd12umd64.dll",
    "igd9trinity32.dll",
    "igd10iumd32.dll",
    "igd12umd32.dll",
    "igdrcl64.dll",
    "igvk64.json",
    "igvk32.json",
    "ze_intel_gpu64.dll",
];

/// Each kind of card's own drivers, as the daemon names them for the generations it knows: the
/// OpenGL pair, the kernel driver and, for most, 32-bit OpenCL. The kernel driver is named as the
/// adapter's image path. The guest never loads it, as the paravirtual adapter's kernel driver is
/// Windows' own, but Windows won't start the adapter unless the file is in the guest's copy of the
/// driver, as NVIDIA's and AMD's are. A driver folder is unpacked for one card, so it holds one kind's.
pub fn card_kinds() -> Vec<CardDrivers> {
    let mut kinds: Vec<CardDrivers> = Vec::new();
    for generation in generation::known() {
        if !kinds.contains(&generation.drivers) {
            kinds.push(generation.drivers);
        }
    }
    kinds
}

/// The files one kind of card's drivers are.
pub fn kind_files(drivers: &CardDrivers) -> Vec<&'static str> {
    let mut files = vec![drivers.opengl, drivers.opengl_32_bit, drivers.kernel];
    files.extend(drivers.opencl_32_bit.then_some(OPENCL_32_BIT));
    files
}

/// The Linux driver the Intel daemon works through, and the older one that takes most Intel cards
/// from before Lunar Lake unless told not to.
const XE_DRIVER: &str = "xe";
const I915_DRIVER: &str = "i915";

/// Cards xe takes that Intel's Windows package has no driver for, by PCI device id: Nova Lake's and
/// Crescent Island's, from the kernel's list.
const NO_WINDOWS_DRIVER: [u32; 21] = [
    0xD740, 0xD741, 0xD742, 0xD743, 0xD745, 0xD74A, 0xD74B, 0xD750, 0xD751, 0xD752, 0xD753, 0xD754, 0xD755,
    0xD756, 0xD757, 0xD75F, 0x674C, 0x674D, 0x674E, 0x674F, 0x6750,
];

/// What a Windows machine can make of an Intel card in this PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntelCard {
    /// It can be set up for.
    Offered,
    /// It's of a generation the daemon knows, but i915 has it, and the daemon needs xe.
    OnI915(&'static Generation),
    /// It's not one the daemon is written for: an older Intel iGPU, which every PC with an Intel
    /// processor has, or a newer card Intel's Windows package doesn't cover.
    NotOffered,
}

/// The Linux driver bound to a card, from the link sysfs keeps beside its render node.
fn bound_driver(context: &Context, card: &RenderCard) -> Option<String> {
    let node = card.render_node.file_name()?;
    let link = fs::read_link(context.drm_class.join(node).join("device").join("driver")).ok()?;
    Some(link.file_name()?.to_string_lossy().into_owned())
}

/// What a Windows machine can make of `card`. A card of a generation the daemon knows is offered,
/// whatever its id, as the daemon tells its generation from the card itself. So is a card on xe of
/// an id no generation lists, as the daemon says when it can't place one and asks for a recording.
pub fn intel_card(context: &Context, card: &RenderCard) -> IntelCard {
    // A recorded card has no node, and stands in for a card on xe.
    if card.render_node.as_os_str().is_empty() {
        return IntelCard::Offered;
    }
    let driver = bound_driver(context, card);
    let known = generation::for_device(card.device as u16);
    match (driver.as_deref(), known) {
        (Some(I915_DRIVER), Some(generation)) => IntelCard::OnI915(generation),
        (Some(I915_DRIVER), None) => IntelCard::NotOffered,
        (_, Some(_)) => IntelCard::Offered,
        (Some(XE_DRIVER), None) if !NO_WINDOWS_DRIVER.contains(&card.device) => IntelCard::Offered,
        _ => IntelCard::NotOffered,
    }
}

/// The Intel cards in this PC that a Windows machine can be set up for.
pub fn supported_intel_cards(context: &Context) -> Vec<RenderCard> {
    let cards = render_cards(context, GpuMaker::Intel);
    cards.into_iter().filter(|card| intel_card(context, card) == IntelCard::Offered).collect()
}

/// The Intel cards in this PC the daemon could use if xe had them instead of i915, with their
/// generation.
pub fn intel_cards_on_i915(context: &Context) -> Vec<(RenderCard, &'static Generation)> {
    let cards = render_cards(context, GpuMaker::Intel).into_iter();
    cards
        .filter_map(|card| match intel_card(context, &card) {
            IntelCard::OnI915(generation) => Some((card, generation)),
            _ => None,
        })
        .collect()
}

/// What to add to the kernel's command line so xe takes the card `device` from i915.
pub fn xe_force_probe(device: u32) -> String {
    format!("i915.force_probe=!{device:04x} xe.force_probe={device:04x}")
}

/// Vulkan's manifests name the library the loader opens, beside them.
const VULKAN_MANIFESTS: [&str; 2] = ["igvk64.json", "igvk32.json"];
const SAME_FOLDER: &str = ".\\";

fn intel_setup_file(context: &Context) -> PathBuf {
    context.paths.setup_folder().join(INTEL_SETUP_FILE)
}

/// Where Intel's Windows driver was unpacked, if it was, whatever it holds.
fn unpacked_folder(context: &Context) -> Option<PathBuf> {
    let text = fs::read_to_string(intel_setup_file(context)).ok()?;
    let setup: Value = serde_json::from_str(&text).ok()?;
    let folder = PathBuf::from(setup.get(DRIVER_FOLDER_FIELD)?.as_str()?);
    folder.is_dir().then_some(folder)
}

/// Where Intel's Windows driver was unpacked, if it was and has every file Windows is told of. One
/// unpacked by an older version may lack some, and counts as not unpacked, so it's asked for again.
pub fn intel_driver_folder(context: &Context) -> Option<PathBuf> {
    unpacked_folder(context).filter(|folder| folder_problem(folder).is_none())
}

/// The version of Intel's Windows driver that was unpacked, if it says.
pub fn intel_driver_version(context: &Context) -> Option<String> {
    let text = fs::read_to_string(intel_setup_file(context)).ok()?;
    let setup: Value = serde_json::from_str(&text).ok()?;
    Some(setup.get(DRIVER_VERSION_FIELD)?.as_str()?.to_owned())
}

/// Writes down where Intel's driver was unpacked, and its version, for the daemon.
pub fn write_intel_setup(context: &Context, folder: &Path, version: &str) -> Result<()> {
    let setup = json!({ DRIVER_FOLDER_FIELD: folder, DRIVER_VERSION_FIELD: version });
    write_json(&intel_setup_file(context), &setup)
}

/// The libraries a Vulkan manifest in the folder names for its driver.
fn manifest_libraries(folder: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for manifest in VULKAN_MANIFESTS {
        let Ok(text) = fs::read_to_string(folder.join(manifest)) else { continue };
        let Ok(manifest) = serde_json::from_str::<Value>(&text) else { continue };
        if let Some(path) = manifest["ICD"]["library_path"].as_str() {
            found.push(path.strip_prefix(SAME_FOLDER).unwrap_or(path).to_owned());
        }
    }
    found
}

/// The files Windows will be told to load that aren't in the folder.
pub fn missing_files(folder: &Path) -> Vec<String> {
    let mut wanted: Vec<String> = NAMED_FILES.iter().map(|name| (*name).to_owned()).collect();
    wanted.extend(manifest_libraries(folder));
    // The OpenGL pair the folder has most of says which kind of card it's for.
    let lacking = |name: &str| !folder.join(name).is_file();
    let kinds = card_kinds();
    let kind = kinds.iter().min_by_key(|kind| {
        [kind.opengl, kind.opengl_32_bit].into_iter().filter(|name| lacking(name)).count()
    });
    wanted.extend(kind.map(kind_files).unwrap_or_default().into_iter().map(str::to_owned));
    wanted.retain(|name| !folder.join(name).is_file());
    wanted.sort();
    wanted
}

/// Says what the folder lacks, if it lacks anything.
pub fn folder_problem(folder: &Path) -> Option<String> {
    let missing = missing_files(folder);
    (!missing.is_empty()).then(|| {
        format!(
            "Intel's driver lacks {}, which Windows is told of, so it can't use the card.",
            missing.join(", ")
        )
    })
}

/// Why a Windows machine can't start on the Intel card at `pci`, if it can't.
pub fn intel_start_problems(context: &Context, pci: &str) -> Vec<String> {
    let mut found = Vec::new();
    match unpacked_folder(context).as_deref().map(missing_files) {
        None => found.push(NO_INTEL_SETUP.to_owned()),
        Some(missing) if !missing.is_empty() => found.push(format!(
            "Intel's driver on this PC lacks {}, which Windows needs. It was unpacked by an older version \
             of Guesthouse, which left that out. Give Intel's installer again: choose New Machine, pick a \
             Windows ISO and the Intel card, and choose the installer where it is asked for. Close the \
             dialog once it's unpacked, and start this machine again.",
            missing.join(", ")
        )),
        Some(_) => {}
    }
    if render_card(context, GpuMaker::Intel, pci).is_none() {
        found.push(format!(
            "The Intel card this machine runs on, at {pci}, isn't in this PC now. Choose another Intel card \
             in its settings."
        ));
    }
    found
}

#[cfg(test)]
pub mod stand_in {
    use crate::testing::Scratch;
    use guesthouse_backend_intel::protocol::guest_driver::XE2_LPG_DRIVERS;

    /// A Vulkan manifest whose driver is `library`, beside it.
    pub fn manifest(library: &str) -> String {
        serde_json::json!({"ICD": {"library_path": format!(".\\{library}")}}).to_string()
    }

    /// Writes an unpacked driver folder `folder` in `scratch` with every file the daemon names, and
    /// the libraries the manifests name. Each file holds its own path.
    pub fn lay_out_driver(scratch: &Scratch, folder: &str) {
        let lunar_lake = super::kind_files(&XE2_LPG_DRIVERS);
        for name in super::NAMED_FILES.iter().chain(&lunar_lake) {
            scratch.write(&format!("{folder}/{name}"), format!("{folder}/{name}"));
        }
        scratch.write(&format!("{folder}/igvk64.json"), manifest("igvk64.dll"));
        scratch.write(&format!("{folder}/igvk32.json"), manifest("igvk32.dll"));
        scratch.write(&format!("{folder}/igvk64.dll"), "vulkan");
        scratch.write(&format!("{folder}/igvk32.dll"), "vulkan");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::stand_in::lay_out_driver;
    use super::*;
    use crate::linux::amd::stand_in::fake_render_node;
    use crate::testing::{Scratch, scratch_context};
    use guesthouse_backend_intel::protocol::guest_driver::{
        XE2_HPG_DRIVERS, XE2_LPG_DRIVERS, XE3_LPG_DRIVERS,
    };
    use regex::Regex;
    use std::collections::BTreeSet;

    const CARD: &str = "0000:00:02.0";

    #[test]
    fn only_the_card_the_daemon_is_written_for_counts() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_render_node(&scratch, 128, CARD, "0x8086", 0x46a6);
        fake_render_node(&scratch, 129, "0000:00:03.0", "0x8086", 0x64a0);
        let found: Vec<String> = supported_intel_cards(&context).into_iter().map(|card| card.pci).collect();
        assert_eq!(found, ["0000:00:03.0"]);
    }

    #[test]
    fn with_no_driver_and_no_card_both_are_said() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let found = intel_start_problems(&context, CARD);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0], NO_INTEL_SETUP);
        assert!(found[1].contains("at 0000:00:02.0, isn't in this PC now"));
    }

    #[test]
    fn with_the_driver_unpacked_and_the_card_here_it_can_start() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        lay_out_driver(&scratch, "intel-driver");
        let folder = scratch.path("intel-driver");
        write_intel_setup(&context, &folder, "32.0.101.9175").unwrap();
        fake_render_node(&scratch, 128, CARD, "0x8086", 0x64a0);
        fake_render_node(&scratch, 129, "0000:7c:00.0", "0x1002", 0x13c0);
        assert_eq!(intel_driver_folder(&context), Some(folder));
        assert_eq!(intel_start_problems(&context, CARD), Vec::<String>::new());
        let on_amd = intel_start_problems(&context, "0000:7c:00.0");
        assert_eq!(on_amd.len(), 1, "an AMD card is not an Intel one: {on_amd:?}");
    }

    #[test]
    fn setup_is_written_for_the_daemon_by_the_names_it_reads() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = scratch.path("driver");
        fs::create_dir_all(&folder).unwrap();
        write_intel_setup(&context, &folder, "32.0.101.9175").unwrap();
        let text = fs::read_to_string(intel_setup_file(&context)).unwrap();
        let written: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(written["driver_folder"], folder.display().to_string());
        assert_eq!(written["driver_version"], "32.0.101.9175");
    }

    #[test]
    fn a_file_the_daemon_names_or_a_manifest_names_has_to_be_there() {
        let scratch = Scratch::new();
        lay_out_driver(&scratch, "driver");
        let folder = scratch.path("driver");
        assert_eq!(folder_problem(&folder), None);
        fs::remove_file(folder.join("igvk32.dll")).unwrap();
        fs::remove_file(folder.join("igd12umd64.dll")).unwrap();
        let problem = folder_problem(&folder).unwrap();
        assert!(problem.contains("igd12umd64.dll, igvk32.dll"), "{problem}");
    }

    #[test]
    fn a_folder_needs_one_cards_opengl_drivers_whole() {
        let scratch = Scratch::new();
        lay_out_driver(&scratch, "driver");
        let folder = scratch.path("driver");
        for name in [XE2_LPG_DRIVERS.opengl, XE2_LPG_DRIVERS.opengl_32_bit] {
            fs::rename(folder.join(name), folder.join(name.replace("lpg", "hpg"))).unwrap();
        }
        assert_eq!(missing_files(&folder), [XE2_HPG_DRIVERS.kernel], "that kind of card's kernel driver too");
        fs::rename(folder.join(XE2_LPG_DRIVERS.kernel), folder.join(XE2_HPG_DRIVERS.kernel)).unwrap();
        assert_eq!(folder_problem(&folder), None, "the other kind of card's files will do");
        fs::remove_file(folder.join("igxe2hpgicd32.dll")).unwrap();
        assert_eq!(missing_files(&folder), ["igxe2hpgicd32.dll"]);
    }

    /// A B580's folder as 0.6.2-intel.2 and .3 unpacked it, with the files they named, does for this
    /// version too: Intel's installer needn't be given again.
    #[test]
    fn a_b580_folder_an_older_app_unpacked_needs_nothing_more() {
        let scratch = Scratch::new();
        let older_named = [
            "igd9trinity64.dll",
            "igd10iumd64.dll",
            "igd12umd64.dll",
            "igd9trinity32.dll",
            "igd10iumd32.dll",
            "igd12umd32.dll",
            "igdrcl64.dll",
            "igdrcl32.dll",
            "igvk64.json",
            "igvk32.json",
            "ze_intel_gpu64.dll",
        ];
        let older_b580 = ["igxe2hpgicd64.dll", "igxe2hpgicd32.dll", "igdkmdnd64.sys"];
        for name in older_named.iter().chain(&older_b580) {
            scratch.write(&format!("driver/{name}"), name);
        }
        scratch.write("driver/igvk64.json", stand_in::manifest("igvk64.dll"));
        scratch.write("driver/igvk32.json", stand_in::manifest("igvk32.dll"));
        scratch.write("driver/igvk64.dll", "vulkan");
        scratch.write("driver/igvk32.dll", "vulkan");
        assert_eq!(missing_files(&scratch.path("driver")), Vec::<String>::new());
    }

    /// Panther Lake's folder has no 32-bit OpenCL, as Intel's INF gives it none, and needs none.
    #[test]
    fn a_kind_of_card_without_32_bit_opencl_does_without_it() {
        let scratch = Scratch::new();
        lay_out_driver(&scratch, "driver");
        let folder = scratch.path("driver");
        fs::remove_file(folder.join(OPENCL_32_BIT)).unwrap();
        assert_eq!(missing_files(&folder), [OPENCL_32_BIT], "Lunar Lake's needs it");
        for (from, to) in [
            (XE2_LPG_DRIVERS.opengl, XE3_LPG_DRIVERS.opengl),
            (XE2_LPG_DRIVERS.opengl_32_bit, XE3_LPG_DRIVERS.opengl_32_bit),
        ] {
            fs::rename(folder.join(from), folder.join(to)).unwrap();
        }
        assert_eq!(folder_problem(&folder), None);
    }

    /// A folder unpacked by 0.4.1-intel.2 or 0.6.2-intel.1 has everything but the kernel driver. It
    /// isn't taken for Intel's driver, so the dialog asks for the installer again, and a start says why.
    #[test]
    fn a_folder_unpacked_without_the_kernel_driver_is_asked_for_again() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        lay_out_driver(&scratch, "intel-driver");
        let folder = scratch.path("intel-driver");
        fs::remove_file(folder.join(XE2_LPG_DRIVERS.kernel)).unwrap();
        write_intel_setup(&context, &folder, "32.0.101.9175").unwrap();
        fake_render_node(&scratch, 128, CARD, "0x8086", 0x64a0);
        assert_eq!(intel_driver_folder(&context), None);
        let found = intel_start_problems(&context, CARD);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("Intel's driver on this PC lacks igdkmdn64.sys"), "{}", found[0]);
        assert!(found[0].contains("Give Intel's installer again"), "{}", found[0]);
    }

    #[test]
    fn every_generations_drivers_are_a_kind_of_card() {
        let kinds = card_kinds();
        for generation in generation::known() {
            assert!(kinds.contains(&generation.drivers), "{}", generation.name);
        }
        let mut opengl: Vec<&str> = kinds.iter().map(|kind| kind.opengl).collect();
        opengl.dedup();
        assert_eq!(opengl.len(), kinds.len(), "the OpenGL pair tells the kinds apart");
    }

    /// Binds the card at `pci` to `driver`, as sysfs shows it.
    pub fn bind(scratch: &Scratch, pci: &str, driver: &str) {
        let folder = scratch.path(&format!("drivers/{driver}"));
        fs::create_dir_all(&folder).unwrap();
        std::os::unix::fs::symlink(folder, scratch.path(&format!("pci/{pci}/driver"))).unwrap();
    }

    #[test]
    fn a_card_of_a_known_generation_is_offered_by_any_id_on_xe_but_not_on_i915() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        for (node, pci, device, driver) in [
            (128, "0000:00:02.0", 0xB080, "xe"),
            (129, "0000:03:00.0", 0xE2FF, "xe"),
            (130, "0000:04:00.0", 0xD740, "xe"),
            (131, "0000:05:00.0", 0x46A6, "i915"),
            (132, "0000:06:00.0", 0x64A0, "i915"),
        ] {
            fake_render_node(&scratch, node, pci, "0x8086", device);
            bind(&scratch, pci, driver);
        }
        let found: Vec<String> = supported_intel_cards(&context).into_iter().map(|card| card.pci).collect();
        assert_eq!(
            found,
            ["0000:00:02.0", "0000:03:00.0"],
            "Panther Lake, and an id no generation lists yet"
        );
        let on_i915 = intel_cards_on_i915(&context);
        assert_eq!(on_i915.len(), 1, "Alder Lake's iGPU is not one the daemon knows");
        assert_eq!((on_i915[0].0.device, on_i915[0].1.name), (0x64A0, "Xe2 LPG (Lunar Lake)"));
    }

    #[test]
    fn xe_is_told_to_take_a_card_by_its_id_in_hex() {
        assert_eq!(xe_force_probe(0x56A0), "i915.force_probe=!56a0 xe.force_probe=56a0");
    }

    #[test]
    fn a_battlemage_card_counts_too() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_render_node(&scratch, 128, "0000:03:00.0", "0x8086", 0xe20b);
        assert_eq!(supported_intel_cards(&context).len(), 1);
    }

    /// The file names in the daemon's answers, from its source: a name the daemon gives that isn't
    /// here would never be checked for.
    #[test]
    fn the_named_files_are_the_ones_the_daemon_names() {
        let file_name = Regex::new(r#""([A-Za-z0-9_-]+\.(?:dll|json|sys))""#).unwrap();
        let path = format!("{}/../backend-intel/src/protocol/guest_driver.rs", env!("CARGO_MANIFEST_DIR"));
        let text = fs::read_to_string(path).unwrap();
        let named: BTreeSet<String> =
            file_name.captures_iter(&text).map(|found| found[1].to_owned()).collect();
        let kinds = card_kinds();
        let per_kind = kinds.iter().flat_map(kind_files);
        let ours: BTreeSet<String> = NAMED_FILES.iter().copied().chain(per_kind).map(str::to_owned).collect();
        assert_eq!(named, ours);
    }
}
