//! Which host made a report, where a report's own text says it: the summary's `host` and the
//! manifest's first sentence. The Linux and Windows hosts' values are what each wrote before, and
//! another host names itself with `Host::new`.

use guesthouse_report_core::host::Host;
use guesthouse_report_core::issue::issue_link;
use guesthouse_report_core::manifest::{Missing, manifest, manifest_for, manifest_from};
use guesthouse_report_core::summary::{self, HOST, HOST_WINDOWS, Summary};

fn files() -> Vec<(String, usize)> {
    vec![("service.log".to_owned(), 1200), ("summary.json".to_owned(), 80)]
}

fn missing() -> Vec<Missing> {
    vec![Missing { name: "crash dumps".into(), reason: "there were none".into(), how: None }]
}

#[test]
fn the_windows_host_is_named_as_it_was() {
    assert_eq!(Host::WINDOWS.word(), Some(HOST_WINDOWS));
    assert_eq!(Host::WINDOWS.word(), Some("windows"));
    assert_eq!(Host::WINDOWS.phrase(), Some("a Windows host"));
    let pairs = [("crash dumps".to_owned(), "there were none".to_owned())];
    assert_eq!(manifest_for(Host::WINDOWS, &files(), &missing(), &[]), manifest(&files(), &pairs));
    let mut summary = Summary::default();
    summary.set_host(Host::WINDOWS);
    let mut before = Summary::default();
    before.set(HOST, HOST_WINDOWS);
    assert_eq!(summary, before);
}

#[test]
fn the_linux_host_names_none_as_before() {
    assert_eq!((Host::LINUX.word(), Host::LINUX.phrase()), (None, None));
    let notes = ["A note.".to_owned()];
    assert_eq!(
        manifest_for(Host::LINUX, &files(), &missing(), &notes),
        manifest_from(None, &files(), &missing(), &notes)
    );
    assert!(manifest_for(Host::LINUX, &[], &[], &[]).starts_with("A Guesthouse bug report. It goes"));
    let mut summary = Summary::default();
    summary.set_host(Host::LINUX);
    assert_eq!(summary, Summary::default(), "no host fact");
}

#[test]
fn another_host_names_itself_where_the_windows_host_does() {
    const TABLET: Host = Host::new("tablet", "a tablet");
    let windows = manifest_for(Host::WINDOWS, &files(), &missing(), &[]);
    let tablet = manifest_for(TABLET, &files(), &missing(), &[]);
    assert!(tablet.starts_with("A Guesthouse bug report from a tablet. It goes to the developer"));
    assert_eq!(tablet.replace("a tablet", "a Windows host"), windows, "only the host differs");
    let mut summary = Summary::default();
    summary.set(summary::KIND, "crashed");
    summary.set_host(TABLET);
    assert_eq!(summary.get(HOST), Some("tablet"));
    assert!(issue_link("0a1b2c3d4e", &summary).contains("host%3A%20tablet"));
}

#[test]
#[should_panic(expected = "a host names itself in both places")]
fn a_host_with_no_word_is_refused() {
    let word = String::new().leak();
    let _ = Host::new(word, "a tablet");
}

#[test]
fn the_new_names_have_these_shapes() {
    let _: fn(&'static str, &'static str) -> Host = Host::new;
    let _: fn(Host) -> Option<&'static str> = Host::word;
    let _: fn(Host) -> Option<&'static str> = Host::phrase;
    type Sizes<'a> = &'a [(String, usize)];
    let _: fn(Host, Sizes, &[Missing], &[String]) -> String = manifest_for;
    let _: fn(&mut Summary, Host) = Summary::set_host;
    let _: [Host; 2] = [Host::LINUX, Host::WINDOWS];
}
