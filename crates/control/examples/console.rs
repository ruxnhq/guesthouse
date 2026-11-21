//! Opens a machine's console, the display HCS serves as Remote Desktop on a pipe, in Windows' own
//! Remote Desktop client, which can't open a pipe: this listens on a port of this PC's loopback only,
//! carries each connection to the pipe and back, and starts the client on that port.
//!
//!     cargo run -p guesthouse-control --example console -- NAME
//!
//! For trying the console before the app has a button for it.

use std::fs::OpenOptions;
use std::io;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::{Command, ExitCode};
use std::thread;

fn main() -> ExitCode {
    let Some(name) = std::env::args().nth(1) else {
        eprintln!("Give the machine's name.");
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
    if let Err(error) = Command::new("mstsc.exe").arg(format!("/v:127.0.0.1:{port}")).spawn() {
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

/// Carries one connection to the pipe and back, until either end closes.
fn carry(connection: TcpStream, pipe: &str) -> io::Result<()> {
    let opened = OpenOptions::new().read(true).write(true).open(pipe)?;
    println!("connected to {pipe}");
    let (mut from_pipe, mut to_pipe) = (opened.try_clone()?, opened);
    let (mut from_client, mut to_client) = (connection.try_clone()?, connection);
    let back = thread::spawn(move || io::copy(&mut from_pipe, &mut to_client));
    let sent = io::copy(&mut from_client, &mut to_pipe);
    let _ = back.join();
    sent.map(|_| ())
}
