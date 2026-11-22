//! On Windows: the exe's icon and its name, and the Windows App Runtime staged beside the exe.
//! `windows-reactor-setup` copies the runtime's DLLs and resources into the profile's output
//! folder, such as `target\release`, and embeds a manifest that finds them there, so the app needs
//! nothing installed. Elsewhere the crate is a stub and this does nothing.
//!
//! The build dependencies are Windows-only, so the code that uses them is compiled only when the
//! build itself runs on Windows, as a build script is compiled for the machine that builds.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/guesthouse.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
        windows_only();
    }
}

#[cfg(windows)]
fn windows_only() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let icon = std::path::Path::new(&manifest_dir).join("assets/guesthouse.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_str().expect("the icon's path is text"));
    res.set("ProductName", "Guesthouse");
    res.set("FileDescription", "Guesthouse");
    res.set("CompanyName", "RUXN LLC");
    res.set("LegalCopyright", "Copyright (C) 2026 RUXN LLC and contributors");
    res.set("OriginalFilename", "guesthouse-app.exe");
    res.set("InternalName", "guesthouse-app");
    res.compile().expect("embed the app's icon");
    // The first build on a PC downloads the runtime from NuGet into
    // %LOCALAPPDATA%\windows-reactor-setup, and later builds use that copy.
    windows_reactor_setup::as_self_contained();
}

/// A build for Windows from another system has neither tool, so its exe has no icon and needs the
/// Windows App Runtime installed.
#[cfg(not(windows))]
fn windows_only() {}
