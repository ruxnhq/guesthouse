//! What a report's sender and the report server agree on. Each value is the Linux host's, and a test
//! reads the Linux sources, so the two can't drift apart.

/// The drop box that takes reports for several projects, each under its own name.
pub const ADDRESS: &str = "https://files.ruxn.com/r/";
pub const PROJECT: &str = "guesthouse";
/// Sends somewhere else, for a test against a stand-in. Unset, reports go to Guesthouse's.
pub const ADDRESS_VARIABLE: &str = "GUESTHOUSE_REPORT_ADDRESS";
/// How the report is packed, said in the `X-Report-Format` header.
pub const FORMAT: &str = "tar.gz";
/// The header that says how the report is packed.
pub const FORMAT_HEADER: &str = "X-Report-Format";
/// The header that carries the summary.
pub const SUMMARY_HEADER: &str = "X-Report-Summary";
/// The largest report the server takes, in bytes.
pub const MOST_REPORT_BYTES: u64 = 50_000_000;
/// How long the server keeps a report.
pub const KEPT_DAYS: u32 = 30;
/// The status the server answers with when it took the report.
pub const CREATED: u16 = 201;
/// How many hex digits a report's ID has.
pub const ID_LENGTH: usize = 10;
/// Where new issues are opened, with the report's ID in them.
pub const NEW_ISSUE: &str = "https://github.com/ruxnlabs/guesthouse/issues/new";

/// The address a report is sent to.
pub fn report_address() -> String {
    std::env::var(ADDRESS_VARIABLE)
        .ok()
        .filter(|address| !address.is_empty())
        .unwrap_or_else(|| format!("{ADDRESS}{PROJECT}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENDPOINT: &str = include_str!("../../../app-core/src/report/endpoint.rs");
    const SEND: &str = include_str!("../../../app-core/src/report/send.rs");
    const BUNDLE: &str = include_str!("../../../app-core/src/report/bundle.rs");

    #[test]
    fn the_address_is_the_linux_hosts() {
        assert!(ENDPOINT.contains(&format!("const ADDRESS: &str = \"{ADDRESS}\";")));
        assert!(ENDPOINT.contains(&format!("const PROJECT: &str = \"{PROJECT}\";")));
        assert!(ENDPOINT.contains(&format!("const ADDRESS_VARIABLE: &str = \"{ADDRESS_VARIABLE}\";")));
        assert!(ENDPOINT.contains(&format!("pub const NEW_ISSUE: &str = \"{NEW_ISSUE}\";")));
    }

    #[test]
    fn the_limits_are_the_linux_hosts() {
        assert!(ENDPOINT.contains("pub const MOST_REPORT_BYTES: u64 = 50_000_000;"));
        assert_eq!(MOST_REPORT_BYTES, 50_000_000);
        assert!(ENDPOINT.contains(&format!("pub const KEPT_DAYS: u32 = {KEPT_DAYS};")));
    }

    #[test]
    fn the_format_and_headers_are_the_linux_hosts() {
        assert!(BUNDLE.contains(&format!("pub const FORMAT: &str = \"{FORMAT}\";")));
        assert!(SEND.contains(&format!("\"{FORMAT_HEADER}: {{FORMAT}}\"")));
        assert!(SEND.contains(&format!("\"{SUMMARY_HEADER}: {{summary}}\"")));
    }

    #[test]
    fn the_answer_is_the_linux_hosts() {
        assert!(SEND.contains(&format!("const CREATED: u16 = {CREATED};")));
        assert!(SEND.contains(&format!("const ID_LENGTH: usize = {ID_LENGTH};")));
    }

    #[test]
    fn the_address_has_the_project_in_it() {
        // The variable is only read, so a test that finds it unset sees the default.
        if std::env::var(ADDRESS_VARIABLE).is_err() {
            assert_eq!(report_address(), "https://files.ruxn.com/r/guesthouse");
        }
    }
}
