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

/// A guest's Windows volume in a folder of its own, with its list holding `list`, if any, and the
/// copy given at an earlier boot holding `given_before`, if any.
struct Volume(std::path::PathBuf);

impl Volume {
    fn new(list: Option<&[u8]>, given_before: Option<&str>) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("gh-trust-{}-{number}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        if let Some(list) = list {
            write(&root.join(GUEST_TRUST_FILE), list);
        }
        if let Some(before) = given_before {
            write(&root.join(format!(r"{}\trusted", crate::GUEST_FOLDER)), before.as_bytes());
        }
        Self(root)
    }

    fn given(&self) -> Given {
        given(VIEWER, &self.0).unwrap()
    }
}

/// Writes a file, with its folders on Windows, where the guest's paths are folders and not one name.
fn write(path: &std::path::Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

impl Drop for Volume {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_guest_with_no_list_is_given_the_viewer_alone_as_before() {
    let volume = Volume::new(None, None);
    assert_eq!(volume.given(), Given { viewer: trust_list(VIEWER), trusted: trust_list(VIEWER) });
}

#[test]
fn a_guest_with_only_the_viewer_is_given_what_it_always_was() {
    let volume = Volume::new(Some(trust_list(VIEWER).as_bytes()), None);
    assert_eq!(volume.given(), Given { viewer: trust_list(VIEWER), trusted: trust_list(VIEWER) });
}

#[test]
fn a_device_paired_in_the_guest_is_given_back_and_the_viewers_line_stays_alone() {
    let volume = Volume::new(Some(format!("{VIEWER} Renamed\r\n{ONE} Kitchen tablet\r\n").as_bytes()), None);
    let given = volume.given();
    assert_eq!(given.trusted, format!("{VIEWER} Guesthouse\n{ONE} Kitchen tablet\n"));
    assert_eq!(given.viewer, trust_list(VIEWER));
}

#[test]
fn a_revoked_device_is_not_brought_back_from_a_copy_given_before() {
    let before = format!("{VIEWER} Guesthouse\n{ONE} Revoked since\n{TWO} Kept\n");
    let volume = Volume::new(Some(format!("{VIEWER} Guesthouse\n{TWO} Kept\n").as_bytes()), Some(&before));
    assert_eq!(volume.given().trusted, format!("{VIEWER} Guesthouse\n{TWO} Kept\n"));
    let volume = Volume::new(None, Some(&before));
    assert_eq!(volume.given().trusted, trust_list(VIEWER));
}

#[test]
fn a_list_that_is_a_folder_reads_as_none() {
    let volume = Volume::new(None, None);
    std::fs::create_dir_all(volume.0.join(GUEST_TRUST_FILE)).unwrap();
    assert!(volume.0.join(GUEST_TRUST_FILE).is_dir());
    assert_eq!(read_list(&volume.0.join(GUEST_TRUST_FILE)), None);
    assert_eq!(volume.given().trusted, trust_list(VIEWER));
}

#[test]
fn a_byte_order_mark_and_a_name_that_is_not_utf8_lose_no_device() {
    let mut list = "\u{feff}".as_bytes().to_vec();
    list.extend_from_slice(format!("{ONE} Pat").as_bytes());
    list.extend_from_slice(&[0xff, b'\n']);
    list.extend_from_slice(format!("{TWO} B\n").as_bytes());
    let volume = Volume::new(Some(&list), None);
    assert_eq!(volume.given().trusted, format!("{VIEWER} Guesthouse\n{ONE} Pat\u{fffd}\n{TWO} B\n"));
}

#[test]
fn a_very_long_list_is_read_up_to_its_last_whole_line_within_the_limit() {
    let line = format!("{ONE} {}\n", "n".repeat(100));
    let mut list = line.repeat(MOST_LIST_BYTES as usize / line.len());
    let whole = list.len();
    // Longer than the room left, so it is cut by the limit.
    list.push_str(&format!("{TWO} {}\n", "cut off ".repeat(40)).repeat(100));
    let volume = Volume::new(Some(list.as_bytes()), None);
    let read = read_list(&volume.0.join(GUEST_TRUST_FILE)).unwrap();
    assert_eq!(read.len(), whole);
    assert!(read.ends_with('\n') && !read.contains(TWO));
    assert_eq!(volume.given().trusted, format!("{VIEWER} Guesthouse\n{line}"));
}
