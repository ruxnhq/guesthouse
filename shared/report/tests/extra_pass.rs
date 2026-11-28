//! A host's own scrubbing pass, given with `Scrubber::with_extra`: it runs after the shared list, on
//! text that list has already scrubbed, and the shared list runs once more after it, so a pass can
//! take more out but never put back what the list takes out. Without a pass nothing changes; the
//! Windows host's golden copies in `control/tests/report-golden` hold that byte for byte.

use guesthouse_report_core::scrub::{Extra, Scrubber};
use std::cell::RefCell;

/// A user, a PC, a profile and machines that belong to nobody.
fn scrubber() -> Scrubber {
    let machines = ["work-box", "den-pc"].map(str::to_owned);
    Scrubber::new("quillon", r"C:\Users\quillon", "marrowbox", &machines)
}

const RAW: &str = "quillon on marrowbox opened C:\\Users\\quillon\\vm\\work-box.json for quillon@example.org \
                   at 52:54:00:12:34:56 badge=opal-42\n";

thread_local! {
    /// What the pass was given, on this test's thread.
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn noting(text: &str) -> String {
    SEEN.with(|seen| seen.borrow_mut().push(text.to_owned()));
    text.to_owned()
}

fn seen() -> Vec<String> {
    SEEN.with(|seen| seen.borrow_mut().drain(..).collect())
}

/// A rule a host might add: the value after `badge=`, made up for these tests.
fn badges(text: &str) -> String {
    match text.find("badge=") {
        Some(at) => {
            let start = at + "badge=".len();
            let end = text[start..].find(char::is_whitespace).map_or(text.len(), |n| start + n);
            format!("{}[gone]{}", &text[..start], &text[end..])
        }
        None => text.to_owned(),
    }
}

#[test]
fn without_a_pass_it_scrubs_as_it_did() {
    let said = scrubber().scrub(RAW);
    assert_eq!(
        said, "<user> on <host> opened ~\\vm\\machine.json for <email> at <mac> badge=opal-42\n",
        "the shared list alone"
    );
}

#[test]
fn the_pass_runs_and_its_work_is_kept() {
    let said = scrubber().with_extra(badges).scrub(RAW);
    assert_eq!(said, "<user> on <host> opened ~\\vm\\machine.json for <email> at <mac> badge=[gone]\n");
}

#[test]
fn the_pass_is_given_text_already_scrubbed_never_the_raw() {
    let shared = scrubber().scrub(RAW);
    let _ = seen();
    let said = scrubber().with_extra(noting).scrub(RAW);
    assert_eq!(seen(), std::slice::from_ref(&shared), "it ran once, on what the shared list made");
    assert_eq!(said, shared, "a pass that changes nothing changes nothing");
}

#[test]
fn the_pass_runs_after_the_shared_list() {
    // Only the shared list writes `<user>`, so a pass that finds it ran after it.
    let said = scrubber().with_extra(|text| text.replace("<user>", "[the user]")).scrub(RAW);
    assert!(said.starts_with("[the user] on <host>"), "{said}");
}

#[test]
fn a_pass_cannot_put_back_what_the_shared_list_takes_out() {
    let putting_back: Extra = |text| {
        format!(
            "{text}quillon on marrowbox, C:\\Users\\quillon\\x, work-box, den-pc, someone@example.org, \
             aa-bb-cc-dd-ee-ff, 6f1c2a1e-0d6b-4c51-9a37-2b8e6c3f0a11\n"
        )
    };
    let said = scrubber().with_extra(putting_back).scrub(RAW);
    for raw in ["quillon", "marrowbox", "Users", "work-box", "den-pc", "someone@", "aa-bb", "6f1c2a1e"] {
        assert!(!said.contains(raw), "{raw} came back:\n{said}");
    }
    assert!(said.ends_with("<user> on <host>, ~\\x, machine, machine2, <email>, <mac>, <uuid>\n"), "{said}");
}

#[test]
fn passes_run_in_the_order_given_and_none_is_dropped() {
    let first: Extra = |text| text.replace("<host>", "first");
    let second: Extra = |text| text.replace("first", "second");
    let said = scrubber().with_extra(first).with_extra(second).scrub(RAW);
    assert!(said.contains(" on second opened "), "{said}");
    let said = scrubber().with_extra(second).with_extra(first).scrub(RAW);
    assert!(said.contains(" on first opened "), "{said}");
}

#[test]
fn a_copy_keeps_its_passes_and_goes_to_any_thread() {
    fn sendable<T: Send + Sync + Clone + std::fmt::Debug + 'static>(_: &T) {}
    let scrubber = scrubber().with_extra(badges);
    sendable(&scrubber);
    let copy = scrubber.clone();
    let said = std::thread::spawn(move || copy.scrub(RAW)).join().unwrap();
    assert_eq!(said, scrubber.scrub(RAW));
    assert!(said.contains("badge=[gone]"));
}

#[test]
fn the_new_names_have_these_shapes() {
    let _: fn(Scrubber, Extra) -> Scrubber = Scrubber::with_extra;
    let _: Extra = badges;
}
