//! Whose each machine is. A machine belongs to the user who made it: only they and administrators
//! see it in a status, hear its events, read its account or its viewer's key, or do anything to it.
//! The owner is kept in the machine's settings, in the service's own folder, and never comes from
//! what an app sends.
//!
//! A machine made before owners were kept has none. The first user other than an administrator who
//! asks the service anything after that takes every such machine as their own, and it's saved
//! before they're answered. On a PC with one user, that's them, and nothing changes for them. Until
//! then such a machine is an administrator's only. An administrator can give a machine to someone
//! else by changing `owner` in its `machine.json`, or let it be taken again by removing it.

use super::{Service, Work};
use crate::machines::Entry;
use guesthouse_control::Request;

/// Who asked: their account, as a SID, and whether they may do anything to any machine, as the
/// system and an elevated administrator may.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asker {
    pub user: String,
    pub administrator: bool,
}

impl Asker {
    /// Whether `entry`'s machine is theirs to see and to act on.
    pub fn may_use(&self, entry: &Entry) -> bool {
        self.administrator || entry.settings.owner.as_deref() == Some(self.user.as_str())
    }
}

/// The machine `request` acts on or reads, by the name it was asked by. A new machine's name is no
/// machine's yet, and a request for a report names one only if it asks for one's files.
pub fn named(request: &Request) -> Option<&str> {
    match request {
        Request::Start { name }
        | Request::Stop { name }
        | Request::End { name }
        | Request::Screen { name }
        | Request::Delete { name }
        | Request::Grow { name, .. }
        | Request::Snapshot { name, .. }
        | Request::Revert { name, .. }
        | Request::DeleteSnapshot { name, .. }
        | Request::Account { name }
        | Request::Watch { name }
        | Request::Change { name, .. }
        | Request::Move { name, .. }
        | Request::Share { name, .. }
        | Request::RemoteView { name }
        | Request::PairClose { name }
        | Request::PairAccept { name }
        | Request::PairDecline { name }
        | Request::DeviceAllow { name, .. }
        | Request::DeviceRevoke { name, .. } => Some(name),
        Request::ReportFiles { machine } => machine.as_deref(),
        Request::Status | Request::Create { .. } | Request::EnableHyperV => None,
    }
}

impl<W: Work> Service<W> {
    /// Gives `asker` every machine that has no owner, unless they're an administrator, saving each
    /// before anything else is done.
    pub(super) fn take_unowned(&self, asker: &Asker) {
        if asker.administrator {
            return;
        }
        let mut entries = self.machines.lock();
        for (name, entry) in entries.iter_mut().filter(|(_, entry)| entry.settings.owner.is_none()) {
            let mut settings = entry.settings.clone();
            settings.owner = Some(asker.user.clone());
            match self.machines.store.save(name, &settings) {
                Ok(()) => {
                    entry.settings = settings;
                    self.work.log(&format!("{name} is {}'s from now on.", asker.user));
                }
                Err(error) => self.work.log(&format!("{name}'s owner couldn't be saved: {error}")),
            }
        }
    }

    /// Whether the machine asked for as `asked` is one `asker` may use. One they may not is as if it
    /// weren't there.
    pub(super) fn theirs(&self, asked: &str, asker: &Asker) -> bool {
        let entries = self.machines.lock();
        entries
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(asked))
            .is_none_or(|(_, entry)| asker.may_use(entry))
    }

    /// Whether `name` is a machine `asker` may see.
    pub(super) fn visible(&self, name: &str, asker: &Asker) -> bool {
        self.machines.lock().get(name).is_some_and(|entry| asker.may_use(entry))
    }
}
