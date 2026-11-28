//! What the tests share: scratch folders, and contexts that can't reach the real PC.

use crate::context::Context;
use crate::firmware::Firmware;
use crate::guest_end::GuestEnd;
use crate::machine::Machine;
use crate::monitor::MAIN_SOCKET;
use crate::paths::{Paths, Variables};
use guesthouse_basics::capped_line::read_capped_line;
use guesthouse_daemon::slot::Slot;
use std::fs;
use std::io::BufReader;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

/// So that no test depends on how much this PC has free. One about the limit sets its own.
const PLENTY_OF_GIB: u64 = 64;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A folder that goes when the test ends.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new() -> Self {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let folder = std::env::temp_dir().join(format!("guesthouse-test-{}-{number}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        Self(folder)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    pub fn root(&self) -> &Path {
        &self.0
    }

    /// Writes a program that can be run. A child writes it and not this process: a file this process
    /// has open for writing can be held open for a moment by a fork in another test, and then a run of
    /// it fails.
    pub fn program(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut writer = Command::new("sh")
            .args(["-c", "cat > \"$0\" && chmod 755 \"$0\""])
            .arg(&path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        assert!(writer.wait().unwrap().success());
        path
    }

    /// Writes a file, making its folder.
    pub fn write(&self, name: &str, data: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
        path
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Paths that all lie in the scratch folder, so that no test can touch a real machine, a real run
/// folder or real shared memory.
pub fn scratch_paths(scratch: &Scratch) -> Paths {
    let folder = |name: &str| scratch.path(name).to_str().unwrap().to_owned();
    let (home, shm, program) = (folder("home"), folder("shm"), folder("program"));
    // As a real program folder does, this means GPU machines are possible.
    fs::create_dir_all(scratch.path("program/guest")).unwrap();
    let runtime = folder("xdg-run");
    let mut paths = Paths::new(
        &Variables::of(&[
            ("HOME", &home),
            ("XDG_RUNTIME_DIR", &runtime),
            ("GUESTHOUSE_SHARED_MEMORY", &shm),
            ("GUESTHOUSE_PROGRAM", &program),
        ]),
        None,
    );
    paths.firmware = Firmware {
        code: scratch.path("firmware/code"),
        vars: scratch.write("firmware/vars", "blank firmware settings"),
        code_secure: scratch.path("firmware/code-secure.qcow2"),
        vars_secure: scratch.write("firmware/vars-secure", "firmware settings with keys"),
    };
    fs::create_dir_all(&paths.shared_memory).unwrap();
    paths
}

pub fn scratch_context(scratch: &Scratch) -> Context {
    let mut context = Context::new(scratch_paths(scratch), crate::slots::first());
    context.processes = scratch.path("proc");
    context.memory_holders = context.processes.clone();
    fs::create_dir_all(&context.processes).unwrap();
    // Nor does it have drives, or use any, unless a test says so.
    context.block_devices = scratch.path("block");
    context.drive_ids = scratch.path("drive-ids");
    context.mount_table = scratch.path("mountinfo");
    context.swaps = scratch.path("swaps");
    context.udev_data = scratch.path("udev");
    // A test's PC has no card unless it makes one, whatever cards this PC has.
    context.drm_class = scratch.path("drm");
    context.pci_devices = scratch.path("pci");
    context.pci_names = scratch.path("pci.ids");
    context.reserve_memory = crate::guest_memory::GuestMemory::sized;
    // A test's pretend programs are shell scripts, which no jail would let run as they do.
    context.jail_offer = |_| crate::jailing::Offer::nothing();
    context.ram_limit = Some(PLENTY_OF_GIB);
    // Nor is it short of RAM, however little this PC has free.
    let plenty_kib = PLENTY_OF_GIB << 20;
    context.meminfo =
        scratch.write("meminfo", format!("MemTotal: {plenty_kib} kB\nMemAvailable: {plenty_kib} kB\n"));
    // Nor does it lack the jail, its cards' devices or udmabuf's room, whatever this PC has.
    context.jail_support = || guesthouse_jail::Support { landlock_abi: 10, seccomp: true };
    context.may_open = |_| true;
    context.card_memories = String::new;
    context.security_modules = scratch.write("lsm", "capability,landlock");
    context.apparmor = scratch.path("apparmor.d");
    context.udmabuf_limit = scratch.path("udmabuf-limit");
    context
}

/// This test process as the slot's QEMU of `machine`, while this is kept: it answers on the slot's
/// monitor, where it says it forwards each guest port of `forwards` from the loopback port beside it,
/// its words open the machine's disk, and the context's `/proc` shows its open files and this PC's
/// sockets as they are.
pub struct ActingQemu {
    monitor: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Drop for ActingQemu {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the monitor's thread, which then lets go of the socket.
        let _ = UnixStream::connect(&self.monitor);
    }
}

/// QEMU's table of its user network's connections, with a line for each forward. QEMU lines its
/// columns up with more spaces than one.
fn usernet_table(tcp: &[(u16, u16)], udp: &[(u16, u16)]) -> String {
    let mut table = "Protocol[State] FD Source-Address Port Dest.-Address Port RecvQ SendQ\n".to_owned();
    for (kind, forwards) in [("TCP", tcp), ("UDP", udp)] {
        for (guest, host) in forwards {
            table += &format!("{kind}[HOST_FORWARD] 13 127.0.0.1 {host} 10.0.2.15 {guest} 0 0\n");
        }
    }
    table
}

/// The longest line the stand-in monitor reads.
const MOST_MONITOR_LINE: usize = 1 << 16;

fn serve_monitor(stream: UnixStream, table: &str) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let _ = writer.write_all(b"{\"QMP\": {\"version\": {}, \"capabilities\": []}}\n");
    let mut reader = BufReader::new(stream);
    while let Ok(Some(line)) = read_capped_line(&mut reader, MOST_MONITOR_LINE) {
        let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let answer = if request["execute"] == "human-monitor-command"
            && request["arguments"]["command-line"] == "info usernet"
        {
            serde_json::json!({"return": table})
        } else {
            serde_json::json!({"return": {}})
        };
        let _ = writeln!(writer, "{answer}");
    }
}

#[must_use]
pub fn act_as_qemu(
    context: &Context,
    machine: &Machine,
    slot: Option<Slot>,
    forwards: &[(u16, u16)],
) -> ActingQemu {
    act_as_qemu_with_udp(context, machine, slot, forwards, &[])
}

/// The same, with UDP forwards too, each a guest port and the loopback port beside it.
#[must_use]
pub fn act_as_qemu_with_udp(
    context: &Context,
    machine: &Machine,
    slot: Option<Slot>,
    forwards: &[(u16, u16)],
    udp: &[(u16, u16)],
) -> ActingQemu {
    context.make_run_folder(slot).unwrap();
    let pid = std::process::id();
    let folder = context.processes.join(pid.to_string());
    fs::create_dir_all(&folder).unwrap();
    let words = format!("qemu\0-drive\0file={},format=qcow2\0", crate::text::qemu_value(&machine.disk));
    fs::write(folder.join("cmdline"), words).unwrap();
    let _ = symlink(format!("/proc/{pid}/fd"), folder.join("fd"));
    let _ = symlink("/proc/net", context.processes.join("net"));
    let monitor = context.run_folder(slot).join(MAIN_SOCKET);
    let _ = fs::remove_file(&monitor);
    let listener = UnixListener::bind(&monitor).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (stopping, table) = (stop.clone(), usernet_table(forwards, udp));
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            if stopping.load(Ordering::SeqCst) {
                return;
            }
            let table = table.clone();
            thread::spawn(move || serve_monitor(stream, &table));
        }
    });
    ActingQemu { monitor, stop }
}

/// A machine of its own with this test process as its QEMU, which forwards one port of the guest from
/// a loopback port, while it is kept: for a test that only needs a way to a port of a guest.
pub struct QemuStandIn {
    context: Context,
    machine: Machine,
    guest_port: u16,
    _qemu: ActingQemu,
    _scratch: Scratch,
}

impl QemuStandIn {
    pub fn forwarding(guest_port: u16, host_port: u16) -> Self {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = Machine::new("vm", "vm", &scratch.path("vm.qcow2").display().to_string(), "/v");
        let qemu = act_as_qemu(&context, &machine, None, &[(guest_port, host_port)]);
        Self { context, machine, guest_port, _qemu: qemu, _scratch: scratch }
    }

    /// The guest's port, as the app reaches it.
    pub fn end(&self) -> GuestEnd {
        GuestEnd::new(&self.context, &self.machine, None, self.guest_port)
    }
}
