use super::*;
use guesthouse_control::{Card, NetworkMode, System};

const NOW: u64 = 1_795_000_000;
const SHA: &str = "abababababababababababababababababababababababababababababababab";

fn text(version: &str) -> Fetched {
    Fetched::Text(release_file(version))
}

/// An app that has looked, and found `version`.
fn found(version: &str, download_by_itself: bool) -> (Updates, Vec<Effect>) {
    let mut updates = Updates::new(true, "0.13.0").with_stand_in_key();
    updates.download_by_itself = download_by_itself;
    let [Effect::Fetch { attempt }] = updates.step(Input::Look, NOW)[..] else { panic!() };
    let effects = updates.step(Input::Looked { attempt, fetched: text(version) }, NOW);
    (updates, effects)
}

/// An app downloading 0.14.0, and the download's number.
fn downloading() -> (Updates, u64) {
    let (mut updates, _) = found("0.14.0", false);
    let effects = updates.step(Input::Download, NOW);
    let [Effect::Download { attempt, ref latest }] = effects[..] else { panic!("{effects:?}") };
    assert_eq!(latest.version, "0.14.0");
    (updates, attempt)
}

/// An app checking its download, and the check's number.
fn verifying() -> (Updates, u64) {
    let (mut updates, attempt) = downloading();
    let effects = updates.step(Input::Downloaded { attempt, result: Ok(()) }, NOW);
    assert_eq!(effects, [Effect::Hash { attempt }]);
    (updates, attempt)
}

/// An app with 0.14.0 ready to install.
fn ready() -> Updates {
    let (mut updates, attempt) = verifying();
    assert!(updates.step(Input::Hashed { attempt, result: Ok(SHA.to_owned()) }, NOW).is_empty());
    assert!(updates.ready());
    updates
}

/// An app whose installer has started, and the install's number.
fn installing() -> (Updates, u64) {
    let mut updates = ready();
    let effects = updates.step(Input::Install, NOW);
    let [Effect::Launch { attempt, ref latest }] = effects[..] else { panic!("{effects:?}") };
    assert_eq!(latest.version, "0.14.0");
    assert!(matches!(updates.phase, Phase::Installing { .. }));
    (updates, attempt)
}

fn version_of(updates: &Updates) -> Option<&str> {
    updates.newer().map(|latest| latest.version.as_str())
}

// ---- Looking ----

#[test]
fn a_build_of_the_repo_never_looks_downloads_or_installs() {
    let mut updates = Updates::new(false, "0.13.0").with_stand_in_key();
    updates.download_by_itself = true;
    for input in [Input::Look, Input::Download, Input::Install, Input::Cancel] {
        assert!(updates.step(input, NOW).is_empty());
    }
    assert_eq!(updates.phase, Phase::NotInstalled);
    assert!(!updates.can_look() && !updates.dot());
}

#[test]
fn a_look_says_newer_newest_or_cant_tell() {
    let (updates, effects) = found("0.14.0", false);
    assert!(effects.is_empty(), "told, and nothing downloaded: the settings say tell me");
    assert!(matches!(&updates.phase, Phase::Newer { latest, looked: NOW } if latest.version == "0.14.0"));
    assert!(updates.dot());

    let (updates, _) = found("0.13.0", true);
    assert_eq!(updates.phase, Phase::Newest { looked: NOW });
    assert!(!updates.dot());
    let (updates, _) = found("0.9.0", true);
    assert_eq!(updates.phase, Phase::Newest { looked: NOW }, "an older file isn't newer");

    for (fetched, why) in [
        (Fetched::Unreached, CantTell::Unreached),
        (Fetched::Text("<html>".to_owned()), CantTell::Garbled),
        // An installer anywhere but the file drop is never offered.
        (Fetched::Text(release_file("0.14.0").replace("files.ruxn.com", "example.com")), CantTell::Garbled),
    ] {
        let mut updates = Updates::new(true, "0.13.0").with_stand_in_key();
        let [Effect::Fetch { attempt }] = updates.step(Input::Look, NOW)[..] else { panic!() };
        assert!(updates.step(Input::Looked { attempt, fetched }, NOW).is_empty());
        assert_eq!(updates.phase, Phase::CantTell { why, looked: NOW });
        assert!(updates.can_look(), "and it can look again");
    }
}

#[test]
fn a_file_the_apps_keys_did_not_sign_is_never_offered() {
    let look = |mut updates: Updates, text: String| {
        let [Effect::Fetch { attempt }] = updates.step(Input::Look, NOW)[..] else { panic!() };
        assert!(updates.step(Input::Looked { attempt, fetched: Fetched::Text(text) }, NOW).is_empty());
        updates.phase
    };
    let garbled = Phase::CantTell { why: CantTell::Garbled, looked: NOW };
    // Signed, but not by a key this app takes: the release keys, here, not the stand-in.
    assert_eq!(look(Updates::new(true, "0.13.0"), release_file("0.14.0")), garbled);
    // Signed by the key it takes, with the installer's SHA-256 changed after.
    let changed = release_file("0.14.0").replace(&"ab".repeat(32), &"cd".repeat(32));
    assert_eq!(look(Updates::new(true, "0.13.0").with_stand_in_key(), changed), garbled);
    // With no signature at all.
    let mut unsigned: serde_json::Value = serde_json::from_str(&release_file("0.14.0")).unwrap();
    unsigned.as_object_mut().unwrap().remove("signature");
    assert_eq!(look(Updates::new(true, "0.13.0").with_stand_in_key(), unsigned.to_string()), garbled);
    // And the same file as it was signed is.
    assert!(matches!(
        look(Updates::new(true, "0.13.0").with_stand_in_key(), release_file("0.14.0")),
        Phase::Newer { .. }
    ));
}

#[test]
fn a_second_look_while_one_goes_on_starts_nothing_and_an_old_answer_changes_nothing() {
    let mut updates = Updates::new(true, "0.13.0").with_stand_in_key();
    let [Effect::Fetch { attempt }] = updates.step(Input::Look, NOW)[..] else { panic!() };
    assert!(updates.step(Input::Look, NOW).is_empty());
    updates.step(Input::Looked { attempt: attempt + 7, fetched: text("0.14.0") }, NOW);
    assert_eq!(updates.phase, Phase::Looking, "an answer to no fetch of this one's");
    updates.step(Input::Looked { attempt, fetched: text("0.13.0") }, NOW);
    assert_eq!(updates.phase, Phase::Newest { looked: NOW });
    updates.step(Input::Looked { attempt, fetched: text("0.14.0") }, NOW);
    assert_eq!(updates.phase, Phase::Newest { looked: NOW }, "the same answer again, late");
}

#[test]
fn put_aside_hides_the_dot_until_the_app_next_starts() {
    let (mut updates, _) = found("0.14.0", false);
    updates.step(Input::PutAside, NOW);
    assert!(!updates.dot());
    assert_eq!(version_of(&updates), Some("0.14.0"), "the page still says it's out");
    let [Effect::Fetch { attempt }] = updates.step(Input::Look, NOW)[..] else { panic!() };
    updates.step(Input::Looked { attempt, fetched: text("0.14.0") }, NOW);
    assert!(!updates.dot());
}

// ---- Downloading ----

#[test]
fn downloading_by_itself_starts_as_soon_as_a_look_finds_one_and_never_installs() {
    let (mut updates, effects) = found("0.14.0", true);
    let [Effect::Download { attempt, ref latest }] = effects[..] else { panic!("{effects:?}") };
    assert_eq!(latest.version, "0.14.0");
    assert!(matches!(updates.phase, Phase::Downloading { got: 0, total: None, .. }));
    updates.step(Input::Downloaded { attempt, result: Ok(()) }, NOW);
    let effects = updates.step(Input::Hashed { attempt, result: Ok(SHA.to_owned()) }, NOW);
    assert!(effects.is_empty(), "ready, and nothing launched: only Install installs");
    assert!(updates.ready());
}

#[test]
fn a_download_shows_how_far_it_has_got() {
    let (mut updates, attempt) = downloading();
    updates.step(Input::Progress { attempt, got: 10, total: None }, NOW);
    assert!(matches!(updates.phase, Phase::Downloading { got: 10, total: None, .. }));
    updates.step(Input::Progress { attempt, got: 40, total: Some(100) }, NOW);
    assert!(matches!(updates.phase, Phase::Downloading { got: 40, total: Some(100), .. }));
    // An older word of how far it got, or another download's, doesn't go back.
    updates.step(Input::Progress { attempt, got: 30, total: None }, NOW);
    updates.step(Input::Progress { attempt: attempt + 1, got: 99, total: Some(100) }, NOW);
    assert!(matches!(updates.phase, Phase::Downloading { got: 40, total: Some(100), .. }));
}

#[test]
fn a_download_that_stops_fails_with_why_and_its_file_goes() {
    for failed in [
        DownloadError::Unreached,
        DownloadError::Refused(404),
        DownloadError::TooLarge,
        DownloadError::TooLong,
    ] {
        let (mut updates, attempt) = downloading();
        let effects = updates.step(Input::Downloaded { attempt, result: Err(failed.clone()) }, NOW);
        assert_eq!(effects, [Effect::Discard]);
        let Phase::Failed { failure, .. } = &updates.phase else { panic!("{:?}", updates.phase) };
        assert_eq!(failure, &Failure::Download(failed));
        assert_eq!(failure.heading("0.14.0"), "The update wasn't downloaded");
        assert!(failure.words().ends_with("Nothing was installed."));
        assert!(updates.dot(), "the update is still out");
    }
}

#[test]
fn a_failed_download_can_be_tried_again_as_a_new_one() {
    let (mut updates, first) = downloading();
    updates.step(Input::Downloaded { attempt: first, result: Err(DownloadError::Unreached) }, NOW);
    let effects = updates.step(Input::Download, NOW);
    let [Effect::Download { attempt, .. }] = effects[..] else { panic!("{effects:?}") };
    assert!(attempt > first);
    // What the first one says late changes nothing.
    updates.step(Input::Downloaded { attempt: first, result: Ok(()) }, NOW);
    assert!(matches!(updates.phase, Phase::Downloading { .. }));
    updates.step(Input::Downloaded { attempt, result: Ok(()) }, NOW);
    assert!(matches!(updates.phase, Phase::Verifying { .. }));
}

#[test]
fn after_a_failure_it_can_look_again_too() {
    let (mut updates, attempt) = downloading();
    updates.step(Input::Downloaded { attempt, result: Err(DownloadError::TooLong) }, NOW);
    assert!(updates.can_look());
    let [Effect::Fetch { attempt }] = updates.step(Input::Look, NOW)[..] else { panic!() };
    updates.step(Input::Looked { attempt, fetched: text("0.15.0") }, NOW);
    assert_eq!(version_of(&updates), Some("0.15.0"));
}

#[test]
fn a_second_look_while_downloading_starts_nothing() {
    let (mut updates, attempt) = downloading();
    assert!(!updates.can_look());
    assert!(updates.step(Input::Look, NOW).is_empty());
    assert!(updates.step(Input::Download, NOW).is_empty(), "nor a second download");
    // A look's answer that comes now is to no look.
    updates.step(Input::Looked { attempt, fetched: text("0.15.0") }, NOW);
    assert!(matches!(&updates.phase, Phase::Downloading { latest, .. } if latest.version == "0.14.0"));
}

// ---- Checking ----

#[test]
fn a_download_that_matches_its_sha_256_is_ready_in_any_case() {
    let (mut updates, attempt) = verifying();
    assert!(!updates.can_look());
    updates.step(Input::Hashed { attempt, result: Ok(SHA.to_uppercase()) }, NOW);
    assert_eq!(updates.phase, Phase::Ready { latest: updates.newer().unwrap().clone(), note: None });
}

#[test]
fn a_download_that_doesnt_match_is_deleted_and_says_so() {
    let (mut updates, attempt) = verifying();
    let effects = updates.step(Input::Hashed { attempt, result: Ok("00".repeat(32)) }, NOW);
    assert_eq!(effects, [Effect::Discard]);
    let Phase::Failed { failure, .. } = &updates.phase else { panic!() };
    assert_eq!(failure, &Failure::Mismatch);
    assert_eq!(failure.words(), "It didn't match its SHA-256, so it was deleted. Nothing was installed.");
    assert!(!updates.ready());
    // The way out: download it again.
    assert!(matches!(updates.step(Input::Download, NOW)[..], [Effect::Download { .. }]));
}

#[test]
fn a_download_that_cant_be_read_is_deleted_and_says_why() {
    let (mut updates, attempt) = verifying();
    let effects = updates.step(Input::Hashed { attempt, result: Err("it's in use".to_owned()) }, NOW);
    assert_eq!(effects, [Effect::Discard]);
    assert!(
        matches!(&updates.phase, Phase::Failed { failure: Failure::Unreadable(why), .. } if why == "it's in use")
    );
}

#[test]
fn only_a_check_of_this_download_makes_it_ready() {
    let (mut updates, attempt) = verifying();
    updates.step(Input::Hashed { attempt: attempt - 1, result: Ok(SHA.to_owned()) }, NOW);
    updates.step(Input::Hashed { attempt: attempt + 1, result: Ok(SHA.to_owned()) }, NOW);
    assert!(matches!(updates.phase, Phase::Verifying { .. }));
    // Nor is anything ready that wasn't checked: a download's end, however it ended, isn't.
    let (mut updates, attempt) = downloading();
    updates.step(Input::Hashed { attempt, result: Ok(SHA.to_owned()) }, NOW);
    assert!(!updates.ready());
}

#[test]
fn an_app_started_again_trusts_nothing_it_had() {
    // Closed mid-download, or with one ready: the next start knows nothing of it, so whatever is
    // left on disk is downloaded again, and checked again, before it's ready.
    let mut updates = Updates::new(true, "0.13.0").with_stand_in_key();
    assert_eq!(updates.phase, Phase::NotLooked);
    for input in [
        Input::Downloaded { attempt: 1, result: Ok(()) },
        Input::Hashed { attempt: 1, result: Ok(SHA.to_owned()) },
        Input::Install,
    ] {
        assert!(updates.step(input, NOW).is_empty());
    }
    assert_eq!(updates.phase, Phase::NotLooked);
}

// ---- Cancel, at each point ----

#[test]
fn cancel_while_downloading_stops_it_deletes_it_and_goes_back_to_out() {
    let (mut updates, attempt) = downloading();
    let effects = updates.step(Input::Cancel, NOW + 5);
    assert_eq!(effects, [Effect::StopDownload, Effect::Discard]);
    assert!(
        matches!(&updates.phase, Phase::Newer { latest, looked } if latest.version == "0.14.0" && *looked == NOW + 5)
    );
    // The download ends after, as it was stopped, and that changes nothing.
    updates.step(Input::Progress { attempt, got: 5, total: None }, NOW);
    updates.step(Input::Downloaded { attempt, result: Ok(()) }, NOW);
    assert!(matches!(updates.phase, Phase::Newer { .. }));
    // And it can be downloaded again.
    assert!(matches!(updates.step(Input::Download, NOW)[..], [Effect::Download { .. }]));
}

#[test]
fn cancel_while_checking_deletes_it_and_a_late_check_changes_nothing() {
    let (mut updates, attempt) = verifying();
    assert_eq!(updates.step(Input::Cancel, NOW), [Effect::Discard]);
    assert!(matches!(updates.phase, Phase::Newer { .. }));
    updates.step(Input::Hashed { attempt, result: Ok(SHA.to_owned()) }, NOW);
    assert!(!updates.ready(), "a check that ends after Cancel doesn't make it ready");
}

#[test]
fn cancel_does_nothing_where_there_is_nothing_to_stop() {
    let (mut updates, _) = found("0.14.0", false);
    let before = updates.clone();
    assert!(updates.step(Input::Cancel, NOW).is_empty());
    assert_eq!(updates, before);
    let mut updates = ready();
    assert!(updates.step(Input::Cancel, NOW).is_empty(), "Later is what puts a ready one aside");
    assert!(updates.ready());
    // The installer has its own Cancel once it has started.
    let (mut updates, _) = installing();
    assert!(updates.step(Input::Cancel, NOW).is_empty());
    assert!(matches!(updates.phase, Phase::Installing { .. }));
}

// ---- Ready, and installing ----

#[test]
fn a_ready_one_isnt_looked_over_and_later_keeps_it_ready() {
    let mut updates = ready();
    assert!(!updates.can_look());
    assert!(updates.step(Input::Look, NOW).is_empty());
    updates.step(Input::PutAside, NOW);
    assert!(updates.ready() && !updates.dot());
}

#[test]
fn install_starts_the_installer_only_from_ready() {
    let (mut updates, _) = found("0.14.0", false);
    assert!(updates.step(Input::Install, NOW).is_empty());
    let (mut updates, _) = downloading();
    assert!(updates.step(Input::Install, NOW).is_empty());
    let (mut updates, _) = verifying();
    assert!(updates.step(Input::Install, NOW).is_empty());
    let (mut updates, attempt) = installing();
    assert!(updates.step(Input::Install, NOW).is_empty(), "one installer at a time");
    assert!(!updates.can_look());
    updates.step(Input::Launched { attempt, result: Ok(()) }, NOW);
    assert!(matches!(updates.phase, Phase::Installing { .. }));
}

#[test]
fn an_installer_that_doesnt_start_or_ends_with_nothing_done_leaves_it_ready_with_why() {
    let (mut updates, attempt) = installing();
    updates.step(Input::Launched { attempt, result: Err("access is denied".to_owned()) }, NOW);
    let Phase::Ready { note: Some(failure), .. } = &updates.phase else { panic!("{:?}", updates.phase) };
    assert_eq!(failure.words(), "The installer didn't start: access is denied.");
    assert_eq!(failure.heading("0.14.0"), "Guesthouse 0.14.0 wasn't installed");

    for (code, failure) in
        [(INSTALLER_CANCELLED, Failure::Declined), (1, Failure::Installer), (-1, Failure::Installer)]
    {
        let (mut updates, attempt) = installing();
        updates.step(Input::InstallerEnded { attempt, code }, NOW);
        assert_eq!(
            updates.phase,
            Phase::Ready { latest: updates.newer().unwrap().clone(), note: Some(failure) }
        );
        // The way out: install again, which starts it afresh.
        let effects = updates.step(Input::Install, NOW);
        assert!(matches!(effects[..], [Effect::Launch { attempt: again, .. }] if again > attempt));
    }
}

#[test]
fn an_installer_that_installed_while_the_app_was_open_says_so() {
    let (mut updates, attempt) = installing();
    updates.step(Input::InstallerEnded { attempt: attempt + 1, code: INSTALLER_DONE }, NOW);
    assert!(matches!(updates.phase, Phase::Installing { .. }), "another install's end");
    updates.step(Input::InstallerEnded { attempt, code: INSTALLER_DONE }, NOW);
    assert!(matches!(&updates.phase, Phase::Installed { latest } if latest.version == "0.14.0"));
    assert!(!updates.dot() && !updates.can_look());
}

#[test]
fn the_app_closes_when_the_service_goes_only_while_installing() {
    let (mut updates, _) = installing();
    assert_eq!(updates.step(Input::ServiceGone, NOW), [Effect::Close]);
    for mut updates in [ready(), found("0.14.0", false).0, downloading().0] {
        assert!(updates.step(Input::ServiceGone, NOW).is_empty());
    }
}

// ---- What an install stops, and what holds it back ----

fn machine(name: &str, state: State) -> Machine {
    Machine {
        name: name.to_owned(),
        system: System::Windows,
        state,
        set_up: true,
        memory_mb: 8192,
        processors: 4,
        card: None,
        chosen_card: Card::Default,
        nvapi_wrapper: false,
        tpm: true,
        network: true,
        shares: Vec::new(),
        network_mode: NetworkMode::Shared,
        forwards: Vec::new(),
        nested_virtualization: false,
        ssh_port: None,
        size_gb: Some(128),
        folder: std::path::PathBuf::new(),
        problem: None,
        snapshots: Vec::new(),
        working: None,
        address: None,
        remote_view: false,
        remote_port: None,
        console_pipe: None,
    }
}

#[test]
fn an_install_stops_every_machine_thats_on() {
    let machines = [
        machine("gaming", State::Running),
        machine("off", State::Off),
        machine("media", State::Starting),
        machine("failed", State::Failed),
        machine("going", State::Stopping),
    ];
    let names: Vec<&str> =
        stopped_by_install(&machines).iter().map(|machine| machine.name.as_str()).collect();
    assert_eq!(names, ["gaming", "media", "going"]);
    assert!(stopped_by_install(&machines[1..2]).is_empty());
}

#[test]
fn an_install_waits_for_a_machine_being_made_or_worked_on() {
    assert_eq!(install_blocked(&[machine("a", State::Running), machine("b", State::Off)], false), None);
    let making = install_blocked(&[machine("new", State::Installing)], false).unwrap();
    assert!(making.starts_with("new is being made."), "{making}");
    let mut moving = machine("win", State::Off);
    moving.working = Some("Moving its disks.".to_owned());
    let words = install_blocked(&[moving], false).unwrap();
    assert!(words.starts_with("win: Moving its disks. Install once"), "{words}");
    assert!(install_blocked(&[], true).unwrap().starts_with("A new machine is being made."));
}

#[test]
fn the_fake_goes_as_a_real_one_would_when_all_is_well() {
    let fake = Fake::finding(text("0.14.0"));
    assert_eq!(fake.fetch(), text("0.14.0"));
    let mut seen = Vec::new();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let latest = updates::parse(&release_file("0.14.0"), &[&stand_in_key().1]).unwrap();
    assert_eq!(fake.download(&latest, 1, &mut |got, total| seen.push((got, total)), &stop), Ok(()));
    assert_eq!(seen.len(), 2);
    assert_eq!(fake.hash(1), Ok(SHA.to_owned()));
    assert_eq!(fake.launch(1).unwrap().wait(), INSTALLER_CANCELLED, "a fake installs nothing");
}
