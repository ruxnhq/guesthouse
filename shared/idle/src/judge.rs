//! Whether a reading counts as idle. Busy wins: any one signal that says busy is enough. Idle is only
//! the case where every signal was read and every one was quiet. A signal that could not be read
//! leaves the answer open, and so does a guest that nothing but the host can be read from.

use super::{GuestKind, IdleReading, Input, PowerRequest, Unread};

/// Where each signal turns from quiet to busy. These are first guesses that no idle Windows has been
/// measured against yet, and the design asks for them to be measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Input newer than this is someone at the machine.
    pub input_quiet_seconds: u64,
    /// The vCPUs using more than this of one core is work. An idle Windows still has background noise.
    pub vcpu_cores: f64,
}

impl Limits {
    /// The design's default for pausing: half an hour.
    pub const DEFAULT_QUIET_MINUTES: u64 = 30;
    /// A few percent of one core.
    pub const DEFAULT_VCPU_CORES: f64 = 0.05;

    pub fn new(quiet_minutes: u64) -> Self {
        Self { input_quiet_seconds: quiet_minutes.saturating_mul(60), vcpu_cores: Self::DEFAULT_VCPU_CORES }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(Self::DEFAULT_QUIET_MINUTES)
    }
}

/// One signal that says the machine is in use.
#[derive(Debug, Clone, PartialEq)]
pub enum Busy {
    Input { seconds: u64 },
    RemoteSessions(u32),
    PowerRequest(PowerRequest),
    VcpuTime { cores: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Every signal was read, and every one was quiet.
    Idle,
    /// At least one signal says busy. Whatever else could not be read does not change that.
    Busy(Vec<Busy>),
    /// Nothing says busy, but not everything was read, so quiet is not known.
    CannotTell(Vec<String>),
}

pub fn judge(reading: &IdleReading, limits: &Limits) -> Verdict {
    let mut busy = Vec::new();
    let mut open = Vec::new();
    let mut note = |name: &str, unread: &Unread| open.push(format!("{name}: {unread}"));

    match &reading.vcpu_cores {
        Ok(cores) if *cores > limits.vcpu_cores => busy.push(Busy::VcpuTime { cores: *cores }),
        Ok(_) => {}
        Err(unread) => note("CPU time", unread),
    }
    if reading.kind == GuestKind::Other {
        // Only the host's side can be read, and that is not enough to call a machine idle.
        if busy.is_empty() {
            open.push("Only CPU time can be read for this kind of guest.".to_owned());
            return Verdict::CannotTell(open);
        }
        return Verdict::Busy(busy);
    }
    match &reading.input {
        Ok(Input::Seconds(seconds)) if *seconds < limits.input_quiet_seconds => {
            busy.push(Busy::Input { seconds: *seconds });
        }
        Ok(_) => {}
        Err(unread) => note("Last input", unread),
    }
    match &reading.remote_sessions {
        Ok(0) => {}
        Ok(count) => busy.push(Busy::RemoteSessions(*count)),
        Err(unread) => note("Remote sessions", unread),
    }
    match &reading.power_requests {
        Ok(requests) => busy.extend(requests.iter().cloned().map(Busy::PowerRequest)),
        Err(unread) => note("Power requests", unread),
    }
    if !busy.is_empty() {
        Verdict::Busy(busy)
    } else if open.is_empty() {
        Verdict::Idle
    } else {
        Verdict::CannotTell(open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PowerKind;

    fn quiet() -> IdleReading {
        IdleReading {
            kind: GuestKind::Windows,
            input: Ok(Input::Seconds(3 * 3600)),
            remote_sessions: Ok(0),
            power_requests: Ok(Vec::new()),
            vcpu_cores: Ok(0.01),
            vcpu_window_seconds: 5.0,
        }
    }

    fn unread() -> Unread {
        Unread::new("no answer")
    }

    #[test]
    fn a_windows_guest_with_every_signal_read_and_quiet_is_idle() {
        assert_eq!(judge(&quiet(), &Limits::default()), Verdict::Idle);
    }

    #[test]
    fn nobody_logged_in_is_an_answer_and_quiet() {
        let reading = IdleReading { input: Ok(Input::NobodyLoggedIn), ..quiet() };
        assert_eq!(judge(&reading, &Limits::default()), Verdict::Idle);
    }

    #[test]
    fn recent_input_is_busy_and_old_input_is_not() {
        let recent = IdleReading { input: Ok(Input::Seconds(60)), ..quiet() };
        assert_eq!(judge(&recent, &Limits::default()), Verdict::Busy(vec![Busy::Input { seconds: 60 }]));
        let edge = IdleReading { input: Ok(Input::Seconds(1800)), ..quiet() };
        assert_eq!(judge(&edge, &Limits::new(30)), Verdict::Idle, "exactly the wait is long enough");
        assert_eq!(judge(&edge, &Limits::new(31)), Verdict::Busy(vec![Busy::Input { seconds: 1800 }]));
    }

    #[test]
    fn each_other_signal_alone_makes_it_busy() {
        let remote = IdleReading { remote_sessions: Ok(2), ..quiet() };
        assert_eq!(judge(&remote, &Limits::default()), Verdict::Busy(vec![Busy::RemoteSessions(2)]));
        let request = PowerRequest { kind: PowerKind::Display, who: "[PROCESS] vlc.exe".into() };
        let power = IdleReading { power_requests: Ok(vec![request.clone()]), ..quiet() };
        assert_eq!(judge(&power, &Limits::default()), Verdict::Busy(vec![Busy::PowerRequest(request)]));
        let cpu = IdleReading { vcpu_cores: Ok(0.9), ..quiet() };
        assert_eq!(judge(&cpu, &Limits::default()), Verdict::Busy(vec![Busy::VcpuTime { cores: 0.9 }]));
    }

    #[test]
    fn a_signal_that_could_not_be_read_is_never_idle() {
        for reading in [
            IdleReading { input: Err(unread()), ..quiet() },
            IdleReading { remote_sessions: Err(unread()), ..quiet() },
            IdleReading { power_requests: Err(unread()), ..quiet() },
            IdleReading { vcpu_cores: Err(unread()), ..quiet() },
        ] {
            assert!(matches!(judge(&reading, &Limits::default()), Verdict::CannotTell(_)), "{reading:?}");
        }
    }

    #[test]
    fn busy_wins_over_what_could_not_be_read() {
        let reading = IdleReading { power_requests: Err(unread()), vcpu_cores: Ok(2.0), ..quiet() };
        assert_eq!(judge(&reading, &Limits::default()), Verdict::Busy(vec![Busy::VcpuTime { cores: 2.0 }]));
    }

    #[test]
    fn a_guest_only_the_host_can_read_is_never_idle_from_cpu_time_alone() {
        let linux = IdleReading { kind: GuestKind::Other, input: Err(unread()), ..quiet() };
        assert!(matches!(judge(&linux, &Limits::default()), Verdict::CannotTell(_)));
        let working = IdleReading { vcpu_cores: Ok(3.0), ..linux };
        assert_eq!(judge(&working, &Limits::default()), Verdict::Busy(vec![Busy::VcpuTime { cores: 3.0 }]));
    }
}
