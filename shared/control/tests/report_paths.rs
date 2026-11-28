//! The report names other hosts' code reaches through `guesthouse_control::report` and
//! `guesthouse_control::updates`, each at the path, and with the signature, it had before the shared
//! parts moved to `guesthouse-report-core`. A host that pins this crate by its commit builds on
//! these, so a change that drops or reshapes one fails to build here first.

use guesthouse_control::report::send::{curl_arguments, read_reply};
use guesthouse_control::report::stage::{DESCRIPTION_FILE, FIRST_LIMIT, MOST_REPACKS, SUMMARY_FILE, halved};
use guesthouse_control::report::summary::{self, Summary};
use guesthouse_control::report::{
    KEPT_DAYS, MANIFEST_FILE, MOST_REPORT_BYTES, NEW_ISSUE, ProblemKind, Scrubber, clock, issue_link,
    keep_end, report_address,
};
use guesthouse_control::updates::LATEST_URL;
use std::ffi::OsString;
use std::path::Path;

#[test]
fn every_report_name_another_host_uses_is_where_it_was() {
    let _: fn(&str, &str, &Path, &Path) -> Vec<OsString> = curl_arguments;
    let _: fn(&str, &str, bool, &str) -> Result<String, String> = read_reply;
    let _: fn(usize) -> usize = halved;
    let _: fn(&str, usize) -> String = keep_end;
    let _: fn() -> String = report_address;
    let _: fn(&str, &Summary) -> String = issue_link;
    let _: fn(&str, &str, &str, &[String]) -> Scrubber = Scrubber::new;
    let _: fn(&Scrubber, &str) -> String = Scrubber::scrub;
    let _: fn() -> u64 = clock::now;
    let _: fn(u64) -> String = clock::stamp;
    let _: fn(u64) -> String = clock::utc;
    let _: fn(ProblemKind) -> &'static str = ProblemKind::word;
    let _: fn(ProblemKind) -> &'static str = ProblemKind::label;
    let _: fn(&str) -> Option<ProblemKind> = ProblemKind::from_word;
    let _: [ProblemKind; 6] = ProblemKind::ALL;
    let _: fn(&mut Summary, &str, &str) = Summary::set;
    let _: fn(&Summary) -> String = Summary::to_json;
    let _: fn(&Summary) -> String = Summary::header;
    let _: usize = FIRST_LIMIT;
    let _: u32 = MOST_REPACKS;
    let _: u32 = KEPT_DAYS;
    let _: u64 = MOST_REPORT_BYTES;
    let _: [&str; 4] = [DESCRIPTION_FILE, SUMMARY_FILE, MANIFEST_FILE, NEW_ISSUE];
    let _: [&str; 9] = [
        summary::VERSION,
        summary::HOST,
        summary::OS,
        summary::CARD,
        summary::DRIVER,
        summary::MACHINE,
        summary::KIND,
        summary::DESCRIPTION,
        summary::CONTACT,
    ];
    let _: &str = LATEST_URL;
    let mut summary = Summary::default();
    summary.set(summary::KIND, ProblemKind::default().word());
    assert_eq!(summary.get(summary::KIND), Some("other"));
    assert_eq!(summary.facts().count(), 1);
}
