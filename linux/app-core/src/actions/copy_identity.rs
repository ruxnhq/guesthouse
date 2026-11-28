//! What a copy of an Ubuntu machine's guest gets new: SSH host keys and a machine id. Its disk has the
//! original's. An Ubuntu guest is only ever on QEMU's own NAT, with SSH on this PC's loopback, so the
//! two never meet on one network, but what reaches out from a guest is told the same machine by both:
//! anything keyed on the machine id, as Ubuntu Pro and management agents are, and the host key either
//! would show to a client it was forwarded to.
//!
//! Only a running guest can be changed, and only over SSH once it's up, so a Duplicate or an import
//! leaves a mark in the copy's folder saying it's owed, and the copy's next start does it, after the
//! start itself is done. The machine id is read once at boot by systemd and what it starts, so the
//! guest then restarts once. Until then the copy is up under the original's identities.
//!
//! The host forgets the guest's host key before each time it asks, as the guest may have a new one
//! already from a try whose answer was lost, and learns it again, as after an install. A change cut
//! short anywhere leaves it owed, and the next start does it again: it's the same work twice.
//!
//! Once done, the mark stays and says so, as a copy's snapshots from before it hold the original's
//! identities: going back to any snapshot of a copy makes it owed again, which at worst costs one
//! restart more.
//!
//! A throwaway's disk never keeps it, so it would cost a restart at every run. A throwaway copy keeps
//! the original's instead, and the mark, so the first start once Throwaway is off does it. The host
//! name is not changed: every guest Guesthouse installs has the same one.

use super::Task;
use crate::context::Context;
use crate::failure::Result;
use crate::guest_shell::ShellAnswer;
use crate::linux::guest_sources;
use crate::log::write_log;
use crate::machine::{Kind, Machine};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// In the folder of a copy of an Ubuntu machine: whether its guest's identities are owed or done.
pub const MARK: &str = "guest.identity.new";
const OWED: &str = "owed";
const DONE: &str = "done";
const SCRIPT: &str = "copy/new-identity.sh";
/// The script goes in on stdin, as everything the host runs in the guest does.
pub(super) const COMMAND: &str = "sh -s";
const RENEWED: &str = "renewed";
/// What the script exits with while the guest's first boot, its GPU step or a restart is under way.
const NOT_YET: i32 = 3;
const QUICK_SECONDS: u64 = 60;
/// An installed guest is up in a minute or two. The start holds the machine's slot while it waits,
/// so a guest that takes longer, as one building its GPU driver for a new kernel does, is left for the
/// next start.
const UP_MINUTES: u64 = 5;
const UP_POLL: Duration = Duration::from_secs(10);
pub(super) const STEP: &str = "Giving the copy's guest SSH host keys and a machine id of its own";

/// Leaves the mark in the folder of a copy being made, if it's of a kind whose guest this is for.
pub fn plan_at(folder: &Path, kind: Kind) -> Result<()> {
    if kind == Kind::Linux {
        fs::write(folder.join(MARK), OWED)?;
    }
    Ok(())
}

fn mark_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(MARK)
}

fn owed(context: &Context, machine: &Machine) -> bool {
    fs::read_to_string(mark_file(context, machine)).is_ok_and(|said| said.trim() != DONE)
}

/// A copy's disk went back to a snapshot, which may be from before its guest's identities were new.
pub fn went_back(context: &Context, machine: &Machine) {
    let mark = mark_file(context, machine);
    if machine.kind == Kind::Linux && mark.exists() {
        let _ = fs::write(mark, OWED);
        machine.forget_guest_host_key(context);
    }
}

/// A new install is a new guest, with identities of its own.
pub fn forget(context: &Context, machine: &Machine) {
    let _ = fs::remove_file(mark_file(context, machine));
}

fn log(task: &Task, text: &str) {
    let _ = write_log(task.context, &format!("{}: {text}", task.machine.id));
}

fn not_done(task: &mut Task, why: &str) {
    log(task, &format!("its guest keeps the identities it was copied with: {why}"));
    let note = format!(
        "Its guest still has the SSH host keys and machine id of the machine it was copied from: {why} \
         The next start tries again."
    );
    task.error = [task.error.as_str(), note.as_str()].join(" ").trim().to_owned();
}

/// Gives the guest of a copy its own identities, if it still has the original's. Run once the start
/// has the guest going. Nothing here fails the start: the guest works either way, and the next start
/// tries again.
pub(super) fn renew_if_owed(task: &mut Task) {
    if task.machine.kind != Kind::Linux || task.machine.throwaway || !owed(task.context, &task.machine) {
        return;
    }
    let mark = mark_file(task.context, &task.machine);
    if !task.machine.has_ssh_key(task.context) {
        log(task, "it has no SSH key of its own, so its guest keeps the identities it was copied with");
        let _ = fs::write(mark, DONE);
        return;
    }
    let script = match fs::read(guest_sources(task.context).join(SCRIPT)) {
        Ok(script) => script,
        Err(error) => return not_done(task, &format!("The script for it is missing: {error}")),
    };
    task.step(STEP);
    let end = task.now() + Duration::from_secs(UP_MINUTES * 60);
    let answer = loop {
        if task.pid().is_none() {
            return log(task, "the machine stopped before its guest got identities of its own");
        }
        task.machine.forget_guest_host_key(task.context);
        let answer = task.context.ask_shell(&task.machine, COMMAND, &script, QUICK_SECONDS);
        let again = matches!(answer, ShellAnswer::Unreachable(_) | ShellAnswer::Ran { status: NOT_YET, .. });
        if !again || task.now() >= end {
            break answer;
        }
        task.sleep(UP_POLL);
    };
    match answer {
        ShellAnswer::Ran { status: 0, output } if output.lines().any(|line| line.trim() == RENEWED) => {
            // The connection that asked learned the old host key.
            task.machine.forget_guest_host_key(task.context);
            let _ = fs::write(mark, DONE);
            log(task, "its guest has new SSH host keys and a new machine id, and restarts");
        }
        ShellAnswer::Ran { status: NOT_YET, output } => {
            log(
                task,
                &format!("its identities wait for the next start: {}", super::recipe::one_line(&output)),
            );
        }
        ShellAnswer::Ran { output, .. } => not_done(task, &super::recipe::one_line(&output)),
        ShellAnswer::TimedOut => not_done(task, "The guest did not finish in time."),
        ShellAnswer::Unreachable(said) => {
            not_done(task, &format!("The guest could not be reached in {UP_MINUTES} minutes. {said}"))
        }
    }
}

#[cfg(test)]
#[path = "copy_identity_tests.rs"]
mod tests;
