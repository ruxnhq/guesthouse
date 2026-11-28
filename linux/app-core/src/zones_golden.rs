//! The Linux host's zone table as it reads it, entry for entry, kept byte for byte in
//! `tests/answers-golden/linux-zone-table.txt`, as `crate::answers_golden` keeps the answer files.

use super::*;
use crate::answers_golden::golden;
use std::fmt::Write as _;

#[test]
fn the_zone_table_is_as_it_was() {
    let mut made = format!("fallback: {FALLBACK}\n");
    for (zone, name) in table() {
        writeln!(made, "{zone}\t{name}").unwrap();
    }
    for name in ["UTC", "W. Europe Standard Time", "UTC+12", "Name <x>", "Name&", "Name/x", "Naïve", ""] {
        writeln!(made, "plain {name:?} -> {}", plain_name(name)).unwrap();
    }
    golden("linux-zone-table.txt", &made);
}
