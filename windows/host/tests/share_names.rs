//! What a shared folder is called in a Windows host's guest, held as it is today: `shared` for the
//! first, then a number from 2. A guest's report finds the slot from the name, so the two must agree.

use guesthouse_host_windows::guest::REPORT;
use guesthouse_host_windows::settings::share_name;

#[test]
fn each_slot_has_the_name_it_has_today() {
    for (slot, name) in [(0, "shared"), (1, "shared2"), (2, "shared3"), (9, "shared10"), (99, "shared100")] {
        assert_eq!(share_name(slot), name, "{slot}");
    }
    assert!(REPORT.contains("'^shared(\\d*)$'"));
}
