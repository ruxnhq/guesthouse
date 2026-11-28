//! What the service says of its machines: the status the app asks for every moment, and the files
//! a bug report needs from this PC.

use super::{Asker, Connection, REPORT_TAIL, Service, Work, no_machine, send_text, words};
use crate::report_parts::tail_file;
use crate::store::Store;
use guesthouse_control::{Answer, Card, Machine, NetworkMode, SnapshotInfo, System};
use guesthouse_host_windows::machine::Guest;
use guesthouse_host_windows::record::Record;
use guesthouse_host_windows::snapshots;
use std::path::PathBuf;

impl<W: Work> Service<W> {
    pub(super) fn status(&self, asker: &Asker) -> Answer {
        // Each place is held again as it stands: one that's gone is a problem, and one that's back
        // isn't.
        let names: Vec<String> = self.machines.lock().keys().cloned().collect();
        for name in &names {
            self.refresh(name);
        }
        let cards = self.cards();
        let bridging = self.bridging();
        let default_card = cards.iter().find(|card| card.default).map(|card| card.name.clone());
        let machines = self
            .machines
            .lock()
            .iter()
            .filter(|(_, entry)| asker.may_see(entry))
            .map(|(name, entry)| {
                let record = Record::load(&self.machines.store.disk(name)).unwrap_or_default();
                let machine = Machine {
                    name: name.clone(),
                    system: match record.guest {
                        Guest::Windows => System::Windows,
                        Guest::Linux => System::Linux,
                    },
                    state: entry.state,
                    set_up: record.installed,
                    memory_mb: entry.settings.memory_mb,
                    processors: entry.settings.processors,
                    card: match &entry.settings.card {
                        Card::Default => default_card.clone(),
                        Card::Named(name) => Some(name.clone()),
                        Card::None => None,
                    },
                    chosen_card: entry.settings.card.clone(),
                    nvapi_wrapper: entry.settings.nvapi_wrapper,
                    tpm: entry.settings.tpm,
                    network: entry.settings.network,
                    shares: entry.settings.shares.clone(),
                    network_mode: entry.settings.network_mode.clone(),
                    forwards: entry.settings.forwards.clone(),
                    nested_virtualization: entry.settings.nested_virtualization,
                    ssh_port: entry.settings.ssh_port,
                    size_gb: entry.settings.size_gb,
                    folder: self.machines.store.recorded(name),
                    problem: self.machines.store.problem(name),
                    snapshots: snapshots::list(&self.machines.store.disk(name))
                        .unwrap_or_default()
                        .into_iter()
                        .map(|snapshot| SnapshotInfo {
                            id: snapshot.id,
                            label: snapshot.label,
                            taken: snapshot.taken,
                            current: snapshot.current,
                        })
                        .collect(),
                    working: entry.busy.clone(),
                    address: entry.settings.address.clone(),
                    remote_view: entry.remote_view.values().0,
                    remote_port: entry.remote_view.values().1,
                    console_pipe: entry.console_pipe.clone(),
                    needs_owner: entry.settings.owner.is_none(),
                };
                if asker.may_use(entry) { machine } else { only_what_it_is(machine) }
            })
            .collect();
        Answer::Status {
            version: guesthouse_control::WINDOWS_VERSION.into(),
            protocol: guesthouse_control::protocol::PROTOCOL,
            machines,
            cards,
            bridging,
        }
    }

    /// Hands over the files a bug report needs from this PC: what the work reads, a list of the
    /// machines, and if one is named, a few of its own files. Nothing secret goes: see
    /// [`report_machine_files`].
    pub(super) fn report_files(&self, machine: Option<String>, connection: &mut dyn Connection) {
        let named = match machine.as_deref() {
            Some(asked) => match self.machines.find(asked) {
                Some(name) => Some(name),
                None => {
                    let _ = connection.send(&no_machine(asked));
                    return;
                }
            },
            None => None,
        };
        let mut files = self.work.report_parts();
        files.push(("machines.txt".to_owned(), Ok(self.machines_text(&connection.asker()))));
        if let Some(name) = &named {
            for (shown, path) in report_machine_files(&self.machines.store, name) {
                files.push((shown, tail_file(&path, REPORT_TAIL)));
            }
        }
        for (name, text) in files {
            let sent = match text {
                Ok(text) => send_text(connection, &name, &text),
                Err(reason) => connection.send(&Answer::ReportMissing { name, reason }),
            };
            if sent.is_err() {
                return;
            }
        }
        let _ = connection.send(&Answer::Done { reason: None });
    }

    /// Each machine on a line: its name, system, state, and whether its first boot has set it up.
    fn machines_text(&self, asker: &Asker) -> String {
        let Answer::Status { machines, .. } = self.status(asker) else { return String::new() };
        machines
            .iter()
            .map(|machine| {
                let system = match machine.system {
                    System::Windows => "windows",
                    System::Linux => "linux",
                };
                let set_up = if machine.set_up { "set up" } else { "not set up" };
                format!("{}: {system}, {}, {set_up}\n", machine.name, words(machine.state))
            })
            .collect()
    }
}

/// A machine that needs an owner as a user who may not use it sees it: what it is, where it stands
/// and what it's given, and nothing of where it's kept, how it's reached or what it holds.
fn only_what_it_is(machine: Machine) -> Machine {
    Machine {
        shares: Vec::new(),
        network_mode: NetworkMode::Shared,
        forwards: Vec::new(),
        ssh_port: None,
        folder: PathBuf::new(),
        problem: None,
        snapshots: Vec::new(),
        working: None,
        address: None,
        remote_view: false,
        remote_port: None,
        console_pipe: None,
        ..machine
    }
}

/// A report's files for a machine are these and nothing else: its folder also holds the guest's
/// account and password, the viewer's private key, and its disks. Each is optional. A file is
/// named in the report as it's shown here.
fn report_machine_files(store: &Store, name: &str) -> Vec<(String, PathBuf)> {
    let folder = store.folder(name);
    let place = store.place(name);
    let record = Record::path(&store.disk(name));
    let record_name = record.file_name().map(|file| file.to_string_lossy().into_owned()).unwrap_or_default();
    vec![
        ("machine.json".to_owned(), folder.join("machine.json")),
        (record_name, record),
        ("snapshots/index.json".to_owned(), place.join("snapshots").join("index.json")),
        ("events.log".to_owned(), store.events(name)),
        ("events.1.log".to_owned(), store.events_before(name)),
    ]
}
