//! The Linux host's reading of a WIM's XML block and its edition match, kept byte for byte in
//! `tests/answers-golden/linux-wim.txt`, as `crate::answers_golden` keeps the answer files.

use super::stand_in::{make_iso, wim};
use super::*;
use crate::answers_golden::golden;
use crate::testing::Scratch;
use std::fmt::Write as _;

/// Images as a WIM's XML block lists them: one with a display name, one without, one of another
/// architecture, one with no build, one with no name, and XML the reader takes as it is.
const XML: &str = "<WIM><TOTALBYTES>1</TOTALBYTES>\
<IMAGE INDEX=\"1\"><NAME>Windows 11 Home</NAME><DISPLAYNAME>Windows 11 Famille</DISPLAYNAME>\
<WINDOWS><ARCH>9</ARCH><VERSION><BUILD>26100</BUILD></VERSION></WINDOWS></IMAGE>\
<IMAGE INDEX=\"2\"><NAME>Windows 11 Pro</NAME><WINDOWS><ARCH>9</ARCH><VERSION><BUILD>26100</BUILD>\
</VERSION></WINDOWS></IMAGE>\
<IMAGE INDEX=\"3\"><NAME>Windows 11 Pro</NAME><WINDOWS><ARCH>12</ARCH><VERSION><BUILD>26100</BUILD>\
</VERSION></WINDOWS></IMAGE>\
<IMAGE INDEX=\"4\"><NAME>Windows 10 Pro</NAME><WINDOWS><ARCH>9</ARCH><VERSION><BUILD>19045</BUILD>\
</VERSION></WINDOWS></IMAGE>\
<IMAGE INDEX=\"5\"><NAME>No build</NAME><WINDOWS><ARCH>9</ARCH></WINDOWS></IMAGE>\
<IMAGE INDEX=\"6\"><WINDOWS><ARCH>9</ARCH><VERSION><BUILD>26100</BUILD></VERSION></WINDOWS></IMAGE>\
<IMAGE INDEX=\"7\"><NAME>A &amp; B</NAME><DISPLAYNAME></DISPLAYNAME><WINDOWS><ARCH>9</ARCH>\
<VERSION><BUILD>26200</BUILD></VERSION></WINDOWS></IMAGE>\
<IMAGE INDEX=\"8\"><NAME>Windows 11 IoT Enterprise LTSC 2024</NAME><DISPLAYNAME>Windows 11 IoT \
Enterprise LTSC 2024</DISPLAYNAME><WINDOWS><ARCH>9</ARCH><VERSION><BUILD>26100</BUILD></VERSION>\
</WINDOWS></IMAGE>\
<IMAGE INDEX=\"9\"><NAME>Unclosed</NAME></WIM>";

#[test]
fn the_wim_reader_and_the_edition_match_are_as_they_were() {
    let mut made = String::new();
    writeln!(made, "first Windows 11 build: {FIRST_WINDOWS_11_BUILD}, amd64: {WIM_AMD64}").unwrap();
    writeln!(made, "header {WIM_HEADER_SIZE} bytes, XML entry at {WIM_XML_ENTRY:#x}, at most {MOST_WIM_XML}")
        .unwrap();
    for image in images_in(XML) {
        writeln!(made, "image {image:?}").unwrap();
    }
    for (text, tag) in
        [("<A>x</A>", "A"), ("<A></A>", "A"), ("<A>x", "A"), ("<B>x</B>", "A"), ("<A><A>y</A></A>", "A")]
    {
        writeln!(made, "field {text:?} {tag} -> {:?}", field(text, tag)).unwrap();
    }
    let block = wim(&["Windows 11 Home", "Windows 11 Pro"], 26300, 9);
    writeln!(made, "wim -> {:?}", wim_xml(&mut block.as_slice())).unwrap();
    writeln!(made, "not a wim -> {:?}", wim_xml(&mut &b"MSWIM\0\0\x01"[..])).unwrap();
    writeln!(made, "cut short -> {:?}", wim_xml(&mut &block[..1000])).unwrap();
    writeln!(made, "utf-16 -> {:?}", utf16_text(&[0x41, 0, 0xe9, 0, 0x2c, 0x65, 0x42])).unwrap();
    for (image, name) in [
        ("Windows 11 Pro", "Windows 11 Pro"),
        ("Windows 11 IoT Enterprise LTSC 2024", "Windows 11 IoT Enterprise LTSC"),
        ("Windows 11 IoT Enterprise LTSC 202", "Windows 11 IoT Enterprise LTSC"),
        ("Windows 11 IoT Enterprise LTSC-2024", "Windows 11 IoT Enterprise LTSC"),
        ("Windows 11 Pro N", "Windows 11 Pro"),
        ("windows 11 pro", "Windows 11 Pro"),
    ] {
        writeln!(made, "same {image:?} {name:?} -> {}", same_edition(image, name)).unwrap();
    }
    golden("linux-wim.txt", &made);
}

/// Which edition a disc installs, through the reader and the table, as `read_iso` says it.
#[test]
fn the_edition_a_disc_installs_is_as_it_was() {
    let scratch = Scratch::new();
    let mut made = String::new();
    let discs: [(&str, &[&str], u32, u32); 6] = [
        ("home-and-pro", &["Windows 11 Home", "Windows 11 Pro"], 26300, 9),
        ("pro-and-ltsc", &["Windows 11 IoT Enterprise LTSC 2024", "Windows 11 Pro"], 26100, 9),
        ("ltsc-year", &["Windows 11 IoT Enterprise LTSC 2024"], 26100, 9),
        ("home-only", &["Windows 11 Home"], 26100, 9),
        ("windows-10", &["Windows 10 Pro"], 19045, 9),
        ("arm", &["Windows 11 Pro"], 26100, 12),
    ];
    for (name, images, build, architecture) in discs {
        let data = wim(images, build, architecture);
        let iso = make_iso(&scratch, name, &[("sources/install.wim", &data)]);
        let found = read_iso(&iso).unwrap();
        writeln!(
            made,
            "{name} -> {:?} {:?} {:?} {}",
            found.kind, found.edition, found.title, found.windows_disc
        )
        .unwrap();
    }
    golden("linux-disc-editions.txt", &made);
}
