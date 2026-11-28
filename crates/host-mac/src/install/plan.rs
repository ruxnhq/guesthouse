// SPDX-License-Identifier: GPL-2.0-or-later
//! What an install is made of: the same five steps for a Windows machine and an Ubuntu machine,
//! with each step's words as the install screens say them.

use serde::{Deserialize, Serialize};

/// Which guest is being installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Guest {
    Windows,
    Ubuntu,
}

impl Guest {
    pub fn from_name(name: &str) -> Option<Guest> {
        match name {
            "windows" => Some(Guest::Windows),
            "ubuntu" => Some(Guest::Ubuntu),
            _ => None,
        }
    }

    /// What the guest is called on a screen.
    pub fn title(self) -> &'static str {
        match self {
            Guest::Windows => "Windows",
            Guest::Ubuntu => "Ubuntu",
        }
    }

    /// The `kind` the machine's file gets: what the store and the launcher call this guest.
    pub fn machine_kind(self) -> &'static str {
        match self {
            Guest::Windows => "windows",
            Guest::Ubuntu => "linux",
        }
    }
}

/// One thing an install does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Getting the installer: downloaded, or the file the person chose, and checked.
    Fetch,
    /// The machine's folder, its disk, its firmware and its settings.
    MakeDisk,
    /// The guest's own installer, run with no questions.
    InstallSystem,
    /// Our graphics driver, put in the guest.
    InstallDriver,
    /// The machine started on its own disk for the first time, to see it comes up.
    FirstStart,
}

impl Step {
    /// The steps in the order they are done.
    pub const ALL: [Step; 5] =
        [Step::Fetch, Step::MakeDisk, Step::InstallSystem, Step::InstallDriver, Step::FirstStart];

    /// The line for this step on the progress screen. Making the disk is part of installing the
    /// system on a screen, and has none of its own.
    pub fn label(self, guest: Guest) -> Option<String> {
        let title = guest.title();
        match self {
            Step::Fetch => Some(format!("Download {title}")),
            Step::MakeDisk => None,
            Step::InstallSystem => Some(format!("Install {title}")),
            Step::InstallDriver => Some("Install the Guesthouse graphics driver".to_string()),
            Step::FirstStart => Some("First start".to_string()),
        }
    }
}

/// The steps of an install, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub guest: Guest,
    pub steps: Vec<Step>,
}

impl Plan {
    /// Windows and Ubuntu are two plans over the same machine: the same steps, each done its own
    /// way by `steps`.
    pub fn for_guest(guest: Guest) -> Plan {
        Plan { guest, steps: Step::ALL.to_vec() }
    }
}

/// What an install writes on the Mac, in gigabytes. The disk is a sparse file: making it takes
/// no room, and it holds only what the guest writes, so the room an install needs is the
/// downloaded or chosen installer, what the installed system writes and the driver package
/// (under 0.1 GB), with a margin so that the Mac is not left full. The figures are those of a
/// fresh install of each, rounded up.
const WINDOWS_INSTALLER_GB: u64 = 6;
const WINDOWS_SYSTEM_GB: u64 = 25;
const UBUNTU_INSTALLER_GB: u64 = 3;
const UBUNTU_SYSTEM_GB: u64 = 12;
const INSTALL_MARGIN_GB: u64 = 5;

/// The room an install needs on the Mac, in gigabytes: the installer, what the system writes
/// (never more than the disk holds) and the margin. The app counts it the same way
/// (`InstallGuest.spaceNeededGB`) and says separately when the whole disk would not fit.
pub fn space_needed_gb(guest: Guest, disk_gb: u64) -> u64 {
    let (installer, system) = match guest {
        Guest::Windows => (WINDOWS_INSTALLER_GB, WINDOWS_SYSTEM_GB),
        Guest::Ubuntu => (UBUNTU_INSTALLER_GB, UBUNTU_SYSTEM_GB),
    };
    installer + system.min(disk_gb) + INSTALL_MARGIN_GB
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_guests_have_the_same_five_steps_in_order() {
        let windows = Plan::for_guest(Guest::Windows);
        assert_eq!(windows.steps, Plan::for_guest(Guest::Ubuntu).steps);
        assert_eq!(windows.steps.len(), 5);
        assert_eq!(windows.steps[0], Step::Fetch);
        assert_eq!(windows.steps[4], Step::FirstStart);
    }

    #[test]
    fn the_screens_words_are_the_designs() {
        assert_eq!(Step::Fetch.label(Guest::Windows).unwrap(), "Download Windows");
        assert_eq!(Step::InstallSystem.label(Guest::Ubuntu).unwrap(), "Install Ubuntu");
        assert_eq!(
            Step::InstallDriver.label(Guest::Ubuntu).unwrap(),
            "Install the Guesthouse graphics driver"
        );
        assert!(Step::MakeDisk.label(Guest::Windows).is_none());
    }

    #[test]
    fn an_install_needs_what_it_writes_and_not_the_disks_whole_size() {
        assert_eq!(space_needed_gb(Guest::Windows, 64), 36);
        assert_eq!(space_needed_gb(Guest::Ubuntu, 32), 20);
        // A disk bigger or smaller than the system changes it only when it is the smaller.
        assert_eq!(space_needed_gb(Guest::Windows, 256), 36);
        assert_eq!(space_needed_gb(Guest::Ubuntu, 8), 3 + 8 + 5);
    }
}
