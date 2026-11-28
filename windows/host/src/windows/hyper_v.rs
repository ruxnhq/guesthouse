//! `dism.exe` from System32, for Windows' Hyper-V feature. What it's run with and how what it says is
//! read are [`crate::hyper_v`]'s.

use crate::hyper_v::{Dism, Ran};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Run with no window, as the service has none to show it in.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long a read of the feature has before it's stopped, as servicing that's busy can hold it.
/// Turning it on is never stopped, as stopping DISM part way through could leave Windows' servicing
/// half done.
const READ_LIMIT: Duration = Duration::from_secs(120);
/// How much of the end of what DISM has printed is handed on each time, enough for its progress line.
const TAIL: usize = 200;
/// How long it's waited for between looks.
const POLL: Duration = Duration::from_millis(100);
/// The most of its answer that's kept.
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

/// `%SystemRoot%\System32\dism.exe`, run by its full path, with its arguments as they are and no
/// shell between.
pub struct System32Dism;

impl Dism for System32Dism {
    fn run(&self, args: &[String], seen: &mut dyn FnMut(&str)) -> Result<Ran, String> {
        let reading = !args.iter().any(|arg| arg.eq_ignore_ascii_case("/Enable-Feature"));
        let mut child = Command::new(system_root().join("System32").join("dism.exe"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("dism.exe couldn't start: {error}"))?;
        // What it prints comes to this thread piece by piece, so its progress is passed on as it goes.
        let (pieces, inbox) = mpsc::channel::<Vec<u8>>();
        let stdout = child.stdout.take();
        let reader = thread::spawn(move || {
            let Some(mut stdout) = stdout else { return };
            let mut buffer = [0u8; 512];
            while let Ok(read) = stdout.read(&mut buffer) {
                if read == 0 || pieces.send(buffer[..read].to_vec()).is_err() {
                    return;
                }
            }
        });
        let mut output = Vec::new();
        let until = Instant::now() + READ_LIMIT;
        let status = loop {
            loop {
                match inbox.recv_timeout(POLL) {
                    Ok(piece) => {
                        if output.len() < OUTPUT_LIMIT {
                            output.extend_from_slice(&piece);
                        }
                        let tail = &output[output.len().saturating_sub(TAIL)..];
                        seen(&String::from_utf8_lossy(tail));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    // It has closed its output, and is ending.
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        thread::sleep(POLL);
                        break;
                    }
                }
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if reading && Instant::now() >= until => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "DISM didn't say how Hyper-V stands in {} s, so it was stopped.",
                        READ_LIMIT.as_secs()
                    ));
                }
                Ok(None) => {}
                Err(error) => return Err(format!("dism.exe couldn't be waited for: {error}")),
            }
        };
        let _ = reader.join();
        while let Ok(piece) = inbox.try_recv() {
            if output.len() < OUTPUT_LIMIT {
                output.extend_from_slice(&piece);
            }
        }
        let code = status.code().ok_or("dism.exe ended without an exit code.")?;
        Ok(Ran { code, output: String::from_utf8_lossy(&output).into_owned() })
    }

    fn log(&self) -> String {
        system_root().join(r"Logs\DISM\dism.log").display().to_string()
    }
}
