use super::*;
use crate::trust_list;

const VIEWER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const ONE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TWO: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn merged(current: Option<&str>) -> String {
    merge_trust(VIEWER, current).unwrap()
}

#[test]
fn with_nothing_there_it_is_the_list_the_installer_has_always_been_given() {
    assert_eq!(merged(None), trust_list(VIEWER));
    assert_eq!(merged(Some("")), trust_list(VIEWER));
    assert_eq!(merged(Some("  \n\r\n\t\n")), trust_list(VIEWER));
    assert_eq!(VIEWER_NAME, "Guesthouse");
}

#[test]
fn a_device_paired_since_survives_after_the_viewer() {
    let current = format!("{VIEWER} Guesthouse\n{ONE} Kitchen tablet\n{TWO} Pat's phone\n");
    assert_eq!(
        merged(Some(&current)),
        format!("{VIEWER} Guesthouse\n{ONE} Kitchen tablet\n{TWO} Pat's phone\n")
    );
}

#[test]
fn a_list_without_the_viewers_line_gets_it_first() {
    let current = format!("{ONE} Kitchen tablet\n");
    assert_eq!(merged(Some(&current)), format!("{VIEWER} Guesthouse\n{ONE} Kitchen tablet\n"));
}

#[test]
fn the_viewer_is_on_the_list_once_under_its_own_name() {
    let current = format!("{ONE} A\n{VIEWER} Old name\n{} Shouting\n{ONE} A again\n", VIEWER.to_uppercase());
    assert_eq!(merged(Some(&current)), format!("{VIEWER} Guesthouse\n{ONE} A\n"));
}

#[test]
fn a_device_twice_is_kept_once_with_its_first_name() {
    let current = format!("{ONE} First\n{TWO} Second\n{} Third\n{ONE}\n", ONE.to_uppercase());
    assert_eq!(merged(Some(&current)), format!("{VIEWER} Guesthouse\n{ONE} First\n{TWO} Second\n"));
}

#[test]
fn odd_whitespace_is_put_right() {
    let current = format!("  {ONE}\t Kitchen   tablet \r\n\r\n\t{TWO}\r\n{ONE}");
    assert_eq!(merged(Some(&current)), format!("{VIEWER} Guesthouse\n{ONE} Kitchen   tablet\n{TWO}\n"));
}

#[test]
fn a_line_the_installer_would_stop_at_is_left_out() {
    let current =
        format!("# a comment\n{ONE}Kitchen\nnot a fingerprint\n{}\n{TWO} Good\n{}x Bad\n", &ONE[..63], ONE);
    assert_eq!(merged(Some(&current)), format!("{VIEWER} Guesthouse\n{TWO} Good\n"));
}

#[test]
fn fingerprints_are_written_in_lower_case() {
    let current = format!("{} Loud\n", TWO.to_uppercase());
    assert_eq!(merged(Some(&current)), format!("{VIEWER} Guesthouse\n{TWO} Loud\n"));
    assert_eq!(merge_trust(&VIEWER.to_uppercase(), None).unwrap(), trust_list(VIEWER));
}

#[test]
fn a_name_is_kept_as_it_is() {
    let current = format!("{ONE} Pat\u{2019}s \u{1f4f1} (work)\n");
    assert!(merged(Some(&current)).contains("Pat\u{2019}s \u{1f4f1} (work)\n"));
}

#[test]
fn a_viewer_that_is_not_a_fingerprint_is_refused() {
    for viewer in ["", "abc", &ONE[..63], &format!("{ONE}0"), "zz"] {
        assert_eq!(merge_trust(viewer, None).unwrap_err(), "That is not a device's fingerprint.");
    }
}

#[test]
fn the_devices_on_a_list_are_read_in_order() {
    let list = format!("{ONE} A\n{TWO}\njunk\n");
    assert_eq!(devices(&list), [(ONE.to_string(), "A".to_string()), (TWO.to_string(), String::new())]);
    assert!(devices("").is_empty());
}

#[test]
fn merging_again_changes_nothing() {
    let current = format!("{ONE} A\n{TWO} B\n");
    let once = merged(Some(&current));
    assert_eq!(merged(Some(&once)), once);
}
