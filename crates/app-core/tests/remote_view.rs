//! The real `guesthouse-remote-view` program, started the way the app starts it: for a machine whose
//! QEMU is stood in for by a process of the test's own.

use guesthouse_app_core::remote_view::{
    PID_FILE, RelayReply, RelayStatus, Request, Settings, ShareState, Watched, ask, socket_path,
};
use guesthouse_jail::launch::start_time;
use rustix::process::{Pid, Signal, kill_process};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(5);

struct Run {
    folder: PathBuf,
    program: Child,
    /// Stands in for QEMU.
    qemu: Child,
}

impl Drop for Run {
    fn drop(&mut self) {
        // Both are this test's own children.
        let _ = self.program.kill();
        let _ = self.program.wait();
        let _ = self.qemu.kill();
        let _ = self.qemu.wait();
    }
}

fn folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("guesthouse-remote-view-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn watching(child: &Child) -> Watched {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.id())).unwrap();
    Watched { pid: child.id(), started: start_time(&stat).unwrap() }
}

fn program(settings: &Settings) -> Child {
    Command::new(env!("CARGO_BIN_EXE_guesthouse-remote-view"))
        .args(settings.arguments())
        .stdin(Stdio::null())
        .spawn()
        .unwrap()
}

fn start(name: &str, loopback_port: u16, listen: Option<&str>) -> Run {
    let folder = folder(name);
    let qemu = Command::new("sleep").arg("60").spawn().unwrap();
    let settings = Settings {
        machine: name.into(),
        run_folder: folder.clone(),
        loopback_port,
        listen: listen.map(|listen| listen.parse().unwrap()),
        qemu: Some(watching(&qemu)),
        idle_timeout: None,
        max_clients: None,
        name: None,
        fingerprint: None,
    };
    let program = program(&settings);
    let run = Run { folder, program, qemu };
    let end = Instant::now() + WAIT;
    while ask(&run.folder, &Request::Status).is_err() {
        assert!(Instant::now() < end, "the program never answered");
        thread::sleep(Duration::from_millis(20));
    }
    run
}

fn status(run: &Run, request: Request) -> RelayStatus {
    match ask(&run.folder, &request).unwrap() {
        RelayReply::Status(status) => status,
        RelayReply::Error(reason) => panic!("refused: {reason}"),
    }
}

fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    let end = Instant::now() + WAIT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < end, "the program did not end");
        thread::sleep(Duration::from_millis(20));
    }
}

fn nothing_left(folder: &Path) {
    assert!(!folder.join(PID_FILE).exists(), "the pid file is gone");
    assert!(!socket_path(folder).exists(), "the socket is gone");
}

#[test]
fn the_network_side_opens_carries_both_ways_and_closes_while_the_program_runs() {
    let forward = UdpSocket::bind("127.0.0.1:0").unwrap();
    forward.set_read_timeout(Some(WAIT)).unwrap();
    let mut run = start("carry", forward.local_addr().unwrap().port(), None);
    assert_eq!(status(&run, Request::Status).state, ShareState::Off);

    let open = status(
        &run,
        Request::Open {
            address: Some("127.0.0.1".parse().unwrap()),
            port: Some(0),
            name: None,
            fingerprint: None,
        },
    );
    assert_eq!(open.state, ShareState::Listening);
    let port = open.port.unwrap();

    let device = UdpSocket::bind("127.0.0.1:0").unwrap();
    device.set_read_timeout(Some(WAIT)).unwrap();
    device.send_to(b"input", ("127.0.0.1", port)).unwrap();
    let mut buffer = [0u8; 64];
    let (length, from) = forward.recv_from(&mut buffer).unwrap();
    assert_eq!(&buffer[..length], b"input");
    forward.send_to(b"video", from).unwrap();
    assert_eq!(device.recv_from(&mut buffer).unwrap().0, 5);
    let busy = status(&run, Request::Status);
    assert_eq!((busy.clients.len(), busy.counters.up_packets, busy.counters.down_packets), (1, 1, 1));

    assert_eq!(status(&run, Request::Close).state, ShareState::Off);
    UdpSocket::bind(("127.0.0.1", port)).expect("the port is free once closed");
    assert!(run.program.try_wait().unwrap().is_none(), "closing the network side does not end the program");
}

#[test]
fn sigterm_ends_it_cleanly_with_its_files_put_away() {
    let mut run = start("term", 19065, Some("127.0.0.1:0"));
    assert!(run.folder.join(PID_FILE).exists());
    assert_eq!(status(&run, Request::Status).state, ShareState::Listening);
    // The program is this test's own child, started above and still running.
    kill_process(Pid::from_raw(run.program.id() as i32).unwrap(), Signal::TERM).unwrap();
    assert!(wait_for_exit(&mut run.program).success());
    nothing_left(&run.folder);
}

#[test]
fn it_ends_by_itself_when_qemu_has_gone() {
    let mut run = start("qemu-gone", 19065, Some("127.0.0.1:0"));
    run.qemu.kill().unwrap();
    run.qemu.wait().unwrap();
    assert!(wait_for_exit(&mut run.program).success());
    nothing_left(&run.folder);
}

#[test]
fn a_second_program_for_the_same_run_folder_refuses_and_leaves_the_first_alone() {
    let mut run = start("twice", 19065, None);
    let settings = Settings {
        machine: "twice".into(),
        run_folder: run.folder.clone(),
        loopback_port: 19065,
        listen: None,
        qemu: Some(watching(&run.qemu)),
        idle_timeout: None,
        max_clients: None,
        name: None,
        fingerprint: None,
    };
    let mut second = program(&settings);
    assert!(!wait_for_exit(&mut second).success());
    assert_eq!(status(&run, Request::Status).state, ShareState::Off, "the first still answers");
    assert!(run.program.try_wait().unwrap().is_none());
}

#[test]
fn a_port_that_cannot_be_opened_leaves_it_running_and_failed() {
    let taken = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let run = start("taken", 19065, None);
    let refused = ask(
        &run.folder,
        &Request::Open {
            address: Some("127.0.0.1".parse().unwrap()),
            port: Some(port),
            name: None,
            fingerprint: None,
        },
    )
    .unwrap();
    assert!(matches!(refused, RelayReply::Error(_)));
    let state = status(&run, Request::Status);
    assert_eq!(state.state, ShareState::Failed);
    assert!(state.reason.is_some());
}
