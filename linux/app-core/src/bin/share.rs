//! `guesthouse-share`: QEMU starts this for each connection a guest makes to its shared folder, with
//! the connection on stdin and stdout, the folder in `SHARE`, its key in `SHARE_KEY` and whether the
//! guest may change it in `SHARE_ACCESS`.

#[cfg(target_os = "linux")]
#[path = "share/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-share runs on Linux only.");
    std::process::ExitCode::FAILURE
}
