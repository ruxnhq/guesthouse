//! What a guest says on its serial port, and how a host reads it, as each host has them. A guest
//! already installed keeps saying the words its scripts were written with, and those scripts can't
//! change, so the hosts' words differ (an Ubuntu install ends with `nvfwd-install-finished` on the
//! Linux host and `guesthouse-install-finished` on the Windows host) and none stands in for another.
//! Each host's are a module of their own: [`linux_host`] and [`windows_host`].
//!
//! Reading the port, and what a host does once a line has come, stay with each host, as they draw on
//! the system.

pub mod linux_host;
pub mod windows_host;
