//! A bug report, made and sent by the app. The service only hands over the files only it can read.
//! The app, which runs as the user, scrubs them, adds its own, writes them into a folder of the
//! user's own, packs the folder, and sends the archive, all on threads of their own, as each can
//! take a while. What the files hold, and what is cut or left out, is `guesthouse-control`'s, which
//! is tested on every system. This is the part that touches the PC.
//!
//! Nothing is sent but by `send`, which the person asks for once they've seen what's in the report.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use guesthouse_control::client;
use guesthouse_control::report::stage::{self, FIRST_LIMIT, Facts, MOST_REPACKS};
use guesthouse_control::report::{
    MOST_REPORT_BYTES, Parts, ProblemKind, Scrubber, Summary, clock, report_address, send as sending,
};
use guesthouse_control::{Answer, Request, System, WINDOWS_VERSION};

use crate::app::Message;
use crate::bridge::Window;

/// Starts a program with no console window of its own, as the app has none to share.
const NO_WINDOW: u32 = 0x0800_0000;
/// What the report's folder and archive are called, with the time after it.
const NAME: &str = "guesthouse-report-";
/// What the service's files are called in what's missing, when none of them came.
const SERVICE_FILES: &str = "the service's files";

/// What the person chose and wrote, and what the app knows of the PC.
#[derive(Debug, Clone)]
pub struct Asked {
    /// The machine the problem is with, by name, if any.
    pub machine: Option<String>,
    /// Its system, if the last status had it.
    pub machine_system: Option<System>,
    /// Every machine's name, for the scrubber.
    pub machines: Vec<String>,
    /// The default card's name, as the last status said.
    pub card: Option<String>,
    pub kind: ProblemKind,
    pub description: String,
    pub contact: String,
}

/// A report that's been made and is waiting for the person to send or save it.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub archive: PathBuf,
    /// Each file in it with its size in bytes.
    pub files: Vec<(String, usize)>,
    /// Each part that isn't in it, and why.
    pub missing: Vec<(String, String)>,
    pub summary: Summary,
    /// The archive's size.
    pub bytes: u64,
}

/// Where reports are made: a folder of the user's own in their temporary folder.
fn root() -> PathBuf {
    std::env::temp_dir().join("guesthouse-app").join("reports")
}

/// Deletes the reports made so far, once they're no longer wanted.
pub fn clear() {
    let _ = fs::remove_dir_all(root());
}

/// Makes the report on a thread of its own, and tells the window how it went.
pub fn prepare(asked: Asked, window: Window) {
    let failed = window.clone();
    let spawned = thread::Builder::new().name("report".into()).spawn(move || {
        let _ = window.tell(Message::ReportPrepared(make(&asked)));
    });
    if let Err(error) = spawned {
        let _ =
            failed.tell(Message::ReportPrepared(Err(format!("The app couldn't start the work: {error}"))));
    }
}

/// Sends the archive on a thread of its own, and tells the window the ID it was given, or why not.
pub fn send(prepared: &Prepared, window: Window) {
    let failed = window.clone();
    let prepared = prepared.clone();
    let spawned = thread::Builder::new().name("report send".into()).spawn(move || {
        let _ = window.tell(Message::ReportSent(post(&prepared)));
    });
    if let Err(error) = spawned {
        let _ = failed.tell(Message::ReportSent(Err(format!("The app couldn't start sending: {error}"))));
    }
}

/// Puts a copy of the archive where the person chose, and says where it went.
pub fn save(prepared: &Prepared, to: &Path) -> Result<PathBuf, String> {
    fs::copy(&prepared.archive, to)
        .map(|_| to.to_owned())
        .map_err(|error| format!("The report couldn't be saved to {}: {error}", to.display()))
}

fn make(asked: &Asked) -> Result<Prepared, String> {
    let parts = gather(asked);
    let env = |key: &str| std::env::var(key).unwrap_or_default();
    // The machine the report is about is named first, so it's the one that's called machine.
    let mut names: Vec<String> = asked.machine.iter().cloned().collect();
    names.extend(asked.machines.iter().filter(|name| Some(*name) != asked.machine.as_ref()).cloned());
    let scrubber = Scrubber::new(&env("USERNAME"), &env("USERPROFILE"), &env("COMPUTERNAME"), &names);
    let facts = Facts {
        version: WINDOWS_VERSION,
        windows: None,
        card: asked.card.as_deref(),
        machine: asked.machine_system,
        kind: asked.kind,
        description: &asked.description,
        contact: &asked.contact,
    };
    let root = root();
    let mut limit = FIRST_LIMIT;
    for _ in 0..=MOST_REPACKS {
        let staged = stage::stage(&parts, &scrubber, &facts, limit);
        let name = format!("{NAME}{}", clock::stamp(clock::now()));
        let _ = fs::remove_dir_all(&root);
        let folder = root.join(&name);
        write_files(&folder, &staged.files)?;
        let archive = root.join(format!("{name}.tar.gz"));
        pack(&root, &name, &archive)?;
        let bytes = fs::metadata(&archive).map_err(|error| format!("{}: {error}", archive.display()))?.len();
        if bytes <= MOST_REPORT_BYTES {
            return Ok(Prepared {
                archive,
                files: staged.sizes(),
                missing: staged.missing,
                summary: staged.summary,
                bytes,
            });
        }
        limit = stage::halved(limit);
    }
    clear();
    Err(format!(
        "The report is over {} MB even with its files cut short, so it can't be sent.",
        MOST_REPORT_BYTES / 1_000_000
    ))
}

/// The service's files, and the app's own. A service that can't be reached, or says no, is only a
/// part that's missing: the rest of the report is still made.
fn gather(asked: &Asked) -> Parts {
    let mut parts = Parts::default();
    let mut refused = None;
    let streamed = client::stream(&Request::ReportFiles { machine: asked.machine.clone() }, |answer| {
        match answer {
            Answer::ReportPart { name, text } => parts.piece(&name, &text),
            Answer::ReportMissing { name, reason } => parts.miss(&name, &reason),
            Answer::Done { .. } => return false,
            Answer::Refused { reason } => {
                refused = Some(reason);
                return false;
            }
            _ => {}
        }
        true
    });
    match (refused, streamed) {
        (Some(reason), _) => parts.miss(SERVICE_FILES, &reason),
        (None, Err(unreached)) => parts.miss(SERVICE_FILES, unreached.words()),
        (None, Ok(())) => {}
    }
    parts.piece("app.txt", &app_text());
    match fs::read_to_string(crate::system::panic_file()) {
        Ok(text) => parts.piece("panic.txt", &text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => parts.miss("panic.txt", &format!("It couldn't be read: {error}")),
    }
    parts
}

fn app_text() -> String {
    let installed = if crate::system::installed() { "yes" } else { "no" };
    format!("version: {WINDOWS_VERSION}\ninstalled: {installed}\n")
}

fn write_files(folder: &Path, files: &[(String, String)]) -> Result<(), String> {
    for (name, text) in files {
        let path = folder.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

/// A program of Windows' own, by its full path, so nothing else on the path can stand in for it.
fn system_program(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

/// Packs `name`, a folder in `parent`, into `archive` with the tar Windows has had since 10.
fn pack(parent: &Path, name: &str, archive: &Path) -> Result<(), String> {
    let output = Command::new(system_program("tar.exe"))
        .arg("-czf")
        .arg(archive)
        .arg("-C")
        .arg(parent)
        .arg(name)
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|error| format!("tar couldn't run, so the report wasn't packed: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("it said nothing");
    Err(format!("tar couldn't pack the report: {said}"))
}

/// Posts the archive to the report server with the curl Windows has, as the Linux host does.
fn post(prepared: &Prepared) -> Result<String, String> {
    if prepared.bytes > MOST_REPORT_BYTES {
        return Err("The report is too big to send. Save it to a file instead.".to_owned());
    }
    let answer = prepared.archive.with_extension("answer");
    let summary = prepared.summary.header();
    let _ = fs::remove_file(sending::summary_file(&prepared.archive));
    sending::write_summary_file(&prepared.archive, &summary)
        .map_err(|error| format!("The report's summary couldn't be written, so nothing was sent: {error}"))?;
    let args = sending::curl_arguments(&report_address(), &summary, &prepared.archive, &answer);
    let result = Command::new(system_program("curl.exe")).args(args).creation_flags(NO_WINDOW).output();
    let body = fs::read_to_string(&answer).unwrap_or_default();
    let _ = fs::remove_file(&answer);
    let _ = fs::remove_file(sending::summary_file(&prepared.archive));
    let output = result.map_err(|error| format!("curl couldn't run, so nothing was sent: {error}"))?;
    sending::read_reply(
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
        output.status.success(),
        &body,
    )
}
