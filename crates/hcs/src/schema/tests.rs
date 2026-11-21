use super::network::{Endpoint, NETWORK_SCHEMA_VERSION, Network};
use super::*;
use serde_json::json;

fn machine() -> ComputeSystem {
    ComputeSystem {
        owner: "guesthouse".into(),
        schema_version: SCHEMA_VERSION,
        should_terminate_on_last_handle_closed: true,
        virtual_machine: VirtualMachine {
            chipset: Chipset {
                uefi: Some(Uefi {
                    console: Some(UefiConsole::Default),
                    apply_secure_boot_template: Some(SecureBootTemplate::Apply),
                    secure_boot_template_id: Some(SECURE_BOOT_WINDOWS.into()),
                }),
            },
            compute_topology: ComputeTopology {
                memory: Memory {
                    size_in_mb: 8192,
                    allow_overcommit: Some(true),
                    enable_deferred_commit: None,
                },
                processor: Processor { count: 4 },
                gpu: None,
            },
            devices: Devices {
                scsi: [(
                    "Primary".to_owned(),
                    Scsi {
                        attachments: [(
                            "0".to_owned(),
                            Attachment {
                                r#type: AttachmentType::VirtualDisk,
                                path: r"D:\vm\a.vhdx".into(),
                                read_only: None,
                            },
                        )]
                        .into(),
                    },
                )]
                .into(),
                video_monitor: Some(VideoMonitor { horizontal_resolution: 1024, vertical_resolution: 768 }),
                keyboard: Some(Empty {}),
                mouse: Some(Empty {}),
                com_ports: [("0".to_owned(), ComPort { named_pipe: r"\\.\pipe\a-com1".into() })].into(),
                network_adapters: BTreeMap::new(),
            },
            guest_state: None,
            security_settings: Some(SecuritySettings {
                enable_tpm: true,
                isolation: Some(Isolation { isolation_type: IsolationType::GuestStateOnly }),
            }),
        },
    }
}

#[test]
fn fields_are_spelled_the_way_hcs_reads_them() {
    let document = serde_json::to_value(machine()).unwrap();
    assert_eq!(document["SchemaVersion"], json!({"Major": 2, "Minor": 1}));
    assert_eq!(document["ShouldTerminateOnLastHandleClosed"], json!(true));
    let vm = &document["VirtualMachine"];
    assert_eq!(vm["Chipset"]["Uefi"]["ApplySecureBootTemplate"], json!("Apply"));
    assert_eq!(vm["Chipset"]["Uefi"]["SecureBootTemplateId"], json!(SECURE_BOOT_WINDOWS));
    assert_eq!(vm["ComputeTopology"]["Memory"]["SizeInMB"], json!(8192));
    assert_eq!(vm["ComputeTopology"]["Processor"]["Count"], json!(4));
    assert_eq!(
        vm["Devices"]["Scsi"]["Primary"]["Attachments"]["0"],
        json!({"Type": "VirtualDisk", "Path": r"D:\vm\a.vhdx"})
    );
    assert_eq!(vm["Devices"]["ComPorts"]["0"]["NamedPipe"], json!(r"\\.\pipe\a-com1"));
    assert_eq!(vm["Devices"]["Keyboard"], json!({}));
    assert_eq!(vm["SecuritySettings"]["Isolation"]["IsolationType"], json!("GuestStateOnly"));
}

#[test]
fn what_is_left_unset_stays_out_of_the_document() {
    let document = serde_json::to_value(machine()).unwrap();
    let vm = &document["VirtualMachine"];
    assert!(vm.get("GuestState").is_none());
    assert!(vm["ComputeTopology"].get("Gpu").is_none());
    assert!(vm["ComputeTopology"]["Memory"].get("EnableDeferredCommit").is_none());
    assert!(vm["Devices"].get("NetworkAdapters").is_none());
    assert!(vm["Devices"]["Scsi"]["Primary"]["Attachments"]["0"].get("ReadOnly").is_none());
}

#[test]
fn a_document_reads_back_as_it_was_written() {
    let written = serde_json::to_string(&machine()).unwrap();
    let read: ComputeSystem = serde_json::from_str(&written).unwrap();
    assert_eq!(read, machine());
}

#[test]
fn the_gpu_change_names_each_card_by_its_interface_path() {
    let path = r"\\?\PCI#VEN_10DE&DEV_0000#0#{064092b3-625e-43bf-9eb5-dc845897dd59}";
    let request = serde_json::to_value(ModifySettingRequest::gpu(&Gpu::list([path]))).unwrap();
    assert_eq!(
        request,
        json!({
            "ResourcePath": "VirtualMachine/ComputeTopology/Gpu",
            "RequestType": "Update",
            "Settings": {
                "AssignmentMode": "List",
                "AssignmentRequest": {path: 65535},
                "AllowVendorExtension": true,
            },
        })
    );
}

#[test]
fn the_default_card_is_asked_for_with_no_list() {
    let gpu = Gpu { assignment_mode: GpuAssignment::Default, allow_vendor_extension: true, ..Gpu::default() };
    assert_eq!(
        serde_json::to_value(gpu).unwrap(),
        json!({"AssignmentMode": "Default", "AllowVendorExtension": true})
    );
}

#[test]
fn properties_read_the_state_and_ignore_the_rest() {
    let answer = r#"{"Id":"a","SystemType":"VirtualMachine","Name":"a","Owner":"guesthouse",
        "RuntimeId":"6a7f3c3e-0000-0000-0000-000000000000","State":"Running","Stopped":false}"#;
    let properties: Properties = serde_json::from_str(answer).unwrap();
    assert!(properties.running() && !properties.stopped());
    assert_eq!(properties.runtime_id, "6a7f3c3e-0000-0000-0000-000000000000");
    let stopped: Properties = serde_json::from_str(r#"{"State":"Stopped","Stopped":true}"#).unwrap();
    assert!(!stopped.running() && stopped.stopped());
    let silent: Properties = serde_json::from_str(r#"{"Id":"a"}"#).unwrap();
    assert!(!silent.stopped());
}

#[test]
fn the_network_documents_use_hcn_names() {
    assert_eq!(
        serde_json::to_value(Network::ics("guesthouse")).unwrap(),
        json!({"SchemaVersion": {"Major": 2, "Minor": 0}, "Name": "guesthouse", "Type": "ICS"})
    );
    let endpoint = Endpoint {
        schema_version: NETWORK_SCHEMA_VERSION,
        host_compute_network: "3b8f2a10-0000-0000-0000-000000000000".into(),
        mac_address: Some("00-15-5D-00-00-01".into()),
    };
    assert_eq!(
        serde_json::to_value(endpoint).unwrap(),
        json!({
            "SchemaVersion": {"Major": 2, "Minor": 0},
            "HostComputeNetwork": "3b8f2a10-0000-0000-0000-000000000000",
            "MacAddress": "00-15-5D-00-00-01",
        })
    );
}

#[test]
fn a_query_for_basic_properties_names_them() {
    assert_eq!(serde_json::to_value(PropertyQuery::basic()).unwrap(), json!({"PropertyTypes": ["Basic"]}));
}
