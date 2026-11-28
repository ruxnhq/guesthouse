//! A machine's states and the words they are shown with, one list for every host, in [`common`]:
//! the Linux host's, and what the other hosts need beyond them. The Windows host sends them over
//! its pipe by their names. [`linux_host`] keeps the name the Linux host has always used.

pub mod common;
pub mod linux_host;
