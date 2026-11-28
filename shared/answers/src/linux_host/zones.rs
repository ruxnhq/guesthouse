//! The IANA zones and the names Windows Setup wants for them, as the Linux host gives a Windows guest
//! its host's zone.
//!
//! The table comes from Unicode's public CLDR data, under the Unicode License v3: `data/windows-zones.txt`
//! says where. Anything it can't place stays on UTC, which is also what the answer file's template says.
//! Reading the host's zone, and matching a zone that isn't in the table by its offsets, stay with the
//! host, as they read its zone files.

const ZONE_TABLE: &str = include_str!("../../data/windows-zones.txt");
pub const FALLBACK: &str = "UTC";

/// Each IANA zone in the table with Windows' name for it, in the table's order.
pub fn table() -> Vec<(&'static str, &'static str)> {
    let lines = ZONE_TABLE.lines().filter(|line| !line.starts_with('#'));
    lines.filter_map(|line| line.split_once('\t')).filter(|(_, name)| plain_name(name)).collect()
}

/// A bad value must never reach Windows Setup.
pub fn plain_name(name: &str) -> bool {
    name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b" .+()-".contains(&byte))
}
