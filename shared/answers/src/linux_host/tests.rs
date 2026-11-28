//! The Linux host's pieces, as its own tests in `app-core` held them. Its answer files are pinned byte
//! for byte there, in `tests/answers-golden/`.

use super::editions::*;
use super::lean::*;
use super::ubuntu::{META_DATA, fill_template};
use super::wim::*;
use super::windows_guest::*;
use super::xml_text;
use super::zones::*;

#[test]
fn xml_text_escapes_what_xml_reads_the_apostrophe_too() {
    assert_eq!(xml_text("a<b>&\"c'"), "a&lt;b&gt;&amp;&quot;c&apos;");
    assert_eq!(xml_text("plain"), "plain");
    assert_eq!(xml_text("Zoë 日本"), "Zoë 日本");
}

#[test]
fn an_edition_on_the_list_has_its_key_and_a_year_on_the_end_is_the_same_edition() {
    assert_eq!(edition_key(WINDOWS_EDITION), Some("W269N-WFGWX-YVC9B-4J6C9-T83GX"));
    assert_eq!(edition_key("Windows 11 IoT Enterprise LTSC"), Some("KBN8V-HFGQ4-MGXVD-347P6-PDQGT"));
    assert_eq!(edition_key("Windows 11 IoT Enterprise LTSC 2024"), Some("KBN8V-HFGQ4-MGXVD-347P6-PDQGT"));
    assert_eq!(edition_key("Windows 11 Enterprise LTSC"), None, "an edition not run here has no key");
    assert!(!same_edition("Windows 11 Pro N", WINDOWS_EDITION));
}

#[test]
fn the_wim_reader_takes_the_images_a_block_lists() {
    let xml = "<IMAGE INDEX=\"1\"><NAME>x</NAME><BUILD>many</BUILD><ARCH>9</ARCH></IMAGE>";
    assert!(images_in(xml).is_empty(), "a build that is no number is passed over");
    let xml = "<IMAGE INDEX=\"1\"><NAME>Windows 11 Pro</NAME><ARCH>9</ARCH><BUILD>26100</BUILD></IMAGE>";
    assert_eq!(images_in(xml), [("Windows 11 Pro".into(), "Windows 11 Pro".into(), 26100, WIM_AMD64.into())]);
    let text: Vec<u8> = "<WIM/>".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut file = vec![0; WIM_HEADER_SIZE];
    file[..8].copy_from_slice(WIM_MAGIC);
    file[WIM_XML_ENTRY..WIM_XML_ENTRY + 8].copy_from_slice(&(text.len() as u64 | 1 << 56).to_le_bytes());
    file[WIM_XML_ENTRY + 8..WIM_XML_ENTRY + 16].copy_from_slice(&(WIM_HEADER_SIZE as u64).to_le_bytes());
    file.extend(text);
    assert_eq!(wim_xml(&mut file.as_slice()).as_deref(), Some("<WIM/>"));
    assert_eq!(wim_xml(&mut &b"not a wim"[..]), None);
}

#[test]
fn the_zone_table_is_whole_and_holds_plain_names() {
    let table = table();
    assert_eq!(table.len(), 139);
    assert_eq!(table[0], ("Etc/UTC", "UTC"));
    assert_eq!(table[1], ("Europe/Bucharest", "GTB Standard Time"));
    assert_eq!(table[138], ("Pacific/Tongatapu", "Tonga Standard Time"));
    assert!(!plain_name("<TimeZone>"));
    assert_eq!(FALLBACK, "UTC");
}

#[test]
fn the_template_is_filled_with_the_zone_edition_key_and_account() {
    let filled = fill(TEMPLATE, "Tokyo Standard Time", "a'b", "p&w", None);
    assert!(filled.contains("<TimeZone>Tokyo Standard Time</TimeZone>"));
    assert!(filled.contains("<Value>Windows 11 Pro</Value>"));
    assert!(filled.contains("<Key>W269N-WFGWX-YVC9B-4J6C9-T83GX</Key>"));
    assert!(filled.contains("<Name>a&apos;b</Name>"));
    assert!(filled.contains("<Value>p&amp;w</Value>"));
    assert!(!filled.contains(USER_MARK) && !filled.contains(PASSWORD_MARK));
    let other = fill("@PRODUCT_KEY@|@EDITION@", "UTC", "u", "p", Some("Windows 11 Home"));
    assert_eq!(other, "|Windows 11 Home", "an edition with no key gets none");
}

#[test]
fn the_lean_step_goes_last_in_the_specialize_pass_or_nowhere() {
    let lean = with_specialize_step(TEMPLATE).unwrap();
    assert_eq!(lean.replace(SPECIALIZE_COMMAND, ""), TEMPLATE);
    assert_eq!(lean.matches("<Order>5</Order>").count(), 1);
    assert_eq!(with_specialize_step("<unattend/>"), Err(NO_PLACE));
}

#[test]
fn every_mark_in_the_ubuntu_template_is_filled() {
    let filled =
        fill_template("@USER@ @PASSWORD_HASH@ @SSH_KEY@ @KERNEL_PACKAGE@ @SSH_KEY@", "u", "h", "k", "p");
    assert_eq!(filled, "u h k p k");
    let template = super::ubuntu::TEMPLATE;
    let filled = fill_template(template, "desk", "h", "k", "p");
    assert!(!filled.contains("@USER@") && template.contains("@KERNEL_PACKAGE@"));
    assert!(META_DATA.starts_with("instance-id:"));
}
