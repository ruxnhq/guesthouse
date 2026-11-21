//! Opens a machine's console, the display HCS serves as Remote Desktop on a pipe, in Windows' own
//! Remote Desktop client, which can't open a pipe: this listens on a port of this PC's loopback only,
//! carries each connection to the pipe and back, and starts the client on that port.
//!
//!     cargo run -p guesthouse-control --example console -- NAME [VM-ID]
//!
//! With VM-ID, HCS's RuntimeId for the machine, the client is started from an .rdp file that sends
//! it ahead of Remote Desktop, as Hyper-V's own viewer does, in case the console waits for it.
//! For trying the console before the app has a button for it.

use std::fs::{self, OpenOptions};
use std::io;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::{Command, ExitCode};
use std::thread;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        eprintln!("Give the machine's name, and if you like HCS's RuntimeId for it.");
        return ExitCode::FAILURE;
    };
    let pipe = format!(r"\\.\pipe\guesthouse-{name}-console");
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("A port to listen on: {error}");
            return ExitCode::FAILURE;
        }
    };
    let port = listener.local_addr().map(|address| address.port()).unwrap_or_default();
    println!("{pipe} is at 127.0.0.1:{port}");
    let started = match args.get(1) {
        Some(id) => {
            let file = std::env::temp_dir().join(format!("guesthouse-{name}-console.rdp"));
            let settings = format!(
                "full address:s:127.0.0.1:{port}\r\npcb:s:{id}\r\nauthentication level:i:0\r\n\
                 enablecredsspsupport:i:0\r\nprompt for credentials:i:0\r\n"
            );
            fs::write(&file, settings).and_then(|()| Command::new("mstsc.exe").arg(&file).spawn())
        }
        None => Command::new("mstsc.exe").arg(format!("/v:127.0.0.1:{port}")).spawn(),
    };
    if let Err(error) = started {
        eprintln!("Remote Desktop: {error}");
    }
    for connection in listener.incoming() {
        let Ok(connection) = connection else { continue };
        let pipe = pipe.clone();
        thread::spawn(move || {
            if let Err(error) = carry(connection, &pipe) {
                eprintln!("{pipe}: {error}");
            }
        });
    }
    ExitCode::SUCCESS
}

/// Carries one connection to the pipe and back, until either end closes, and says how much went
/// each way, which tells how far Remote Desktop got.
fn carry(connection: TcpStream, pipe: &str) -> io::Result<()> {
    let opened = OpenOptions::new().read(true).write(true).open(pipe)?;
    println!("connected to {pipe}");
    let (mut from_pipe, mut to_pipe) = (opened.try_clone()?, opened);
    let (mut from_client, mut to_client) = (connection.try_clone()?, connection);
    let back = thread::spawn(move || io::copy(&mut from_pipe, &mut to_client));
    let sent = io::copy(&mut from_client, &mut to_pipe);
    let received = back.join().unwrap_or(Ok(0));
    println!(
        "closed: {} bytes to the console, {} bytes back",
        sent.as_ref().unwrap_or(&0),
        received.unwrap_or(0)
    );
    sent.map(|_| ())
}
