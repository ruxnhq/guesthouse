use super::switch::*;
use crate::machine::{Kind, Machine};
use crate::testing::{Scratch, scratch_context};
use std::fs;

fn machine(context: &crate::context::Context, kind: Kind) -> Machine {
    let machine = Machine { kind, ..Machine::new("work", "Work", "/d", "/v") };
    fs::create_dir_all(machine.folder(context)).unwrap();
    machine.save(context).unwrap();
    machine
}

#[test]
fn it_is_off_for_a_machine_that_has_no_file() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let windows = machine(&context, Kind::Windows);
    assert!(!is_on(&context, &windows));
}

#[test]
fn it_is_kept_beside_the_machines_settings_and_off_leaves_no_file() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let windows = machine(&context, Kind::Windows);
    let before = fs::read_to_string(windows.folder(&context).join("machine.json")).unwrap();
    set_on(&context, &windows, true).unwrap();
    assert!(is_on(&context, &windows));
    assert!(windows.folder(&context).join(SWITCH_FILE).is_file());
    assert_eq!(fs::read_to_string(windows.folder(&context).join("machine.json")).unwrap(), before);
    set_on(&context, &windows, false).unwrap();
    assert!(!is_on(&context, &windows));
    assert!(!windows.folder(&context).join(SWITCH_FILE).exists());
    set_on(&context, &windows, false).unwrap();
}

#[test]
fn a_file_that_cannot_be_read_is_off() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let windows = machine(&context, Kind::Windows);
    for text in ["", "{", "{\"on\":\"yes\"}", "{\"on\":true,\"more\":1}"] {
        fs::write(windows.folder(&context).join(SWITCH_FILE), text).unwrap();
        assert!(!is_on(&context, &windows), "{text:?}");
    }
}

#[test]
fn only_a_windows_machine_has_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let plain = machine(&context, Kind::Plain);
    let refused = set_on(&context, &plain, true).unwrap_err();
    assert_eq!(refused.0, "app-windows is only for a Windows machine with GPU-PV.");
    fs::write(plain.folder(&context).join(SWITCH_FILE), "{\"on\":true}").unwrap();
    assert!(!is_on(&context, &plain), "whatever a file says");
}
