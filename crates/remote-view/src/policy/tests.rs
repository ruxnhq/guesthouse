use super::*;

const VIEWER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER: &str = "2222222222222222222222222222222222222222222222222222222222222222";

#[test]
fn a_linux_host_holds_the_switches_and_hides_nothing() {
    let policy = Policy::linux();
    assert!(policy.hold_host_wide);
    assert!(!policy.hides(VIEWER));
    assert!(policy.device_revoke(VIEWER).is_ok());
}

#[test]
fn a_windows_host_never_holds_and_hides_the_viewer_in_any_case() {
    let policy = Policy::windows(&VIEWER.to_uppercase());
    assert!(!policy.hold_host_wide);
    assert!(policy.hides(VIEWER) && policy.hides(&VIEWER.to_uppercase()));
    assert!(!policy.hides(OTHER));
}

#[test]
fn commands_against_the_hidden_viewer_are_refused() {
    let policy = Policy::windows(VIEWER);
    assert_eq!(policy.device_revoke(VIEWER).unwrap_err(), HIDDEN_DEVICE);
    assert_eq!(policy.device_revoke(&VIEWER.to_uppercase()).unwrap_err(), HIDDEN_DEVICE);
    assert_eq!(policy.device_allow(VIEWER, "files", false).unwrap_err(), HIDDEN_DEVICE);
    assert_eq!(policy.device_revoke(OTHER).unwrap().join(" "), format!("revoke {OTHER}"));
    assert_eq!(
        policy.device_allow(OTHER, "input", false).unwrap().join(" "),
        format!("device {OTHER} view-only on")
    );
}

#[test]
fn a_fingerprint_that_is_not_one_is_still_refused_first() {
    let policy = Policy::windows(VIEWER);
    assert_eq!(policy.device_revoke("zz").unwrap_err(), "That is not a device's fingerprint.");
    assert!(policy.device_allow(OTHER, "root", true).is_err());
}
