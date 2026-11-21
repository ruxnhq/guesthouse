//! The documents HCS takes, as types. A field is named the way HCS spells it, as Microsoft's public
//! schema reference for HCS does, since HCS reads the names and refuses what it doesn't know.
//!
//! Only what Guesthouse uses is here. A field left as `None` stays out of the JSON, so HCS takes its
//! own default for it.

pub mod network;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The version of the schema these documents are written to. 2.1 is the first with everything a
/// UEFI VM with a TPM and GPU-PV needs, and every Windows 11 build takes it.
pub const SCHEMA_VERSION: Version = Version { major: 2, minor: 1 };

/// Firmware's own Secure Boot keys for Windows, which HCS knows by this ID.
pub const SECURE_BOOT_WINDOWS: &str = "1734c6e8-3154-4dda-ba5f-a874cc483422";

/// What HCS calls the place in a running machine where its GPU is set.
pub const GPU_RESOURCE_PATH: &str = "VirtualMachine/ComputeTopology/Gpu";

/// The share of a card a machine asks for in `GpuAssignment::List`. HCS takes it as the most a
/// partition may have, and this asks for all of it, as the card is shared by time anyway.
pub const WHOLE_CARD: u16 = u16::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

/// A whole compute system, as `HcsCreateComputeSystem` takes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ComputeSystem {
    pub owner: String,
    pub schema_version: Version,
    /// When the last handle to the machine closes, HCS stops it. That ties the machine's life to
    /// the process that made it, so a crash can't leave a VM running with no one to stop it.
    pub should_terminate_on_last_handle_closed: bool,
    pub virtual_machine: VirtualMachine,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct VirtualMachine {
    pub chipset: Chipset,
    pub compute_topology: ComputeTopology,
    pub devices: Devices,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_state: Option<GuestState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_settings: Option<SecuritySettings>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Chipset {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uefi: Option<Uefi>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Uefi {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub console: Option<UefiConsole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_secure_boot_template: Option<SecureBootTemplate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secure_boot_template_id: Option<String>,
}

/// Where the firmware writes. `Default` is the screen, which is also where a guest's own boot
/// manager draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UefiConsole {
    Default,
    ComPort1,
    ComPort2,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecureBootTemplate {
    /// Leave the firmware's keys as they are.
    Skip,
    /// Put the template's keys in, which turns Secure Boot on.
    Apply,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ComputeTopology {
    pub memory: Memory,
    pub processor: Processor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<Gpu>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Memory {
    #[serde(rename = "SizeInMB")]
    pub size_in_mb: u64,
    /// Backs the guest's memory with the host's pageable memory, so a guest that doesn't touch its
    /// memory doesn't take it from the host. Without it the whole size is locked at the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_overcommit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_deferred_commit: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Processor {
    pub count: u32,
}

/// The machine's share of a GPU, by paravirtualisation: the host's driver does the work, and the
/// guest's driver sends it through the host's graphics kernel.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Gpu {
    pub assignment_mode: GpuAssignment,
    /// For `GpuAssignment::List`: each card's partition adapter, by its device interface path, and
    /// the share of it asked for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assignment_request: BTreeMap<String, u16>,
    /// Lets the vendor's driver send its own private data across, which every vendor's driver
    /// needs to do anything beyond the basics.
    pub allow_vendor_extension: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GpuAssignment {
    #[default]
    Disabled,
    /// The card Windows picks. On a PC with more than one card that may not be the one wanted.
    Default,
    /// The cards in `Gpu::assignment_request`.
    List,
    /// Every card the host has.
    Mirror,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Devices {
    /// SCSI controllers by name, each with its disks by slot number.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scsi: BTreeMap<String, Scsi>,
    /// The machine's screen. HCS needs it even when nothing shows the screen: the worker process
    /// stops at the start without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_monitor: Option<VideoMonitor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<Empty>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse: Option<Empty>,
    /// Serial ports by number, from "0" for COM1.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub com_ports: BTreeMap<String, ComPort>,
    /// Network cards by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub network_adapters: BTreeMap<String, NetworkAdapter>,
}

/// A device with nothing to set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Scsi {
    pub attachments: BTreeMap<String, Attachment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Attachment {
    pub r#type: AttachmentType,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentType {
    VirtualDisk,
    Iso,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct VideoMonitor {
    pub horizontal_resolution: u16,
    pub vertical_resolution: u16,
}

/// A serial port, which HCS serves on a named pipe of the host for anyone to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ComPort {
    pub named_pipe: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkAdapter {
    /// The HCN endpoint the card is plugged into, as a GUID with no braces.
    pub endpoint_id: String,
    /// In the form 00-15-5D-12-34-56.
    pub mac_address: String,
}

/// The files that keep a machine's firmware settings and its TPM between runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GuestState {
    pub guest_state_file_path: String,
    pub runtime_state_file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SecuritySettings {
    pub enable_tpm: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Isolation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Isolation {
    pub isolation_type: IsolationType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsolationType {
    /// The guest's state files are kept from the host's other processes. A TPM needs this, and it
    /// needs no more than Virtual Machine Platform.
    GuestStateOnly,
}

/// A change to a running machine, as `HcsModifyComputeSystem` takes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ModifySettingRequest {
    pub resource_path: String,
    pub request_type: RequestType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestType {
    Add,
    Remove,
    Update,
}

impl ModifySettingRequest {
    /// Gives a running machine its GPU.
    pub fn gpu(gpu: &Gpu) -> Self {
        ModifySettingRequest {
            resource_path: GPU_RESOURCE_PATH.to_owned(),
            request_type: RequestType::Update,
            settings: Some(serde_json::to_value(gpu).expect("a GPU setting always serialises")),
        }
    }
}

/// What `HcsGetComputeSystemProperties` is asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PropertyQuery {
    pub property_types: Vec<String>,
}

impl PropertyQuery {
    /// The machine's state and IDs, which every machine has.
    pub fn basic() -> Self {
        PropertyQuery { property_types: Vec::new() }
    }
}

/// The part of a machine's properties Guesthouse reads.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Properties {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub runtime_id: String,
    #[serde(default)]
    pub stopped: bool,
    #[serde(default)]
    pub exit_type: String,
}

impl Properties {
    pub fn running(&self) -> bool {
        self.state == "Running"
    }
}

impl Gpu {
    /// Asks for the whole of each card, by its partition adapter's device interface path.
    pub fn list<'a>(interface_paths: impl IntoIterator<Item = &'a str>) -> Self {
        Gpu {
            assignment_mode: GpuAssignment::List,
            assignment_request: interface_paths
                .into_iter()
                .map(|path| (path.to_owned(), WHOLE_CARD))
                .collect(),
            allow_vendor_extension: true,
        }
    }
}

#[cfg(test)]
mod tests;
