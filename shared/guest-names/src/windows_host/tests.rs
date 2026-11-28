use super::*;

/// The Windows host's guest report, which finds a share's slot from its name.
const REPORT: &str = include_str!("../../../../windows/host/guest/report.ps1");

#[test]
fn each_slot_has_the_name_the_guest_reads_its_slot_from() {
    for (slot, name) in [(0, "shared"), (1, "shared2"), (2, "shared3"), (9, "shared10"), (99, "shared100")] {
        assert_eq!(share_name(slot), name, "{slot}");
    }
    assert!(REPORT.contains("'^shared(\\d*)$'"));
}
