//! The idle rules held as they are today: what the guest's idle script says, read into signals, the
//! verdict on a reading, and every sentence `guesthouse-vm idle` prints for it. The answers in
//! `idle_rules/` are written from `guest/windows/idle.ps1`'s own layout (its `sessions`, `power` and `input`
//! lines and its `s|` and `p|` prefixes) and from `powercfg /requests`' sections; none is a capture
//! from a guest.

#![cfg(target_os = "linux")]

use guesthouse_app_core::idle::{
    Busy, GuestKind, IdleReading, Input, Limits, PowerKind, PowerRequest, Unread, Verdict, describe, judge,
    limits_text, parse_power, parse_report, parse_sessions,
};

const QUIET: &str = include_str!("idle_rules/quiet.txt");
const BUSY: &str = include_str!("idle_rules/busy.txt");
const ANOTHER_LANGUAGE: &str = include_str!("idle_rules/another_language.txt");
const FAILED: &str = include_str!("idle_rules/failed.txt");
const SCRIPT: &str = include_str!("../../../guest/windows/idle.ps1");

const VLC: &str = r"[PROCESS] \Device\HarddiskVolume3\Program Files\VideoLAN\VLC\vlc.exe";
const AUDIO: &str = r"[DRIVER] Realtek High Definition Audio (HDAUDIO\FUNC_01&VEN_10EC&DEV_0256&SUBSYS";
const GAME: &str = r"[PROCESS] \Device\HarddiskVolume3\Program Files (x86)\Steam\steamapps\common\A V";

fn unread(why: &str) -> Unread {
    Unread::new(why)
}

fn request(kind: PowerKind, who: &str) -> PowerRequest {
    PowerRequest { kind, who: who.into() }
}

fn reading(answer: &str, vcpu_cores: Result<f64, Unread>) -> IdleReading {
    let report = parse_report(answer);
    IdleReading {
        kind: GuestKind::Windows,
        input: report.input,
        remote_sessions: report.remote_sessions,
        power_requests: report.power_requests,
        vcpu_cores,
        vcpu_window_seconds: 5.0,
    }
}

#[test]
fn the_script_says_its_parts_in_the_words_read_here() {
    for line in [
        "'sessions ok'",
        "'sessions failed'",
        "'power ok'",
        "'power failed'",
        "'input nobody'",
        "\"input $idleSeconds\"",
        "'input failed'",
        "'p|' + $idleText.Substring(0, [Math]::Min($idleCharsKept, $idleText.Length))",
        "lines.AppendLine(\"s|\" + one.Id + \"|\" + one.State + \"|\" + Marshal.PtrToStringUni(one.Station));",
        "$idleCharsKept = 150",
    ] {
        assert!(SCRIPT.contains(line), "{line}");
    }
}

#[test]
fn each_answer_is_read_as_today() {
    let quiet = parse_report(QUIET);
    assert_eq!(quiet.remote_sessions, Ok(0));
    assert_eq!(quiet.power_requests, Ok(Vec::new()));
    assert_eq!(quiet.input, Ok(Input::Seconds(7200)));

    let busy = parse_report(BUSY);
    assert_eq!(busy.remote_sessions, Ok(2));
    assert_eq!(
        busy.power_requests,
        Ok(vec![
            request(PowerKind::Display, VLC),
            request(PowerKind::System, AUDIO),
            request(PowerKind::System, GAME),
            request(PowerKind::Execution, "[SERVICE] BITS"),
        ])
    );
    assert_eq!(busy.input, Ok(Input::Seconds(12)));

    let other = parse_report(ANOTHER_LANGUAGE);
    assert_eq!(other.remote_sessions, Ok(0));
    assert_eq!(other.power_requests, Ok(Vec::new()));
    assert_eq!(other.input, Ok(Input::NobodyLoggedIn));

    let failed = parse_report(FAILED);
    assert_eq!(failed.remote_sessions, Err(unread("the guest could not read its sessions")));
    assert_eq!(failed.power_requests, Err(unread("the guest could not read its power requests")));
    assert_eq!(failed.input, Err(unread("nothing in the user's session said when the last input was")));
}

#[test]
fn a_windows_line_ending_reads_the_same() {
    for answer in [QUIET, BUSY, ANOTHER_LANGUAGE, FAILED] {
        assert_eq!(parse_report(&answer.replace('\n', "\r\n")), parse_report(answer));
    }
}

#[test]
fn what_is_missing_or_not_ours_is_unread_with_these_reasons() {
    let none = parse_report("  \n");
    for why in
        [&none.remote_sessions.unwrap_err(), &none.power_requests.unwrap_err(), &none.input.unwrap_err()]
    {
        assert_eq!(why, &unread("the guest's agent did not answer"));
    }
    let strange = parse_report("ERROR: the term is not recognized\n");
    assert_eq!(strange.input, Err(unread("the guest answered something the script does not say")));
    let part = parse_report("sessions ok\ns|0|4|Services\n");
    assert_eq!(part.remote_sessions, Ok(0));
    assert_eq!(part.power_requests, Err(unread("the guest did not say its power requests")));
    assert_eq!(part.input, Err(unread("the guest did not say its last input")));
    let no_lines = parse_report("sessions ok\npower ok\ninput 5\n");
    assert_eq!(no_lines.remote_sessions, Err(unread("no session was listed")));
    assert_eq!(no_lines.power_requests, Err(unread("the power requests were not laid out as expected")));
    assert_eq!(no_lines.input, Ok(Input::Seconds(5)));
    assert_eq!(
        parse_report("input -1\n").input,
        Err(unread("nothing in the user's session said when the last input was"))
    );
    assert_eq!(
        parse_sessions(&["s|1|1"]),
        Err(unread("a session was listed in a form that was not expected"))
    );
    assert_eq!(parse_sessions(&["1|0|RDP-Tcp#2"]), Ok(1), "the prefix is taken off when it is there");
    assert_eq!(
        parse_power(&["DISPLAY:", "None."]),
        Err(unread("the power requests were not laid out as expected"))
    );
}

#[test]
fn each_reading_gets_the_verdict_it_gets_today() {
    let limits = Limits::default();
    assert_eq!(limits, Limits { input_quiet_seconds: 1800, vcpu_cores: 0.05 });
    assert_eq!(Limits::new(1), Limits { input_quiet_seconds: 60, vcpu_cores: 0.05 });
    assert_eq!(Limits::new(u64::MAX).input_quiet_seconds, u64::MAX);
    assert_eq!(judge(&reading(QUIET, Ok(0.05)), &limits), Verdict::Idle, "the limit itself is quiet");
    assert_eq!(
        judge(&reading(QUIET, Ok(0.0501)), &limits),
        Verdict::Busy(vec![Busy::VcpuTime { cores: 0.0501 }])
    );
    assert_eq!(judge(&reading(ANOTHER_LANGUAGE, Ok(0.0)), &limits), Verdict::Idle);
    assert_eq!(
        judge(&reading(BUSY, Ok(0.5)), &limits),
        Verdict::Busy(vec![
            Busy::VcpuTime { cores: 0.5 },
            Busy::Input { seconds: 12 },
            Busy::RemoteSessions(2),
            Busy::PowerRequest(request(PowerKind::Display, VLC)),
            Busy::PowerRequest(request(PowerKind::System, AUDIO)),
            Busy::PowerRequest(request(PowerKind::System, GAME)),
            Busy::PowerRequest(request(PowerKind::Execution, "[SERVICE] BITS")),
        ])
    );
    assert_eq!(
        judge(&reading(FAILED, Err(unread("the guest's process was gone"))), &limits),
        Verdict::CannotTell(vec![
            "CPU time: the guest's process was gone".to_owned(),
            "Last input: nothing in the user's session said when the last input was".to_owned(),
            "Remote sessions: the guest could not read its sessions".to_owned(),
            "Power requests: the guest could not read its power requests".to_owned(),
        ])
    );
}

#[test]
fn every_sentence_for_a_quiet_reading_is_as_today() {
    let lines = describe(&reading(QUIET, Ok(0.012)), &Limits::default(), "a Windows guest");
    assert_eq!(
        lines,
        [
            "Last input       2 hours ago",
            "Remote sessions  none connected",
            "Power requests   none",
            "CPU time         1% of one core, over 5 seconds",
            "",
            "Would count as idle.",
        ]
    );
    let nobody = describe(&reading(ANOTHER_LANGUAGE, Ok(0.0)), &Limits::default(), "a Windows guest");
    assert_eq!(nobody[0], "Last input       nobody is logged in");
}

#[test]
fn every_sentence_for_a_busy_reading_is_as_today() {
    let lines = describe(&reading(BUSY, Ok(0.5)), &Limits::default(), "a Windows guest");
    let power = format!("display: {VLC}; system: {AUDIO}; system: {GAME}; execution: [SERVICE] BITS");
    assert_eq!(
        lines,
        [
            "Last input       12 seconds ago".to_owned(),
            "Remote sessions  2 connected".to_owned(),
            format!("Power requests   {power}"),
            "CPU time         50% of one core, over 5 seconds".to_owned(),
            String::new(),
            "Not idle.".to_owned(),
            "Busy: the guest is using 50% of a core.".to_owned(),
            "Busy: someone used the keyboard or mouse 12 seconds ago.".to_owned(),
            "Busy: 2 remote desktop session(s) connected.".to_owned(),
            format!("Busy: a program asks Windows to stay awake (display): {VLC}."),
            format!("Busy: a program asks Windows to stay awake (system): {AUDIO}."),
            format!("Busy: a program asks Windows to stay awake (system): {GAME}."),
            "Busy: a program asks Windows to stay awake (execution): [SERVICE] BITS.".to_owned(),
        ]
    );
}

#[test]
fn every_sentence_for_a_reading_that_could_not_be_taken_is_as_today() {
    let lines = describe(
        &reading(FAILED, Err(unread("the guest's process was gone"))),
        &Limits::default(),
        "a Windows guest",
    );
    assert_eq!(
        lines,
        [
            "Last input       not read (nothing in the user's session said when the last input was)",
            "Remote sessions  not read (the guest could not read its sessions)",
            "Power requests   not read (the guest could not read its power requests)",
            "CPU time         not read (the guest's process was gone)",
            "",
            "Cannot tell if it is idle.",
            "CPU time: the guest's process was gone",
            "Last input: nothing in the user's session said when the last input was",
            "Remote sessions: the guest could not read its sessions",
            "Power requests: the guest could not read its power requests",
        ]
    );
}

#[test]
fn every_sentence_for_a_guest_only_the_host_can_read_is_as_today() {
    let other = |cores| IdleReading { kind: GuestKind::Other, ..reading("", cores) };
    assert_eq!(
        describe(&other(Ok(0.004)), &Limits::default(), "an Ubuntu guest"),
        [
            "Last input       cannot be read for an Ubuntu guest",
            "Remote sessions  cannot be read for an Ubuntu guest",
            "Power requests   cannot be read for an Ubuntu guest",
            "CPU time         0% of one core, over 5 seconds",
            "",
            "Cannot tell if it is idle.",
            "Only CPU time can be read for this kind of guest.",
        ]
    );
    let working = describe(&other(Ok(1.5)), &Limits::default(), "a plain VM");
    assert_eq!(working[0], "Last input       cannot be read for a plain VM");
    assert_eq!(
        working[3..],
        [
            "CPU time         150% of one core, over 5 seconds",
            "",
            "Not idle.",
            "Busy: the guest is using 150% of a core."
        ]
    );
}

#[test]
fn what_the_numbers_were_judged_by_is_said_as_today() {
    assert_eq!(
        limits_text(&Limits::default()),
        "Input counts as quiet after 30 minutes. More than 5% of one core counts as busy. Nothing was changed."
    );
    assert_eq!(
        limits_text(&Limits::new(1)),
        "Input counts as quiet after 1 minute. More than 5% of one core counts as busy. Nothing was changed."
    );
    assert_eq!(
        limits_text(&Limits::new(185)),
        "Input counts as quiet after 3 hours 5 minutes. More than 5% of one core counts as busy. Nothing was changed."
    );
    assert_eq!(
        limits_text(&Limits::new(0)),
        "Input counts as quiet after 0 seconds. More than 5% of one core counts as busy. Nothing was changed."
    );
}

#[test]
fn the_power_kinds_are_named_and_ordered_as_today() {
    let named: Vec<&str> = PowerKind::IN_ORDER.iter().map(|kind| kind.name()).collect();
    assert_eq!(named, ["display", "system", "away mode", "execution"]);
    assert_eq!(unread("x").to_string(), "x");
}
