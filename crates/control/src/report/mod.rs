//! Bug reports for a Windows host. The Windows app builds a report with these, and sends it to the
//! same server the Linux host's reports go to, so what's on the wire is the same. The service only
//! hands over its files. It never builds or sends a report.
//!
//! - `contract` holds what the server and the Linux host agree on, pinned to the Linux sources.
//! - `answer` reads the server's answer to a sent report.
//! - `kind` and `summary` are what sorts a report before anyone opens it.
//! - `scrub` and `trim` make each file fit to send, and `manifest` says what the bundle holds.
//!
//! Nothing here calls into Windows, so it builds and is tested anywhere.

pub mod answer;
pub mod contract;
pub mod kind;
pub mod manifest;
pub mod scrub;
pub mod summary;
pub mod trim;

pub use answer::read_answer;
pub use contract::{KEPT_DAYS, MOST_REPORT_BYTES, NEW_ISSUE, report_address};
pub use kind::ProblemKind;
pub use manifest::manifest;
pub use scrub::Scrubber;
pub use summary::Summary;
pub use trim::keep_end;
