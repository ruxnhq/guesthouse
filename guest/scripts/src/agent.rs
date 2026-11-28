//! guest/windows/agent.ps1, run for real on this PC's loopback with a token file of its own, asked the
//! way the host asks it: a line with the token and the script in Base64, and the answer until it closes.

use crate::{Scratch, guest_script, quote, shell, shell_command};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
/// PowerShell takes a while to start on a busy PC.
const STARTING: Duration = Duration::from_secs(60);
/// A display key nothing has, so the script names no display of this PC's.
const NO_VIDEO_KEY: &str = "HKCU:\\Software\\guesthouse-test-no-such-key";

fn base64(data: &[u8]) -> String {
    const LETTERS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let bytes = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let number = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for at in 0..4 {
            if at <= chunk.len() {
                out.push(char::from(LETTERS[((number >> (18 - 6 * at)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The agent, running, which is ended when this goes.
struct Agent {
    child: Child,
    port: u16,
    _scratch: Scratch,
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Agent {
    fn start(shell: &str) -> Self {
        let scratch = Scratch::new();
        let token = scratch.path("token.txt");
        std::fs::write(&token, format!("{TOKEN}\r\n")).unwrap();
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        let command = format!(
            "& {} -TokenFile {} -HostAddress 127.0.0.1 -ListenAddress 127.0.0.1 -Port {port} -VideoKey '{NO_VIDEO_KEY}'",
            quote(&guest_script("agent.ps1")),
            quote(&token)
        );
        let child = shell_command(shell)
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &command])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let agent = Self { child, port, _scratch: scratch };
        let until = Instant::now() + STARTING;
        // A connection that closes at once costs the agent nothing.
        while TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err() {
            assert!(Instant::now() < until, "the agent never listened");
            thread::sleep(Duration::from_millis(100));
        }
        agent
    }

    fn ask(&self, token: &str, script: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        writeln!(stream, "{token} {}", base64(script.as_bytes())).unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        answer
    }
}

#[test]
fn the_agent_runs_a_script_with_the_token_and_nothing_without_it() {
    let Some(shell) = shell() else { return };
    let agent = Agent::start(shell);
    assert_eq!(agent.ask(TOKEN, "Write-Output 'ran it'").trim(), "ran it");
    assert_eq!(agent.ask("ffffffffffffffffffffffffffffffff", "Write-Output 'ran it'"), "");
}
