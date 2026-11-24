//! Guest machines on a Windows host: making a machine's disks from a Windows or Ubuntu ISO, and
//! running it through HCS with the host's card by GPU-PV. The work is the same whoever asks for it,
//! `guesthouse-hcs-boot` on the command line or the Guesthouse service for the app, so it says what
//! it's doing through an [`events::Sink`] and is told what to do by [`settings`].
//!
//! What a machine is and what goes on its disks builds on every system, so its tests run anywhere.
//! [`windows`], the work itself, is Windows only.

pub mod events;
pub mod guest;
pub mod machine;
pub mod printable;
pub mod record;
pub mod screen;
pub mod settings;
pub mod snapshots;
pub mod wim_info;
#[cfg(windows)]
pub mod windows;
