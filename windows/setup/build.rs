//! On Windows: the exe's icon and version information, and the Windows App Runtime staged beside
//! the exe (`windows-reactor-setup` copies it into the profile's output folder, `target\release`,
//! and embeds the manifest that finds it beside the exe), which scripts/windows-setup.ps1 then
//! packs into the installer. Elsewhere the crate is a stub and this does nothing.
//!
//! The build dependencies are Windows-only, so the code that uses them is compiled only when the
//! build itself runs on Windows (a build script is compiled for the machine that builds).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/setup.ico");
    // The DLLs the exe imports come from System32 only, never from the folder it is in
    // (LOAD_LIBRARY_SEARCH_SYSTEM32 as its dependent load flags): the elevated part is this same
    // file run from where it was downloaded, and its imports are found before any of its code
    // runs. What it loads by name later comes from System32 too (src/system.rs,
    // `safe_dll_search`). The release is built with MSVC's linker, the one that takes this.
    if std::env::var("CARGO_CFG_TARGET_ENV").ok().as_deref() == Some("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
        windows_only();
    }
}

#[cfg(windows)]
fn windows_only() {
    let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/setup.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_str().expect("icon path"));
    // The UAC prompt names the program by its description.
    res.set("ProductName", "Guesthouse");
    res.set("FileDescription", "Guesthouse setup");
    res.set("CompanyName", "RUXN LLC");
    res.set("LegalCopyright", "Copyright (C) 2026 RUXN LLC and contributors");
    res.set("OriginalFilename", "guesthouse-setup.exe");
    res.set("InternalName", "guesthouse-setup");
    res.compile().expect("embed the installer's icon");
    // The first build on a PC downloads the runtime from NuGet into %LOCALAPPDATA%\
    // windows-reactor-setup; later builds use that copy.
    windows_reactor_setup::as_self_contained();
}
