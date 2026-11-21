//! What a machine is given, and the HCS document that asks for it. Nothing here touches the host, so
//! it's tested on any system.

use guesthouse_hcs::schema::{
    Attachment, AttachmentType, Chipset, ComPort, ComputeSystem, ComputeTopology, Devices, Empty, GuestState,
    Isolation, IsolationType, Memory, NetworkAdapter, Processor, RdpConnectionOptions, SCHEMA_VERSION,
    SECURE_BOOT_WINDOWS, Scsi, SecureBootTemplate, SecuritySettings, Uefi, UefiConsole, VideoMonitor,
    VirtualMachine,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const OWNER: &str = "guesthouse";
/// The screen HCS needs a machine to have. Nothing shows it yet, so its size is the smallest
/// Windows is happy with.
const SCREEN: (u16, u16) = (1024, 768);
const SCSI_CONTROLLER: &str = "Primary";
const COM1: &str = "0";
const NETWORK_CARD: &str = "Default";

/// The system a machine runs, which decides its firmware's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Guest {
    #[default]
    Windows,
    Linux,
}

#[derive(Debug, Clone)]
pub struct Machine {
    /// HCS's ID for the machine, which also names its serial port's pipe.
    pub name: String,
    /// The boot disk, as an absolute path, since HCS resolves nothing.
    pub disk: PathBuf,
    pub memory_mb: u64,
    pub processors: u32,
    pub tpm: bool,
    pub network: Option<NetworkAdapter>,
    pub guest: Guest,
    /// Disks after the boot disk, in the slots after it, such as a Linux guest's files disk and,
    /// while it installs, its installer.
    pub more_disks: Vec<Attachment>,
    /// Who may open the machine's console, as SIDs' text. With nobody, it has no console.
    pub console_access: Vec<String>,
}

impl Machine {
    /// Where the machine's firmware settings and TPM are kept, beside its disk.
    pub fn guest_state_path(&self) -> PathBuf {
        self.disk.with_extension("vmgs")
    }

    pub fn runtime_state_path(&self) -> PathBuf {
        self.disk.with_extension("vmrs")
    }

    /// The host's end of the guest's COM1.
    pub fn serial_pipe(&self) -> String {
        format!(r"\\.\pipe\{}-com1", self.name)
    }

    /// The pipe HCS serves the machine's display on, as Remote Desktop: its console, which shows
    /// the firmware, the installer and a boot that's stuck as well as the desktop.
    pub fn console_pipe(&self) -> String {
        format!(r"\\.\pipe\{}-console", self.name)
    }

    pub fn document(&self) -> ComputeSystem {
        let boot_disk =
            Attachment { r#type: AttachmentType::VirtualDisk, path: text(&self.disk), read_only: None };
        let disks = std::iter::once(boot_disk)
            .chain(self.more_disks.iter().cloned())
            .enumerate()
            .map(|(slot, disk)| (slot.to_string(), disk))
            .collect();
        let windows = self.guest == Guest::Windows;
        ComputeSystem {
            owner: OWNER.to_owned(),
            schema_version: SCHEMA_VERSION,
            should_terminate_on_last_handle_closed: true,
            virtual_machine: VirtualMachine {
                chipset: Chipset {
                    // A Linux guest gets its GPU through a kernel module it builds itself, which no one
                    // signs, so its firmware is given no Secure Boot keys at all, as in the Linux
                    // document App Sandbox has booted.
                    uefi: Some(Uefi {
                        console: Some(UefiConsole::Default),
                        apply_secure_boot_template: windows.then_some(SecureBootTemplate::Apply),
                        secure_boot_template_id: windows.then(|| SECURE_BOOT_WINDOWS.to_owned()),
                    }),
                },
                compute_topology: ComputeTopology {
                    memory: Memory {
                        size_in_mb: self.memory_mb,
                        allow_overcommit: Some(true),
                        enable_deferred_commit: Some(true),
                    },
                    processor: Processor { count: self.processors },
                    // GPU-PV is given once the machine runs, as HCS takes it then.
                    gpu: None,
                },
                devices: Devices {
                    scsi: BTreeMap::from([(SCSI_CONTROLLER.to_owned(), Scsi { attachments: disks })]),
                    video_monitor: Some(VideoMonitor {
                        horizontal_resolution: SCREEN.0,
                        vertical_resolution: SCREEN.1,
                        connection_options: (!self.console_access.is_empty()).then(|| RdpConnectionOptions {
                            access_sids: self.console_access.clone(),
                            named_pipe: self.console_pipe(),
                        }),
                    }),
                    keyboard: Some(Empty {}),
                    mouse: Some(Empty {}),
                    com_ports: BTreeMap::from([(
                        COM1.to_owned(),
                        ComPort { named_pipe: self.serial_pipe() },
                    )]),
                    network_adapters: self
                        .network
                        .iter()
                        .map(|card| (NETWORK_CARD.to_owned(), card.clone()))
                        .collect(),
                },
                guest_state: Some(GuestState {
                    guest_state_file_path: text(&self.guest_state_path()),
                    runtime_state_file_path: text(&self.runtime_state_path()),
                }),
                // Only Windows asks for a TPM.
                security_settings: (self.tpm && windows).then_some(SecuritySettings {
                    enable_tpm: true,
                    isolation: Some(Isolation { isolation_type: IsolationType::GuestStateOnly }),
                }),
            },
        }
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn machine() -> Machine {
        Machine {
            name: "guesthouse-test".into(),
            disk: PathBuf::from(r"D:\vms\test.vhdx"),
            memory_mb: 8192,
            processors: 4,
            tpm: true,
            network: None,
            guest: Guest::Windows,
            more_disks: Vec::new(),
            console_access: Vec::new(),
        }
    }

    #[test]
    fn a_console_is_served_only_to_whom_it_names() {
        let named = Machine { console_access: vec!["S-1-5-21-1-2-3-1001".into()], ..machine() };
        let document = serde_json::to_value(named.document()).unwrap();
        assert_eq!(
            document["VirtualMachine"]["Devices"]["VideoMonitor"]["ConnectionOptions"],
            json!({"AccessSids": ["S-1-5-21-1-2-3-1001"], "NamedPipe": r"\\.\pipe\guesthouse-test-console"})
        );
        let none = serde_json::to_value(machine().document()).unwrap();
        assert!(none["VirtualMachine"]["Devices"]["VideoMonitor"].get("ConnectionOptions").is_none());
    }

    fn linux() -> Machine {
        let disk = |kind, path: &str| Attachment { r#type: kind, path: path.into(), read_only: None };
        let files = disk(AttachmentType::VirtualDisk, r"D:\vms\test.files.vhdx");
        let installer = disk(AttachmentType::Iso, r"D:\iso\ubuntu.iso");
        Machine { guest: Guest::Linux, more_disks: vec![files, installer], ..machine() }
    }

    #[test]
    fn a_linux_guest_has_no_secure_boot_and_no_tpm() {
        let document = serde_json::to_value(linux().document()).unwrap();
        let vm = &document["VirtualMachine"];
        assert_eq!(vm["Chipset"]["Uefi"], json!({"Console": "Default"}));
        assert!(vm.get("SecuritySettings").is_none());
        assert!(vm.get("GuestState").is_some(), "its firmware still keeps its boot entries");
    }

    #[test]
    fn more_disks_take_the_slots_after_the_boot_disk() {
        let document = serde_json::to_value(linux().document()).unwrap();
        let attachments = &document["VirtualMachine"]["Devices"]["Scsi"]["Primary"]["Attachments"];
        assert_eq!(attachments["0"]["Path"], json!(r"D:\vms\test.vhdx"));
        assert_eq!(attachments["1"], json!({"Type": "VirtualDisk", "Path": r"D:\vms\test.files.vhdx"}));
        assert_eq!(attachments["2"], json!({"Type": "Iso", "Path": r"D:\iso\ubuntu.iso"}));
    }

    #[test]
    fn the_state_files_sit_beside_the_disk() {
        let document = serde_json::to_value(machine().document()).unwrap();
        assert_eq!(
            document["VirtualMachine"]["GuestState"],
            json!({
                "GuestStateFilePath": r"D:\vms\test.vmgs",
                "RuntimeStateFilePath": r"D:\vms\test.vmrs",
            })
        );
    }

    #[test]
    fn the_machine_boots_its_disk_with_secure_boot_and_a_screen() {
        let document = serde_json::to_value(machine().document()).unwrap();
        let vm = &document["VirtualMachine"];
        assert_eq!(vm["Devices"]["Scsi"]["Primary"]["Attachments"]["0"]["Path"], json!(r"D:\vms\test.vhdx"));
        assert_eq!(vm["Chipset"]["Uefi"]["ApplySecureBootTemplate"], json!("Apply"));
        assert_eq!(
            vm["Devices"]["VideoMonitor"],
            json!({"HorizontalResolution": 1024, "VerticalResolution": 768})
        );
        assert_eq!(vm["Devices"]["ComPorts"]["0"]["NamedPipe"], json!(r"\\.\pipe\guesthouse-test-com1"));
        assert_eq!(vm["SecuritySettings"]["EnableTpm"], json!(true));
    }

    #[test]
    fn no_gpu_is_asked_for_at_creation() {
        let document = serde_json::to_value(machine().document()).unwrap();
        assert!(document["VirtualMachine"]["ComputeTopology"].get("Gpu").is_none());
    }

    #[test]
    fn without_a_tpm_or_network_neither_is_in_the_document() {
        let document = serde_json::to_value(Machine { tpm: false, ..machine() }.document()).unwrap();
        assert!(document["VirtualMachine"].get("SecuritySettings").is_none());
        assert!(document["VirtualMachine"]["Devices"].get("NetworkAdapters").is_none());
    }

    #[test]
    fn a_network_card_is_plugged_into_its_endpoint() {
        let card = NetworkAdapter { endpoint_id: "e".into(), mac_address: "00-15-5D-00-00-01".into() };
        let document = serde_json::to_value(Machine { network: Some(card), ..machine() }.document()).unwrap();
        assert_eq!(
            document["VirtualMachine"]["Devices"]["NetworkAdapters"]["Default"],
            json!({"EndpointId": "e", "MacAddress": "00-15-5D-00-00-01"})
        );
    }
}
