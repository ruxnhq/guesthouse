//! What making and running a machine are given. The command line fills these from its options, and
//! the service from what an app window asks for.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which of the host's cards a machine is given.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuChoice {
    /// The card with the most memory of its own, which is the discrete one whoever makes it.
    #[default]
    Default,
    /// The card whose name or device ID holds this text, ignoring case.
    Matching(String),
    /// No card at all.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareSettings {
    /// The new disk, which must not be there yet.
    pub disk: PathBuf,
    /// A Windows ISO, or its install.wim or install.esd, or an Ubuntu Server ISO.
    pub image: PathBuf,
    /// The guest's administrator.
    pub user: String,
    /// The Windows edition in the image.
    pub edition: String,
    /// The disk's largest size.
    pub size_gb: u64,
    pub gpu: GpuChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootSettings {
    /// HCS's ID for the machine, which also names its serial port's pipe.
    pub name: String,
    pub disk: PathBuf,
    pub memory_mb: u64,
    pub processors: u32,
    pub tpm: bool,
    pub network: bool,
    /// Whether the guest's serial port is read in the jail, as it always should be.
    pub jail: bool,
    pub gpu: GpuChoice,
    /// The most the machine may run, after which it's asked to shut down.
    pub for_minutes: Option<u64>,
    /// Whether the machine is asked to shut down once the guest has reported.
    pub until_report: bool,
    /// Who may open the machine's console, as SIDs' text: the display HCS serves as Remote Desktop
    /// on the machine's console pipe. With nobody, it has none.
    #[serde(default)]
    pub console_access: Vec<String>,
}

impl PrepareSettings {
    pub fn new(disk: PathBuf, image: PathBuf) -> Self {
        PrepareSettings {
            disk,
            image,
            user: "guesthouse".into(),
            edition: "Windows 11 Pro".into(),
            size_gb: 128,
            gpu: GpuChoice::Default,
        }
    }
}

impl BootSettings {
    pub fn new(name: &str, disk: PathBuf) -> Self {
        BootSettings {
            name: name.to_owned(),
            disk,
            memory_mb: 8192,
            processors: 4,
            tpm: true,
            network: true,
            jail: true,
            gpu: GpuChoice::Default,
            for_minutes: None,
            until_report: false,
            console_access: Vec::new(),
        }
    }
}
