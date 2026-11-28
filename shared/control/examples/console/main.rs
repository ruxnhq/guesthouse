//! Opens a machine's console, the display HCS serves as Remote Desktop on a pipe, in Windows' own
//! Remote Desktop client, which can't open a pipe: this listens on a port of this PC's loopback only,
//! carries each connection to the pipe and back, and starts the client on that port.
//!
//!     cargo run -p guesthouse-control --example console -- NAME [VM-ID]
//!
//! With VM-ID, the relay sends Remote Desktop's pre-connection message naming it ahead of what the
//! client sends, as Hyper-V's own viewer does, as the console waits for it.
//! For trying the console before the app has a button for it.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    windows::main()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("The console runs on a Windows host only.");
    std::process::ExitCode::FAILURE
}
