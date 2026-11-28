//! The DLLs the exe imports come from System32, never from the folder it is in (build.rs): its
//! load configuration says so (LOAD_LIBRARY_SEARCH_SYSTEM32 as its dependent load flags), and a
//! copy of the exe with a `bcryptprimitives.dll` beside it (an import that isn't one of Windows'
//! known DLLs) runs as it does anywhere else. The DLL beside it is a real one without the
//! function the exe imports, so had it been loaded, Windows would have stopped the process
//! before its first line (0xC0000139, an entry point not found).

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("guesthouse-setup-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_dll_beside_the_exe_is_not_loaded() {
    let dir = folder("imports");
    let exe = dir.join("guesthouse-setup.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_guesthouse-setup"), &exe).unwrap();
    let system32 = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
    std::fs::copy(system32.join("version.dll"), dir.join("bcryptprimitives.dll")).unwrap();

    // `--pack` with nothing else prints its usage and ends with 2, touching nothing.
    let status = Command::new(&exe).arg("--pack").current_dir(&dir).status().unwrap();
    let code = status.code().unwrap() as u32;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, 2, "the exe ended with {code:#x}, not its own 2: the DLL beside it was loaded");
}

/// The exe's dependent load flags, read from its load configuration directory.
fn dependent_load_flags(pe: &[u8]) -> u16 {
    let u16_at = |at: usize| u16::from_le_bytes(pe[at..at + 2].try_into().unwrap());
    let u32_at = |at: usize| u32::from_le_bytes(pe[at..at + 4].try_into().unwrap());
    let nt = u32_at(0x3c) as usize;
    assert_eq!(&pe[nt..nt + 4], b"PE\0\0");
    let sections = u16_at(nt + 6) as usize;
    let optional = nt + 24;
    let optional_len = u16_at(nt + 20) as usize;
    assert_eq!(u16_at(optional), 0x20b, "a 64-bit exe");
    // The data directories start at 112 in a PE32+ optional header; the load configuration's is
    // the 11th.
    let rva = u32_at(optional + 112 + 10 * 8) as usize;
    assert_ne!(rva, 0, "the exe has a load configuration");
    let table = optional + optional_len;
    let offset = (0..sections)
        .map(|i| table + i * 40)
        .find_map(|s| {
            let (virtual_size, virtual_address) = (u32_at(s + 8) as usize, u32_at(s + 12) as usize);
            let raw = u32_at(s + 20) as usize;
            (virtual_address..virtual_address + virtual_size)
                .contains(&rva)
                .then(|| raw + rva - virtual_address)
        })
        .expect("the load configuration is in a section");
    // IMAGE_LOAD_CONFIG_DIRECTORY64.DependentLoadFlags.
    assert!(u32_at(offset) as usize >= 0x50, "the load configuration has the field");
    u16_at(offset + 0x4e)
}

#[test]
fn imports_come_from_system32_only() {
    let pe = std::fs::read(env!("CARGO_BIN_EXE_guesthouse-setup")).unwrap();
    // LOAD_LIBRARY_SEARCH_SYSTEM32.
    assert_eq!(dependent_load_flags(&pe), 0x800);
}
