//! Where reports go. The server keeps each one for 30 days, for the developer and their own tools to
//! read, and never serves one back: a report's ID opens nothing for anyone else.
//!
//! What the server takes is the same for every host, so it's `guesthouse-report-core`'s.

pub use guesthouse_report_core::contract::{KEPT_DAYS, MOST_REPORT_BYTES, NEW_ISSUE, report_address};
