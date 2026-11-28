//! Tests that run the guest's PowerShell scripts for real, against folders that stand in for the
//! driver store, the disc and Windows' own folders.
//!
//! In a guest the scripts run under Windows PowerShell 5.1. `GUESTHOUSE_GUEST_SHELL` names the shell
//! these tests run them with, `powershell` or `pwsh`, and then a shell that isn't there fails every
//! test. Without it they use pwsh where it is installed and do nothing where it isn't, as on a Linux
//! PC without PowerShell. CI's Windows job runs them under both shells. Nothing here needs the app, so
//! the crate builds in seconds on Windows.
//!
//! The tests that need Windows itself, as a file in use or the registry, run only on Windows.
#![cfg(test)]

mod agent;
mod ctl_channel;
mod display_settings;
mod driver_certificate;
mod guest_rights;
mod host_rules;
mod nv_files;
mod nv_store;
mod screen;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

const SHELL_VARIABLE: &str = "GUESTHOUSE_GUEST_SHELL";

/// The shell the scripts run in, or None when none was asked for and pwsh isn't installed.
fn shell() -> Option<&'static str> {
    static FOUND: OnceLock<Option<&'static str>> = OnceLock::new();
    *FOUND.get_or_init(|| match std::env::var(SHELL_VARIABLE).as_deref() {
        Ok("powershell") => Some(must_run("powershell")),
        Ok("pwsh") => Some(must_run("pwsh")),
        Ok("") | Err(_) => version("pwsh").map(|_| "pwsh"),
        Ok(other) => panic!("{SHELL_VARIABLE} is {other:?}: it can be powershell or pwsh"),
    })
}

fn version(program: &str) -> Option<String> {
    let done = shell_command(program)
        .args(["-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.ToString()"])
        .output()
        .ok()?;
    done.status.success().then(|| String::from_utf8_lossy(&done.stdout).trim().to_owned())
}

fn must_run(program: &'static str) -> &'static str {
    match version(program) {
        Some(found) => {
            eprintln!("the guest's scripts run under {program} {found}");
            program
        }
        None => panic!("{SHELL_VARIABLE} asks for {program}, and it doesn't run here"),
    }
}

/// Each shell finds its own modules: a Windows PowerShell started under pwsh, as CI starts these tests,
/// would otherwise look in pwsh's module folders first.
fn shell_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env_remove("PSModulePath");
    command
}

/// What a run of the shell said, with everything a failed assertion should show.
struct Said {
    stdout: String,
    stderr: String,
    code: Option<i32>,
    command: String,
}

impl Said {
    fn lines(&self) -> Vec<&str> {
        self.stdout.lines().map(str::trim_end).filter(|line| !line.is_empty()).collect()
    }

    fn last(&self) -> &str {
        self.lines().last().copied().unwrap_or("")
    }
}

impl std::fmt::Display for Said {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\n--- command\n{}\n--- exit code {:?}\n--- stdout\n{}\n--- stderr\n{}",
            self.command, self.code, self.stdout, self.stderr
        )
    }
}

/// Runs PowerShell commands in a shell of their own, as the guest's agent runs a script.
fn run(shell: &str, command: &str) -> Said {
    let done: Output = shell_command(shell)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", command])
        .output()
        .unwrap();
    Said {
        stdout: String::from_utf8_lossy(&done.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&done.stderr).into_owned(),
        code: done.status.code(),
        command: command.to_owned(),
    }
}

/// A path as a PowerShell string that takes it literally.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

fn guest_script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../windows").join(name)
}

/// PowerShell that loads these functions, and nothing else, from the script at `script`, as a script
/// that runs Windows' own commands at its end can't be run whole in a test.
fn functions_from(script: &Path, functions: &[&str]) -> String {
    let names = functions.iter().map(|name| format!("'{name}'")).collect::<Vec<_>>().join(", ");
    format!(
        "$script = [System.Management.Automation.Language.Parser]::ParseFile({}, [ref]$null, [ref]$null)
        foreach ($name in {names}) {{
            $found = $script.Find({{ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }}, $true)
            if (-not $found) {{ throw \"no function $name\" }}
            . ([scriptblock]::Create($found.Extent.Text))
        }}
        ",
        quote(script)
    )
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A folder that goes when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let folder =
            std::env::temp_dir().join(format!("guesthouse-guest-scripts-{}-{number}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        Self(folder)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Each file under a folder by its path inside it, with `/` between the parts on every system.
fn names(folder: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(folder).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            found.extend(names(&entry.path()).into_iter().map(|inner| format!("{name}/{inner}")));
        } else {
            found.push(name);
        }
    }
    found.sort();
    found
}
