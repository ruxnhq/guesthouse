//! The rules of a machine's snapshots that take plain values, as each host has them. The Linux host
//! keeps a snapshot inside its qcow2 disk under a tag, with a file beside it that names it; the Windows
//! host keeps a chain of disks, one for each snapshot, and an index of them. Each host's rules are a
//! module of their own, [`linux_host`] and [`windows_host`], as the two differ and none stands in for
//! the other: the Linux host takes a name of 1 to 60 characters, any character between, that no other
//! of the machine's snapshots has unless Guesthouse takes it by itself; the Windows host a label of 1 to
//! 60 characters with no control character, which may repeat, and at most 32 snapshots a machine.
//!
//! Taking, going back to and deleting a snapshot, and reading and writing the files, stay with each
//! host, as they draw on qemu-img or on Windows' virtual disks.

pub mod linux_host;
pub mod windows_host;
