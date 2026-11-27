//! Bug reports for a Windows host. The Windows app builds a report with these, and sends it to the
//! same server the Linux host's reports go to, so what's on the wire is the same. The service only
//! hands over its files. It never builds or sends a report.
//!
//! Most of it is `guesthouse-report-core`'s, which every host shares, and is re-exported here under
//! the names it had:
//!
//! - `contract` is what the server and every host agree on, and `answer` reads its answer.
//! - `kind` and `summary` are what sorts a report before anyone opens it, and `host` says which host
//!   made it.
//! - `send` is how a report is posted, and `issue` the link that opens an issue about a sent one.
//! - `clock` writes times in UTC.
//! - `scrub` and `trim` make each file fit to send, and `manifest` says what the bundle holds.
//!
//! Two parts are this host's own: `parts` is how the service's files come over the pipe, and are
//! put back together, and `stage` makes the files from what was gathered.
//!
//! Nothing here calls into Windows, so it builds and is tested anywhere.

pub mod parts;
pub mod stage;

// The pieces every host shares, under the names they had here.
pub use guesthouse_report_core::{
    answer, clock, contract, host, issue, kind, manifest, scrub, send, summary, trim,
};

pub use answer::read_answer;
pub use contract::{KEPT_DAYS, MOST_REPORT_BYTES, NEW_ISSUE, report_address};
pub use issue::issue_link;
pub use kind::ProblemKind;
pub use manifest::{MANIFEST_FILE, manifest};
pub use parts::{PIECE_BYTES, Parts, pieces};
pub use scrub::Scrubber;
pub use summary::Summary;
pub use trim::keep_end;
