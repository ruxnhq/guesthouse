//! What one graphics architecture changes, picked by the IP version xe reports for the card.
//!
//! The adapter record names the platform and core families, which have to be right for the card,
//! so those come from here, along with what the card can't be asked for. The numbers are Intel's
//! open ones: the family and core enums from gmmlib's `igfxfmid.h`, and the rest from
//! compute-runtime's hardware tables for the platform. Both are MIT. The driver file names are the
//! ones Intel's INF gives these cards. The device ids are the Linux kernel's, from
//! `include/drm/intel/pciids.h`.
//!
//! A card is matched by its render IP first, so a card of a known generation works whatever its
//! device id. The device id settles it only where the IP doesn't: two platforms that share an IP,
//! and a platform from before GMD_ID, for which xe reports no IP at all. A card that matches nothing
//! is refused, as the guest would be told the wrong platform.
//!
//! What no card of a generation has confirmed yet is in its [`Generation::guesses`], which the
//! daemon logs at start.

use crate::protocol::guest_driver::{
    CardDrivers, XE_HPG_DRIVERS, XE_LPG_DRIVERS, XE2_HPG_DRIVERS, XE2_LPG_DRIVERS, XE3_LPG_DRIVERS,
};
use crate::xe_query::IpVersion;

/// One architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation {
    pub name: &'static str,
    /// The render IPs' major and minor versions, as xe gives them from the card's GMD_ID. Empty for
    /// a platform from before GMD_ID.
    pub render_ips: &'static [(u16, u16)],
    /// The platform's PCI device ids, as the kernel lists them.
    pub devices: &'static [u16],
    /// Whether xe takes these cards by default. Otherwise i915 does, and xe only with its
    /// `force_probe` option naming the card.
    pub xe_by_default: bool,
    /// PRODUCT_FAMILY.
    pub product_family: u32,
    /// PCH_PRODUCT_FAMILY. The platform has no separate PCH to name.
    pub pch_family: u32,
    /// GFXCORE_FAMILY of the display and render cores.
    pub display_core: u32,
    pub render_core: u32,
    /// PLATFORM_TYPE.
    pub platform_type: u32,
    /// GTTYPE, left undefined.
    ///
    /// The display core, the platform type and the GT type are the record's only for a card the
    /// kernel driver's data doesn't list: one it lists gets that driver's,
    /// [`crate::protocol::card_data`], whose GT type is GTTYPE_GT1 and whose platform type is
    /// desktop or mobile.
    pub gt_type: u32,
    /// Hardware threads in each EU, for a card whose hwconfig doesn't say.
    pub threads_per_eu: u32,
    /// Whether the platform tiles as Xe2 and later do (FtrXe2PlusTiling).
    pub xe2_tiling: bool,
    /// Each slice's URB in KB, for a card whose hwconfig doesn't say.
    pub urb_kb_per_slice: u32,
    /// The drivers Intel's INF names for these cards alone.
    pub drivers: CardDrivers,
    /// What in this profile no card has confirmed, for the log.
    pub guesses: &'static [&'static str],
}

/// GTTYPE_UNDEFINED, which every platform here gives.
const GT_TYPE_UNDEFINED: u32 = 18;
/// PCH_UNKNOWN and PLATFORM_NONE.
const NO_PCH: u32 = 0;
const NO_PLATFORM_TYPE: u32 = 0;

/// IGFX_XE_HPG_CORE: Alchemist's, and Meteor Lake's and Arrow Lake's, whose Xe LPG is built on it.
const XE_HPG_CORE: u32 = 0x0C07;
/// IGFX_XE2_LPG_CORE. gmmlib defines IGFX_XE2_HPG_CORE as the same number.
const XE2_CORE: u32 = 0x0C09;
/// IGFX_XE3_CORE.
const XE3_CORE: u32 = 0x1E00;

/// Hardware threads in each EU by compute-runtime's tables: eight up to Xe2, ten on Xe3.
const XE_THREADS_PER_EU: u32 = 8;
const XE3_THREADS_PER_EU: u32 = 10;

/// Each slice's URB in KB on Lunar Lake, Battlemage and Alchemist. The other generations take the
/// smallest of these.
const XE2_LPG_URB_KB_PER_SLICE: u32 = 600;
const XE2_HPG_URB_KB_PER_SLICE: u32 = 768;
const XE_HPG_URB_KB_PER_SLICE: u32 = 480;
const GUESS_URB: &str = "the URB size for a card whose hwconfig doesn't give it is Alchemist's";

/// What is unconfirmed in the same way for every generation no card of has been recorded.
const GUESS_NO_RECORDING: &str = "no card of this generation has been recorded";
const GUESS_RECORD_LAYOUT: &str = "the record's layout was read along Lunar Lake's path of the driver";
const GUESS_BEFORE_XE2: &str = "on a device the kernel driver's data doesn't list, the feature bits besides \
     Xe2's tiling are Xe2's, and this generation's may differ";
const GUESS_ON_XE: &str = "xe takes these cards only when told to, and has seldom been run on them";

/// Lunar Lake's graphics, the Arc 130V and 140V: IGFX_LUNARLAKE on IGFX_XE2_LPG_CORE.
pub const XE2_LPG: Generation = Generation {
    name: "Xe2 LPG (Lunar Lake)",
    render_ips: &[(20, 4)],
    devices: &[0x6420, 0x64A0, 0x64B0],
    xe_by_default: true,
    product_family: 1275,
    pch_family: NO_PCH,
    display_core: XE2_CORE,
    render_core: XE2_CORE,
    platform_type: NO_PLATFORM_TYPE,
    gt_type: GT_TYPE_UNDEFINED,
    threads_per_eu: XE_THREADS_PER_EU,
    xe2_tiling: true,
    urb_kb_per_slice: XE2_LPG_URB_KB_PER_SLICE,
    drivers: XE2_LPG_DRIVERS,
    guesses: &[GUESS_NO_RECORDING],
};

/// Battlemage's graphics, the Arc B570 and B580 on G21 (20.1) and the cards on G31 (20.2): IGFX_BMG
/// on IGFX_XE2_HPG_CORE, which is the same core number as Lunar Lake's.
pub const XE2_HPG: Generation = Generation {
    name: "Xe2 HPG (Battlemage)",
    render_ips: &[(20, 1), (20, 2)],
    devices: &[
        0xE202, 0xE209, 0xE20B, 0xE20C, 0xE20D, 0xE210, 0xE211, 0xE212, 0xE216, 0xE220, 0xE221, 0xE222,
        0xE223,
    ],
    xe_by_default: true,
    product_family: 1274,
    pch_family: NO_PCH,
    display_core: XE2_CORE,
    render_core: XE2_CORE,
    platform_type: NO_PLATFORM_TYPE,
    gt_type: GT_TYPE_UNDEFINED,
    threads_per_eu: XE_THREADS_PER_EU,
    xe2_tiling: true,
    urb_kb_per_slice: XE2_HPG_URB_KB_PER_SLICE,
    drivers: XE2_HPG_DRIVERS,
    guesses: &["only G21, the B580, has been recorded: G31's IP 20.2 is compute-runtime's"],
};

/// Panther Lake's graphics (30.0 and 30.1) and Wildcat Lake's (30.3), which compute-runtime counts
/// as one platform: IGFX_PTL on IGFX_XE3_CORE.
pub const XE3_LPG: Generation = Generation {
    name: "Xe3 LPG (Panther Lake, Wildcat Lake)",
    render_ips: &[(30, 0), (30, 1), (30, 3)],
    devices: &[
        0xB080, 0xB081, 0xB082, 0xB083, 0xB084, 0xB085, 0xB086, 0xB087, 0xB08F, 0xB090, 0xB0A0, 0xB0B0,
        0xFD80, 0xFD81,
    ],
    xe_by_default: true,
    product_family: 1300,
    pch_family: NO_PCH,
    display_core: XE3_CORE,
    render_core: XE3_CORE,
    platform_type: NO_PLATFORM_TYPE,
    gt_type: GT_TYPE_UNDEFINED,
    threads_per_eu: XE3_THREADS_PER_EU,
    xe2_tiling: true,
    urb_kb_per_slice: XE_HPG_URB_KB_PER_SLICE,
    drivers: XE3_LPG_DRIVERS,
    guesses: &[GUESS_URB, GUESS_NO_RECORDING, GUESS_RECORD_LAYOUT],
};

/// Meteor Lake's graphics: IGFX_METEORLAKE on IGFX_XE_HPG_CORE. Its IPs are Arrow Lake's too, so
/// the id says which a card is, and a card of an id neither lists is refused.
pub const XE_LPG_METEOR_LAKE: Generation = Generation {
    name: "Xe LPG (Meteor Lake)",
    render_ips: &[(12, 70), (12, 71)],
    devices: &[0x7D40, 0x7D45, 0x7D55, 0x7D60, 0x7DD5],
    xe_by_default: false,
    product_family: 1272,
    pch_family: NO_PCH,
    display_core: XE_HPG_CORE,
    render_core: XE_HPG_CORE,
    platform_type: NO_PLATFORM_TYPE,
    gt_type: GT_TYPE_UNDEFINED,
    threads_per_eu: XE_THREADS_PER_EU,
    xe2_tiling: false,
    urb_kb_per_slice: XE_HPG_URB_KB_PER_SLICE,
    drivers: XE_LPG_DRIVERS,
    guesses: &[GUESS_URB, GUESS_NO_RECORDING, GUESS_RECORD_LAYOUT, GUESS_BEFORE_XE2, GUESS_ON_XE],
};

/// Arrow Lake's graphics: IGFX_ARROWLAKE on IGFX_XE_HPG_CORE. Its H parts are Xe LPG+, 12.74; that
/// its S and U parts give Meteor Lake's IPs is a guess from compute-runtime having no IP of their own.
pub const XE_LPG_ARROW_LAKE: Generation = Generation {
    name: "Xe LPG (Arrow Lake)",
    render_ips: &[(12, 70), (12, 71), (12, 74)],
    devices: &[0x7D41, 0x7D51, 0x7D67, 0x7DD1, 0xB640],
    xe_by_default: false,
    product_family: 1273,
    pch_family: NO_PCH,
    display_core: XE_HPG_CORE,
    render_core: XE_HPG_CORE,
    platform_type: NO_PLATFORM_TYPE,
    gt_type: GT_TYPE_UNDEFINED,
    threads_per_eu: XE_THREADS_PER_EU,
    xe2_tiling: false,
    urb_kb_per_slice: XE_HPG_URB_KB_PER_SLICE,
    drivers: XE_LPG_DRIVERS,
    guesses: &[
        GUESS_URB,
        GUESS_NO_RECORDING,
        GUESS_RECORD_LAYOUT,
        GUESS_BEFORE_XE2,
        GUESS_ON_XE,
        "the IPs of Arrow Lake's S and U parts",
    ],
};

/// Alchemist's graphics, the Arc A-series: IGFX_DG2 on IGFX_XE_HPG_CORE. It is from before GMD_ID,
/// so xe gives no IP for it, and a card is known by the ids the kernel lists alone.
pub const XE_HPG: Generation = Generation {
    name: "Xe HPG (Alchemist)",
    render_ips: &[],
    devices: &[
        0x5690, 0x5691, 0x5692, 0x5693, 0x5694, 0x5695, 0x5696, 0x5697, 0x56A0, 0x56A1, 0x56A2, 0x56A3,
        0x56A4, 0x56A5, 0x56A6, 0x56B0, 0x56B1, 0x56B2, 0x56B3, 0x56BA, 0x56BB, 0x56BC, 0x56BD, 0x56BE,
        0x56BF,
    ],
    xe_by_default: false,
    product_family: 1270,
    pch_family: NO_PCH,
    display_core: XE_HPG_CORE,
    render_core: XE_HPG_CORE,
    platform_type: NO_PLATFORM_TYPE,
    gt_type: GT_TYPE_UNDEFINED,
    threads_per_eu: XE_THREADS_PER_EU,
    xe2_tiling: false,
    urb_kb_per_slice: XE_HPG_URB_KB_PER_SLICE,
    drivers: XE_HPG_DRIVERS,
    guesses: &[
        GUESS_NO_RECORDING,
        GUESS_RECORD_LAYOUT,
        GUESS_BEFORE_XE2,
        GUESS_ON_XE,
        "the record's render IP is 0, as xe gives none for this card",
    ],
};

const KNOWN: [Generation; 6] = [XE2_LPG, XE2_HPG, XE3_LPG, XE_LPG_METEOR_LAKE, XE_LPG_ARROW_LAKE, XE_HPG];

/// Every generation this build has a profile for.
pub fn known() -> &'static [Generation] {
    &KNOWN
}

/// The profile for a card whose render IP is `ip` and whose PCI device id is `device`, if there is
/// one. The IP's revision doesn't matter.
pub fn for_card(ip: IpVersion, device: u16) -> Option<&'static Generation> {
    let ip = (ip.major, ip.minor);
    let has_ip = |generation: &&Generation| generation.render_ips.contains(&ip);
    // A platform from before GMD_ID is known by its id alone, as xe gives its IP as 0.0.
    let before_gmd_id = |generation: &&Generation| generation.render_ips.is_empty();
    let lists_device = |generation: &&Generation| generation.devices.contains(&device);
    let by_both = KNOWN
        .iter()
        .find(|generation| lists_device(generation) && (has_ip(generation) || before_gmd_id(generation)));
    if by_both.is_some() {
        return by_both;
    }
    let mut by_ip = KNOWN.iter().filter(has_ip);
    match (by_ip.next(), by_ip.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// The generation that lists `device` as its own, whatever the card's IP, for the app, which can't
/// ask the card.
pub fn for_device(device: u16) -> Option<&'static Generation> {
    KNOWN.iter().find(|generation| generation.devices.contains(&device))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(major: u16, minor: u16) -> IpVersion {
        IpVersion { major, minor, revision: 0 }
    }

    #[test]
    fn lunar_lake_is_found_by_its_render_ip_at_any_revision() {
        for revision in [0, 4] {
            let ip = IpVersion { major: 20, minor: 4, revision };
            assert_eq!(for_card(ip, 0x64A0), Some(&XE2_LPG));
        }
    }

    #[test]
    fn battlemage_is_found_by_its_render_ip() {
        assert_eq!(for_card(ip(20, 1), 0xE20B), Some(&XE2_HPG));
        assert_eq!(for_card(ip(20, 2), 0xE223), Some(&XE2_HPG), "G31");
        assert_eq!(XE2_HPG.product_family, 1274);
    }

    #[test]
    fn panther_lake_and_wildcat_lake_are_one_profile_with_more_threads() {
        assert_eq!(for_card(ip(30, 0), 0xB080), Some(&XE3_LPG));
        assert_eq!(for_card(ip(30, 1), 0xB0A0), Some(&XE3_LPG));
        assert_eq!(for_card(ip(30, 3), 0xFD80), Some(&XE3_LPG), "Wildcat Lake");
        let numbers = (XE3_LPG.product_family, XE3_LPG.render_core, XE3_LPG.threads_per_eu);
        assert_eq!(numbers, (1300, 0x1E00, 10));
        assert_eq!(XE3_LPG.drivers, XE3_LPG_DRIVERS, "with no 32-bit OpenCL, as the INF gives none");
    }

    #[test]
    fn meteor_lake_and_arrow_lake_share_ips_and_are_told_apart_by_id() {
        assert_eq!(for_card(ip(12, 70), 0x7D55), Some(&XE_LPG_METEOR_LAKE));
        assert_eq!(for_card(ip(12, 70), 0x7D67), Some(&XE_LPG_ARROW_LAKE));
        assert_eq!(for_card(ip(12, 74), 0x7DD1), Some(&XE_LPG_ARROW_LAKE));
        assert_eq!(for_card(ip(12, 70), 0x7DFF), None, "an id neither lists");
        assert_eq!(for_card(ip(12, 74), 0x7DFF), Some(&XE_LPG_ARROW_LAKE), "only Arrow Lake has 12.74");
        assert_eq!(XE_LPG_METEOR_LAKE.render_core, 0x0C07);
        let older = [XE_LPG_METEOR_LAKE, XE_LPG_ARROW_LAKE, XE_HPG];
        assert!(older.iter().all(|generation| !generation.xe2_tiling && !generation.xe_by_default));
    }

    #[test]
    fn alchemist_is_known_by_its_id_when_xe_gives_no_ip() {
        assert_eq!(for_card(IpVersion::default(), 0x56A0), Some(&XE_HPG));
        assert_eq!(for_card(IpVersion::default(), 0x56C0), None, "the Flex cards, which the package lacks");
        assert_eq!(for_card(ip(12, 55), 0x56A0), Some(&XE_HPG), "nor by an IP a kernel may yet give");
        assert_eq!(XE_HPG.drivers, XE_HPG_DRIVERS);
    }

    #[test]
    fn a_device_id_the_profile_lacks_is_taken_by_its_ip() {
        assert_eq!(for_card(ip(20, 4), 0x64FF), Some(&XE2_LPG));
        assert_eq!(for_card(ip(30, 0), 0xB0FF), Some(&XE3_LPG));
    }

    #[test]
    fn another_ip_has_no_profile() {
        // Nova Lake's Xe3 is another product family, and Intel's package has no driver for it.
        assert_eq!(for_card(ip(30, 4), 0xD740), None);
        assert_eq!(for_card(ip(35, 10), 0xD750), None);
        assert_eq!(for_card(ip(20, 3), 0xE20B), None);
        assert_eq!(for_card(IpVersion::default(), 0x64A0), None, "a kernel that gives no IP version");
    }

    #[test]
    fn the_app_knows_a_card_by_its_id_alone() {
        assert_eq!(for_device(0xE20B), Some(&XE2_HPG));
        assert_eq!(for_device(0xFD81), Some(&XE3_LPG));
        assert_eq!(for_device(0x46A6), None, "Alder Lake");
    }

    #[test]
    fn no_two_generations_list_the_same_device() {
        let mut seen = std::collections::BTreeSet::new();
        for device in KNOWN.iter().flat_map(|generation| generation.devices) {
            assert!(seen.insert(device), "{device:04x} twice");
        }
    }

    #[test]
    fn every_generation_says_what_is_unconfirmed() {
        for generation in KNOWN {
            assert!(!generation.guesses.is_empty(), "{}", generation.name);
        }
    }
}
