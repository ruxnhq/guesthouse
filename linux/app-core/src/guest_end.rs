//! The way to a port QEMU forwards from this PC's loopback to the guest: the agent's, the files
//! helper's and the app windows helper's. Each request starts with the machine's token, so a
//! connection is only handed out once its far end is known to be the machine's own QEMU.
//!
//! QEMU is the process that listens on the monitor socket in the slot's run folder, which only this
//! user can open, and its words have to open the machine's disk. The connection is QEMU's when the
//! socket that accepted it is one of that process's open files, as `/proc` shows. With no QEMU,
//! nothing connects at all.

use crate::context::Context;
use crate::machine::Machine;
use crate::monitor::MAIN_SOCKET;
use guesthouse_daemon::slot::Slot;
use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with, sockopt};
use std::fs;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// How long QEMU may take to take a connection. It takes one as soon as it sees it.
const ACCEPT_PATIENCE: Duration = Duration::from_secs(2);
const LOOK_AGAIN: Duration = Duration::from_millis(5);

/// One port of one machine's guest, as this PC reaches it.
#[derive(Debug, Clone)]
pub struct GuestEnd {
    pub port: u16,
    /// QEMU's monitor in the slot's run folder.
    monitor: PathBuf,
    /// Where processes and sockets are read from: `/proc`, or a test's stand-in.
    processes: PathBuf,
    machine: Machine,
}

fn refused(why: &str) -> io::Error {
    io::Error::new(ErrorKind::ConnectionRefused, why.to_owned())
}

impl GuestEnd {
    /// `port` on this PC, for the machine in `slot`, or in this process's slot.
    pub fn new(context: &Context, machine: &Machine, slot: Option<Slot>, port: u16) -> Self {
        Self {
            port,
            monitor: context.run_folder(slot).join(MAIN_SOCKET),
            processes: context.processes.clone(),
            machine: machine.clone(),
        }
    }

    /// The machine's agent.
    pub fn agent(context: &Context, machine: &Machine, slot: Option<Slot>) -> Self {
        Self::new(context, machine, slot, context.agent_port(slot))
    }

    /// A connection to the port, once the socket that took it is known to be the machine's QEMU's.
    /// Nothing has been written to it.
    pub fn connect(&self, patience: Duration) -> io::Result<TcpStream> {
        let qemu = self.qemu()?;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let stream = TcpStream::connect_timeout(&address, patience)?;
        let local = stream.local_addr()?.port();
        let inode = self.taken_by(&stream, local)?;
        if !self.holds(qemu, inode) {
            return Err(refused("the port is not the machine's QEMU's"));
        }
        Ok(stream)
    }

    /// The pid of the slot's QEMU, if it runs this machine.
    fn qemu(&self) -> io::Result<u32> {
        let pid = listener_pid(&self.monitor)?;
        let words = fs::read(self.processes.join(pid.to_string()).join("cmdline"))?;
        let runs =
            words.split(|byte| *byte == 0).any(|word| crate::processes::opens_disk(word, &self.machine));
        if runs { Ok(pid) } else { Err(refused("the slot's QEMU runs another machine")) }
    }

    /// The socket that took this connection, by its inode, once one has. Until the listener takes
    /// it, the kernel holds it with none.
    fn taken_by(&self, stream: &TcpStream, local: u16) -> io::Result<u64> {
        let end = Instant::now() + ACCEPT_PATIENCE;
        loop {
            let table = fs::read_to_string(self.processes.join("net/tcp"))?;
            if let Some(inode) = far_end(&table, self.port, local).filter(|inode| *inode != 0) {
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

/// Who listens on a unix socket: the process that started listening, which keeps its pid if it
/// then runs another program, as the jail's launcher does to become QEMU. A socket nobody listens
/// on any more refuses. The connection is never used, and doesn't wait if the listener is busy.
fn listener_pid(socket: &Path) -> io::Result<u32> {
    let fd = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )?;
    connect(&fd, &SocketAddrUnix::new(socket)?)?;
    let pid = sockopt::socket_peercred(&fd)?.pid;
    Ok(pid.as_raw_nonzero().get().unsigned_abs())
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
    fn a_port_this_process_holds_as_the_slots_qemu_is_reached() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.slot = crate::slots::number(2);
        let vm = machine(&context, "vm");
        let _qemu = act_as_qemu(&context, &vm, None);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let guest = thread::spawn(move || {
            let mut line = String::new();
            listener.accept().unwrap().0.read_to_string(&mut line).unwrap();
            line
        });
        let mut stream = GuestEnd::new(&context, &vm, None, port).connect(Duration::from_secs(1)).unwrap();
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
        assert!(GuestEnd::new(&context, &vm, None, port).connect(Duration::from_secs(1)).is_err());
        // A monitor left by a QEMU that has gone refuses too.
        drop(act_as_qemu(&context, &vm, None));
        assert!(GuestEnd::new(&context, &vm, None, port).connect(Duration::from_secs(1)).is_err());
        assert!(listener.accept().is_err(), "no connection was made");
    }

    #[test]
    fn the_slots_qemu_of_another_machine_is_not_connected_to() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let (vm, other) = (machine(&context, "vm"), machine(&context, "other"));
        let _qemu = act_as_qemu(&context, &other, None);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(GuestEnd::new(&context, &vm, None, port).connect(Duration::from_secs(1)).is_err());
        assert!(listener.accept().is_err(), "no connection was made");
    }

    #[test]
    fn a_port_another_process_holds_gets_nothing_while_qemu_runs() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let vm = machine(&context, "vm");
        let _qemu = act_as_qemu(&context, &vm, None);
        let (other, port) = other_listener();
        let end = GuestEnd::new(&context, &vm, None, port);
        assert!(end.connect(Duration::from_secs(1)).is_err());
        let said = String::from_utf8(other.wait_with_output().unwrap().stdout).unwrap();
        assert!(said.contains("heard 0\n"), "it heard something: {said}");
    }

    /// The agent asked the way the app asks it, through the real connection, on `port`.
    fn asking_on(scratch: &Scratch, port: u16) -> (Context, Machine) {
        let mut context = scratch_context(scratch);
        context.agent = std::sync::Arc::new(crate::agent::TcpAgent);
        context.paths.own_agent_port = Some(port);
        let vm = machine(&context, "vm");
        crate::agent::make_token(&context, &vm).unwrap();
        (context, vm)
    }

    #[test]
    fn the_agent_is_not_asked_once_qemu_has_gone() {
        let scratch = Scratch::new();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (context, vm) = asking_on(&scratch, listener.local_addr().unwrap().port());
        // A token change waiting to be taken, which an ask that got no answer tries next.
        let waiting = vm.token_file(&context).with_extension("token.next");
        fs::write(&waiting, format!("{}\n", "ab".repeat(16))).unwrap();
        drop(act_as_qemu(&context, &vm, None));
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
    fn the_agent_port_held_by_another_process_while_qemu_runs_is_not_sent_the_token() {
        let scratch = Scratch::new();
        let (other, port) = other_listener();
        let (context, vm) = asking_on(&scratch, port);
        let _qemu = act_as_qemu(&context, &vm, None);
        assert_eq!(context.ask_guest(&vm, "Get-Date", 1, None), "");
        let said = String::from_utf8(other.wait_with_output().unwrap().stdout).unwrap();
        assert!(said.contains("heard 0\n"), "it heard something: {said}");
    }
}
