//! A reading and its verdict in plain words, for `guesthouse-vm idle`. Short lines, one for each signal.

use super::{Busy, GuestKind, IdleReading, Input, Limits, Verdict, judge};

#[cfg(test)]
const MINUTE: u64 = 60;
#[cfg(test)]
const HOUR: u64 = 60 * MINUTE;
const PERCENT: f64 = 100.0;

/// "45 seconds", "12 minutes", "3 hours 5 minutes".
use guesthouse_basics::words::span;

fn percent(cores: f64) -> String {
    format!("{:.0}%", cores * PERCENT)
}

fn busy_text(busy: &Busy) -> String {
    match busy {
        Busy::Input { seconds } => format!("someone used the keyboard or mouse {} ago", span(*seconds)),
        Busy::RemoteSessions(count) => format!("{count} remote desktop session(s) connected"),
        Busy::PowerRequest(request) => {
            format!("a program asks Windows to stay awake ({}): {}", request.kind.name(), request.who)
        }
        Busy::VcpuTime { cores } => format!("the guest is using {} of a core", percent(*cores)),
    }
}

/// The lines to print. `guest` is how the machine is called, such as "an Ubuntu guest".
pub fn describe(reading: &IdleReading, limits: &Limits, guest: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = |label: &str, text: String| lines.push(format!("{label:<17}{text}"));
    let not_read = |why: &dyn std::fmt::Display| format!("not read ({why})");
    if reading.kind == GuestKind::Other {
        let why = format!("cannot be read for {guest}");
        line("Last input", why.clone());
        line("Remote sessions", why.clone());
        line("Power requests", why);
    } else {
        line(
            "Last input",
            match &reading.input {
                Ok(Input::Seconds(seconds)) => format!("{} ago", span(*seconds)),
                Ok(Input::NobodyLoggedIn) => "nobody is logged in".to_owned(),
                Err(why) => not_read(why),
            },
        );
        line(
            "Remote sessions",
            match &reading.remote_sessions {
                Ok(0) => "none connected".to_owned(),
                Ok(count) => format!("{count} connected"),
                Err(why) => not_read(why),
            },
        );
        line(
            "Power requests",
            match &reading.power_requests {
                Ok(requests) if requests.is_empty() => "none".to_owned(),
                Ok(requests) => requests
                    .iter()
                    .map(|request| format!("{}: {}", request.kind.name(), request.who))
                    .collect::<Vec<_>>()
                    .join("; "),
                Err(why) => not_read(why),
            },
        );
    }
    line(
        "CPU time",
        match &reading.vcpu_cores {
            Ok(cores) => {
                format!("{} of one core, over {:.0} seconds", percent(*cores), reading.vcpu_window_seconds)
            }
            Err(why) => not_read(why),
        },
    );
    lines.push(String::new());
    match judge(reading, limits) {
        Verdict::Idle => lines.push("Would count as idle.".to_owned()),
        Verdict::Busy(why) => {
            lines.push("Not idle.".to_owned());
            lines.extend(why.iter().map(|busy| format!("Busy: {}.", busy_text(busy))));
        }
        Verdict::CannotTell(open) => {
            lines.push("Cannot tell if it is idle.".to_owned());
            lines.extend(open);
        }
    }
    lines
}

/// What the numbers were judged by.
pub fn limits_text(limits: &Limits) -> String {
    format!(
        "Input counts as quiet after {}. More than {} of one core counts as busy. Nothing was changed.",
        span(limits.input_quiet_seconds),
        percent(limits.vcpu_cores)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PowerKind, PowerRequest, Unread};

    fn quiet() -> IdleReading {
        IdleReading {
            kind: GuestKind::Windows,
            input: Ok(Input::Seconds(2 * HOUR + 5 * MINUTE)),
            remote_sessions: Ok(0),
            power_requests: Ok(Vec::new()),
            vcpu_cores: Ok(0.012),
            vcpu_window_seconds: 5.0,
        }
    }

    #[test]
    fn spans_are_said_in_the_largest_unit() {
        let said: Vec<String> = [1, 45, 60, 720, 3600, 7500, 7200].map(span).into();
        assert_eq!(
            said,
            ["1 second", "45 seconds", "1 minute", "12 minutes", "1 hour", "2 hours 5 minutes", "2 hours"]
        );
    }

    /// Each unit's edges, as today's wording says them: seconds are dropped once there are minutes, and
    /// a part of a unit is dropped, not rounded.
    #[test]
    fn spans_at_the_edges_of_each_unit() {
        for (seconds, said) in [
            (0, "0 seconds"),
            (1, "1 second"),
            (2, "2 seconds"),
            (59, "59 seconds"),
            (60, "1 minute"),
            (61, "1 minute"),
            (119, "1 minute"),
            (120, "2 minutes"),
            (3599, "59 minutes"),
            (3600, "1 hour"),
            (3659, "1 hour"),
            (3660, "1 hour 1 minute"),
            (7199, "1 hour 59 minutes"),
            (86_400, "24 hours"),
            (90_061, "25 hours 1 minute"),
            (u64::MAX, "5124095576030431 hours"),
        ] {
            assert_eq!(span(seconds), said, "{seconds} s");
        }
    }

    #[test]
    fn a_quiet_windows_guest_says_each_signal_and_that_it_would_count_as_idle() {
        let text = describe(&quiet(), &Limits::default(), "a Windows guest").join("\n");
        assert_eq!(
            text,
            "Last input       2 hours 5 minutes ago\nRemote sessions  none connected\nPower requests   none\n\
             CPU time         1% of one core, over 5 seconds\n\nWould count as idle."
        );
    }

    #[test]
    fn what_is_busy_and_what_was_not_read_are_both_said() {
        let request = PowerRequest { kind: PowerKind::System, who: "[PROCESS] vlc.exe".into() };
        let reading = IdleReading {
            power_requests: Ok(vec![request]),
            remote_sessions: Err(Unread::new("no answer")),
            ..quiet()
        };
        let text = describe(&reading, &Limits::default(), "a Windows guest").join("\n");
        assert!(text.contains("Remote sessions  not read (no answer)"), "{text}");
        assert!(text.contains("Power requests   system: [PROCESS] vlc.exe"), "{text}");
        assert!(
            text.contains(
                "Not idle.\nBusy: a program asks Windows to stay awake (system): [PROCESS] vlc.exe."
            )
        );
    }

    #[test]
    fn a_reading_with_a_gap_says_it_cannot_tell() {
        let reading = IdleReading { input: Err(Unread::new("the guest's agent did not answer")), ..quiet() };
        let text = describe(&reading, &Limits::default(), "a Windows guest").join("\n");
        assert!(
            text.contains("Cannot tell if it is idle.\nLast input: the guest's agent did not answer"),
            "{text}"
        );
    }

    #[test]
    fn another_kind_of_guest_says_what_cannot_be_read() {
        let reading = IdleReading {
            kind: GuestKind::Other,
            input: Err(Unread::new("x")),
            remote_sessions: Err(Unread::new("x")),
            power_requests: Err(Unread::new("x")),
            ..quiet()
        };
        let text = describe(&reading, &Limits::default(), "an Ubuntu guest").join("\n");
        assert!(text.starts_with("Last input       cannot be read for an Ubuntu guest\n"), "{text}");
        assert!(text.contains("CPU time         1% of one core"), "{text}");
        assert!(
            text.contains("Cannot tell if it is idle.\nOnly CPU time can be read for this kind of guest."),
            "{text}"
        );
    }
}
