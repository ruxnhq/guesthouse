//! Where reports go. The server keeps each one for 30 days, for the developer and their own tools to
//! read, and never serves one back: a report's ID opens nothing for anyone else.

/// The drop box that takes reports for several projects, each under its own name.
const ADDRESS: &str = "https://files.ruxn.com/r/";
const PROJECT: &str = "guesthouse";
/// Sends somewhere else, for a test against a stand-in. Unset,
/// reports go to Guesthouse's.
const ADDRESS_VARIABLE: &str = "GUESTHOUSE_REPORT_ADDRESS";

/// The largest report the server takes, in bytes.
pub const MOST_REPORT_BYTES: u64 = 50_000_000;
/// How long the server keeps a report.
pub const KEPT_DAYS: u32 = 30;
/// Where new issues are opened, with the report's ID in them.
pub const NEW_ISSUE: &str = "https://github.com/ruxnlabs/guesthouse/issues/new";

/// The address a report is sent to.
pub fn report_address() -> String {
    std::env::var(ADDRESS_VARIABLE)
        .ok()
        .filter(|address| !address.is_empty())
        .unwrap_or_else(|| format!("{ADDRESS}{PROJECT}"))
}
