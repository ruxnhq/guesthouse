//! What a machine is given, and the HCS document that asks for it. Nothing here touches the host, so
//! it's tested on any system.

use guesthouse_hcs::schema::{
    Attachment, AttachmentType, Chipset, ComPort, ComputeSystem, ComputeTopology, Devices, Empty, GuestState,
    Isolation, IsolationType, Memory, NetworkAdapter, Processor, SCHEMA_VERSION, SECURE_BOOT_WINDOWS, Scsi,
    SecureBootTemplate, SecuritySettings, Uefi, UefiConsole, VideoMonitor, VirtualMachine,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const OWNER: &str = "guesthouse";
/// The screen HCS needs a machine to have. Nothing shows it yet, so its size is the smallest
/// Windows is happy with.
const SCREEN: VideoMonitor = VideoMonitor { horizontal_resolution: 1024, vertical_resolution: 768 };
const SCSI_CONTROLLER: &str = "Primary";
const BOOT_DISK_SLOT: &str = "0";
const COM1: &str = "0";
const NETWORK_CARD: &str = "Default";

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

    pub fn document(&self) -> ComputeSystem {
        let boot_disk =
            Attachment { r#type: AttachmentType::VirtualDisk, path: text(&self.disk), read_only: None };
        ComputeSystem {
            owner: OWNER.to_owned(),
            schema_version: SCHEMA_VERSION,
            should_terminate_on_last_handle_closed: true,
            virtual_machine: VirtualMachine {
                chipset: Chipset {
                    uefi: Some(Uefi {
                        console: Some(UefiConsole::Default),
                        apply_secure_boot_template: Some(SecureBootTemplate::Apply),
                        secure_boot_template_id: Some(SECURE_BOOT_WINDOWS.to_owned()),
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
                    scsi: BTreeMap::from([(
                        SCSI_CONTROLLER.to_owned(),
                        Scsi { attachments: BTreeMap::from([(BOOT_DISK_SLOT.to_owned(), boot_disk)]) },
                    )]),
                    video_monitor: Some(SCREEN),
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
                security_settings: self.tpm.then_some(SecuritySettings {
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
        }
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
