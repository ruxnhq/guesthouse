// SPDX-License-Identifier: GPL-2.0-or-later
//! Apple's paravirtualized graphics device on a macOS host, as calls a program may make.
//!
//! macOS has a framework that plays the host's half of a graphics device for a guest: registers,
//! a ring of commands in the guest's memory, a display, and a second small device that turns
//! pages of the guest's memory into surfaces a display command can show. This crate is the one
//! place that calls it. A program that owns a guest's graphics makes a [`Device`] over the guest's
//! memory, forwards the guest's register reads and writes to it, and hears of interrupts and new
//! pictures.
//!
//! The framework's device was written for Apple's own guest driver and trusts it in places: a few
//! things a guest can say end the host's process. So nothing a guest says reaches the framework
//! unread. The [`Guard`] reads it first.
//!
//! What a guest sees of the device is not a published interface and moves between macOS builds.
//! So each build has a [`Profile`], and a build with no profile is refused.
//!
//! - `profile`: one build's registers, commands and limits, as far as the guard reads them.
//! - `memory`: [`GuestMemory`], the guest's memory as ranges of this process's own.
//! - `guard`: [`Guard`], which judges every word a guest says. It needs no framework and builds
//!   on any Unix, as its tests do.
//! - `framework`: the calls into Apple's framework, through an Objective-C file that `build.rs`
//!   compiles. macOS only.
//! - `device`: [`Device`], the guard and the framework together, which is what a host program
//!   uses. macOS only.
//!
//! The guest's memory is mappings made with `mmap`, so the crate builds on Unix only, and on
//! Windows it is empty, as the Mac host's core is off macOS.
#![cfg(unix)]

#[cfg(target_os = "macos")]
mod device;
mod error;
#[cfg(target_os = "macos")]
mod framework;
pub mod guard;
pub mod memory;
pub mod profile;

#[cfg(target_os = "macos")]
pub use device::{Config, Device, Event, Frame, Mode, Source};
pub use error::Error;
#[cfg(target_os = "macos")]
pub use framework::{FrameSource, ModesDone, display_look, draw_step, host_build, option_rom, watch_reasons};
pub use guard::{Glyph, Guard, Refusal};
pub use memory::{GuestMemory, Range};
pub use profile::Profile;
