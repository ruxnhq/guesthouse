//! The way to a port of the guest that QEMU forwards from this PC's loopback: the agent's, the files
//! helper's and the app windows helper's. Each request starts with the machine's token, so a
//! connection is only handed out once its far end is known to be the machine's own QEMU.
//!
//! QEMU is the process that listens on the monitor socket in the slot's run folder, which only this
//! user can open, and its words have to open the machine's disk. QEMU picks the loopback port of each
//! of these forwards when it starts, and its monitor says which it picked. The connection is QEMU's
//! when the socket that accepted it is one of that process's open files, as `/proc` shows. With no
//! QEMU, nothing connects at all.

use crate::context::Context;
use crate::machine::Machine;
use crate::monitor::{Command, MAIN_SOCKET, ask_with_listener};
use guesthouse_daemon::slot::Slot;
use serde_json::json;
use std::fs;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

/// How long QEMU may take to take a connection. It takes one as soon as it sees it.
const ACCEPT_PATIENCE: Duration = Duration::from_secs(2);
const LOOK_AGAIN: Duration = Duration::from_millis(5);

/// One port of one machine's guest, as this PC reaches it.
#[derive(Debug, Clone)]
pub struct GuestEnd {
    /// The port in the guest.
    pub guest_port: u16,
    /// QEMU's monitor in the slot's run folder.
    pub monitor: PathBuf,
    /// Where processes and sockets are read from: `/proc`, or a test's stand-in.
    processes: PathBuf,
    machine: Machine,
}

fn refused(why: &str) -> io::Error {
    io::Error::new(ErrorKind::ConnectionRefused, why.to_owned())
}

impl GuestEnd {
    /// `guest_port` of the machine in `slot`, or in this process's slot.
    pub fn new(context: &Context, machine: &Machine, slot: Option<Slot>, guest_port: u16) -> Self {
        Self {
            guest_port,
            monitor: context.run_folder(slot).join(MAIN_SOCKET),
            processes: context.processes.clone(),
            machine: machine.clone(),
        }
    }

    /// The machine's agent.
    pub fn agent(context: &Context, machine: &Machine, slot: Option<Slot>) -> Self {
        Self::new(context, machine, slot, crate::network::GUEST_AGENT_PORT)
    }

    /// A connection to the port, once the socket that took it is known to be the machine's QEMU's.
    /// Nothing has been written to it.
    pub fn connect(&self, patience: Duration) -> io::Result<TcpStream> {
        let (qemu, port) = self.qemu()?;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let stream = TcpStream::connect_timeout(&address, patience)?;
        let local = stream.local_addr()?.port();
        let inode = self.taken_by(&stream, port, local)?;
        if !self.holds(qemu, inode) {
            return Err(refused("the port is not the machine's QEMU's"));
        }
        Ok(stream)
    }

    /// The pid of the slot's QEMU, if it runs this machine, and the port on this PC it forwards to the
    /// guest's port.
    fn qemu(&self) -> io::Result<(u32, u16)> {
        let (pid, answer) = ask_with_listener(&self.monitor, &usernet())
            .ok_or_else(|| refused("no QEMU answers in the slot"))?;
        let words = fs::read(self.processes.join(pid.to_string()).join("cmdline"))?;
        let runs =
            words.split(|byte| *byte == 0).any(|word| crate::processes::opens_disk(word, &self.machine));
        if !runs {
            return Err(refused("the slot's QEMU runs another machine"));
        }
        let table = answer["return"].as_str().unwrap_or_default();
        let port =
            forwarded_port(table, self.guest_port).ok_or_else(|| refused("QEMU has no such forward"))?;
        Ok((pid, port))
    }

    /// The socket that took this connection, by its inode, once one has. Until the listener takes
    /// it, the kernel holds it with none.
    fn taken_by(&self, stream: &TcpStream, port: u16, local: u16) -> io::Result<u64> {
        let end = Instant::now() + ACCEPT_PATIENCE;
        loop {
            let table = fs::read_to_string(self.processes.join("net/tcp"))?;
            if let Some(inode) = far_end(&table, port, local).filter(|inode| *inode != 0) {
                return Ok(inode);
            }
            if closed(stream)? {
                return Err(refused("the port closed the connection"));
            }
            if Instant::now() >= end {
                return Err(refused("nothing took the connection"));
            }
            thread::sleep(LOOK_AGAIN);
        }
    }

    /// Whether the process has this socket open.
    fn holds(&self, pid: u32, inode: u64) -> bool {
        let wanted = format!("socket:[{inode}]");
        let Ok(files) = fs::read_dir(self.processes.join(pid.to_string()).join("fd")) else { return false };
        files
            .flatten()
            .any(|file| fs::read_link(file.path()).is_ok_and(|link| link.as_os_str() == wanted.as_str()))
    }
}

/// The loopback port QEMU's user network forwards to `guest_port`, from its own table of
/// connections, where a forward is a line like
/// `  TCP[HOST_FORWARD]  14       127.0.0.1 40123       10.0.2.15  8765     0     0`.
fn forwarded_port(table: &str, guest_port: u16) -> Option<u16> {
    forwarded("TCP[HOST_FORWARD]", table, guest_port)
}

fn forwarded(kind: &str, table: &str, guest_port: u16) -> Option<u16> {
    table.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        match fields.as_slice() {
            [found, _, "127.0.0.1", port, _, guest, ..]
                if *found == kind && guest.parse() == Ok(guest_port) =>
            {
                port.parse().ok().filter(|port| *port != 0)
            }
            _ => None,
        }
    })
}

/// The loopback port QEMU in `slot` forwards to `guest_port` in the guest over UDP, as its monitor
/// says, or `None` when no QEMU answers there or it has no such forward.
pub fn udp_forwarded_port(context: &Context, slot: Option<Slot>, guest_port: u16) -> Option<u16> {
    let (_, answer) = ask_with_listener(&context.run_folder(slot).join(MAIN_SOCKET), &usernet())?;
    forwarded("UDP[HOST_FORWARD]", answer["return"].as_str()?, guest_port)
}

fn usernet() -> Command {
    Command::new("human-monitor-command", json!({"command-line": "info usernet"}))
}

/// Whether the far end has closed the connection, without taking anything it sent.
fn closed(stream: &TcpStream) -> io::Result<bool> {
    stream.set_nonblocking(true)?;
    let mut byte = [0; 1];
    let said = stream.peek(&mut byte);
    stream.set_nonblocking(false)?;
    match said {
        Ok(0) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error),
    }
}

/// The inode of the socket at the other end of a loopback connection from `local` to `port`, as the
/// kernel's table of TCP sockets has it: 0 while nothing has taken it.
fn far_end(table: &str, port: u16, local: u16) -> Option<u64> {
    let loopback = u32::from_ne_bytes(Ipv4Addr::LOCALHOST.octets());
    let (here, there) = (format!("{loopback:08X}:{port:04X}"), format!("{loopback:08X}:{local:04X}"));
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let matches = fields.get(1) == Some(&here.as_str()) && fields.get(2) == Some(&there.as_str());
        if matches { fields.get(9)?.parse().ok() } else { None }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, act_as_qemu, scratch_context};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::process::{Child, Command, Stdio};

    const AGENT: u16 = crate::network::GUEST_AGENT_PORT;

    fn machine(context: &Context, id: &str) -> Machine {
        let machine = Machine::new(id, id, &format!("/disks/{id}.qcow2"), "/v");
        machine.save(context).unwrap();
        machine
    }

    const LISTENER_VARIABLE: &str = "GUEST_END_LISTENER";

    /// Run by [`other_listener`] in a process of its own: listens on a loopback port, says which, and
    /// then says what it was sent.
    #[test]
    #[ignore = "a part of another test"]
    fn listener_in_another_process() {
        if std::env::var_os(LISTENER_VARIABLE).is_none() {
            return;
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        println!("port {}", listener.local_addr().unwrap().port());
        let mut stream = listener.accept().unwrap().0;
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut heard = Vec::new();
        let _ = stream.read_to_end(&mut heard);
        println!("heard {}", String::from_utf8_lossy(&heard).len());
    }

    /// The caller waits for the process, once it has connected to it.
    #[expect(clippy::zombie_processes, reason = "the caller waits for it")]
    fn other_listener() -> (Child, u16) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "guest_end::tests::listener_in_another_process", "--nocapture"])
            .env(LISTENER_VARIABLE, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = child.stdout.as_mut().unwrap();
        let (mut line, mut byte) = (String::new(), [0; 1]);
        loop {
            line.clear();
            while out.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
                line.push(byte[0] as char);
            }
            if let Some(port) = line.strip_prefix("port ") {
                return (child, port.trim().parse().unwrap());
            }
        }
    }

    #[test]
    fn the_far_end_is_read_from_the_kernels_table() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
            0: 0100007F:494D 0100007F:A001 01 00000000:00000000 00:00000000 00000000  1000        0 0 1 0000\n\
            1: 0100007F:494D 0100007F:A002 01 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000\n";
        if cfg!(target_endian = "little") {
            assert_eq!(far_end(table, 18765, 0xA001), Some(0), "not taken yet");
            assert_eq!(far_end(table, 18765, 0xA002), Some(4242));
            assert_eq!(far_end(table, 18765, 0xA003), None);
        }
    }

    #[test]
    fn the_port_qemu_picked_is_read_from_its_table_of_connections() {
        // As QEMU's monitor gives it, around libslirp's own table for three forwards from port 0.
        let table = "Hub -1 (#net125):\n\
            \x20 Protocol[State]    FD  Source Address  Port   Dest. Address  Port RecvQ SendQ\n\
            \x20 TCP[HOST_FORWARD]   5       127.0.0.1 45339       10.0.2.15  8769     0     0\n\
            \x20 TCP[HOST_FORWARD]   4       127.0.0.1 39763       10.0.2.15  8767     0     0\n\
            \x20 TCP[HOST_FORWARD]   3       127.0.0.1 42837       10.0.2.15  8765     0     0\n\
            \x20 TCP[ESTABLISHED]   20       10.0.2.15 49800        10.0.2.2   443     0     0\n\
            \x20 UDP[HOST_FORWARD]   6       127.0.0.1 57822       10.0.2.15  7791     0     0\n";
        assert_eq!(forwarded_port(table, 8765), Some(42837));
        assert_eq!(forwarded_port(table, 8767), Some(39763));
        assert_eq!(forwarded_port(table, 8769), Some(45339));
        assert_eq!(forwarded_port(table, 7791), None, "a UDP forward is not one");
        assert_eq!(forwarded("UDP[HOST_FORWARD]", table, 7791), Some(57822));
        assert_eq!(forwarded_port(table, 443), None);
    }
    #[test]
    fn a_port_of_the_slots_qemu_is_reached() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.slot = crate::slots::number(2);
        let vm = machine(&context, "vm");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let _qemu = act_as_qemu(&context, &vm, None, &[(AGENT, port)]);
        let guest = thread::spawn(move || {
            let mut line = String::new();
            listener.accept().unwrap().0.read_to_string(&mut line).unwrap();
            line
        });
        let mut stream = GuestEnd::agent(&context, &vm, None).connect(Duration::from_secs(1)).unwrap();
        stream.write_all(b"hello").unwrap();
        drop(stream);
        assert_eq!(guest.join().unwrap(), "hello");
    }

    #[test]
    fn nothing_connects_with_no_qemu_in_the_slot() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let vm = machine(&context, "vm");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(GuestEnd::agent(&context, &vm, None).connect(Duration::from_secs(1)).is_err());
        // A monitor left by a QEMU that has gone refuses too.
        drop(act_as_qemu(&context, &vm, None, &[(AGENT, port)]));
        assert!(GuestEnd::agent(&context, &vm, None).connect(Duration::from_secs(1)).is_err());
        assert!(listener.accept().is_err(), "no connection was made");
    }

    #[test]
    fn the_slots_qemu_of_another_machine_is_not_connected_to() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let (vm, other) = (machine(&context, "vm"), machine(&context, "other"));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let _qemu = act_as_qemu(&context, &other, None, &[(AGENT, port)]);
        assert!(GuestEnd::agent(&context, &vm, None).connect(Duration::from_secs(1)).is_err());
        assert!(listener.accept().is_err(), "no connection was made");
    }

    #[test]
    fn only_a_socket_of_qemus_own_is_handed_out() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let vm = machine(&context, "vm");
        let (other, port) = other_listener();
        let _qemu = act_as_qemu(&context, &vm, None, &[(AGENT, port)]);
        let end = GuestEnd::agent(&context, &vm, None);
        assert!(end.connect(Duration::from_secs(1)).is_err());
        let said = String::from_utf8(other.wait_with_output().unwrap().stdout).unwrap();
        assert!(said.contains("heard 0\n"), "it heard something: {said}");
    }

    /// The agent asked the way the app asks it, through the real connection.
    fn asking(scratch: &Scratch) -> (Context, Machine) {
        let mut context = scratch_context(scratch);
        context.agent = std::sync::Arc::new(crate::agent::TcpAgent);
        let vm = machine(&context, "vm");
        crate::agent::make_token(&context, &vm).unwrap();
        (context, vm)
    }

    #[test]
    fn the_agent_is_not_asked_once_qemu_has_gone() {
        let scratch = Scratch::new();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (context, vm) = asking(&scratch);
        // A token change waiting to be taken, which an ask that got no answer tries next.
        let waiting = vm.token_file(&context).with_extension("token.next");
        fs::write(&waiting, format!("{}\n", "ab".repeat(16))).unwrap();
        drop(act_as_qemu(&context, &vm, None, &[(AGENT, port)]));
        let heard = thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let mut heard = Vec::new();
            let end = Instant::now() + Duration::from_secs(3);
            while Instant::now() < end {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
                    let _ = stream.read_to_end(&mut heard);
                }
                thread::sleep(Duration::from_millis(10));
            }
            heard
        });
        assert_eq!(context.ask_guest(&vm, "Stop-Computer", 1, None), "");
        let heard = heard.join().unwrap();
        assert!(heard.is_empty(), "the port was sent {:?}", String::from_utf8_lossy(&heard));
    }

    #[test]
    fn the_agent_is_asked_only_through_a_socket_of_qemus_own() {
        let scratch = Scratch::new();
        let (other, port) = other_listener();
        let (context, vm) = asking(&scratch);
        let _qemu = act_as_qemu(&context, &vm, None, &[(AGENT, port)]);
        assert_eq!(context.ask_guest(&vm, "Get-Date", 1, None), "");
        let said = String::from_utf8(other.wait_with_output().unwrap().stdout).unwrap();
        assert!(said.contains("heard 0\n"), "it heard something: {said}");
    }

    #[test]
    fn a_monitor_that_takes_no_connection_is_not_waited_on_for_long() {
        use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, socket_with};
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let vm = machine(&context, "vm");
        context.make_run_folder(None).unwrap();
        let monitor = context.run_folder(None).join(MAIN_SOCKET);
        // A queue of one, so it fills after a few connections whatever the system's limit on queues.
        let address = SocketAddrUnix::new(&monitor).unwrap();
        let listener =
            socket_with(AddressFamily::UNIX, SocketType::STREAM, SocketFlags::CLOEXEC, None).unwrap();
        rustix::net::bind(&listener, &address).unwrap();
        rustix::net::listen(&listener, 1).unwrap();
        // Its queue full of connections it never took, as a QEMU that has stopped answering leaves it.
        let mut waiting = Vec::new();
        loop {
            let flags = SocketFlags::CLOEXEC | SocketFlags::NONBLOCK;
            let fd = socket_with(AddressFamily::UNIX, SocketType::STREAM, flags, None).unwrap();
            if rustix::net::connect(&fd, &address).is_err() {
                break;
            }
            waiting.push(fd);
            assert!(waiting.len() < 64, "the queue never filled");
        }
        let (done, finished) = std::sync::mpsc::channel();
        let end = GuestEnd::agent(&context, &vm, None);
        thread::spawn(move || done.send(end.connect(Duration::from_secs(1)).is_err()));
        assert_eq!(finished.recv_timeout(Duration::from_secs(5)), Ok(true), "it waited on the monitor");
        drop(waiting);
    }
}
