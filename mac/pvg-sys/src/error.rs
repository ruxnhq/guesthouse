// SPDX-License-Identifier: GPL-2.0-or-later
use std::fmt;

/// What went wrong in making or using the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// This macOS build has no profile, so the framework was not called.
    UnknownBuild(String),
    /// The ranges of guest memory can't be used as they were given.
    Memory(String),
    /// The framework made no device, or refused a call that a guest has no part in.
    Framework(String),
    /// A hand-over failed in the device, which leaves its display in a state that a reset doesn't
    /// survive. See [`crate::Device::spoiled`].
    Spoiled,
    /// The guard won't let this through as the guest's memory is now.
    Refused(crate::guard::Refusal),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBuild(build) => write!(f, "macOS build {build} has no profile"),
            Self::Memory(why) | Self::Framework(why) => f.write_str(why),
            Self::Spoiled => f.write_str("a hand-over failed in the device before, and it can't be reset"),
            Self::Refused(refusal) => write!(f, "{refusal}"),
        }
    }
}

impl std::error::Error for Error {}
