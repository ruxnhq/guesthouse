//! A machine's remote view on the network: the program that carries it, and everything the app needs
//! to start it, find it, ask it things and end it.
//!
//! QEMU forwards the guest's remoteman port to a loopback port on this PC, which nothing else can reach.
//! `guesthouse-remote-view` stands between that and the network. It is a program of its own, like the
//! daemon and passt, so that other devices can reach a running machine when the app's window is closed.
//! It lives as long as the machine's QEMU does, and ends by itself once that has gone.
//!
//! What the app calls, and where:
//!
//! - [`start`] after QEMU is up, with [`Settings::for_machine`] for what to start it with, and
//!   [`end`] when the machine stops. Both are safe to call when the other has already happened.
//! - [`ask`] with a [`Request`] for the switch (`Open` and `Close`) and for the state (`RelayStatus`).
//!   While the program runs, the LAN side comes and goes with these; the loopback side is QEMU's.
//! - [`pid`] to tell whether it is running, as the app may have been closed and opened again since.
//!
//! The machine's side of it (the switch, remoteman's host in the guest, pairing and the devices) is
//! `machine.rs`, which the settings page and the start and stop paths call.

mod control;
mod lan;
mod machine;
mod mdns;
mod process;
mod settings;

pub use control::{
    Counters, RelayClient, RelayReply, RelayStatus, Request, ShareState, ask, listener, serve, socket_path,
};
pub use guesthouse_remote_view_core::announce::{
    Announcement, Announcer, BadAnnouncement, FINGERPRINT_DIGITS, SERVICE_TYPE, host_name, lan_interface,
};
pub use lan::Lan;
pub use machine::{
    HELD_FILE, TOO_OLD, allowed, bring_up, device_allow, device_drop, device_revoke, encoder,
    ensure_installed, info, installer, lan_address, launched, pair_accept, pair_close, pair_decline, port,
    quitting, reap, set_allowed, share, stopped,
};
pub use mdns::Mdns;
pub use process::{end, pid, start, write_pid_file};
pub use settings::{Settings, UsageError, Watched};

/// What `guesthouse-vm set` calls the switch.
pub const SETTING: &str = "remote-view";
/// The program's name, and what the app recognises it by in `/proc`.
pub const PROGRAM: &str = "guesthouse-remote-view";
/// In the run folder: the program's pid and its start time, as passt's pid file has them.
pub const PID_FILE: &str = "remote-view.pid";
/// In the run folder: where the app asks the program things.
pub const SOCKET: &str = "remote-view.sock";
pub use guesthouse_remote_view_core::share::{FIRST_PORT, LAST_PORT};
