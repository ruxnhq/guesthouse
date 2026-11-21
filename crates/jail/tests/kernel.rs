//! The jail on this PC's kernel. Each case runs in a child of its own, as a jail can't be undone,
//! and the child's exit says whether everything it tried went as it should.

#[cfg(target_os = "linux")]
#[path = "kernel/linux.rs"]
mod linux;
#[cfg(windows)]
#[path = "kernel/windows.rs"]
mod windows;

fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(windows)]
    windows::main();
}
