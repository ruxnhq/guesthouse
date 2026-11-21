//! The case for Windows' jail: the test program runs itself jailed, and from inside it tries what
//! the jail must allow and what it must refuse.

use guesthouse_jail::windows::{Handles, Jail, applied};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::process::{self, Command};
use std::time::Duration;

/// The first argument that makes the program the jailed side.
const INSIDE: &str = "--inside-the-jail";
const PROFILE: &str = "guesthouse.jail-test";
const CONNECT_SECONDS: u64 = 3;

pub fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some(INSIDE) {
        inside(Path::new(&arguments[1]), arguments[2].parse().unwrap());
        return;
    }
    let folder = env::temp_dir().join(format!("gh-jail-{}", process::id()));
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("theirs"), "theirs").unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port().to_string();

    let (child_input, mut to_child) = io::pipe().unwrap();
    let (mut from_child, child_output) = io::pipe().unwrap();
    let child_errors = child_output.try_clone().unwrap();
    let handles = Handles {
        stdin: Some(OwnedHandle::from(child_input)),
        stdout: Some(OwnedHandle::from(child_output)),
        stderr: Some(OwnedHandle::from(child_errors)),
    };
    let program = env::current_exe().unwrap();
    let arguments = [OsStr::new(INSIDE), folder.as_os_str(), OsStr::new(&port)];
    let jailed = Jail::new(PROFILE).spawn(&program, &arguments, handles).unwrap();
    to_child
        .write_all(
            b"ping
",
        )
        .unwrap();
    drop(to_child);
    let mut said = String::new();
    from_child.read_to_string(&mut said).unwrap();
    let code = jailed.wait().unwrap();
    let _ = fs::remove_dir_all(&folder);

    assert!(!applied().whole(), "the test's own process isn't jailed, so the check can tell");
    let passed = code == 0 && said.contains("pong");
    println!("windows jail ... {}", if passed { "ok" } else { "FAILED" });
    assert!(
        passed,
        "the jailed side exited {code} and said:
{said}"
    );
}

fn inside(folder: &Path, port: u16) {
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "ping", "its own input reaches it");
    let got = applied();
    assert!(got.whole(), "Windows says the jail is all there: {got:?}");

    assert!(refused(fs::read(folder.join("theirs"))), "a user's file can't be read");
    assert!(refused(fs::write(folder.join("new"), "")), "a user's folder can't be written");
    assert!(Command::new(env::current_exe().unwrap()).spawn().is_err(), "nothing can be started");
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let connected = TcpStream::connect_timeout(&address, Duration::from_secs(CONNECT_SECONDS));
    assert!(connected.is_err(), "there's no network, not even loopback");

    println!("pong");
    io::stdout().flush().unwrap();
}

fn refused(result: io::Result<impl Sized>) -> bool {
    matches!(result, Err(error) if error.kind() == ErrorKind::PermissionDenied)
}
