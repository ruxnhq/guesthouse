//! The workaround table Intel's kernel driver gives a card, for the adapter record.
//!
//! Intel's user-mode drivers change how they draw by the table's entries, so the record carries the
//! table the card would have under Intel's own kernel driver. That driver builds it from the card's
//! graphics, media and display IP versions and their revisions, or, on Alchemist, from the PCI device
//! id and revision. It starts from an empty table, makes a few changes, sets the entries each IP
//! version's part of `data` gives for the revisions the card has, and then makes the rest of its
//! changes, some of which depend on the card, the table so far and the record's feature table.
//!
//! What Linux can't say is filled in, and [`Built::defaults`] names each:
//! - the display IP, which xe doesn't report: `display_ip` picks it by platform;
//! - the display IP's revision, and a second display revision only Lunar Lake's display reads: zero;
//! - the settings that come from the guest's Windows, folded into `data` already: a Windows 11 guest
//!   (24H2 or later), Intel's registry settings absent, an adapter that is not an SR-IOV virtual
//!   function and is a virtual machine's, a card whose display is present and not fused down, and no
//!   hybrid-graphics display switch.
//!
//! A card whose render IP the data doesn't cover gets no table: [`fill`] says which IP it was. One
//! whose render IP is covered but whose media or display IP is not gets the parts that are, and
//! [`Built::uncovered`] names the parts left out.

mod data;

use super::bytes::dword;
use super::features;

/// The entries the table has room for.
const ENTRIES: usize = 3048;
/// Where the feature table starts in the record. [`Cond::Feature`] counts its dwords from here.
const FEATURE_TABLE: usize = 0x2B4;
/// The record's product families, gmmlib's PRODUCT_FAMILY.
const ALCHEMIST: u32 = 1270;
const METEOR_LAKE: u32 = 1272;
const ARROW_LAKE: u32 = 1273;
const BATTLEMAGE: u32 = 1274;
const LUNAR_LAKE: u32 = 1275;
const PANTHER_LAKE: u32 = 1300;
/// Wildcat Lake's device ids, in Panther Lake's product family, as [`crate::generation`] lists them.
const WILDCAT_LAKE_DEVICES: [u16; 2] = [0xFD80, 0xFD81];

/// What the table is built from, as Linux knows the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Card {
    pub product_family: u32,
    pub device_id: u16,
    /// The PCI revision, which Alchemist's groups read.
    pub pci_revision: u8,
    /// The main and media GTs' IP versions, from xe's GT list. A card with no media GT has none.
    pub render: Option<Ip>,
    pub media: Option<Ip>,
}

/// An IP version and its revision, as xe gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ip {
    pub architecture: u16,
    pub release: u16,
    pub revision: u16,
}

impl Ip {
    pub const fn new(architecture: u16, release: u16, revision: u16) -> Self {
        Self { architecture, release, revision }
    }

    fn revision(self) -> u8 {
        (self.revision & 0x3F) as u8
    }
}

impl std::fmt::Display for Ip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{:02}r{}", self.architecture, self.release, self.revision)
    }
}

/// The three blocks a GMD_ID card's table is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Render,
    Media,
    Display,
}

impl Block {
    fn name(self) -> &'static str {
        match self {
            Block::Render => "render",
            Block::Media => "media",
            Block::Display => "display",
        }
    }
}

/// What one IP version sets.
#[derive(Debug)]
pub struct Component {
    pub block: Block,
    /// Architecture and release.
    pub ip: (u16, u16),
    pub rows: &'static [Rows],
}

/// What one Alchemist device group sets.
#[derive(Debug)]
pub struct DeviceGroup {
    pub name: &'static str,
    /// Empty for the group that takes every device the others don't name.
    pub devices: &'static [u16],
    pub rows: &'static [Rows],
}

/// Entries set to `value` when `when` holds. Values are ORed into what an entry already has.
#[derive(Debug)]
pub struct Rows {
    pub value: u8,
    pub when: When,
    pub entries: &'static [u16],
}

/// When a row is set. Ranges are inclusive.
#[derive(Debug, Clone, Copy)]
pub enum When {
    Always,
    /// The block's own IP revision, or for an Alchemist group the PCI revision.
    Revision(u8, u8),
    /// The second display revision.
    DisplayAux(u8, u8),
}

/// A change to entries, made when `when` holds: each `(entry, keep, or)` becomes
/// `entry & keep | or`.
#[derive(Debug)]
pub struct Op {
    pub when: Cond,
    pub sets: &'static [(u16, u8, u8)],
}

/// What an [`Op`] depends on.
#[derive(Debug)]
pub enum Cond {
    Always,
    All(&'static [Cond]),
    Any(&'static [Cond]),
    Not(&'static Cond),
    Family(&'static [u32]),
    FamilyAtLeast(u32),
    Device(&'static [u16]),
    RenderArchitecture(&'static [u16]),
    MediaArchitecture(&'static [u16]),
    RenderArchitectureAtLeast(u16),
    MediaRevision(u8, u8),
    /// The entry is not zero in the table so far.
    Set(u16),
    /// A bit of the record's feature table: the dword at this offset into it, and the bit.
    Feature(usize, u32),
}

/// A table that was built, and what it was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub render: Option<Ip>,
    pub media: Option<Ip>,
    pub display: Option<Ip>,
    /// The Alchemist device group, and the PCI revision it was read at.
    pub device_group: Option<(&'static str, u8)>,
    /// The entries that are not zero.
    pub entries: usize,
    /// The inputs that are defaults rather than the card's.
    pub defaults: Vec<&'static str>,
    /// The parts the data has nothing for, named as "media 13.07" and the like, whose entries are
    /// not set.
    pub uncovered: Vec<String>,
}

/// Whether the table was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Built(Built),
    /// The data has nothing for the render IP, named as "render 12.55" and the like.
    Uncovered(String),
}

impl std::fmt::Display for Built {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ip = |ip: Option<Ip>| ip.map_or_else(|| "-".to_owned(), |ip| ip.to_string());
        if let Some((group, revision)) = self.device_group {
            write!(f, "group=\"{group}\" pci_rev={revision:#04x}")?;
        } else {
            write!(f, "render={} media={} display={}", ip(self.render), ip(self.media), ip(self.display))?;
        }
        write!(f, " entries={} defaults=\"{}\"", self.entries, self.defaults.join(", "))
    }
}

/// The display IP a platform's cards have, which xe doesn't report. Its revision is taken as zero.
fn display_ip(card: &Card) -> Option<(u16, u16)> {
    match card.product_family {
        METEOR_LAKE | ARROW_LAKE => Some((14, 0)),
        BATTLEMAGE => Some((14, 1)),
        LUNAR_LAKE => Some((20, 0)),
        PANTHER_LAKE if WILDCAT_LAKE_DEVICES.contains(&card.device_id) => Some((30, 2)),
        PANTHER_LAKE => Some((30, 0)),
        _ => None,
    }
}

/// The display IP a platform's cards have, for the record's platform words, which xe doesn't report:
/// the one the table is built from.
pub fn platform_display_ip(product_family: u32, device_id: u16) -> Option<(u16, u16)> {
    display_ip(&Card { product_family, device_id, ..Card::default() })
}

/// The default for an input Linux can't give.
pub const DISPLAY_IP_DEFAULT: &str = "display IP by platform";
pub const DISPLAY_REVISION_DEFAULT: &str = "display revision 0";
const DISPLAY_AUX_DEFAULT: &str = "second display revision 0";
pub const GUEST_SETTINGS_DEFAULT: &str = "the guest's Windows settings";

/// The table as nibbles, one per entry, and the inputs it was read at.
struct Table {
    nibbles: [u8; ENTRIES],
    card: Card,
}

impl Table {
    fn apply(&mut self, ops: &[Op], record: &[u8]) {
        for op in ops {
            if self.holds(&op.when, record) {
                for &(entry, keep, or) in op.sets {
                    let nibble = &mut self.nibbles[usize::from(entry)];
                    *nibble = (*nibble & keep | or) & 0xF;
                }
            }
        }
    }

    fn holds(&self, cond: &Cond, record: &[u8]) -> bool {
        let card = &self.card;
        match *cond {
            Cond::Always => true,
            Cond::All(all) => all.iter().all(|cond| self.holds(cond, record)),
            Cond::Any(any) => any.iter().any(|cond| self.holds(cond, record)),
            Cond::Not(cond) => !self.holds(cond, record),
            Cond::Family(families) => families.contains(&card.product_family),
            Cond::FamilyAtLeast(family) => card.product_family >= family,
            Cond::Device(devices) => devices.contains(&card.device_id),
            Cond::RenderArchitecture(list) => card.render.is_some_and(|ip| list.contains(&ip.architecture)),
            Cond::MediaArchitecture(list) => card.media.is_some_and(|ip| list.contains(&ip.architecture)),
            Cond::RenderArchitectureAtLeast(least) => card.render.is_some_and(|ip| ip.architecture >= least),
            Cond::MediaRevision(low, high) => {
                card.media.is_some_and(|ip| (low..=high).contains(&ip.revision()))
            }
            Cond::Set(entry) => self.nibbles[usize::from(entry)] != 0,
            Cond::Feature(offset, bit) => {
                dword(record, FEATURE_TABLE + offset).is_some_and(|word| word >> bit & 1 == 1)
            }
        }
    }

    fn set(&mut self, rows: &[Rows], revision: u8, aux: u8) {
        for row in rows {
            let on = match row.when {
                When::Always => true,
                When::Revision(low, high) => (low..=high).contains(&revision),
                When::DisplayAux(low, high) => (low..=high).contains(&aux),
            };
            if on {
                for &entry in row.entries {
                    self.nibbles[usize::from(entry)] |= row.value;
                }
            }
        }
    }
}

fn component(block: Block, ip: Ip) -> Result<&'static Component, String> {
    data::COMPONENTS
        .iter()
        .find(|component| component.block == block && component.ip == (ip.architecture, ip.release))
        .ok_or_else(|| format!("{} {}.{:02}", block.name(), ip.architecture, ip.release))
}

fn device_group(device_id: u16) -> &'static DeviceGroup {
    let groups = data::DEVICE_GROUPS;
    groups.iter().find(|group| group.devices.contains(&device_id)).unwrap_or(&groups[groups.len() - 1])
}

/// The table for `card`, read against the feature table already in `record`, as an entry and value
/// for each entry that is not zero.
fn table(card: &Card, record: &[u8]) -> Result<(Vec<(usize, u8)>, Built), String> {
    let mut table = Table { nibbles: [0; ENTRIES], card: *card };
    let mut built = Built {
        render: None,
        media: None,
        display: None,
        device_group: None,
        entries: 0,
        defaults: Vec::new(),
        uncovered: Vec::new(),
    };
    let mut parts: Vec<(&'static [Rows], u8)> = Vec::new();
    if card.product_family == ALCHEMIST {
        // Alchemist's table goes by device and PCI revision, not by IP version, and the IP
        // versions play no part in the changes after.
        table.card.render = None;
        table.card.media = None;
        let group = device_group(card.device_id);
        built.device_group = Some((group.name, card.pci_revision));
        parts.push((group.rows, card.pci_revision));
    } else {
        let render = card.render.ok_or_else(|| "render IP unknown".to_owned())?;
        parts.push((component(Block::Render, render)?.rows, render.revision()));
        built.render = Some(render);
        // A media or display IP the data lacks leaves out that part's entries alone.
        if let Some(media) = card.media {
            match component(Block::Media, media) {
                Ok(component) => {
                    parts.push((component.rows, media.revision()));
                    built.media = Some(media);
                }
                Err(uncovered) => built.uncovered.push(uncovered),
            }
        }
        match display_ip(card) {
            Some((architecture, release)) => {
                // xe doesn't report the display IP or its revision: the IP is the platform's, the
                // revision 0.
                let display = Ip::new(architecture, release, 0);
                built.defaults.extend([DISPLAY_IP_DEFAULT, DISPLAY_REVISION_DEFAULT, DISPLAY_AUX_DEFAULT]);
                match component(Block::Display, display) {
                    Ok(component) => {
                        parts.push((component.rows, display.revision()));
                        built.display = Some(display);
                    }
                    Err(uncovered) => built.uncovered.push(uncovered),
                }
            }
            None => {
                built.uncovered.push(format!("display IP unknown for product family {}", card.product_family))
            }
        }
    }
    built.defaults.push(GUEST_SETTINGS_DEFAULT);

    table.apply(data::BEFORE, record);
    for (rows, revision) in parts {
        // The second display revision is taken as 0.
        table.set(rows, revision, 0);
    }
    table.apply(data::AFTER, record);
    let entries: Vec<(usize, u8)> = table
        .nibbles
        .iter()
        .enumerate()
        .filter(|&(_, &nibble)| nibble != 0)
        .map(|(entry, &nibble)| (entry, nibble))
        .collect();
    built.entries = entries.len();
    Ok((entries, built))
}

/// Writes the table Intel's kernel driver would give `card` into `record`, whose feature table must
/// already be filled. A card whose render IP the data doesn't cover leaves the record as it was.
pub fn fill(record: &mut [u8], card: &Card) -> Outcome {
    match table(card, record) {
        Ok((entries, built)) => {
            for (entry, value) in entries {
                features::set_workaround(record, entry, value);
            }
            Outcome::Built(built)
        }
        Err(uncovered) => Outcome::Uncovered(uncovered),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: usize = 0x2574;

    fn values(record: &[u8]) -> Vec<(usize, u8)> {
        (0..ENTRIES)
            .filter_map(|entry| Some((entry, features::workaround(record, entry)?)))
            .filter(|e| e.1 != 0)
            .collect()
    }

    #[test]
    fn the_data_has_every_part_once() {
        assert_eq!(data::COMPONENTS.len(), 36);
        assert_eq!(data::DEVICE_GROUPS.len(), 5);
        assert!(data::DEVICE_GROUPS.last().is_some_and(|group| group.devices.is_empty()));
        for (at, one) in data::COMPONENTS.iter().enumerate() {
            for other in &data::COMPONENTS[at + 1..] {
                assert!((one.block, one.ip) != (other.block, other.ip), "{:?}", one.ip);
            }
        }
        let rows = |block: Block, ip: (u16, u16)| {
            let component = component(block, Ip::new(ip.0, ip.1, 0)).unwrap();
            component.rows.iter().map(|rows| rows.entries.len()).sum::<usize>()
        };
        assert_eq!(rows(Block::Render, (20, 1)), 113);
        assert_eq!(rows(Block::Media, (13, 1)), 28);
        assert_eq!(rows(Block::Display, (14, 1)), 42);
    }

    #[test]
    fn an_empty_record_reads_no_feature_bits() {
        let card = Card {
            product_family: BATTLEMAGE,
            device_id: 0xE20B,
            render: Some(Ip::new(20, 1, 0)),
            media: Some(Ip::new(13, 1, 1)),
            ..Card::default()
        };
        let mut record = vec![0u8; RECORD];
        let Outcome::Built(built) = fill(&mut record, &card) else { panic!("covered") };
        assert_eq!(built.entries, values(&record).len());
        assert_eq!(built.display, Some(Ip::new(14, 1, 0)));
        assert_eq!(
            built.to_string(),
            format!(
                "render=20.01r0 media=13.01r1 display=14.01r0 entries={} defaults=\"display IP by platform, \
                 display revision 0, second display revision 0, the guest's Windows settings\"",
                built.entries
            )
        );
    }

    #[test]
    fn every_ip_and_device_group_gives_the_virtual_machines_entry_and_tiled_resources_one() {
        let record = vec![0u8; RECORD];
        let check = |card: Card| {
            let (entries, _) = table(&card, &record).unwrap();
            for entry in [535, 968] {
                assert!(entries.contains(&(entry, 1)), "{entry} {card:?}");
            }
        };
        for render in data::COMPONENTS.iter().filter(|component| component.block == Block::Render) {
            for revision in [0, 4, 63] {
                let render = Some(Ip::new(render.ip.0, render.ip.1, revision));
                check(Card { product_family: BATTLEMAGE, render, ..Card::default() });
            }
        }
        for group in data::DEVICE_GROUPS {
            for pci_revision in [0, 4, 255] {
                let device_id = group.devices.first().copied().unwrap_or(0x56A0);
                check(Card { product_family: ALCHEMIST, device_id, pci_revision, ..Card::default() });
            }
        }
    }

    #[test]
    fn wildcat_lake_is_told_from_panther_lake_by_device() {
        let card = |device_id| Card { product_family: PANTHER_LAKE, device_id, ..Card::default() };
        assert_eq!(display_ip(&card(0xB080)), Some((30, 0)));
        assert_eq!(display_ip(&card(0xFD80)), Some((30, 2)));
    }

    #[test]
    fn an_uncovered_render_ip_is_named_and_the_record_left_alone() {
        let mut record = vec![0u8; RECORD];
        record[0x3A0 + 968 / 2] = 1;
        let before = record.clone();
        let card = Card {
            product_family: BATTLEMAGE,
            render: Some(Ip::new(20, 9, 0)),
            media: Some(Ip::new(13, 1, 1)),
            ..Card::default()
        };
        assert_eq!(fill(&mut record, &card), Outcome::Uncovered("render 20.09".into()));
        assert_eq!(record, before);
        let card = Card { render: None, ..card };
        assert_eq!(fill(&mut record, &card), Outcome::Uncovered("render IP unknown".into()));
        assert_eq!(record, before);
    }

    /// The entries one part of the table sets for a card, alone.
    fn part(block: Block, ip: Ip) -> Vec<usize> {
        let mut table = Table { nibbles: [0; ENTRIES], card: Card::default() };
        table.set(component(block, ip).unwrap().rows, ip.revision(), 0);
        (0..ENTRIES).filter(|&entry| table.nibbles[entry] != 0).collect()
    }

    #[test]
    fn an_uncovered_media_or_display_ip_leaves_out_that_part_alone() {
        let record = vec![0u8; RECORD];
        let b580 = Card {
            product_family: BATTLEMAGE,
            device_id: 0xE20B,
            render: Some(Ip::new(20, 1, 0)),
            media: Some(Ip::new(13, 1, 1)),
            ..Card::default()
        };
        let (whole, built) = table(&b580, &record).unwrap();
        assert!(built.uncovered.is_empty());
        let has = |entries: &[(usize, u8)], entry: usize| entries.iter().any(|&(at, _)| at == entry);
        let media_only: Vec<usize> = part(Block::Media, Ip::new(13, 1, 1))
            .into_iter()
            .filter(|&entry| !part(Block::Render, Ip::new(20, 1, 0)).contains(&entry))
            .filter(|&entry| !part(Block::Display, Ip::new(14, 1, 0)).contains(&entry))
            .collect();
        assert!(!media_only.is_empty());

        let unknown_media = Card { media: Some(Ip::new(13, 7, 0)), ..b580 };
        let (entries, built) = table(&unknown_media, &record).unwrap();
        assert_eq!(built.uncovered, ["media 13.07"]);
        assert_eq!((built.render, built.media, built.display), (b580.render, None, Some(Ip::new(14, 1, 0))));
        assert!(entries.len() < whole.len() && has(&entries, 2964), "the render part is there");
        for entry in &media_only {
            assert!(has(&whole, *entry) && !has(&entries, *entry), "{entry}");
        }

        let unknown_family = Card { product_family: 9999, ..b580 };
        let mut record = vec![0u8; RECORD];
        let Outcome::Built(built) = fill(&mut record, &unknown_family) else { panic!("render is covered") };
        assert_eq!(built.uncovered, ["display IP unknown for product family 9999"]);
        assert_eq!((built.media, built.display), (b580.media, None));
        assert_eq!(features::workaround(&record, 2525), Some(4), "media 13.01's");
        assert_eq!(built.defaults, [GUEST_SETTINGS_DEFAULT], "no display IP was taken");
    }
}
