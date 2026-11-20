//! A recording of a tester's Arc B580 stands in for the card: the backend built from it gives the
//! guest the record the daemon on the real card gave, as that daemon's log line says it.

use guesthouse_backend_intel::IntelBackend;
use guesthouse_backend_intel::protocol::features::{self, Feature};
use guesthouse_backend_intel::protocol::{card_data, record};
use guesthouse_backend_intel::recording;
use std::collections::BTreeMap;

const ARC_B580: &str = include_str!("cards/arc-b580.card");
/// What the daemon logged at the start of the tester's run on the card itself.
const LOGGED_RECORD: &str =
    "product_family=1274 render_core=0xc09 device=0xe20b render_ip=0x5004000 eus=160 vm=1 compression=off";

#[test]
fn the_recorded_b580_gives_the_record_its_own_daemon_gave() {
    let card = recording::read(ARC_B580).unwrap();
    assert_eq!((card.eu_count(), card.card_memory()), (160, 0x2_FB80_0000));
    let backend = IntelBackend::from_card(card.clone()).unwrap();
    assert_eq!(backend.generation().name, "Xe2 HPG (Battlemage)");
    assert_eq!(record::summary(backend.record()).to_string(), LOGGED_RECORD);
    assert_eq!(recording::read(&recording::write(&card)), Ok(card), "and it writes back the same");
}

#[test]
fn the_recorded_b580_gives_the_guest_its_own_urb() {
    let backend = IntelBackend::from_card(recording::read(ARC_B580).unwrap()).unwrap();
    assert_eq!(
        record::urb(backend.record()).to_string(),
        "size_kb=3840 vs=64-5888 hs=0-1548 gs=2-1952 ds=50-4480 task=0-1548 mesh=4-1952 max_start=127",
        "768 KB a slice in five slices, and the stages' limits as its hwconfig gives them"
    );
}

#[test]
fn the_recorded_b580_gets_its_workaround_table_from_its_render_and_media_ips() {
    let backend = IntelBackend::from_card(recording::read(ARC_B580).unwrap()).unwrap();
    let record = backend.record();
    let set = (0..3048).filter(|&entry| features::workaround(record, entry).is_some_and(|value| value != 0));
    assert_eq!(set.count(), 192, "a real card's 193 but the render cache's keying");
    assert_eq!(features::workaround(record, 2287), Some(0), "the render cache's keying, kept clear");
    assert_eq!(features::workaround(record, 2964), Some(2), "render 20.01");
    assert_eq!(features::workaround(record, 2525), Some(4), "media 13.01");
    assert_eq!(features::workaround(record, 2571), Some(8), "display 14.01 at revision 0");
}

/// FNV-1a over a record, so a test can say a record is byte for byte the one it was.
fn fingerprint(record: &[u8]) -> u64 {
    record
        .iter()
        .fold(0xCBF2_9CE4_8422_2325, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3))
}

/// What the record of the recorded card with its device made 0xE20D was before the kernel driver's
/// data, a Battlemage device that data has no table for, but for the workaround entry for the render
/// cache's keying, which the record now keeps clear on every card, and the Direct3D 11 interface
/// versions, which it now offers all of on a card that gives the URB stage limits, with the
/// workaround entries that go with them.
const UNLISTED_DEVICE_RECORD: u64 = 0xA94C_B27A_A85B_6AA7;

#[test]
fn a_device_the_kernel_drivers_data_does_not_list_gets_the_record_it_got_before() {
    let mut card = recording::read(ARC_B580).unwrap();
    card.pci.device = 0xE20D;
    card.config.device_id = 0xE20D;
    let backend = IntelBackend::from_card(card).unwrap();
    assert_eq!(fingerprint(backend.record()), UNLISTED_DEVICE_RECORD);
}

/// A card's feature table as Intel's kernel driver gives it: the values of its known bits, and
/// which bits are known, in hex, the table's first byte first.
struct DataTable {
    values: &'static str,
    known: &'static str,
}

/// 8086:e20b, its known bits and which are known.
const B580_DATA: DataTable = DataTable {
    values: "\
        a5d50500000002012011efe6ae771000000000001520d0075080cc1d35e4008a84e804aaa00000000100000001000000 \
        30f7daea095079b7d7453188917780e29502002000000000010000002000000000df23c01f3304000800000005000000 \
        f33100f8073c0238417cb9f16ee31300bd1b5e608fb7c10732eb8da43b8402f823a67827a00840000c37c0bcff0fee3b \
        e400c7321f716fe0ffdf059eff46042ce1b7ff3f3000d10200000000000000001f800000000000002900000000000000 \
        010000000a000000000000000000000000000000000000000000000000000000a70e00000000000000000000",
    known: "\
        a5dd05000001f7013851ffe6ae771800000000001d20d0075080cc1d35f4408a84e814aaa00100000100000001000000 \
        30fffeea1b7979ffd7473188917780e2d502002200000000010000002000000000ff33e07f3315000900000007000000 \
        f33111fb073cc23a57fcf9f16fe31300bd1b5f60aff7c10732ef8de43b8412f827b67827a0fe41004cb7debfff0fee3b \
        e400c7f61f716feafffff5bfff66042cedb7ff3f3000d10200000000000000001f800100000000002900000000000000 \
        010000000a000000060000000000000000000000000000000000000000000000a70e00000000000000000000",
};

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
    digits.chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
}

fn place(feature: Feature) -> usize {
    (feature.byte - features::TABLE) * 8 + usize::from(feature.bit)
}

#[test]
fn the_recorded_b580_gets_the_kernel_drivers_feature_table_but_where_this_record_departs() {
    let backend = IntelBackend::from_card(recording::read(ARC_B580).unwrap()).unwrap();
    let record = backend.record();
    let (values, known) = (hex(B580_DATA.values), hex(B580_DATA.known));
    let is = |bytes: &[u8], at: usize| bytes[at / 8] >> (at % 8) & 1 == 1;
    let table = &record[features::TABLE..];
    let departures: BTreeMap<usize, (bool, bool)> = (0..card_data::TABLE_BYTES * 8)
        .filter(|&at| is(&known, at) && is(&values, at) != is(table, at))
        .map(|at| (at, (is(&values, at), is(table, at))))
        .collect();
    let unnamed = |index| features::bit("unnamed", index);
    // Each departure by name: the driver's value, then the record's.
    let expected = vec![
        // The GPU can't share the guest's pointers.
        (features::SVM, true, false),
        // Compression stays off.
        (features::COMPRESSION[0], true, false),
        (features::COMPRESSION[1], true, false),
        (features::COMPRESSION[2], true, false),
        (features::COMPRESSION[3], true, false),
        (features::RENDER_COMPRESSION, true, false),
        (unnamed(684), true, false),
        // xe gives this card 48 bits of address.
        (features::PML5, true, false),
        // Compute-runtime's mid-thread preemption.
        (features::GPGPU_MID_THREAD_PREEMPT, false, true),
        // Tiled resources' translation table, null pages and tile mapping.
        (features::TILE_MAPPED_RESOURCE, true, false),
        (features::TRANSLATION_TABLE, true, false),
        (features::USER_MODE_TRANSLATION_TABLE, true, false),
        (features::NULL_PAGES, true, false),
        // The kernel driver's own reset.
        (features::DRIVER_FLR, true, false),
        // The Direct3D 12 capability that wants resources shared across adapters.
        (unnamed(505), true, false),
    ];
    // The card gives the URB stage limits, so the record offers every Direct3D 11 interface version,
    // the older ones the driver's table has among them.
    for version in features::every_ddi_version() {
        assert!(features::is_set(record, version), "{}", version.name);
    }
    let expected: BTreeMap<usize, (bool, bool)> =
        expected.iter().map(|&(feature, data, ours)| (place(feature), (data, ours))).collect();
    assert_eq!(departures, expected);
    for feature in [
        features::LOCAL_MEMORY,
        features::LOCAL_MEMORY_ALLOWS_4KB,
        features::XE2_TILING,
        features::WALKER_MTP,
        features::CCS_RING,
        features::BLITTER_RING,
        features::HW_SCHEDULING,
        features::WDDM2_64KB_PAGES,
    ] {
        assert!(features::is_set(record, feature), "{}", feature.name);
    }
    for feature in [features::CCS_MULTI_INSTANCE, features::TILE_Y, features::WDDM2_SVM] {
        assert!(!features::is_set(record, feature), "{}", feature.name);
    }
}

#[test]
fn the_recorded_b580_gets_the_kernel_drivers_platform_words() {
    let backend = IntelBackend::from_card(recording::read(ARC_B580).unwrap()).unwrap();
    let record = backend.record();
    let read = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
    let word = |at: usize| u16::from_le_bytes(record[at..at + 2].try_into().unwrap());
    assert_eq!(read(0x210), 0xE01, "display core, display 14.01's");
    assert_eq!(read(0x214), 0xC09, "render core");
    assert_eq!(read(0x218), 1, "platform type, a desktop card");
    assert_eq!((word(0x220), word(0x222)), (0x8086, 0x1100), "subsystem");
    assert_eq!(read(0x228), 0, "GT type");
    assert_eq!(read(0x22C), 0xD01, "media core, media 13.01's");
    assert_eq!(read(0x230), 14 << 22 | 1 << 14, "display 14.01 at revision 0");
    assert_eq!(read(0x23C), 13 << 22 | 1 << 14 | 1, "media 13.01 revision 1");
    assert_eq!(read(0x244), 0xE01);
    assert_eq!([read(0x254), read(0x258), read(0x25C), read(0x260)], [0xC09; 4]);
    assert_eq!([read(0x264), read(0x268), read(0x26C), read(0x270)], [0xD01; 4]);
}

#[test]
fn the_recorded_b580_gives_the_guest_every_hwconfig_field_where_the_kernel_driver_puts_it() {
    let backend = IntelBackend::from_card(recording::read(ARC_B580).unwrap()).unwrap();
    let record = backend.record();
    let read = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
    for (name, at, value) in [
        ("NUM_PIXEL_PIPES", 0x2114, 2),
        ("MAX_MEMORY_CHANNELS", 0x2188, 2),
        ("MEMORY_TYPE", 0x219C, 4),
        ("CACHE_TYPES", 0x218C, 1),
        ("LOCAL_MEMORY_PAGE_SIZES_SUPPORTED", 0x2190, 0x4021_1000),
        ("MIN_PCS_URB_ENTRIES", 0x2128, 0),
        ("MAX_PCS_URB_ENTRIES", 0x212C, 0),
        ("PUSH_CONSTANT_URB_RESERVED_SIZE", 0x2160, 32768),
        ("URB_REGION_ALIGNMENT_SIZE_IN_BYTES", 0x2168, 8192),
        ("URB_ALLOCATION_SIZE_UNITS_IN_BYTES", 0x216C, 64),
        ("VS_MIN_DEREF_BLOCK_SIZE_HANDLE_COUNT", 0x2178, 192),
        ("DS_MIN_DEREF_BLOCK_SIZE_HANDLE_COUNT", 0x217C, 324),
        ("NUM_RT_STACKS_PER_DSS", 0x2180, 2048),
        ("MAX_NUM_L3_CONFIGS", 0x21B4, 0),
        // Kept at zero on purpose: see the record.
        ("BINDLESS_SURFACE_OFFSET_BIT_COUNT", 0x22FC, 0),
        ("MAX_GSC", 0x2314, 1),
        ("MAX_GSC again", 0x23A4, 1),
        ("MAX_VCS again", 0x239C, 2),
        ("MAX_VECS again", 0x23A0, 2),
        ("MAX_SUBSLICE", 0xB9C, 20),
        ("MAX_EU_PER_SUBSLICE", 0xB94, 8),
        // No key 2 on this card: the most cores worked out from key 70 stay.
        ("most dual subslices", 0xBA0, 20),
    ] {
        assert_eq!(read(at), value, "{name}");
    }
}
