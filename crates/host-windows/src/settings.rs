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

/// A folder of the host's the guest sees, under the name `share_name(slot)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub folder: PathBuf,
    pub read_only: bool,
    /// Its place in the machine's settings, from 0, which names it. A folder left out of a boot,
    /// as it was refused, leaves its place empty, so the ones after keep their names.
    #[serde(default)]
    pub slot: usize,
}

impl Share {
    /// What the folder is called, to HCS and in the guest.
    pub fn name(&self) -> String {
        share_name(self.slot)
    }
}

/// What the folder in a machine's settings at `slot` is called, to HCS and in the guest: `shared`
/// for the first, as when there could be only one, then `shared2` and on.
pub fn share_name(slot: usize) -> String {
    match slot {
        0 => "shared".to_owned(),
        _ => format!("shared{}", slot + 1),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareSettings {
    /// The new disk, which must not be there yet.
    pub disk: PathBuf,
    /// A Windows ISO, or its install.wim or install.esd, or an Ubuntu Server ISO.
    pub image: PathBuf,
    /// The guest's administrator.
    pub user: String,
    /// The Windows edition in the image, by name. Empty takes Pro if the image has it, and its
    /// first edition if not.
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
    /// Whether the guest may run virtual machines of its own: the processor's
    /// `ExposeVirtualizationExtensions`.
    #[serde(default)]
    pub nested_virtualization: bool,
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
    /// Whether an NVIDIA guest gets Guesthouse's NVAPI wrapper in System32 in place of NVIDIA's NVAPI.
    /// DLSS needs it; some games' anti-cheat refuses it, as it isn't signed. Off, NVIDIA's goes back.
    #[serde(default = "on")]
    pub nvapi_wrapper: bool,
    /// The folders of the host's the guest sees, by slot.
    #[serde(default)]
    pub shares: Vec<Share>,
    /// The physical adapter the machine is bridged onto, by name, when it joins the host's own
    /// network and doesn't take Guesthouse's shared one.
    #[serde(default)]
    pub bridge: Option<String>,
    /// The port on the host's loopback that reaches an Ubuntu guest's SSH over a Hyper-V socket
    /// while it runs. None has no such port.
    #[serde(default)]
    pub ssh_port: Option<u16>,
}

fn on() -> bool {
    true
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
            nested_virtualization: false,
            network: true,
            jail: true,
            gpu: GpuChoice::Default,
            for_minutes: None,
            until_report: false,
            console_access: Vec::new(),
            nvapi_wrapper: true,
            shares: Vec::new(),
            bridge: None,
            ssh_port: None,
        }
    }
}
