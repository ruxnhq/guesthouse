//! Bug reports: what's gathered about one machine and this PC, made into one file the tester can send
//! to the developer with a click, or save. Both `guesthouse-report` and the app's dialog use this.
//!
//! - `collect` gathers each part from a list of its own, and never stops for one that can't be read.
//! - `scrub` and `trim` make each file fit to send, and `bundle` packs them with a manifest.
//! - `send` posts the bundle, and `issue_link` offers a GitHub issue with its ID afterwards.
//!
//! Nothing here sends anything unless the tester asks, and nothing comes from inside the guest but
//! its serial console, which the tester can leave out.

pub mod bundle;
pub mod collect;
pub mod endpoint;
pub mod issue_link;
pub mod kind;
pub mod scrub;
pub mod send;
pub mod summary;
mod trim;

pub use bundle::{Bundle, Words, write_bundle};
pub use collect::{Collection, Request, chosen_machine, collect, latest_machine};
pub use kind::ProblemKind;
pub use scrub::Scrubber;

use crate::context::Context;
use crate::log::read_log;
use std::fs;
use std::path::PathBuf;

const REPORTS_FOLDER: &str = "reports";
/// Reports the app made and kept, newest first, so a tester can save one after sending it.
const KEPT_REPORTS: usize = 3;

/// Where the app writes the reports it makes, beside its log.
pub fn reports_folder(context: &Context) -> PathBuf {
    context.paths.state.join(REPORTS_FOLDER)
}

/// Gathers and writes a report in the app's own folder, and leaves only the newest few there.
pub fn write_in_reports_folder(
    context: &Context,
    collection: &Collection,
    words: &Words,
) -> crate::Result<Bundle> {
    let folder = reports_folder(context);
    fs::create_dir_all(&folder)?;
    let bundle = write_bundle(collection, words, &Scrubber::for_this_pc(&context.paths.home), &folder)?;
    let mut kept: Vec<PathBuf> = fs::read_dir(&folder)?.flatten().map(|entry| entry.path()).collect();
    // The names hold the time, so the newest sort last.
    kept.sort();
    let old = kept.len().saturating_sub(KEPT_REPORTS);
    for path in &kept[..old] {
        let _ = fs::remove_file(path);
    }
    Ok(bundle)
}

/// The link to a new issue for a sent report, with the app's own log, scrubbed, for its excerpt.
pub fn new_issue_link(context: &Context, report_id: &str, bundle: &Bundle, description: &str) -> String {
    let log = Scrubber::for_this_pc(&context.paths.home).scrub(&read_log(context));
    issue_link::issue_link(report_id, &bundle.summary, description, &issue_link::failure_excerpt(&log))
}
