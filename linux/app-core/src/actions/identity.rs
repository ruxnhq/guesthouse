//! The card maker's vendor number going into a machine. A start on Microsoft's number has Windows
//! write the driver package for the maker's, and then starts the VM again to show it (see identity_switch.rs),
//! restarting Windows once on the way (see identity_restart.rs). Nothing here can stop a machine: a
//! guest that won't take the package keeps Microsoft's number, and the row says why.

use super::Task;
use super::guest_files::give_display_its_name;
use crate::agent::guest_script;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::gpu_identity::{
    FELL_BACK, IdentityState, STAYS_ON_INTEL, Shown, falls_back_after_failed_check, machine_maker,
    maker_identity, maker_name, read_state, write_state,
};
use crate::gpu_maker::GpuMaker;
use crate::gpu_support::qemu_sets_identity;
use crate::log::write_log;
use crate::machine::Machine;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const SCRIPT: &str = "gpu-identity.ps1";
/// Making and signing the package took about 20 seconds in a guest, and this is what the agent gets.
const STAGE_SECONDS: u64 = 110;
const STAGED_MARK: &str = "identity staged";
const STAY_MARK: &str = "stay: ";

fn qemu_without(maker: &str) -> String {
    format!("This QEMU can't show Windows {maker}'s vendor ID, so Windows sees Microsoft's.")
}

fn no_script(maker: &str) -> String {
    format!("This build has no script for {maker}'s vendor ID, so Windows sees Microsoft's.")
}

fn did_not_stage(maker: &str) -> String {
    format!(
        "Windows could not make the driver package for {maker}'s vendor ID, so it keeps Microsoft's. \
         The log has what Windows said."
    )
}

fn next_start(maker: &str) -> String {
    format!("Windows sees Microsoft's vendor ID until its next start, when it will see {maker}'s.")
}

fn tries_again(maker: &str) -> String {
    format!("That start then tries {maker}'s once more.")
}

fn stays(maker: &str, failed: u32) -> String {
    format!("It stays there, as {maker}'s has now failed {failed} times in a row.")
}

/// Tells one version of the guest script from another, so a failed one isn't run again.
fn script_id(script: &str) -> String {
    let mut hasher = DefaultHasher::new();
    script.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Whether this machine's next boot will ask Windows to write the package for its maker's number,
/// from what the state files and the scripts say now. The same checks `set_up` makes, in the same
/// order, and the cheap ones first as this is asked at every start: the QEMU is only asked about a
/// machine that has something to stage. It says which script would run, as a build with a new one
/// tries again what an older one failed at, and with it which try this is, as a machine that fell back
/// has the same script run once more.
pub(crate) fn will_stage(context: &Context, machine: &Machine) -> Option<String> {
    let state = read_state(context, machine);
    if state.shown == Shown::Maker {
        return None;
    }
    let id = script_id(&guest_script(context, SCRIPT).ok()?);
    if state.failed_script == id || !qemu_sets_identity(&context.paths.qemu) {
        return None;
    }
    Some(if state.failed_checks == 0 { id } else { format!("{id}:try-{}", state.failed_checks + 1) })
}

/// Has Windows write the package, and records what came of it.
fn stage(task: &Task, state: &mut IdentityState, script: &str) -> Result<()> {
    let identity = maker_identity(&task.machine);
    let maker = maker_name(&task.machine);
    let (vendor, device) = (identity.vendor, identity.device);
    let sent = format!("$vendor = '{vendor:04X}'\n$device = '{device:04X}'\n{script}");
    let said = task.ask_guest(&sent, STAGE_SECONDS);
    write_log(task.context, &format!("gpu identity: {}", said.trim()))?;
    if said.lines().any(|line| line.trim() == STAGED_MARK) {
        if !state.failed_script.is_empty() {
            // Another script than the one that was given up on, so it gets its own tries.
            state.failed_checks = 0;
        }
        state.shown = Shown::Maker;
        state.package_vendor = vendor;
        state.failed_script.clear();
        state.reason.clear();
        state.note = next_start(maker);
        state.restart_due = true;
        return Ok(());
    }
    state.failed_script = script_id(script);
    let why = said.lines().find_map(|line| line.trim().strip_prefix(STAY_MARK));
    state.reason = match why {
        Some(why) => format!("Windows keeps Microsoft's vendor ID. {why}"),
        None => did_not_stage(maker),
    };
    Ok(())
}

fn set_up(task: &Task) -> Result<()> {
    let mut state = read_state(task.context, &task.machine);
    state.note.clear();
    if state.shown == Shown::Maker {
        // This is asked once the GPU check has passed, so the maker's number works here.
        state.failed_checks = 0;
        return write_state(task.context, &task.machine, &state);
    }
    let maker = maker_name(&task.machine);
    task.step(&format!("Preparing {maker}'s vendor ID"));
    if !qemu_sets_identity(&task.context.paths.qemu) {
        state.reason = qemu_without(maker);
        return write_state(task.context, &task.machine, &state);
    }
    let Ok(script) = guest_script(task.context, SCRIPT) else {
        state.reason = no_script(maker);
        return write_state(task.context, &task.machine, &state);
    };
    if state.failed_script != script_id(&script) {
        stage(task, &mut state, &script)?;
    }
    write_state(task.context, &task.machine, &state)
}

/// What each boot does, once the GPU and the desktop are up. Whatever goes wrong here goes in the log
/// and on the row, and can't reach the code that turns off a VM whose start has failed.
pub(super) fn set_up_identity(task: &mut Task) {
    if let Err(failure) = set_up(task) {
        let said = format!("{}'s vendor ID could not be prepared: {}", maker_name(&task.machine), failure.0);
        let _ = write_log(task.context, &said);
        task.error = said;
    }
}

/// Windows started without its GPU. If it was shown its maker's number, that's the first suspect, and
/// the next start goes back to the one that always works, unless the maker's card can't do without
/// its number: see [`falls_back_after_failed_check`].
///
/// A check can fail once on a guest that is fine on the number, so the start after this one has
/// Windows take the package again and tries the number once more. Only when that fails as well is
/// the script named as failed, which keeps the machine on Microsoft's number: see [`crate::gpu_identity::CHECK_TRIES`].
pub(super) fn after_failed_gpu(task: &Task, failure: Failure) -> Failure {
    let mut state = read_state(task.context, &task.machine);
    if state.shown != Shown::Maker {
        return failure;
    }
    if !falls_back_after_failed_check(machine_maker(&task.machine)) {
        return Failure(format!("{} {STAYS_ON_INTEL}", failure.0));
    }
    state.shown = Shown::Microsoft;
    state.restart_due = false;
    state.note.clear();
    state.reason = FELL_BACK.to_owned();
    state.failed_checks += 1;
    let maker = maker_name(&task.machine);
    let mut next = tries_again(maker);
    if state.out_of_tries() {
        // Staging the same package at every start would end the same way each time, and no start
        // would ever finish. A new build's script is tried afresh.
        if let Ok(script) = guest_script(task.context, SCRIPT) {
            state.failed_script = script_id(&script);
        }
        next = stays(maker, state.failed_checks);
    }
    if let Err(problem) = write_state(task.context, &task.machine, &state) {
        let _ = write_log(task.context, &format!("gpu identity: could not fall back: {}", problem.0));
    }
    if task.machine.windows_maker() == Some(GpuMaker::Nvidia) {
        // The guest is still up, so it can have the name the next start's driver needs.
        let _ = give_display_its_name(task);
    }
    Failure(format!("{} {FELL_BACK} {next}", failure.0))
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
