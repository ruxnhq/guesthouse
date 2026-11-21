//! QEMU's jail on this PC's kernel, with no VM. A GPU machine's plan is made as a start would make
//! it, and its words are run with this program standing in for QEMU. In there it tries what a guest
//! that had taken QEMU over would, and says how that went with its exit code. It has no sound, so the
//! launcher opens QEMU's unix sockets and QEMU may make none. Its disk is a qcow2 on a base outside
//! its folder, as a disk made by hand can be. On passt, the launcher starts the real passt and the
//! stand-in asks it for the gateway's address through the socket it was handed. Bridged, this program
//! also stands in for the bridge helper, and hands back an open file for the device. With the
//! forwarded GPU, it's an Ubuntu guest's QEMU, whose input and frames sockets the launcher opens too.

#[cfg(target_os = "linux")]
#[path = "jail/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("jail tests Linux, so it does nothing here.");
}
