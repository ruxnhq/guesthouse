//! The way in to the helper: a forward of its own on the machine, from a loopback port of this PC that
//! QEMU picks, to the port the helper listens on in the guest, as the files helper has one. It goes through QEMU's own network, as
//! the agent's and the files helper's do, because that is the one thing that can run a connection to
//! the guest in every network mode. It is made only for a machine that has app windows on.

use crate::context::Context;
use crate::machine::Machine;
use crate::network::GUEST_APP_WINDOWS_PORT;
use crate::processes::running_vms;
use guesthouse_daemon::slot::Slot;

/// QEMU's own word for the forward, from a loopback port it picks.
pub fn forward_word() -> String {
    crate::vm_args::picked_port_forward(GUEST_APP_WINDOWS_PORT)
}

/// Whether the machine's running QEMU has the forward. `Some(false)` is a machine that was started
/// before app windows was switched on, or by a build before it: nothing on this PC can reach its
/// helper until it is started again. `None` when no QEMU of the machine can be found to look at.
pub fn running_machine_has_forward(context: &Context, machine: &Machine, slot: Option<Slot>) -> Option<bool> {
    let wanted = forward_word();
    let vms = running_vms(context);
    let qemu = vms.get(&slot.unwrap_or(context.slot).number()).filter(|qemu| qemu.runs(machine))?;
    Some(qemu.words.iter().any(|word| String::from_utf8_lossy(word).contains(&wanted)))
}
