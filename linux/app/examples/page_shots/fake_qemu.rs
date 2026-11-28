//! This program under QEMU's name: monitors that answer as a running VM's do, and a display. No VM runs.

#![allow(clippy::disallowed_methods)] // This is the fake QEMU of the page shots.

use crate::fake_display;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

const LIFE: Duration = Duration::from_secs(60);
const DISPLAY_SOCKET: &str = "display.sock";

fn serve_monitor(mut stream: UnixStream) {
    let _ = stream.write_all(b"{\"QMP\": {}}\n");
    let reader = BufReader::new(stream.try_clone().unwrap());
    for line in reader.lines().map_while(Result::ok) {
        let request: Value = serde_json::from_str(&line).unwrap_or_default();
        let answer = match request["execute"].as_str() {
            Some("query-status") => json!({"return": {"status": "running"}}),
            Some("query-mice") => json!({"return": [], "id": "mice"}),
            Some("system_powerdown") => process::exit(0),
            Some("human-monitor-command") if request["arguments"]["command-line"] == "info usernet" => {
                json!({"return": FORWARDS.lock().unwrap().clone()})
            }
            _ => json!({"return": {}}),
        };
        let _ = writeln!(stream, "{answer}");
    }
}

fn listen_on(path: &Path) {
    if path.ends_with(DISPLAY_SOCKET) {
        fake_display::serve(path);
        return;
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            thread::spawn(move || serve_monitor(stream));
        }
    });
}

/// Where a forward's connections go: a port on this PC, as no guest is behind them.
pub const FORWARD_TO_VARIABLE: &str = "PAGE_SHOTS_FORWARD_TO";

/// The forwards' lines as QEMU's `info usernet` gives them.
static FORWARDS: Mutex<String> = Mutex::new(String::new());

/// Passes each connection to a port of the user network's forwards on to `to`, as QEMU passes it
/// to the guest, so the app reaches a pretend helper through this process as it would through QEMU.
fn forward(word: &str, to: u16) {
    for rule in word.split(',').filter_map(|part| part.strip_prefix("hostfwd=tcp:127.0.0.1:")) {
        let Some((Ok(port), Ok(guest))) =
            rule.split_once("-:").map(|(port, guest)| (port.parse::<u16>(), guest.parse::<u16>()))
        else {
            continue;
        };
        let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) else { continue };
        let Ok(bound) = listener.local_addr() else { continue };
        *FORWARDS.lock().unwrap() +=
            &format!("TCP[HOST_FORWARD] 13 127.0.0.1 {} 10.0.2.15 {guest} 0 0\n", bound.port());
        thread::spawn(move || {
            for outside in listener.incoming().map_while(Result::ok) {
                let Ok(inside) = TcpStream::connect((Ipv4Addr::LOCALHOST, to)) else { continue };
                for (mut from, mut into) in
                    [(outside.try_clone(), inside.try_clone()), (Ok(inside), Ok(outside))]
                        .into_iter()
                        .filter_map(|(from, into)| Some((from.ok()?, into.ok()?)))
                {
                    thread::spawn(move || {
                        let _ = io::copy(&mut from, &mut into);
                        let _ = into.shutdown(Shutdown::Write);
                    });
                }
            }
        });
    }
}

/// Takes the arguments the app gave QEMU.
pub fn run(arguments: &[String]) -> ! {
    for pair in arguments.windows(2).filter(|pair| pair[0] == "-qmp") {
        listen_on(Path::new(pair[1].trim_start_matches("unix:").split(',').next().unwrap()));
    }
    let to = std::env::var(FORWARD_TO_VARIABLE).ok().and_then(|port| port.parse().ok());
    for pair in arguments.windows(2).filter(|pair| pair[0] == "-nic") {
        if let Some(to) = to {
            forward(&pair[1], to);
        }
    }
    thread::sleep(LIFE);
    process::exit(0)
}
