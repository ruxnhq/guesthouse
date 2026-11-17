//! Bug reports: what's gathered about one machine and this PC, made into one file the tester can send
//! to the developer with a click, or save. Both `guesthouse-report` and the app's dialog use this.
//!
//! - `collect` gathers each part from a list of its own, and never stops for one that can't be read.
//! - `scrub` and `trim` make each file fit to send, and `bundle` packs them with a manifest.
//! - `send` posts the bundle, and `issue_link` offers a GitHub issue with its ID afterwards.
//!
//! Nothing here sends anything unless the tester asks. From the guest, a report has its serial port's
//! output, which the tester can leave out, and whatever the app's log quotes of its answers to the
//! app's own scripts. Never its memory, its disk or a core file.

pub mod bundle;
pub mod collect;
pub mod endpoint;
pub mod issue_link;
pub mod kind;
pub mod scrub;
pub mod send;
pub mod summary;
pub(crate) mod trim;

pub use bundle::{Bundle, Words, write_bundle};
pub use collect::{Collection, Request, chosen_machine, collect, latest_machine};
pub use kind::ProblemKind;
pub use scrub::Scrubber;

use crate::context::Context;
use crate::log::read_log;
use crate::machine::{sorted_names, valid_id};
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
    let scrubber = scrubber_for(context, collection.machine.as_deref());
    let bundle = write_bundle(collection, words, &scrubber, &folder)?;
    let mut kept: Vec<PathBuf> = fs::read_dir(&folder)?.flatten().map(|entry| entry.path()).collect();
    // The names hold the time, so the newest sort last.
    kept.sort();
    let old = kept.len().saturating_sub(KEPT_REPORTS);
    for path in &kept[..old] {
        let _ = fs::remove_file(path);
    }
    Ok(bundle)
}

/// What takes this PC's names out of a report, and every machine's ID, with `about` as `<machine>`.
pub fn scrubber_for(context: &Context, about: Option<&str>) -> Scrubber {
    let ids: Vec<String> =
        sorted_names(&context.paths.machines).into_iter().filter(|name| valid_id(name)).collect();
    Scrubber::for_this_pc(&context.paths.home).with_machines(about, &ids)
}

/// The link to a new issue for a sent report, with the app's own log, scrubbed, for its excerpt.
pub fn new_issue_link(context: &Context, report_id: &str, bundle: &Bundle, description: &str) -> String {
    let scrubber = scrubber_for(context, bundle.machine.as_deref());
    let log = scrubber.scrub(&read_log(context));
    let description = scrubber.scrub_machines(description);
    issue_link::issue_link(report_id, &bundle.summary, &description, &issue_link::failure_excerpt(&log))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::write_log;
    use crate::testing::{Scratch, scratch_context};

    /// Two machines, named by their users, as the store has them.
    fn store_with_machines(scratch: &Scratch) -> Context {
        let context = scratch_context(scratch);
        for id in ["quillons-games", "work-box"] {
            fs::create_dir_all(context.paths.machines.join(id)).unwrap();
        }
        context
    }

    #[test]
    fn no_machines_id_reaches_the_bundle_or_the_issue() {
        let scratch = Scratch::new();
        let context = store_with_machines(&scratch);
        let disk = context.paths.machines.join("quillons-games/disk.qcow2");
        write_log(&context, &format!("start opened {}", disk.display())).unwrap();
        write_log(&context, "app: quillons-games did not start, as work-box has the card").unwrap();
        let mut collection = Collection { machine: Some("quillons-games".into()), ..Collection::default() };
        collection.add("app/guesthouse.log", crate::log::read_log(&context));
        let words = Words { description: "quillons-games went black".into(), contact: String::new() };
        let bundle = write_in_reports_folder(&context, &collection, &words).unwrap();
        let unpacked = scratch.path("unpacked");
        fs::create_dir_all(&unpacked).unwrap();
        let tar =
            std::process::Command::new("tar").arg("-xzf").arg(&bundle.path).arg("-C").arg(&unpacked).status();
        assert!(tar.unwrap().success());
        let all = std::process::Command::new("grep").args(["-r", "-h", ""]).arg(&unpacked).output().unwrap();
        let all = String::from_utf8_lossy(&all.stdout);
        assert!(!all.contains("quillons-games") && !all.contains("work-box"), "{all}");
        assert!(all.contains("machines/<machine>/disk.qcow2"), "{all}");
        assert!(all.contains("<machine> did not start, as <machine-2> has the card"));
        assert!(all.contains("<machine> went black"));

        let link = new_issue_link(&context, "0123456789", &bundle, &words.description);
        assert!(!link.contains("quillons") && !link.contains("work-box"), "{link}");
        assert!(link.contains("%3Cmachine%3E%20did%20not%20start%2C%20as%20%3Cmachine-2%3E"), "{link}");
        assert!(link.contains("%3Cmachine%3E%20went%20black"));
    }
}
