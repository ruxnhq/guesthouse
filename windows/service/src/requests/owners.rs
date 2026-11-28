//! Whose each machine is. A machine belongs to the user who made it: only they and administrators
//! see it in a status, hear its events, read its account or its viewer's key, or do anything to it.
//! The owner is kept in the machine's settings, in the service's own folder, and never comes from
//! what an app sends.
//!
//! A machine made before owners were kept has none, and one rule gives it one. As the service
//! starts, the owner of the machine's own folder, then of its disk, is its owner if that's a user's
//! account, not the system's, the Administrators group's or another of Windows' own. On a PC that
//! keeps a profile for one user's account alone, that user takes every machine still waiting at
//! their first request, saved before it's answered. Otherwise a machine waits for an owner: every
//! user at the console sees it as one that needs an owner, by its name, system and state only, and
//! nothing can be done to it until an administrator, asking elevated, gives it to a user with
//! `Assign`. The app asks that from a helper Windows asks the person to allow, for the app's own
//! user. No account takes a waiting machine by asking. An elevated administrator sees and acts on
//! every machine. An administrator can also give a machine to another user by changing `owner` in
//! its `machine.json`.

use super::{Service, Work, no_machine};
use crate::machines::{Entry, Machines};
use guesthouse_control::{Answer, Request};

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

    /// Whether they see `entry`'s machine in a status: theirs, or one that needs an owner.
    pub fn may_see(&self, entry: &Entry) -> bool {
        self.may_use(entry) || entry.settings.owner.is_none()
    }
}

/// What a machine that needs an owner answers to anything but `Assign`, for anyone but an elevated
/// administrator.
pub fn needs_an_owner(name: &str) -> String {
    format!("{name} needs an owner first. Make it yours, which an administrator of this PC allows.")
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
        | Request::DeviceRevoke { name, .. }
        | Request::Assign { name, .. } => Some(name),
        Request::ReportFiles { machine } => machine.as_deref(),
        Request::Status | Request::Create { .. } | Request::EnableHyperV => None,
    }
}

impl<W: Work> Service<W> {
    /// Gives `asker` every machine that has no owner, if they're the one user this PC keeps a
    /// profile for, saving each before anything else is done.
    pub(super) fn take_unowned(&self, asker: &Asker) {
        if !self.machines.lock().values().any(|entry| entry.settings.owner.is_none()) {
            return;
        }
        if !self.the_one_user(&asker.user) {
            return;
        }
        let mut entries = self.machines.lock();
        for (name, entry) in entries.iter_mut().filter(|(_, entry)| entry.settings.owner.is_none()) {
            let mut settings = entry.settings.clone();
            settings.owner = Some(asker.user.clone());
            match self.machines.store.save(name, &settings) {
                Ok(()) => {
                    entry.settings = settings;
                    self.work.log(&format!("{name} is {}'s from now on, this PC's one user.", asker.user));
                }
                Err(error) => self.work.log(&format!("{name}'s owner couldn't be saved: {error}")),
            }
        }
    }

    /// Whether `user` is the one user this PC keeps a profile for.
    fn the_one_user(&self, user: &str) -> bool {
        let profiles = self.work.profiles();
        let users: Vec<&String> = profiles.iter().filter(|sid| a_users(sid)).collect();
        users == [user]
    }

    /// What to answer `asker` instead, if the machine asked for as `asked` isn't one they may use:
    /// another user's is as if it weren't there, and one that needs an owner says so.
    pub(super) fn not_theirs(&self, asked: &str, asker: &Asker) -> Option<Answer> {
        let entries = self.machines.lock();
        let (name, entry) = entries.iter().find(|(name, _)| name.eq_ignore_ascii_case(asked))?;
        match &entry.settings.owner {
            _ if asker.may_use(entry) => None,
            None => Some(Answer::Refused { reason: needs_an_owner(name) }),
            Some(_) => Some(no_machine(asked)),
        }
    }

    /// Whether `name` is a machine `asker` may see.
    pub(super) fn visible(&self, name: &str, asker: &Asker) -> bool {
        self.machines.lock().get(name).is_some_and(|entry| asker.may_use(entry))
    }

    /// Gives `asked`, a machine that needs an owner, to `owner`, an account this PC keeps a profile
    /// for. Only an elevated administrator may.
    pub(super) fn assign(&self, asked: &str, owner: &str, asker: &Asker) -> Answer {
        let refused = |reason: String| Answer::Refused { reason };
        if !asker.administrator {
            return refused("Only an administrator of this PC can give a machine its owner.".into());
        }
        if !a_users(owner) || !self.work.profiles().iter().any(|profile| profile == owner) {
            return refused(format!("{owner} isn't a user of this PC."));
        }
        let mut entries = self.machines.lock();
        let Some((name, entry)) = entries.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(asked))
        else {
            return no_machine(asked);
        };
        if entry.settings.owner.is_some() {
            return refused(format!("{name} has an owner already."));
        }
        let mut settings = entry.settings.clone();
        settings.owner = Some(owner.to_owned());
        if let Err(error) = self.machines.store.save(name, &settings) {
            return refused(format!("{name}'s owner couldn't be saved: {error}"));
        }
        entry.settings = settings;
        self.work.log(&format!("{name} is {owner}'s from now on, as {} allowed.", asker.user));
        Answer::Ok
    }
}
