//! The guest's agent: PowerShell over a socket, with each machine's own token.

use crate::context::Context;
use crate::driver_modes::{DriverModes, Mode};
use crate::failure::{Failure, Result};
use crate::guest_end::GuestEnd;
use crate::guest_files::Helper;
use crate::machine::Machine;
use crate::random::random_hex;
use crate::text::{base64_decode, base64_encode};
use guesthouse_daemon::slot::Slot;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

const CONNECT_SECONDS: u64 = 5;
/// Far more than any script of ours prints.
const MOST_ANSWER: usize = 4 << 20;
const ANSWER_CHUNK: usize = 64 << 10;
const TOKEN_BYTES: usize = 16;
/// The most text the clipboard passes either way.
pub const MOST_CLIPBOARD_TEXT: usize = 1 << 20;
pub const QUICK_SECONDS: u64 = 10;
const CLIPBOARD_START_SECONDS: u64 = 20;
const WINDOW_SIZE_START_SECONDS: u64 = 20;
/// A new size took about a second in a guest. The helper itself gives up on Windows after four.
const WINDOW_SIZE_SECONDS: u64 = 12;
/// What the helper says, before the size, when Windows has taken it.
const SIZE_SET: &str = "set";
/// What the helper says when an app in the guest is fullscreen, and the screen was left alone.
const SIZE_HELD: &str = "held";
/// The first start on a machine also opens the helper's port in Windows' firewall.
const FILES_START_SECONDS: u64 = 30;
/// The helpers in the user's session, by the name their file, start script and task share.
const CLIPBOARD_HELPER: &str = "clipboard";
const FILES_HELPER: &str = "files";
const WINDOW_SIZE_HELPER: &str = "window-size";
const HELPER_STOP_SCRIPT: &str = "helper-stop.ps1";

pub trait GuestAgent: Send + Sync {
    /// Runs PowerShell in the guest, and returns what it said. Nothing if the agent isn't up yet, or
    /// the machine's QEMU isn't there to reach it through.
    fn ask(&self, end: &GuestEnd, token: &str, script: &str, seconds: u64) -> String;
}

pub struct TcpAgent;

impl GuestAgent for TcpAgent {
    fn ask(&self, end: &GuestEnd, token: &str, script: &str, seconds: u64) -> String {
        talk(end, token, script, seconds).unwrap_or_default()
    }
}

fn talk(end: &GuestEnd, token: &str, script: &str, seconds: u64) -> std::io::Result<String> {
    let mut stream = end.connect(Duration::from_secs(CONNECT_SECONDS))?;
    stream.set_write_timeout(Some(Duration::from_secs(seconds.max(1))))?;
    writeln!(stream, "{token} {}", base64_encode(script.as_bytes()))?;
    let answer = read_answer(&mut stream, Duration::from_secs(seconds))?;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// As much of the answer as comes in `patience`, up to the most an answer can be. Each read has what
/// is left of the time, so an answer that keeps trickling ends as one that stops does.
fn read_answer(stream: &mut TcpStream, patience: Duration) -> std::io::Result<Vec<u8>> {
    let end = Instant::now() + patience;
    let mut answer = Vec::new();
    let mut chunk = [0; ANSWER_CHUNK];
    while answer.len() < MOST_ANSWER {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        stream.set_read_timeout(Some(left))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(size) => answer.extend_from_slice(&chunk[..size]),
            // A read with a timeout isn't restarted for us, and this says nothing about the answer.
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
            Err(error) => return Err(error),
        }
    }
    answer.truncate(MOST_ANSWER);
    Ok(answer)
}

/// The machine's token, made the first time. Only an install calls this: the guest learns the token
/// from the install disc, so a new one for a machine already installed would lock the app out of it.
pub fn make_token(context: &Context, machine: &Machine) -> Result<String> {
    make_secret(&machine.token_file(context))
}

/// The Windows account's password, made the first time. It logs in by itself, so nobody has to know
/// it, but it's kept in the machine's folder for whoever wants to.
pub fn make_password(context: &Context, machine: &Machine) -> Result<String> {
    make_secret(&machine.password_file(context))
}

/// The key the machine's shared folder is served under, made the first time. It stays, so an address
/// the guest keeps, such as a Mac's in Finder, keeps working.
pub fn share_key(context: &Context, machine: &Machine) -> Result<String> {
    make_secret(&machine.share_key_file(context))
}

/// Where the guest finds the machine's shared folder.
pub fn share_address(key: &str) -> String {
    format!("http://{}/{key}/", crate::vm_args::SHARE_ADDRESS)
}

/// A file only this user can read, with a random value that stays.
fn make_secret(path: &Path) -> Result<String> {
    if !path.exists() {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        writeln!(file, "{}", random_hex::<TOKEN_BYTES>())?;
    }
    Ok(fs::read_to_string(path)?.trim().to_owned())
}

/// Takes a token from a file the user chose, for a guest installed before each machine had its own.
/// It's done once: a machine that has a token keeps it.
pub fn import_token(context: &Context, machine: &Machine, file: &Path) -> Result<()> {
    let text = fs::read_to_string(file)?;
    let token = text.trim();
    if token.len() != TOKEN_BYTES * 2 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Failure::new("This file does not hold a guest token."));
    }
    let path = machine.token_file(context);
    if path.exists() {
        return Err(Failure::new("This machine already has its token."));
    }
    let mut new = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    writeln!(new, "{token}")?;
    Ok(())
}

pub(crate) fn read_token(context: &Context, machine: &Machine) -> Option<String> {
    // A throwaway run's guest is moved to a token of its own at each start.
    let text = fs::read_to_string(crate::token_change::token_in_use(context, machine)).ok()?;
    Some(text.trim().to_owned())
}

/// A script from the program folder.
pub fn guest_script(context: &Context, name: &str) -> Result<String> {
    Ok(fs::read_to_string(context.paths.guest_files().join(name))?)
}

impl Context {
    /// Asks this machine's agent. Nothing comes back if the agent isn't up, or the machine has no
    /// token, which from the app's side is the same thing.
    pub fn ask_guest(&self, machine: &Machine, script: &str, seconds: u64, slot: Option<Slot>) -> String {
        let Some(token) = read_token(self, machine) else { return String::new() };
        let end = GuestEnd::agent(self, machine, slot);
        let answer = self.agent.ask(&end, &token, script, seconds);
        if !answer.is_empty() {
            return answer;
        }
        match crate::token_change::recovered_token(self, machine, &end) {
            Some(token) => self.agent.ask(&end, &token, script, seconds),
            None => answer,
        }
    }
}

/// The agent's own file, which setup puts in the guest.
const AGENT_SCRIPT: &str = "agent.ps1";
const AGENT_UPDATE_SCRIPT: &str = "agent-update.ps1";
/// Where setup puts it in the guest, and the guest's task runs it from.
const AGENT_IN_GUEST: &str = "C:\\gpupv\\agent.ps1";

/// Puts this app's agent in the guest in place of an earlier one, for the guest to run from its next
/// boot. What the guest said, or why nothing was sent.
pub fn keep_agent_current(context: &Context, machine: &Machine, slot: Option<Slot>) -> String {
    let script =
        fs::read(context.paths.guest_files().join(AGENT_SCRIPT)).map_err(Failure::from).and_then(|agent| {
            Ok(format!(
                "$agent = '{}'\n$path = '{AGENT_IN_GUEST}'\n{}",
                base64_encode(&agent),
                guest_script(context, AGENT_UPDATE_SCRIPT)?
            ))
        });
    match script {
        Ok(script) => context.ask_guest(machine, &script, QUICK_SECONDS, slot).trim().to_owned(),
        Err(failure) => failure.0,
    }
}

/// What ends a helper in the guest: its task, and the PowerShell the task's console left running.
fn helper_stop(context: &Context, name: &str) -> Result<String> {
    Ok(format!("$helperName = '{name}'\n{}", guest_script(context, HELPER_STOP_SCRIPT)?))
}

/// What starts a helper: the helper's own file in Base64, and its start script. Any helper of the
/// same name is ended first, as one left by an earlier viewer holds the port the new one needs.
pub(crate) fn helper_start(context: &Context, name: &str) -> Result<String> {
    helper_start_with(context, name, &[])
}

/// The same, with more files for the guest to have: each is a variable the start script finds set, and
/// the file it holds in Base64. A file that is not in the program folder is left out.
pub(crate) fn helper_start_with(context: &Context, name: &str, more: &[(&str, &str)]) -> Result<String> {
    let helper = base64_encode(&fs::read(context.paths.guest_files().join(format!("{name}.ps1")))?);
    let start = guest_script(context, &format!("{name}-start.ps1"))?;
    let mut extra = String::new();
    for (variable, file) in more {
        if let Ok(data) = fs::read(context.paths.guest_files().join(file)) {
            extra += &format!("${variable} = '{}'\n", base64_encode(&data));
        }
    }
    Ok(format!("{}\n$helper = '{helper}'\n{extra}{start}", helper_stop(context, name)?))
}

pub(crate) fn stop_helper(context: &Context, machine: &Machine, slot: Option<Slot>, name: &str) {
    if let Ok(script) = helper_stop(context, name) {
        context.ask_guest(machine, &script, QUICK_SECONDS, slot);
    }
}

/// Says whether the helper is running in the user's session.
pub fn start_guest_clipboard(context: &Context, machine: &Machine, slot: Option<Slot>) -> Result<bool> {
    let script = helper_start(context, CLIPBOARD_HELPER)?;
    Ok(context.ask_guest(machine, &script, CLIPBOARD_START_SECONDS, slot).contains("started"))
}

pub fn stop_guest_clipboard(context: &Context, machine: &Machine, slot: Option<Slot>) {
    stop_helper(context, machine, slot, CLIPBOARD_HELPER);
}

/// Says whether the helper for dropped and dragged files was started in the user's session. It
/// takes a moment more to listen: [`Helper::answers`] says when it does.
pub fn start_guest_files(context: &Context, machine: &Machine, slot: Option<Slot>) -> Result<bool> {
    let script = helper_start(context, FILES_HELPER)?;
    Ok(context.ask_guest(machine, &script, FILES_START_SECONDS, slot).contains("started"))
}

pub fn stop_guest_files(context: &Context, machine: &Machine, slot: Option<Slot>) {
    stop_helper(context, machine, slot, FILES_HELPER);
}

/// The way to the machine's files helper. `None` for a machine with no token, which has no agent
/// to start one.
pub fn files_helper(context: &Context, machine: &Machine, slot: Option<Slot>) -> Option<Helper> {
    let token = read_token(context, machine)?;
    Some(Helper::new(GuestEnd::new(context, machine, slot, crate::network::GUEST_FILES_PORT), &token))
}

/// What the helper answered, or `None` if it didn't. The agent can wrap long lines.
fn ask_guest_clipboard(
    context: &Context,
    machine: &Machine,
    request: &str,
    slot: Option<Slot>,
) -> Result<Option<String>> {
    let script = format!("$request = '{request}'\n{}", guest_script(context, "clipboard-ask.ps1")?);
    let answer = context.ask_guest(machine, &script, QUICK_SECONDS, slot);
    if !answer.starts_with("answer") {
        return Ok(None);
    }
    Ok(Some(answer.split_whitespace().skip(1).collect()))
}

/// Says whether the helper that sizes the screen is running in the user's session.
pub fn start_guest_window_size(context: &Context, machine: &Machine, slot: Option<Slot>) -> Result<bool> {
    let script = helper_start(context, WINDOW_SIZE_HELPER)?;
    Ok(context.ask_guest(machine, &script, WINDOW_SIZE_START_SECONDS, slot).contains("started"))
}

pub fn stop_guest_window_size(context: &Context, machine: &Machine, slot: Option<Slot>) {
    stop_helper(context, machine, slot, WINDOW_SIZE_HELPER);
}

/// What became of a size the guest's screen was asked to take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sized {
    Set,
    /// An app in the guest has the screen in fullscreen, so its size was left alone. Changing it
    /// under a game can close the game.
    Held,
    /// The helper's own words: no display of the driver's, or Windows refusing the mode.
    Not(String),
    /// The helper didn't answer.
    NoHelper,
}

/// Has the guest's screen take a size, at a rate. `modes` is the driver's own list, which the helper
/// adds the size to if Windows doesn't list it yet.
pub fn size_guest_screen(
    context: &Context,
    machine: &Machine,
    (width, height, hz): Mode,
    modes: &DriverModes,
    slot: Option<Slot>,
) -> Result<Sized> {
    let request = format!("{width} {height} {hz} {}", modes.list());
    let script = format!("$request = '{request}'\n{}", guest_script(context, "window-size-ask.ps1")?);
    let answer = context.ask_guest(machine, &script, WINDOW_SIZE_SECONDS, slot);
    let Some(said) = answer.trim().strip_prefix("answer") else { return Ok(Sized::NoHelper) };
    let said = said.trim();
    Ok(match said.split_whitespace().next() {
        Some(SIZE_SET) => Sized::Set,
        Some(SIZE_HELD) => Sized::Held,
        Some(_) => Sized::Not(said.to_owned()),
        // It took the request and said nothing, which is what it does with one it can't read.
        None => Sized::NoHelper,
    })
}

/// The guest's text with Linux line ends, or `None` if the helper didn't answer.
pub fn read_guest_clipboard(
    context: &Context,
    machine: &Machine,
    slot: Option<Slot>,
) -> Result<Option<String>> {
    let Some(answer) = ask_guest_clipboard(context, machine, "get", slot)? else { return Ok(None) };
    let data = base64_decode(&answer).unwrap_or_default();
    Ok(Some(String::from_utf8_lossy(&data).replace("\r\n", "\n")))
}

/// Says whether it reached the guest. Windows expects its own line ends.
pub fn write_guest_clipboard(
    context: &Context,
    machine: &Machine,
    text: &str,
    slot: Option<Slot>,
) -> Result<bool> {
    let data = base64_encode(text.replace('\n', "\r\n").as_bytes());
    Ok(ask_guest_clipboard(context, machine, &data, slot)?.as_deref() == Some("set"))
}

#[cfg(test)]
pub mod stand_in {
    //! An agent that answers from a list and remembers what it was asked.

    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct ScriptedAgent {
        pub answers: Mutex<VecDeque<String>>,
        /// What was asked: the monitor of the slot it went through, the token and the script.
        pub requests: Mutex<Vec<(std::path::PathBuf, String, String)>>,
    }

    impl ScriptedAgent {
        pub fn answering(answers: &[&str]) -> Self {
            let answers = answers.iter().map(|text| (*text).to_owned()).collect();
            Self { answers: Mutex::new(answers), requests: Mutex::default() }
        }
    }

    impl GuestAgent for ScriptedAgent {
        fn ask(&self, end: &GuestEnd, token: &str, script: &str, _seconds: u64) -> String {
            self.requests.lock().unwrap().push((end.monitor.clone(), token.to_owned(), script.to_owned()));
            self.answers.lock().unwrap().pop_front().unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::ScriptedAgent;
    use super::*;
    use crate::slots;
    use crate::testing::{QemuStandIn, Scratch, scratch_context};
    use std::io::{BufRead, BufReader};
    use std::net::{Ipv4Addr, TcpListener};
    use std::os::unix::fs::MetadataExt;
    use std::sync::Arc;
    use std::thread;

    fn machine(context: &Context, id: &str) -> Machine {
        let machine = Machine::new(id, id, "/d", "/v");
        machine.save(context).unwrap();
        machine
    }

    fn with_guest_files(scratch: &Scratch) {
        scratch.write("program/guest/clipboard.ps1", "the helper");
        scratch.write("program/guest/clipboard-start.ps1", "start it");
        scratch.write("program/guest/clipboard-ask.ps1", "ask it");
        scratch.write("program/guest/helper-stop.ps1", "stop it");
    }

    #[test]
    fn a_helper_is_ended_by_name_and_a_start_ends_the_one_before_it() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        with_guest_files(&scratch);
        assert_eq!(helper_stop(&context, CLIPBOARD_HELPER).unwrap(), "$helperName = 'clipboard'\nstop it");
        let start = helper_start(&context, CLIPBOARD_HELPER).unwrap();
        let helper = base64_encode(b"the helper");
        assert_eq!(start, format!("$helperName = 'clipboard'\nstop it\n$helper = '{helper}'\nstart it"));
    }

    fn answering_with(reply: impl FnOnce(TcpStream) + Send + 'static) -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let stream = listener.accept().unwrap().0;
            // Read first: closing on a request nobody read could lose the answer.
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            reply(stream);
        });
        port
    }

    #[test]
    fn an_answer_that_keeps_trickling_ends_on_time() {
        let port = answering_with(|mut stream| {
            while stream.write_all(b"x").is_ok() {
                thread::sleep(Duration::from_millis(100));
            }
        });
        let start = Instant::now();
        let qemu = QemuStandIn::forwarding(crate::network::GUEST_AGENT_PORT, port);
        let answer = TcpAgent.ask(&qemu.end(), "token", "up", 1);
        assert!(start.elapsed() < Duration::from_secs(3), "it took {:?}", start.elapsed());
        assert!(!answer.is_empty());
    }

    #[test]
    fn an_answer_has_a_size_limit() {
        let port = answering_with(|mut stream| {
            let block = vec![b'x'; 1 << 20];
            while stream.write_all(&block).is_ok() {}
        });
        let qemu = QemuStandIn::forwarding(crate::network::GUEST_AGENT_PORT, port);
        assert_eq!(TcpAgent.ask(&qemu.end(), "token", "up", 20).len(), MOST_ANSWER);
    }

    #[test]
    fn a_whole_answer_comes_back() {
        let port = answering_with(|mut stream| stream.write_all(b"finished\r\n").unwrap());
        let qemu = QemuStandIn::forwarding(crate::network::GUEST_AGENT_PORT, port);
        assert_eq!(TcpAgent.ask(&qemu.end(), "token", "up", 5), "finished\r\n");
    }

    #[test]
    fn each_machine_keeps_its_own_password() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let (first, second) = (machine(&context, "first"), machine(&context, "second"));
        let password = make_password(&context, &first).unwrap();
        assert_eq!(make_password(&context, &first).unwrap(), password);
        assert_ne!(make_password(&context, &second).unwrap(), password);
        let mode = fs::metadata(first.password_file(&context)).unwrap().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn each_machine_has_its_own_private_token() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let (first, second) = (machine(&context, "first"), machine(&context, "second"));
        let token = make_token(&context, &first).unwrap();
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert_eq!(make_token(&context, &first).unwrap(), token, "made once");
        assert_ne!(make_token(&context, &second).unwrap(), token);
        assert_eq!(fs::metadata(first.token_file(&context)).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn an_old_token_is_taken_once_from_the_file_the_user_chose() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let old = machine(&context, "old");
        let junk = scratch.write("junk", "not a token");
        assert!(import_token(&context, &old, &junk).unwrap_err().0.contains("does not hold"));
        assert!(!old.token_file(&context).exists());
        let file = scratch.write("token", format!("{}\n", "ab".repeat(TOKEN_BYTES)));
        import_token(&context, &old, &file).unwrap();
        assert_eq!(fs::read_to_string(old.token_file(&context)).unwrap().trim(), "ab".repeat(TOKEN_BYTES));
        assert_eq!(fs::metadata(old.token_file(&context)).unwrap().mode() & 0o777, 0o600);
        assert!(import_token(&context, &old, &file).unwrap_err().0.contains("already has"));
    }

    #[test]
    fn a_machine_with_no_token_gets_no_new_one_by_being_asked() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let agent = Arc::new(ScriptedAgent::answering(&["up"]));
        context.agent = agent.clone();
        let old = machine(&context, "old");
        assert_eq!(context.ask_guest(&old, "x", 1, None), "");
        assert!(agent.requests.lock().unwrap().is_empty());
        assert!(!old.token_file(&context).exists());
    }

    #[test]
    fn the_right_token_goes_to_the_right_slots_port() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let agent = Arc::new(ScriptedAgent::answering(&["up"]));
        context.agent = agent.clone();
        let vm = machine(&context, "vm");
        let token = make_token(&context, &vm).unwrap();
        assert_eq!(context.ask_guest(&vm, "x", 1, Some(slots::number(2))), "up");
        let requests = agent.requests.lock().unwrap();
        let monitor = context.run_folder(Some(slots::number(2))).join(crate::monitor::MAIN_SOCKET);
        assert_eq!(requests[0], (monitor, token, "x".to_owned()));
    }

    #[test]
    fn the_request_is_one_line_with_the_script_in_base64() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            stream.write_all(b"answered").unwrap();
            line
        });
        let qemu = QemuStandIn::forwarding(crate::network::GUEST_AGENT_PORT, port);
        assert_eq!(TcpAgent.ask(&qemu.end(), "token", "Get-Date", 5), "answered");
        let line = server.join().unwrap();
        let (token, script) = line.trim().split_once(' ').unwrap();
        assert_eq!((token, base64_decode(script).unwrap()), ("token", b"Get-Date".to_vec()));
    }

    #[test]
    fn no_listener_is_an_empty_answer() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let qemu = QemuStandIn::forwarding(crate::network::GUEST_AGENT_PORT, port);
        assert_eq!(TcpAgent.ask(&qemu.end(), "token", "up", 1), "");
    }

    #[test]
    fn a_size_a_fullscreen_app_holds_back_is_told_apart() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        scratch.write("program/guest/window-size-ask.ps1", "ask it");
        let agent = Arc::new(ScriptedAgent::answering(&[
            "answer set 1920x1080\r\n",
            "answer held by a fullscreen app\r\n",
            "answer no driver display\r\n",
            "answer \r\n",
            "",
        ]));
        context.agent = agent.clone();
        let vm = machine(&context, "vm");
        make_token(&context, &vm).unwrap();
        let modes = DriverModes::default();
        let size = || size_guest_screen(&context, &vm, (1920, 1080, 144), &modes, None).unwrap();
        assert_eq!(size(), Sized::Set);
        assert_eq!(size(), Sized::Held, "a fullscreen app has the screen");
        assert_eq!(size(), Sized::Not("no driver display".into()));
        assert_eq!(size(), Sized::NoHelper, "a request it couldn't read");
        assert_eq!(size(), Sized::NoHelper, "no answer at all");
        let requests = agent.requests.lock().unwrap();
        assert!(requests[0].2.starts_with("$request = '1920 1080 144 '\n"), "{}", requests[0].2);
    }

    #[test]
    fn the_clipboard_goes_through_the_helper() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        with_guest_files(&scratch);
        let text = "ünï\nline 'two'";
        let encoded = base64_encode(text.replace('\n', "\r\n").as_bytes());
        let wrapped = format!("answer {}\r\n{}\r\n", &encoded[..10], &encoded[10..]);
        let agent = Arc::new(ScriptedAgent::answering(&[
            &wrapped,
            "answer \r\n",
            "ERROR: refused\r\n",
            "",
            "answer set\r\n",
            "answer \r\n",
            "started\r\n",
            "nobody is logged in\r\n",
        ]));
        context.agent = agent.clone();
        let vm = machine(&context, "vm");
        make_token(&context, &vm).unwrap();
        let read = || read_guest_clipboard(&context, &vm, None).unwrap();
        assert_eq!(read().as_deref(), Some(text), "long lines come back wrapped, and Windows line ends go");
        assert_eq!(read().as_deref(), Some(""), "an empty clipboard is not the helper missing");
        assert_eq!(read(), None);
        assert_eq!(read(), None);
        assert!(write_guest_clipboard(&context, &vm, text, None).unwrap());
        assert!(!write_guest_clipboard(&context, &vm, text, None).unwrap());
        assert!(start_guest_clipboard(&context, &vm, None).unwrap());
        assert!(!start_guest_clipboard(&context, &vm, None).unwrap());
        let requests = agent.requests.lock().unwrap();
        assert!(requests[0].2.starts_with("$request = 'get'\n"));
        assert!(requests[4].2.contains(&format!("$request = '{encoded}'\n")));
        let start = requests[6].2.strip_prefix("$helperName = 'clipboard'\nstop it\n").unwrap();
        let helper = start.strip_prefix("$helper = '").unwrap().split('\'').next().unwrap();
        assert_eq!(base64_decode(helper).unwrap(), b"the helper");
    }
}
