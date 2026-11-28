//! Reading DISM, and turning Hyper-V on through a fake DISM. Every answer here is written in the
//! shape `dism.exe /English` prints: reading the feature needs an administrator, and this was never
//! run against a real PC's DISM, so none of it is captured.

use super::*;
use std::collections::VecDeque;
use std::sync::Mutex;

fn info(state: &str) -> String {
    format!(
        "\r\nDeployment Image Servicing and Management tool\r\nVersion: 10.0.26100.1\r\n\r\nImage Version: 10.0.26100.1\r\n\r\n\
         Feature Information:\r\n\r\nFeature Name : Microsoft-Hyper-V\r\nRestart Required : Possible\r\nState : {state}\r\n\r\n\
         Custom Properties:\r\n\r\n(No custom properties found)\r\n\r\nThe operation completed successfully.\r\n"
    )
}

const UNKNOWN: &str = "\r\nDeployment Image Servicing and Management tool\r\nVersion: 10.0.26100.1\r\n\r\n\
    Image Version: 10.0.26100.1\r\n\r\n\r\nError: 0x800f080c\r\n\r\nFeature name Microsoft-Hyper-V is unknown.\r\n\
    A Windows feature name was not recognized.\r\n\r\nThe DISM log file can be found at C:\\WINDOWS\\Logs\\DISM\\dism.log\r\n";

const ENABLING: &str = "\r\nEnabling feature(s)\r\n[==                         4.0%                           ]\r\
    [=============             25.0%                          ]\r[==========================100.0%==========================]\r\n\
    The operation completed successfully.\r\nRestart Windows to complete this operation.\r\n";

/// A DISM that answers each run with the next of its answers, and keeps what it was run with.
struct Fake {
    answers: Mutex<VecDeque<Result<Ran, String>>>,
    asked: Mutex<Vec<Vec<String>>>,
}

impl Fake {
    fn new(answers: Vec<Result<(i32, String), &str>>) -> Fake {
        Fake {
            answers: Mutex::new(
                answers
                    .into_iter()
                    .map(|answer| answer.map(|(code, output)| Ran { code, output }).map_err(str::to_owned))
                    .collect(),
            ),
            asked: Mutex::default(),
        }
    }

    fn asked(&self) -> Vec<Vec<String>> {
        self.asked.lock().unwrap().clone()
    }
}

impl Dism for Fake {
    fn run(&self, args: &[String], seen: &mut dyn FnMut(&str)) -> Result<Ran, String> {
        self.asked.lock().unwrap().push(args.to_vec());
        let answer =
            self.answers.lock().unwrap().pop_front().expect("DISM was run more often than the test said");
        if let Ok(ran) = &answer {
            // As the real one does, the end of what's been printed so far, piece by piece.
            for (at, _) in ran.output.char_indices().filter(|(_, c)| *c == '%') {
                seen(&ran.output[..=at]);
            }
        }
        answer
    }

    fn log(&self) -> String {
        r"C:\WINDOWS\Logs\DISM\dism.log".to_owned()
    }
}

fn hresult(code: u32) -> i32 {
    code as i32
}

#[test]
fn the_feature_is_read_and_turned_on_with_these_exact_arguments() {
    assert_eq!(info_args(), ["/English", "/Online", "/Get-FeatureInfo", "/FeatureName:Microsoft-Hyper-V"]);
    assert_eq!(
        enable_args(),
        ["/English", "/Online", "/Enable-Feature", "/FeatureName:Microsoft-Hyper-V", "/All", "/NoRestart"]
    );
    // Nothing in them is anything a shell or DISM would read as more than one word.
    for word in info_args().iter().chain(&enable_args()) {
        assert!(word.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | ':' | '-')), "{word}");
    }
}

#[test]
fn each_state_dism_prints_is_read_and_anything_else_is_not() {
    for (said, feature) in [
        ("Enabled", Feature::Enabled),
        ("Disabled", Feature::Disabled),
        ("Enable Pending", Feature::EnablePending),
        ("Disable Pending", Feature::DisablePending),
        ("Disabled with Payload Removed", Feature::PayloadRemoved),
    ] {
        assert_eq!(feature_state(&info(said)), Some(feature), "{said}");
    }
    assert_eq!(feature_state(&info("Sideways")), None);
    assert_eq!(feature_state(UNKNOWN), None);
    assert_eq!(feature_state(""), None);
}

#[test]
fn progress_is_the_last_percentage_printed() {
    assert_eq!(progress_in("[==     4.0%     ]"), Some(4));
    assert_eq!(progress_in(ENABLING), Some(100));
    assert_eq!(progress_in("[====   45.7%"), Some(45));
    assert_eq!(progress_in("Enabling feature(s)"), None);
    assert_eq!(progress_in("[  %  ]"), None);
    assert_eq!(progress_in("[ 250.0% ]"), None);
}

#[test]
fn each_exit_code_is_said_in_words_and_an_unknown_one_with_what_dism_said() {
    assert!(exit_words(hresult(0x800F_081F), "").contains("couldn't find the files"));
    assert!(exit_words(hresult(0x800F_0906), "").contains("couldn't download"));
    assert!(exit_words(hresult(0x800F_0922), "").contains("0x800F0922"));
    assert!(exit_words(hresult(0x8007_0005), "").contains("refused"));
    assert!(exit_words(hresult(0x8007_0070), "").contains("room"));
    assert!(exit_words(hresult(0x8007_0422), "").contains("turned off"));
    assert!(exit_words(740, "").contains("administrator"));
    assert!(exit_words(87, "").contains("options"));
    assert!(exit_words(hresult(UNKNOWN_FEATURE), UNKNOWN).contains("doesn't have Hyper-V"));
    let other = "\r\nError: 0x800f0950\r\n\r\nSomething Windows says.\r\n";
    assert_eq!(
        exit_words(hresult(0x800F_0950), other),
        "DISM ended with error 0x800F0950: Something Windows says."
    );
    assert_eq!(exit_words(2, "nothing useful"), "DISM ended with error 0x00000002.");
}

#[test]
fn a_reading_is_each_case_the_app_words() {
    let reading = |service, feature: Option<Read>, hypervisor, firmware| {
        hyper_v_from(&Reading { service, feature, hypervisor, firmware })
    };
    let off = || Some(Read::Feature(Feature::Disabled));
    assert_eq!(reading(true, None, Some(true), Some(true)), HyperV::On);
    // The service is what bridging needs, whatever the feature says.
    assert_eq!(reading(true, Some(Read::Feature(Feature::DisablePending)), None, None), HyperV::On);
    assert_eq!(reading(false, off(), Some(true), Some(true)), HyperV::Off);
    assert_eq!(reading(false, Some(Read::Feature(Feature::PayloadRemoved)), None, None), HyperV::Off);
    assert_eq!(reading(false, Some(Read::NotInEdition), Some(false), Some(false)), HyperV::NotInEdition);
    assert_eq!(
        reading(false, Some(Read::Feature(Feature::EnablePending)), None, None),
        HyperV::RestartPending
    );
    assert!(
        matches!(reading(false, Some(Read::Feature(Feature::Enabled)), None, None), HyperV::Unknown { reason } if reason.contains("vmms")),
        "on with no service isn't a restart to ask for again"
    );
    assert_eq!(reading(false, off(), Some(false), Some(false)), HyperV::NoVirtualization);
    // A hypervisor running says enough, whatever the firmware's flag says under it.
    assert_eq!(reading(false, off(), Some(true), Some(false)), HyperV::Off);
    // Either one not known isn't counted against it.
    assert_eq!(reading(false, off(), None, Some(false)), HyperV::Off);
    assert_eq!(reading(false, off(), Some(false), None), HyperV::Off);
    assert_eq!(
        reading(false, Some(Read::Failed("DISM is busy.".into())), None, None),
        HyperV::Unknown { reason: "DISM is busy.".into() }
    );
    assert!(
        matches!(reading(false, None, None, None), HyperV::Unknown { reason } if reason.contains("being read"))
    );
}

#[test]
fn a_read_tells_the_feature_from_an_edition_without_it_and_a_failure() {
    let fake = Fake::new(vec![
        Ok((0, info("Disabled"))),
        Ok((hresult(UNKNOWN_FEATURE), UNKNOWN.to_owned())),
        Ok((740, "Error: 740\r\n\r\nElevated permissions are required to run DISM.\r\n".to_owned())),
        Ok((0, "The operation completed successfully.".to_owned())),
        Err("dism.exe couldn't start"),
    ]);
    assert_eq!(read(&fake), Read::Feature(Feature::Disabled));
    assert_eq!(read(&fake), Read::NotInEdition);
    assert!(matches!(read(&fake), Read::Failed(why) if why.contains("administrator")));
    assert!(matches!(read(&fake), Read::Failed(why) if why.contains("didn't say")));
    assert_eq!(read(&fake), Read::Failed("dism.exe couldn't start".into()));
    assert!(fake.asked().iter().all(|args| *args == info_args()), "only ever read");
}

#[test]
fn turning_it_on_reads_runs_and_reads_again_and_passes_the_progress_on() {
    let fake = Fake::new(vec![
        Ok((0, info("Disabled"))),
        Ok((RESTART_NEEDED, ENABLING.to_owned())),
        Ok((0, info("Enable Pending"))),
    ]);
    let mut seen = Vec::new();
    assert_eq!(enable(&fake, &mut |percent| seen.push(percent)), Enabled::Done);
    assert_eq!(seen, [4, 25, 100]);
    assert_eq!(fake.asked(), [info_args(), enable_args(), info_args()]);
    // A plain 0 is as good as 3010, and a payload that was removed is turned on as well.
    let fake = Fake::new(vec![
        Ok((0, info("Disabled with Payload Removed"))),
        Ok((0, ENABLING.to_owned())),
        Ok((0, info("Enabled"))),
    ]);
    assert_eq!(enable(&fake, &mut |_| {}), Enabled::Done);
}

#[test]
fn turning_it_on_twice_runs_nothing_the_second_time() {
    for state in ["Enabled", "Enable Pending"] {
        let fake = Fake::new(vec![Ok((0, info(state)))]);
        assert_eq!(enable(&fake, &mut |_| panic!("no progress")), Enabled::Already, "{state}");
        assert_eq!(fake.asked(), [info_args()], "only read");
    }
}

#[test]
fn an_edition_without_it_runs_nothing() {
    let fake = Fake::new(vec![Ok((hresult(UNKNOWN_FEATURE), UNKNOWN.to_owned()))]);
    assert_eq!(enable(&fake, &mut |_| {}), Enabled::NotInEdition);
    assert_eq!(fake.asked(), [info_args()]);
}

#[test]
fn every_way_it_fails_says_why_with_the_log() {
    let failed = |answers: Vec<Result<(i32, String), &str>>| match enable(&Fake::new(answers), &mut |_| {}) {
        Enabled::Failed { reason, log } => {
            assert_eq!(log, r"C:\WINDOWS\Logs\DISM\dism.log");
            reason
        }
        other => panic!("{other:?}"),
    };
    let off = || Ok((0, info("Disabled")));
    assert!(failed(vec![Err("dism.exe couldn't start")]).contains("couldn't be read"));
    assert!(failed(vec![off(), Err("dism.exe couldn't start")]).contains("couldn't start"));
    for (code, words) in [
        (0x800F_081F, "couldn't find the files"),
        (0x800F_0906, "couldn't download"),
        (0x800F_0922, "couldn't finish"),
        (0x8007_0005, "refused"),
        (0x8007_0070, "room"),
        (0x8007_0422, "turned off"),
        (740, "administrator"),
        (87, "options"),
        (5, "0x00000005"),
    ] {
        let reason = failed(vec![off(), Ok((hresult(code), format!("Error: {code}\r\n")))]);
        assert!(reason.contains(words), "{code:#x}: {reason}");
    }
    // Worked, by its code, but the feature says otherwise after.
    assert!(failed(vec![off(), Ok((0, ENABLING.to_owned())), Ok((0, info("Disabled")))]).contains("after"));
    assert!(
        failed(vec![off(), Ok((RESTART_NEEDED, String::new())), Err("gone")])
            .contains("couldn't be read after")
    );
}

#[test]
fn the_log_is_where_dism_says_it_is() {
    assert_eq!(log_in(UNKNOWN).as_deref(), Some(r"C:\WINDOWS\Logs\DISM\dism.log"));
    assert_eq!(log_in(ENABLING), None);
    let elsewhere =
        "Error: 5\r\n\r\nAccess is denied.\r\n\r\nThe DISM log file can be found at D:\\Logs\\dism.log\r\n";
    let fake = Fake::new(vec![Ok((0, info("Disabled"))), Ok((5, elsewhere.to_owned()))]);
    assert!(matches!(enable(&fake, &mut |_| {}), Enabled::Failed { log, .. } if log == r"D:\Logs\dism.log"));
}

#[test]
fn a_read_is_kept_begun_once_when_due_and_never_while_it_is_turned_on() {
    let start = Instant::now();
    let mut kept = Kept::default();
    assert_eq!(kept.take(start), (None, true), "the first is begun");
    assert_eq!(kept.take(start), (None, false), "and not again while it goes on");
    kept.put(start, Read::Feature(Feature::Disabled));
    let off = Some(Read::Feature(Feature::Disabled));
    assert_eq!(kept.take(start + READ_KEPT / 2), (off.clone(), false));
    assert_eq!(kept.take(start + READ_KEPT), (off.clone(), true), "due again, with the old one meanwhile");
    kept.put(start + READ_KEPT, Read::Feature(Feature::EnablePending));
    kept.turning = true;
    let later = start + READ_KEPT * 3;
    assert_eq!(kept.take(later), (Some(Read::Feature(Feature::EnablePending)), false), "DISM is busy");
    kept.turning = false;
    assert!(kept.take(later).1);
}
