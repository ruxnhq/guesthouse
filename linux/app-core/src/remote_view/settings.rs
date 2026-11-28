//! What the program is started with, as arguments: `guesthouse-remote-view MACHINE --run-folder DIR ...`.

use crate::context::Context;
use crate::failure::Failure;
use crate::processes::qemu_pid;
use guesthouse_daemon::slot::Slot;
use guesthouse_jail::launch::start_time;
use guesthouse_relay::LanSettings;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

const RUN_FOLDER: &str = "--run-folder";
const LOOPBACK_PORT: &str = "--loopback-port";
const LISTEN: &str = "--listen";
const QEMU: &str = "--qemu";
const IDLE_SECONDS: &str = "--idle-seconds";
const MAX_CLIENTS: &str = "--max-clients";
const NAME: &str = "--name";
const FINGERPRINT: &str = "--fingerprint";
const HOLD_CLOCKS: &str = "--hold-clocks";

pub const USAGE: &str = "guesthouse-remote-view MACHINE --run-folder DIR --loopback-port PORT \
[--listen ADDRESS:PORT] [--qemu PID:START] [--idle-seconds N] [--max-clients N] [--name NAME] \
[--fingerprint HEX] [--hold-clocks PCI]";

/// A process as `/proc` knows it: its pid, and the time it started in the kernel's ticks, so that a
/// pid given to something else later is not taken for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watched {
    pub pid: u32,
    pub started: u64,
}

impl Watched {
    pub fn parse(text: &str) -> Option<Self> {
        let (pid, started) = text.split_once(':')?;
        Some(Self { pid: pid.parse().ok()?, started: started.parse().ok()? })
    }

    /// Whether this same process is still going, by `stat` as `/proc/PID/stat` has it.
    pub fn is_running(&self, processes: &Path) -> bool {
        let stat = std::fs::read_to_string(processes.join(self.pid.to_string()).join("stat")).ok();
        let same = stat.as_deref().and_then(start_time) == Some(self.started);
        same && !crate::process_stat::has_ended(stat.as_deref())
    }
}

impl fmt::Display for Watched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.pid, self.started)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The machine's id, which only names it in the program's command line and messages.
    pub machine: String,
    pub run_folder: PathBuf,
    /// The loopback port QEMU forwards the guest's remoteman port to.
    pub loopback_port: u16,
    /// Where to listen on the network from the start. Without it the program starts closed, and
    /// listens once asked to.
    pub listen: Option<SocketAddr>,
    /// The machine's QEMU. The program ends when it has gone. A trial run by hand can leave it out.
    pub qemu: Option<Watched>,
    pub idle_timeout: Option<Duration>,
    pub max_clients: Option<usize>,
    /// The name the machine is announced by on the network, which viewers show.
    pub name: Option<String>,
    /// The guest host's fingerprint. Nothing is announced until it is known.
    pub fingerprint: Option<String>,
    /// The NVIDIA card whose memory clock is held up while a viewer streams, by its PCI address.
    /// Left out, nothing is held.
    pub hold_clocks: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\nusage: {USAGE}", self.0)
    }
}

impl Settings {
    /// The settings for a machine running in `slot`: its run folder and loopback port are the slot's,
    /// and the program ends with the QEMU found there now. Without a QEMU there is no machine to
    /// carry for, and the program would never end by itself, so there are no settings.
    pub fn for_machine(
        context: &Context,
        machine: &str,
        slot: Slot,
        listen: Option<SocketAddr>,
    ) -> crate::Result<Self> {
        let qemu = qemu_pid(context, Some(slot))
            .and_then(|pid| {
                let stat = crate::process_stat::read(context, pid)?;
                Some(Watched { pid, started: start_time(&stat)? })
            })
            .ok_or_else(|| {
                Failure::new("The machine is not running, so its remote view has nothing to carry.")
            })?;
        Ok(Self {
            machine: machine.to_owned(),
            run_folder: context.run_folder(Some(slot)),
            loopback_port: slot.remote_view_port(),
            listen,
            qemu: Some(qemu),
            idle_timeout: None,
            max_clients: None,
            name: None,
            fingerprint: None,
            hold_clocks: None,
        })
    }

    /// What the network side starts with: QEMU's loopback forward upstream.
    pub fn lan(&self) -> LanSettings {
        LanSettings {
            machine: self.machine.clone(),
            upstream: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.loopback_port),
            listen: self.listen,
            idle_timeout: self.idle_timeout,
            max_clients: self.max_clients,
            name: self.name.clone(),
            fingerprint: self.fingerprint.clone(),
        }
    }

    pub fn arguments(&self) -> Vec<String> {
        let mut words = vec![self.machine.clone(), RUN_FOLDER.into(), self.run_folder.display().to_string()];
        words.extend([LOOPBACK_PORT.to_owned(), self.loopback_port.to_string()]);
        if let Some(listen) = self.listen {
            words.extend([LISTEN.to_owned(), listen.to_string()]);
        }
        if let Some(qemu) = self.qemu {
            words.extend([QEMU.to_owned(), qemu.to_string()]);
        }
        if let Some(idle) = self.idle_timeout {
            words.extend([IDLE_SECONDS.to_owned(), idle.as_secs().to_string()]);
        }
        if let Some(most) = self.max_clients {
            words.extend([MAX_CLIENTS.to_owned(), most.to_string()]);
        }
        if let Some(name) = &self.name {
            words.extend([NAME.to_owned(), name.clone()]);
        }
        if let Some(fingerprint) = &self.fingerprint {
            words.extend([FINGERPRINT.to_owned(), fingerprint.clone()]);
        }
        if let Some(card) = &self.hold_clocks {
            words.extend([HOLD_CLOCKS.to_owned(), card.clone()]);
        }
        words
    }

    /// Reads what [`arguments`](Self::arguments) wrote, without the program's own name.
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, UsageError> {
        let mut words = arguments.into_iter();
        let machine =
            words.next().filter(|word| !word.starts_with("--")).ok_or_else(|| missing("MACHINE"))?;
        let (mut run_folder, mut loopback_port, mut listen) = (None, None, None);
        let (mut qemu, mut idle_timeout, mut max_clients) = (None, None, None);
        let (mut name, mut fingerprint, mut hold_clocks) = (None, None, None);
        while let Some(flag) = words.next() {
            let value = words.next().ok_or_else(|| UsageError(format!("{flag} needs a value")))?;
            let bad = || UsageError(format!("{flag} cannot be {value:?}"));
            match flag.as_str() {
                RUN_FOLDER => run_folder = Some(PathBuf::from(&value)),
                LOOPBACK_PORT => loopback_port = Some(value.parse().map_err(|_| bad())?),
                LISTEN => listen = Some(value.parse().map_err(|_| bad())?),
                QEMU => qemu = Some(Watched::parse(&value).ok_or_else(bad)?),
                IDLE_SECONDS => idle_timeout = Some(Duration::from_secs(value.parse().map_err(|_| bad())?)),
                MAX_CLIENTS => max_clients = Some(value.parse().map_err(|_| bad())?),
                NAME => name = Some(value),
                FINGERPRINT => fingerprint = Some(value),
                HOLD_CLOCKS => hold_clocks = Some(value),
                _ => return Err(UsageError(format!("unknown argument {flag}"))),
            }
        }
        Ok(Self {
            machine,
            run_folder: run_folder.ok_or_else(|| missing(RUN_FOLDER))?,
            loopback_port: loopback_port.ok_or_else(|| missing(LOOPBACK_PORT))?,
            listen,
            qemu,
            idle_timeout,
            max_clients,
            name,
            fingerprint,
            hold_clocks,
        })
    }
}

fn missing(what: &str) -> UsageError {
    UsageError(format!("{what} is missing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_stat::stand_in::write_stat;
    use crate::testing::{Scratch, scratch_context};

    fn full() -> Settings {
        Settings {
            machine: "win11".into(),
            run_folder: "/run/user/1000/guesthouse/run-2".into(),
            loopback_port: 19067,
            listen: Some("192.168.1.20:7800".parse().unwrap()),
            qemu: Some(Watched { pid: 4242, started: 987_654 }),
            idle_timeout: Some(Duration::from_secs(30)),
            max_clients: Some(8),
            name: Some("Windows 11".into()),
            fingerprint: Some("ab".repeat(32)),
            hold_clocks: Some("0000:01:00.0".into()),
        }
    }

    #[test]
    fn arguments_read_back_as_they_were_written() {
        let settings = full();
        assert_eq!(Settings::parse(settings.arguments()), Ok(settings));
        let bare = Settings {
            listen: None,
            qemu: None,
            idle_timeout: None,
            max_clients: None,
            name: None,
            fingerprint: None,
            hold_clocks: None,
            ..full()
        };
        assert_eq!(Settings::parse(bare.arguments()), Ok(bare));
    }

    #[test]
    fn the_network_side_goes_to_qemus_loopback_forward_with_the_rest_as_given() {
        let lan = full().lan();
        assert_eq!(lan.upstream, "127.0.0.1:19067".parse().unwrap());
        assert_eq!(
            (lan.machine.as_str(), lan.listen, lan.idle_timeout, lan.max_clients),
            ("win11", Some("192.168.1.20:7800".parse().unwrap()), Some(Duration::from_secs(30)), Some(8))
        );
        assert_eq!((lan.name, lan.fingerprint), (Some("Windows 11".into()), Some("ab".repeat(32))));
    }

    #[test]
    fn arguments_that_cannot_work_are_refused_with_the_usage() {
        let words = |text: &str| text.split(' ').map(str::to_owned).collect::<Vec<_>>();
        for text in [
            "",
            "--run-folder /x --loopback-port 1",
            "m --loopback-port 1",
            "m --run-folder /x",
            "m --run-folder /x --loopback-port notaport",
            "m --run-folder /x --loopback-port 1 --listen nonsense",
            "m --run-folder /x --loopback-port 1 --qemu 12",
            "m --run-folder /x --loopback-port 1 --surprise 1",
            "m --run-folder /x --loopback-port",
        ] {
            let error = Settings::parse(words(text)).expect_err(text);
            assert!(error.to_string().contains("usage: guesthouse-remote-view"), "{text}");
        }
    }

    #[test]
    fn a_machine_in_a_slot_gets_that_slots_folder_loopback_port_and_qemu() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let slot = crate::slots::number(2);
        let qemu = context.paths.qemu.display().to_string();
        let pid_file = context.run_folder(Some(slot)).join(crate::vm_args::PID_FILE);
        crate::processes::stand_in::fake_process(
            &context,
            300,
            &[&qemu, "-pidfile", &pid_file.display().to_string()],
            &[],
            "",
        );
        let settings = Settings::for_machine(&context, "win11", slot, None).unwrap();
        assert_eq!(settings.run_folder, context.run_folder(Some(slot)));
        assert_eq!(settings.loopback_port, 19067);
        assert_eq!(settings.qemu, Some(Watched { pid: 300, started: 300 }));
    }

    #[test]
    fn a_machine_that_is_not_running_gets_no_settings() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let slot = crate::slots::number(2);
        assert!(
            Settings::for_machine(&context, "win11", slot, None).is_err(),
            "no QEMU is running in the test's /proc, and without one the program could never end"
        );
    }

    #[test]
    fn a_watched_process_is_the_same_one_only_if_it_started_when_noted() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        write_stat(&context, 4242, 'S', 987_654);
        let watched = Watched { pid: 4242, started: 987_654 };
        assert!(watched.is_running(&context.processes));
        assert!(!Watched { started: 1, ..watched }.is_running(&context.processes), "the pid was reused");
        write_stat(&context, 4242, 'Z', 987_654);
        assert!(!watched.is_running(&context.processes), "a zombie has ended");
        assert!(!Watched { pid: 1, ..watched }.is_running(&context.processes));
    }
}
