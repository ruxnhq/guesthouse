//! A shell in an Ubuntu guest, as root, over the SSH port QEMU forwards to the host's loopback. The
//! install gave the guest a key of the machine's own, and this is the first thing the host does with it.
//!
//! The command is always a fixed text from this program, and what a command works on goes in on its
//! standard input. SSH hands a command to the guest's shell as one line to be read again, so anything
//! of a user's put in the command could change what it means. On stdin it is only data.

use crate::context::Context;
use crate::machine::Machine;
use crate::vm_args::ssh_port;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// The user's own `ssh_config`, agent and known hosts have no part in this: the guest is the machine's
/// own, and its key and its host key are in the machine's folder.
const SSH_FOLDER: &str = "ssh";
const KEY_FILE: &str = "id";
const KNOWN_HOSTS_FILE: &str = "known_hosts";
const CONNECT_SECONDS: u64 = 10;
const SSH_FAILED: i32 = 255;
/// The end of what a command said is what a person needs: the reason is last.
const MOST_OUTPUT: usize = 8 * 1024;
/// Reads `stream` to its end and keeps only the last `most` bytes, so what a guest says over a long
/// time takes no more room than that.
fn read_tail(stream: &mut impl Read, most: usize) -> Vec<u8> {
    let (mut kept, mut piece) = (Vec::new(), [0u8; 8192]);
    while let Ok(read) = stream.read(&mut piece) {
        if read == 0 {
            break;
        }
        kept.extend_from_slice(&piece[..read]);
        if kept.len() > most * 2 {
            kept.drain(..kept.len() - most);
        }
    }
    let start = kept.len().saturating_sub(most);
    kept.split_off(start)
}

const WATCH: Duration = Duration::from_millis(100);

pub struct ShellRequest<'a> {
    pub port: u16,
    pub key: &'a Path,
    pub known_hosts: &'a Path,
    pub command: &'a str,
    pub input: &'a [u8],
    pub seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellAnswer {
    /// SSH never got a shell: the guest isn't up, or its SSH isn't, or the key isn't one it knows.
    Unreachable(String),
    /// The command was still going when the time was up, and was stopped.
    TimedOut,
    /// What the command said, its errors included, and its exit status.
    Ran { status: i32, output: String },
}

pub trait GuestShell: Send + Sync {
    fn run(&self, request: &ShellRequest) -> ShellAnswer;
}

/// The program is `ssh` unless a test says another.
pub struct SshShell {
    pub program: PathBuf,
}

impl Default for SshShell {
    fn default() -> Self {
        Self { program: PathBuf::from("ssh") }
    }
}

/// A value in ssh's own options, which reads a space or a quote as the end of the value.
fn option_safe(path: &Path) -> Option<&str> {
    path.to_str().filter(|text| !text.chars().any(|c| c.is_control() || c == '"'))
}

fn tail(mut bytes: Vec<u8>) -> String {
    if bytes.len() > MOST_OUTPUT {
        bytes.drain(..bytes.len() - MOST_OUTPUT);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

impl GuestShell for SshShell {
    fn run(&self, request: &ShellRequest) -> ShellAnswer {
        let (Some(key), Some(known_hosts)) = (option_safe(request.key), option_safe(request.known_hosts))
        else {
            return ShellAnswer::Unreachable(
                "The machine's folder has a character SSH can't take in a path.".into(),
            );
        };
        let mut child = match Command::new(&self.program)
            // The user's own configuration, agent and keys are not this machine's.
            .args(["-F", "/dev/null", "-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none"])
            .args(["-o", "BatchMode=yes", "-o", "RequestTTY=no", "-o", "ControlPath=none"])
            // The key the guest has is one only the machine's own install made, and its host key is
            // learned on the first connection of an install and kept in the machine's folder.
            .args(["-o", "StrictHostKeyChecking=accept-new", "-o", "LogLevel=ERROR"])
            .args(["-o", &format!("UserKnownHostsFile=\"{known_hosts}\"")])
            .args(["-o", &format!("ConnectTimeout={CONNECT_SECONDS}"), "-o", "ServerAliveInterval=30"])
            .args(["-i", key, "-p", &request.port.to_string(), "root@127.0.0.1", request.command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => return ShellAnswer::Unreachable(format!("ssh could not run: {error}")),
        };
        // The input goes from a thread of its own, as a guest that stops reading would hold this one.
        let input = request.input.to_vec();
        let stdin = child.stdin.take();
        let feeder = thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(&input);
            }
        });
        let readers: Vec<_> = [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .map(|mut stream| thread::spawn(move || read_tail(&mut stream, MOST_OUTPUT)))
        .collect();
        let end = Instant::now() + Duration::from_secs(request.seconds);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < end => thread::sleep(WATCH),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let _ = feeder.join();
        let output: Vec<u8> =
            readers.into_iter().flat_map(|reader| reader.join().unwrap_or_default()).collect();
        let output = tail(output);
        match status.map(|status| status.code().unwrap_or(SSH_FAILED)) {
            None => ShellAnswer::TimedOut,
            Some(SSH_FAILED) => ShellAnswer::Unreachable(output.trim().to_owned()),
            Some(status) => ShellAnswer::Ran { status, output },
        }
    }
}

impl Machine {
    /// The key the guest knows this machine's host by, and the file its host key is learned into.
    fn ssh_files(&self, context: &Context) -> (PathBuf, PathBuf) {
        let folder = self.folder(context).join(SSH_FOLDER);
        (folder.join(KEY_FILE), folder.join(KNOWN_HOSTS_FILE))
    }

    pub fn has_ssh_key(&self, context: &Context) -> bool {
        self.ssh_files(context).0.is_file()
    }

    /// The host key this machine's guest had, which a new install no longer has.
    pub fn forget_guest_host_key(&self, context: &Context) {
        let _ = std::fs::remove_file(self.ssh_files(context).1);
    }
}

impl Context {
    /// Runs a fixed command in this machine's Ubuntu guest, as root, with `input` on its stdin.
    pub fn ask_shell(&self, machine: &Machine, command: &str, input: &[u8], seconds: u64) -> ShellAnswer {
        let (key, known_hosts) = machine.ssh_files(self);
        if !key.is_file() {
            return ShellAnswer::Unreachable("This machine has no SSH key of its own.".into());
        }
        let port = ssh_port(self.slot);
        self.shell.run(&ShellRequest { port, key: &key, known_hosts: &known_hosts, command, input, seconds })
    }
}

pub mod stand_in {
    //! A shell that answers from a list and remembers what it was asked.

    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// What one request was: the command and what went in on stdin.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Asked {
        pub command: String,
        pub input: Vec<u8>,
    }

    #[derive(Default)]
    pub struct ScriptedShell {
        pub answers: Mutex<VecDeque<ShellAnswer>>,
        pub requests: Mutex<Vec<Asked>>,
        /// What it says when the list has run out.
        pub otherwise: Mutex<Option<ShellAnswer>>,
    }

    impl ScriptedShell {
        pub fn answering(answers: Vec<ShellAnswer>) -> Self {
            Self { answers: Mutex::new(answers.into()), ..Self::default() }
        }

        pub fn asked(&self) -> Vec<Asked> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl GuestShell for ScriptedShell {
        fn run(&self, request: &ShellRequest) -> ShellAnswer {
            let asked = Asked { command: request.command.to_owned(), input: request.input.to_vec() };
            self.requests.lock().unwrap().push(asked);
            let next = self.answers.lock().unwrap().pop_front();
            next.or_else(|| self.otherwise.lock().unwrap().clone())
                .unwrap_or(ShellAnswer::Unreachable("nothing answers".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};
    use std::fs;

    #[test]
    fn a_guest_that_never_stops_talking_leaves_only_the_end_of_it() {
        let mut endless = std::io::repeat(b'x').take(64 << 20).chain(&b"the end"[..]);
        let kept = read_tail(&mut endless, MOST_OUTPUT);
        assert_eq!(kept.len(), MOST_OUTPUT);
        assert!(kept.ends_with(b"the end"));
        assert_eq!(read_tail(&mut &b"short"[..], MOST_OUTPUT), b"short");
    }

    /// An `ssh` that writes down its arguments and stdin, and then does what the test's first word said.
    fn fake_ssh(scratch: &Scratch, behaviour: &str) -> SshShell {
        let text = format!(
            "#!/bin/sh\nfor word in \"$@\"; do printf '%s\\n' \"$word\"; done > {args}\ncat > {stdin}\n{behaviour}\n",
            args = scratch.path("args").display(),
            stdin = scratch.path("stdin").display()
        );
        SshShell { program: scratch.program("ssh", &text) }
    }

    fn request<'a>(command: &'a str, input: &'a [u8], seconds: u64, key: &'a Path) -> ShellRequest<'a> {
        ShellRequest { port: 2228, key, known_hosts: key, command, input, seconds }
    }

    #[test]
    fn the_command_is_one_argument_and_the_input_goes_in_on_stdin() {
        let scratch = Scratch::new();
        let shell = fake_ssh(&scratch, "echo said; exit 3");
        let hostile = "x; rm -rf / $(id) `id`";
        let key = scratch.path("id");
        let answer = shell.run(&request("sh /var/lib/x/run.sh ready", hostile.as_bytes(), 20, &key));
        assert_eq!(answer, ShellAnswer::Ran { status: 3, output: "said\n".into() });
        let args = fs::read_to_string(scratch.path("args")).unwrap();
        let lines: Vec<&str> = args.lines().collect();
        assert_eq!(lines.last(), Some(&"sh /var/lib/x/run.sh ready"));
        assert_eq!(lines[lines.len() - 2], "root@127.0.0.1");
        for needed in ["-F", "/dev/null", "IdentityAgent=none", "IdentitiesOnly=yes", "BatchMode=yes", "2228"]
        {
            assert!(lines.contains(&needed), "{needed} in {lines:?}");
        }
        assert_eq!(fs::read_to_string(scratch.path("stdin")).unwrap(), hostile);
    }

    #[test]
    fn ssh_failing_is_the_guest_not_being_there_and_a_late_command_is_stopped() {
        let scratch = Scratch::new();
        let key = scratch.path("id");
        let down = fake_ssh(&scratch, "echo 'Connection refused' >&2; exit 255");
        let answer = down.run(&request("true", b"", 20, &key));
        assert_eq!(answer, ShellAnswer::Unreachable("Connection refused".into()));
        let slow = fake_ssh(&scratch, "exec sleep 30");
        assert_eq!(slow.run(&request("true", b"", 1, &key)), ShellAnswer::TimedOut);
    }

    #[test]
    fn only_the_end_of_a_long_answer_is_kept() {
        let scratch = Scratch::new();
        let key = scratch.path("id");
        let shell = fake_ssh(&scratch, "head -c 20000 /dev/zero | tr '\\0' a; echo END");
        let ShellAnswer::Ran { output, .. } = shell.run(&request("true", b"", 20, &key)) else { panic!() };
        assert!(output.len() <= MOST_OUTPUT && output.ends_with("END\n"));
    }

    #[test]
    fn a_machine_with_no_key_has_no_way_in() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let vm = Machine::new("box", "Box", "/d", "");
        assert!(matches!(context.ask_shell(&vm, "true", b"", 5), ShellAnswer::Unreachable(_)));
    }

    #[test]
    fn a_path_ssh_would_read_wrong_is_refused_before_ssh_runs() {
        let scratch = Scratch::new();
        let shell = fake_ssh(&scratch, "exit 0");
        let odd = scratch.path("a\"b");
        let answer = shell.run(&request("true", b"", 5, &odd));
        assert!(matches!(answer, ShellAnswer::Unreachable(_)));
        assert!(!scratch.path("args").exists(), "ssh was not run");
    }
}
