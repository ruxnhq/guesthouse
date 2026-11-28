//! Whose each machine is. A machine belongs to the user who made it: only they and administrators
//! see it in a status, hear its events, read its account or its viewer's key, or do anything to it.
//! The owner is kept in the machine's settings, in the service's own folder, and never comes from
//! what an app sends.
//!
//! A machine made before owners were kept has none, and one rule gives it one. As the service
//! starts, the owner of the machine's own folder, then of its disk, is its owner if that's a user's
//! account, not the system's, the Administrators group's or another of Windows' own. A machine
//! whose files name no user waits for an administrator: an administrator's account asking as
//! itself, unelevated as the app runs, takes every machine still waiting, saved before it's
//! answered, and nobody else sees one. An elevated administrator sees them all without taking any.
//! An administrator can give a machine to another user by changing `owner` in its `machine.json`.

use super::{Service, Work};
use crate::machines::{Entry, Machines};
use guesthouse_control::Request;

/// Who asked: their account, as a SID, whether they may do anything to any machine, as the system
/// and an elevated administrator may, and whether their account is an administrator's at all, as
/// one whose app runs unelevated is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asker {
    pub user: String,
    pub administrator: bool,
    pub administrators_account: bool,
}

impl Asker {
    /// Whether `entry`'s machine is theirs to see and to act on.
    pub fn may_use(&self, entry: &Entry) -> bool {
        self.administrator || entry.settings.owner.as_deref() == Some(self.user.as_str())
    }
}

/// Whether `sid` is a user's account: a local, domain or Microsoft Entra one, not one of Windows'
/// own accounts or groups.
pub fn a_users(sid: &str) -> bool {
    ["S-1-5-21-", "S-1-12-1-"].iter().any(|prefix| sid.starts_with(prefix))
}

/// Gives `name`, if it has no owner, the user its own folder or its disk belongs to, if either
/// belongs to one.
pub(super) fn from_files<W: Work>(machines: &Machines, work: &W, name: &str) {
    let mut entries = machines.lock();
    let Some(entry) = entries.get_mut(name).filter(|entry| entry.settings.owner.is_none()) else { return };
    let files = [machines.store.folder(name), machines.store.disk(name)];
    let Some(owner) = files.iter().filter_map(|file| work.files_owner(file)).find(|sid| a_users(sid)) else {
        return;
    };
    let mut settings = entry.settings.clone();
    settings.owner = Some(owner.clone());
    match machines.store.save(name, &settings) {
        Ok(()) => {
            entry.settings = settings;
            work.log(&format!("{name} is {owner}'s, as its files say."));
        }
        Err(error) => work.log(&format!("{name}'s owner couldn't be saved: {error}")),
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
    /// Gives `asker` every machine that has no owner, if their account is an administrator's and
    /// they ask as themselves, saving each before anything else is done.
    pub(super) fn take_unowned(&self, asker: &Asker) {
        if asker.administrator || !asker.administrators_account {
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
