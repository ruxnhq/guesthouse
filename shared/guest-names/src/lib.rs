//! The rules for names and paths in a Windows guest, as each host has them: what a file is called
//! when it goes into the guest, which folders Defender may leave out of its scanning, and what a
//! shared folder is called there. Each host's rules are a module of their own, [`linux_host`] and
//! [`windows_host`], as no rule is shared by both today and none stands in for another.
//!
//! Sending the files, keeping the folders beside a machine and asking the guest stay with each host,
//! as they draw on the system.

pub mod linux_host;
pub mod windows_host;
