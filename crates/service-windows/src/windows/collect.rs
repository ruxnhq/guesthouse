//! The facts about the PC a bug report carries that only the service can read: the Windows version,
//! the state of the services the guests need, the Hyper-V events about Guesthouse's machines, and
//! its crashes. Each is a [`Part`], gathered on its own, so one that fails is only missing.

use crate::report_parts::{keep_events, registry_values};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// CREATE_NO_WINDOW, so a program run by the service never opens a console.
const NO_WINDOW: u32 = 0x0800_0000;
/// How long a program may run before it is killed, unless a part says otherwise.
const LIMIT: Duration = Duration::from_secs(20);

/// One file of a report: its name, and its text, or in a plain sentence why it is missing.
pub struct Part {
    pub name: String,
    pub text: Result<String, String>,
}

/// A program of Windows' own, by its full path, so nothing else on PATH can stand in for it.
fn system_program(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

/// Runs a program of Windows' own with fixed arguments, no shell, and gives what it printed. It is
/// killed if it runs longer than `limit`, and a non-zero exit is an error with the first line it
/// said on stderr.
fn run(program: &str, args: &[&str], limit: Duration) -> Result<String, String> {
    let mut child = Command::new(system_program(program))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(NO_WINDOW)
        .spawn()
        .map_err(|error| format!("{program} could not be started: {error}."))?;
    // Read both pipes on threads, so a program that fills one can't stall waiting for us.
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} ran longer than {} seconds and was stopped.",
                    limit.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(format!("{program} could not be waited for: {error}.")),
        }
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if status.success() {
        return Ok(stdout);
    }
    // Some programs say why on stdout instead.
    let said = [stderr.as_str(), stdout.as_str()]
        .into_iter()
        .flat_map(str::lines)
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it said nothing");
    Err(format!("{program} failed ({status}): {said}"))
}

/// A thread that reads a pipe to its end.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

/// A part from a name and what gathering it gave.
fn part(name: &str, text: Result<String, String>) -> Part {
    Part { name: name.to_string(), text }
}

/// Everything about the host a report carries, each part on its own.
pub fn host_parts() -> Vec<Part> {
    vec![
        part("windows.txt", windows_version()),
        part("services.txt", services()),
        part(
            "virtual-machine-platform.txt",
            run(
                "dism.exe",
                &["/English", "/Online", "/Get-FeatureInfo", "/FeatureName:VirtualMachinePlatform"],
                Duration::from_secs(60),
            ),
        ),
        part("hyper-v-events.txt", hyper_v_events()),
        part("crashes.txt", crashes()),
        part("wsl.txt", wsl()),
    ]
}

/// The Windows edition, version and build, from the registry, as `Name: value` lines.
fn windows_version() -> Result<String, String> {
    let text = run("reg.exe", &["query", r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion"], LIMIT)?;
    let values =
        registry_values(&text, &["ProductName", "EditionID", "DisplayVersion", "CurrentBuild", "UBR"]);
    if values.is_empty() {
        return Err("The registry had none of the Windows version's values.".to_string());
    }
    Ok(values.iter().map(|(name, value)| format!("{name}: {value}\n")).collect())
}

/// Whether the compute service, the host network service and Guesthouse's own are running, each
/// as `sc query` says, under a heading. A service that can't be asked about says why under its own.
fn services() -> Result<String, String> {
    let mut text = String::new();
    for service in ["vmcompute", "hns", "Guesthouse"] {
        text.push_str(&format!("== {service} ==\n"));
        match run("sc.exe", &["query", service], LIMIT) {
            Ok(said) => text.push_str(said.trim()),
            Err(why) => text.push_str(&why),
        }
        text.push_str("\n\n");
    }
    Ok(text)
}

/// The latest events of Hyper-V's logs that mention a Guesthouse machine, a heading for each log.
/// These logs also hold WSL's and Docker's, so only the events naming `guesthouse-` are kept. A log
/// that is missing or can't be read is said so under its heading.
fn hyper_v_events() -> Result<String, String> {
    let mut text = String::new();
    for channel in [
        "Microsoft-Windows-Hyper-V-Compute-Admin",
        "Microsoft-Windows-Hyper-V-Compute-Operational",
        "Microsoft-Windows-Hyper-V-Worker-Admin",
        "Microsoft-Windows-Hyper-V-VmSwitch-Operational",
    ] {
        text.push_str(&format!("== {channel} ==\n"));
        match run("wevtutil.exe", &["qe", channel, "/c:300", "/rd:true", "/f:text"], LIMIT) {
            Ok(said) => {
                let kept = keep_events(&said, "guesthouse-");
                if kept.is_empty() {
                    text.push_str("No events about a Guesthouse machine.");
                } else {
                    text.push_str(&kept);
                }
            }
            Err(why) => text.push_str(&format!("Could not be read: {why}")),
        }
        text.push_str("\n\n");
    }
    Ok(text)
}

/// The latest application crash and error-report events that mention Guesthouse.
fn crashes() -> Result<String, String> {
    let said = run(
        "wevtutil.exe",
        &[
            "qe",
            "Application",
            "/q:*[System[(EventID=1000 or EventID=1001)]]",
            "/c:200",
            "/rd:true",
            "/f:text",
        ],
        LIMIT,
    )?;
    let kept = keep_events(&said, "guesthouse");
    Ok(if kept.is_empty() {
        "No crashes of Guesthouse in the latest 200 application crash events.\n".to_string()
    } else {
        kept
    })
}

/// Whether WSL's `libdxcore.so` is on the PC, which an Ubuntu guest's CUDA needs, in either place
/// it may be.
fn wsl() -> Result<String, String> {
    let places = [
        PathBuf::from(r"C:\Program Files\WSL\lib\libdxcore.so"),
        system_program("lxss").join(r"lib\libdxcore.so"),
    ];
    Ok(places.iter().map(|place| format!("{}: {}\n", place.display(), found(place))).collect())
}

/// Whether a file is there, in a word.
fn found(path: &Path) -> &'static str {
    if path.exists() { "present" } else { "missing" }
}
