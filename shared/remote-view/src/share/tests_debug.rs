//! A waiting device's name and code never show in a `{:?}`, alone or inside what holds it.

use super::*;
use crate::status::Status;

#[test]
fn debug_never_prints_a_waiting_devices_name_or_code() {
    let fingerprint = "a".repeat(64);
    let pending = Pending {
        name: "Kitchen tablet".into(),
        fingerprint: fingerprint.clone(),
        code: "123 456".into(),
        seconds: 55,
    };
    assert_eq!(
        format!("{pending:?}"),
        format!("Pending {{ fingerprint: \"{fingerprint}\", seconds: 55, .. }}")
    );
    let share = ShareInfo { pending: Some(pending.clone()), ..ShareInfo::default() };
    let status = Status { pending: Some(pending.clone()), ..Status::default() };
    for printed in [
        format!("{pending:?}"),
        format!("{pending:#?}"),
        format!("{share:?}"),
        format!("{share:#?}"),
        format!("{status:?}"),
        format!("{:?}", Some(&pending)),
    ] {
        assert!(!printed.contains("Kitchen"), "{printed}");
        assert!(!printed.contains("123") && !printed.contains("456"), "{printed}");
        assert!(printed.contains(&fingerprint), "{printed}");
    }
}
