// SPDX-License-Identifier: GPL-2.0-or-later
//! Adding, renaming and finding QEMU through the C functions, in a support folder of their own.
#![cfg(target_os = "macos")]

use guesthouse_core::machines_ffi::{
    guesthouse_machine_check, guesthouse_machine_create, guesthouse_machine_rename, guesthouse_tool_locate,
};
use guesthouse_core::{guesthouse_machine_list, guesthouse_string_free};
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char};
use std::fs;

/// Calls a C function that takes strings and returns JSON, and frees what it returned.
fn call(function: impl FnOnce() -> *mut c_char) -> Value {
    let text = function();
    assert!(!text.is_null());
    // SAFETY: the core returned a nul-terminated string, freed once below.
    let value = serde_json::from_str(unsafe { CStr::from_ptr(text) }.to_str().unwrap()).unwrap();
    unsafe { guesthouse_string_free(text) };
    value
}

fn c(text: &str) -> CString {
    CString::new(text).unwrap()
}

#[test]
fn a_machine_is_added_listed_renamed_and_the_search_for_qemu_answers() {
    let support = std::env::temp_dir().join(format!("gh-ffi-{}", std::process::id()));
    let _ = fs::remove_dir_all(&support);
    fs::create_dir_all(&support).unwrap();
    // SAFETY: this is the only test in this file, so nothing else reads the environment.
    unsafe { std::env::set_var("GUESTHOUSE_SUPPORT_DIR", &support) };
    for name in ["disk.img", "code.fd", "vars.fd"] {
        fs::write(support.join(name), name).unwrap();
    }
    let request = |name: &str| {
        c(&json!({
            "name": name, "cores": 2, "memory_mb": 4096,
            "disk": support.join("disk.img"), "firmware_code": support.join("code.fd"),
            "firmware_vars": support.join("vars.fd"),
            "host_memory_mb": 16384, "host_cores": 8,
        })
        .to_string())
    };

    let bad = call(|| unsafe {
        guesthouse_machine_check(
            c(&json!({
                "name": "", "cores": 2, "memory_mb": 4096, "disk": "/none", "firmware_code": "/none",
                "firmware_vars": "/none", "host_memory_mb": 16384, "host_cores": 8
            })
            .to_string())
            .as_ptr(),
        )
    });
    assert_eq!(bad["problems"].as_array().unwrap().len(), 4);

    let made = call(|| unsafe { guesthouse_machine_create(request("Test Machine").as_ptr()) });
    assert_eq!(made["id"], "test-machine");
    let again = call(|| unsafe { guesthouse_machine_create(request("Test Machine").as_ptr()) });
    assert!(again["problems"][0].as_str().unwrap().contains("already exists"));

    let renamed =
        call(|| unsafe { guesthouse_machine_rename(c("test-machine").as_ptr(), c("Other").as_ptr()) });
    assert_eq!(renamed["ok"], true);
    let listing = call(|| unsafe { guesthouse_machine_list(std::ptr::null()) });
    assert_eq!(listing["machines"][0]["name"], "Other");
    assert_eq!(listing["machines"][0]["startable"], true);

    // A QEMU named in the settings is the one in use.
    let qemu = call(|| unsafe {
        guesthouse_tool_locate(
            c("qemu").as_ptr(),
            c(&support.join("code.fd").display().to_string()).as_ptr(),
            std::ptr::null(),
        )
    });
    assert_eq!(qemu["source"], "setting");
    let _ = fs::remove_dir_all(&support);
}
