//! What Intel's kernel driver puts in the adapter record for each card it knows, besides the
//! workaround table: the feature table, the platform words and where each hwconfig key goes.
//!
//! The feature table is one bit per feature, [`TABLE_BYTES`] bytes from the record's
//! [`super::features`] start. For each card the data gives the table and which of its bits are
//! known; a bit it doesn't know is left to the backend. The tables are the driver's for its default
//! inputs: a Windows 11 guest with none of Intel's registry settings, a CPU that is not a Xeon, PCI
//! capabilities as the platform's cards usually have them, every display pipe present, and the fuse
//! words xe doesn't report at zero. The PCI class is the one input the backend reads itself:
//! [`not_vga_clears`].
//!
//! A card is known by its product family, device and PCI revision. A device the data doesn't list
//! has none of this, and its record stays as the backend builds it without it.

mod data;

pub use data::HWCONFIG_PLACES;

/// The feature table's size in bytes, and in the dwords the data gives it in.
pub const TABLE_BYTES: usize = 0xEC;
pub const TABLE_DWORDS: usize = TABLE_BYTES / 4;

/// A platform's cards.
#[derive(Debug, PartialEq, Eq)]
pub struct Platform {
    pub name: &'static str,
    pub product_family: u32,
    /// The devices the data has tables for.
    pub devices: &'static [u16],
    /// PLATFORM_TYPE: 1 for a desktop card, 2 for a mobile one.
    pub platform_type: u32,
    /// The display core family the driver gives the platform's display IP.
    pub display_core: u32,
    /// Whether the record's per-IP core families are filled: on every platform with GMD_ID.
    pub per_ip_cores: bool,
    /// The table most of the platform's cards have, and which of its bits are known.
    pub features: [u32; TABLE_DWORDS],
    pub known: [u32; TABLE_DWORDS],
    /// The cards whose tables differ from it.
    pub cards: &'static [Card],
}

/// Cards whose table differs from their platform's, by the bits that differ, each counted from the
/// table's first bit.
#[derive(Debug, PartialEq, Eq)]
pub struct Card {
    pub devices: &'static [u16],
    /// The PCI revisions, inclusive.
    pub revisions: (u8, u8),
    /// Known and set, known and clear, and not known.
    pub on: &'static [u16],
    pub off: &'static [u16],
    pub unknown: &'static [u16],
}

/// A card's table, and which of its bits are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub platform: &'static Platform,
    pub features: [u32; TABLE_DWORDS],
    pub known: [u32; TABLE_DWORDS],
}

fn put(words: &mut [u32; TABLE_DWORDS], bit: u16, on: bool) {
    let (word, bit) = (usize::from(bit) / 32, bit % 32);
    if on {
        words[word] |= 1 << bit;
    } else {
        words[word] &= !(1 << bit);
    }
}

/// The table for a card, or none for a card the data doesn't list.
pub fn table(product_family: u32, device: u16, revision: u8) -> Option<Table> {
    let platform = data::PLATFORMS
        .iter()
        .find(|platform| platform.product_family == product_family && platform.devices.contains(&device))?;
    let mut table = Table { platform, features: platform.features, known: platform.known };
    let card = platform.cards.iter().find(|card| {
        card.devices.contains(&device) && (card.revisions.0..=card.revisions.1).contains(&revision)
    });
    if let Some(card) = card {
        for &bit in card.on.iter().chain(card.off) {
            put(&mut table.known, bit, true);
            put(&mut table.features, bit, card.on.contains(&bit));
        }
        for &bit in card.unknown {
            put(&mut table.known, bit, false);
            put(&mut table.features, bit, false);
        }
    }
    Some(table)
}

/// The media core family for a media IP version, if the data has one.
pub fn media_core(architecture: u16, release: u16) -> Option<u32> {
    data::MEDIA_CORES.iter().find(|(ip, _)| *ip == (architecture, release)).map(|&(_, core)| core)
}

/// The feature bits the driver clears on a card whose PCI class says it is not VGA-compatible:
/// a display controller of the "other" kind.
pub fn not_vga_clears() -> &'static [u16] {
    data::NOT_VGA_CLEARS
}

/// Whether a PCI class is a display controller of the "other" kind rather than VGA-compatible.
pub fn is_not_vga(class: u32) -> bool {
    class >> 8 == 0x0380
}

/// The record's places for a hwconfig key, none for a key the driver keeps elsewhere.
pub fn hwconfig_places(key: u32) -> &'static [usize] {
    HWCONFIG_PLACES.iter().find(|(at, _)| *at == key).map_or(&[], |&(_, places)| places)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_set(words: &[u32; TABLE_DWORDS], bit: usize) -> bool {
        words[bit / 32] >> (bit % 32) & 1 == 1
    }

    #[test]
    fn the_data_has_each_platform_once_and_each_device_in_one() {
        assert_eq!(data::PLATFORMS.len(), 7);
        let mut seen = std::collections::BTreeSet::new();
        for platform in data::PLATFORMS {
            for &device in platform.devices {
                assert!(seen.insert(device), "{device:04x} twice");
            }
            for card in platform.cards {
                assert!(
                    card.devices.iter().all(|device| platform.devices.contains(device)),
                    "{}",
                    platform.name
                );
            }
            for (at, word) in platform.features.iter().enumerate() {
                assert_eq!(word & !platform.known[at], 0, "{}: only known bits are set", platform.name);
            }
        }
        assert_eq!(seen.len(), 45);
    }

    #[test]
    fn a_card_takes_its_platforms_table_with_its_own_bits() {
        let b580 = table(1274, 0xE20B, 0).unwrap();
        assert_eq!(b580.platform.name, "Battlemage");
        let g31 = table(1274, 0xE223, 0).unwrap();
        for bit in [688, 692] {
            assert!(!is_set(&b580.features, bit) && is_set(&g31.features, bit), "{bit}");
        }
        let a770 = |revision| table(1270, 0x56A0, revision).unwrap();
        assert!(
            !is_set(&a770(0).features, 436) && is_set(&a770(0).known, 436),
            "no compression at revision 0"
        );
        assert!(is_set(&a770(8).features, 436));
        assert_eq!(a770(2), a770(3), "one band");
    }

    #[test]
    fn a_device_the_data_does_not_list_has_no_table() {
        assert_eq!(table(1274, 0xE20D, 0), None);
        assert_eq!(table(1270, 0xE20B, 0), None, "a device of another family");
        assert_eq!(table(9999, 0x64A0, 0), None);
    }

    #[test]
    fn wildcat_lake_is_its_own_platform_in_panther_lakes_family() {
        assert_eq!(table(1300, 0xB080, 0).unwrap().platform.name, "Panther Lake");
        assert_eq!(table(1300, 0xFD80, 0).unwrap().platform.name, "Wildcat Lake");
    }

    #[test]
    fn media_cores_and_hwconfig_places() {
        assert_eq!(media_core(13, 1), Some(0xD01));
        assert_eq!(media_core(13, 7), None);
        assert_eq!(hwconfig_places(46), [0x2180]);
        assert_eq!(hwconfig_places(26), [0x21AC, 0x23A0]);
        assert_eq!(hwconfig_places(68), [] as [usize; 0], "kept elsewhere");
        assert!(is_not_vga(0x03_8000) && !is_not_vga(0x03_0000));
        assert_eq!(not_vga_clears(), [8]);
    }
}
