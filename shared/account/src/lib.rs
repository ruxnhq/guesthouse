//! The rules for a guest's account: which names a Windows and an Ubuntu guest's user may have, the
//! reserved names, and the password. Every host uses the same rules, in [`common`], and keeps its
//! own default names: [`linux_host`] has the Linux host's, and the Windows host's is in its control
//! crate.
//!
//! Making a password and keeping it stay with each host, as they draw on the system.

pub mod common;
pub mod linux_host;
