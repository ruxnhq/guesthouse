//! The way in to the helper: a forward of its own on the machine, on this PC's loopback, to the port the
//! helper listens on in the guest, as the files helper has one. It goes through QEMU's own network, as
//! the agent's and the files helper's do, because that is the one thing that can run a connection to
//! the guest in every network mode. It is made only for a machine that has app windows on.

use crate::context::Context;
use crate::machine::Machine;
use crate::network::GUEST_APP_WINDOWS_PORT;
use crate::processes::running_vms;
use guesthouse_daemon::slot::Slot;

/// Where this PC reaches slot 0's helper. Each slot adds its number, as the agent's port and the
/// files helper's do, and the three ranges do not meet: the agent has 18765 on, the files helper 18865.
pub const HOST_PORT_BASE: u16 = 18965;

pub fn host_port_of(slot: Slot) -> u16 {
    HOST_PORT_BASE + u16::from(slot.number())
}

/// The port on this PC for the machine in `slot`, or this process's slot. A test names its own.
pub fn host_port(context: &Context, slot: Option<Slot>) -> u16 {
    context.app_windows.port.unwrap_or_else(|| host_port_of(slot.unwrap_or(context.slot)))
}

/// QEMU's own word for the forward.
pub fn forward_word(port: u16) -> String {
    format!("hostfwd=tcp:127.0.0.1:{port}-:{GUEST_APP_WINDOWS_PORT}")
}

/// Whether the machine's running QEMU has the forward. `Some(false)` is a machine that was started
/// before app windows was switched on, or by a build before it: nothing on this PC can reach its
/// helper until it is started again. `None` when no QEMU of the machine can be found to look at.
pub fn running_machine_has_forward(context: &Context, machine: &Machine, slot: Option<Slot>) -> Option<bool> {
    let wanted = forward_word(host_port(context, slot));
    let vms = running_vms(context);
    let qemu = vms.get(&slot.unwrap_or(context.slot).number()).filter(|qemu| qemu.runs(machine))?;
    Some(qemu.words.iter().any(|word| String::from_utf8_lossy(word).contains(&wanted)))
}
