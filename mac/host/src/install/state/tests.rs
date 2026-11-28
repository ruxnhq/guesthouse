// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;

fn info() -> InstallInfo {
    InstallInfo::new("win", &Plan::for_guest(Guest::Windows), Source::Download)
}

#[test]
fn a_new_install_has_every_step_pending_and_a_guess_of_the_time() {
    let info = info();
    assert_eq!(info.state, Phase::Running);
    assert!(info.steps.iter().all(|s| s.status == StepStatus::Pending));
    assert_eq!(info.next_step(), Some(Step::Fetch));
    // The usual times, added: 600 + 20 + 1500 + 120 + 90.
    assert_eq!(info.seconds_left, Some(2330));
}

#[test]
fn a_download_says_how_far_it_has_got_and_how_long_is_left_by_its_pace() {
    let mut info = info();
    info.start_step(Step::Fetch, 0);
    // A quarter in 10 seconds: 30 seconds more.
    info.progress(Step::Fetch, 250, Some(1000), 10_000);
    let progress = info.steps[0].progress;
    assert_eq!(progress.fraction, 0.25);
    assert_eq!((progress.bytes_done, progress.bytes_total), (Some(250), Some(1000)));
    assert_eq!(progress.seconds_left, Some(30));
    // The whole is that plus the usual for the rest.
    assert_eq!(info.seconds_left, Some(30 + 20 + 1500 + 120 + 90));
}

#[test]
fn the_pace_waits_until_there_is_enough_to_go_by() {
    let mut info = info();
    info.start_step(Step::Fetch, 0);
    info.progress(Step::Fetch, 1, Some(1000), 1_000);
    // Under two percent: the usual time, scaled.
    assert_eq!(info.steps[0].progress.seconds_left, Some(599));
}

#[test]
fn finishing_every_step_ends_in_done_and_the_time_left_goes() {
    let mut info = info();
    for step in Step::ALL {
        info.start_step(step, 0);
        info.finish_step(step);
    }
    assert_eq!(info.state, Phase::Done);
    assert_eq!(info.current, None);
    assert_eq!(info.seconds_left, None);
    assert_eq!(info.sidebar_text(), "Ready");
}

#[test]
fn each_step_offers_what_the_failure_screen_says() {
    let offered = |step| {
        let mut info = info();
        info.fail(step, Some("0x800b0109".into()), "it stopped");
        let failure = info.failure.clone().unwrap();
        assert_eq!(failure.code.as_deref(), Some("0x800b0109"));
        failure.choices
    };
    assert_eq!(offered(Step::Fetch), [Choice::TryAgain, Choice::ChooseInstaller]);
    assert_eq!(offered(Step::InstallSystem), [Choice::TryAgain, Choice::SendReport]);
    assert_eq!(
        offered(Step::InstallDriver),
        [Choice::TryAgain, Choice::OpenWithoutDriver, Choice::SendReport]
    );
    assert_eq!(offered(Step::FirstStart), [Choice::TryAgain, Choice::SendReport]);
}

#[test]
fn try_again_puts_the_failed_step_back_and_leaves_the_rest() {
    let mut info = info();
    info.start_step(Step::Fetch, 0);
    info.finish_step(Step::Fetch);
    info.fail(Step::MakeDisk, None, "not enough room");
    assert_eq!(info.state, Phase::Failed);
    assert_eq!(info.seconds_left, None);
    assert!(info.retry());
    assert_eq!(info.state, Phase::Running);
    assert_eq!(info.status_of(Step::Fetch), StepStatus::Done);
    assert_eq!(info.status_of(Step::MakeDisk), StepStatus::Pending);
    assert_eq!(info.next_step(), Some(Step::MakeDisk));
    assert!(!info.retry(), "nothing failed any more");
}

#[test]
fn a_chosen_installer_replaces_the_download_only_where_it_is_offered() {
    let mut info = info();
    info.fail(Step::InstallSystem, None, "x");
    assert!(!info.choose_installer("/a.iso"));
    assert_eq!(info.source, Source::Download);
    let mut info = self::info();
    info.fail(Step::Fetch, None, "the network went");
    assert!(info.choose_installer("/Users/me/Win11.iso"));
    assert_eq!(info.source, Source::File { path: "/Users/me/Win11.iso".into() });
    assert_eq!(info.status_of(Step::Fetch), StepStatus::Pending);
}

#[test]
fn opening_without_the_driver_skips_it_and_goes_on_to_the_first_start() {
    let mut info = info();
    for step in [Step::Fetch, Step::MakeDisk, Step::InstallSystem] {
        info.start_step(step, 0);
        info.finish_step(step);
    }
    info.fail(Step::InstallDriver, Some("0x80070005".into()), "turned away");
    assert!(info.open_without_driver());
    assert_eq!(info.status_of(Step::InstallDriver), StepStatus::Skipped);
    assert_eq!(info.next_step(), Some(Step::FirstStart));
    assert_eq!(info.state, Phase::Running);
    // Not offered for another step.
    let mut other = self::info();
    other.fail(Step::FirstStart, None, "x");
    assert!(!other.open_without_driver());
}

#[test]
fn an_install_found_after_the_app_closed_goes_on_from_where_it_was() {
    let mut info = info();
    info.start_step(Step::Fetch, 0);
    info.finish_step(Step::Fetch);
    info.start_step(Step::MakeDisk, 0);
    info.finish_step(Step::MakeDisk);
    info.start_step(Step::InstallSystem, 0);
    info.resume();
    assert_eq!(info.status_of(Step::InstallSystem), StepStatus::Pending);
    assert_eq!(info.next_step(), Some(Step::InstallSystem));
    assert_eq!(info.status_of(Step::Fetch), StepStatus::Done);
    // A failure is left as it was: the person decides.
    let mut failed = self::info();
    failed.fail(Step::Fetch, None, "x");
    failed.resume();
    assert_eq!(failed.state, Phase::Failed);
}

#[test]
fn the_state_is_kept_in_the_machines_folder_and_reads_back() {
    let folder = std::env::temp_dir().join(format!("gh-install-state-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let mut info = info();
    info.start_step(Step::Fetch, 0);
    info.progress(Step::Fetch, 5, Some(10), 4_000);
    save(&folder, &info).unwrap();
    let back = load(&folder).unwrap();
    assert_eq!(back.steps[0].progress, info.steps[0].progress);
    assert_eq!(back.guest, Guest::Windows);
    assert!(load(&folder.join("nothing")).is_none());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_sidebar_says_how_far_the_whole_has_got() {
    let mut info = info();
    assert_eq!(info.sidebar_text(), "Installing, 0%");
    info.start_step(Step::Fetch, 0);
    info.finish_step(Step::Fetch);
    // One of the four shown steps is done.
    assert_eq!(info.sidebar_text(), "Installing, 25%");
    info.cancel();
    assert_eq!(info.sidebar_text(), "Install stopped");
}

#[test]
fn the_json_the_screens_read_has_the_names_the_design_uses() {
    let mut info = info();
    info.fail(Step::InstallDriver, Some("0x1".into()), "no");
    let value = serde_json::to_value(&info).unwrap();
    assert_eq!(value["state"], "failed");
    assert_eq!(value["guest"], "windows");
    assert_eq!(value["failure"]["step"], "install_driver");
    assert_eq!(value["failure"]["choices"][1], "open_without_driver");
    assert_eq!(value["steps"][0]["status"], "pending");
    assert_eq!(value["source"]["kind"], "download");
}
