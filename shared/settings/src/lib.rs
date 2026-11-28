//! The checks of a machine's settings, as each host has them. Each takes plain values, not a host's
//! machine record, and says what is wrong in a sentence for a person, or whether a value will do.
//! The hosts' rules differ, and a check made stricter would refuse a machine that is already there
//! (the Linux host checks each machine's file when it loads it), so each host's checks are a module
//! of their own and none stands in for another: [`linux_host`] and [`windows_host`].
//!
//! The records the checks are about, and the checks that take them whole, stay with each host.

pub mod linux_host;
pub mod windows_host;
