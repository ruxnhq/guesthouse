//! The guard on a passt that may use one of the PC's network interfaces and no other, a VPN's for
//! one. passt binds each socket it opens for the guest to that interface, and a UDP or ping socket it
//! can't bind is closed. A TCP socket it can't bind is connected all the same, by whatever route the
//! PC has, so a VPN that goes away would leave the guest's new connections on the normal network.
//!
//! So passt's `connect` is held by a seccomp filter until the guard has answered it, and the guard
//! lets it through only while the interface still has the number it had before passt started. passt
//! has one thread, so the bind of a socket lies between the connect before it and its own. An
//! interface that has the same number at both was there all the while, as the kernel hands a number
//! out once, and so the bind worked. Once the number has changed, or the name is gone, every
//! connect is refused until the machine is started again, even if the VPN comes back: a socket opened
//! while it was away can't be told from one opened after.
//!
//! The guard is a process of its own, started from this program, as the launcher becomes QEMU. If
//! it dies, the kernel refuses what it would have been asked.

use super::handover::{checked, keep_only, socket_pair};
use crate::seccomp::{self, Action, Filter, Rule};
use std::ffi::{CString, OsString};
use std::fs;
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The launcher's first word when it's to be the guard: `--only-through-guard INTERFACE NUMBER
/// LISTENER CUT_FILE`.
pub const GUARD: &str = "--only-through-guard";
/// This program, whatever it's called and wherever it is.
const SELF: &str = "/proc/self/exe";
/// What a refused connect tells passt, as the kernel would with no route.
const REFUSAL: i32 = libc::ENETUNREACH;
/// The control message that carries one descriptor, with room for its padding.
const ONE_DESCRIPTOR: usize = 32;

/// The interface a passt keeps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlyThrough {
    pub interface: String,
    /// Where the guard says so once it has cut the network, for the app to show.
    pub cut_file: PathBuf,
}

/// The interface's number, which the kernel gives each one once, or `None` if there's none of that
/// name now.
pub fn interface_number(name: &str) -> Option<u32> {
    let name = CString::new(name).ok()?;
    // SAFETY: the name ends in a zero and is only read.
    let number = unsafe { libc::if_nametoindex(name.as_ptr()) };
    (number != 0).then_some(number)
}

/// Whether a process like passt, with no privileges on the PC's network, may bind a socket to the
/// interface. A kernel older than 5.7 says no, and passt would then connect unbound each time.
fn can_bind(name: &str) -> io::Result<()> {
    // SAFETY: a new socket, owned from here on.
    let socket = unsafe {
        OwnedFd::from_raw_fd(checked(libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0))?)
    };
    // SAFETY: the name is read for the length given.
    let bound = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            name.len() as libc::socklen_t,
        )
    };
    checked(bound).map(drop)
}

/// The interface's number, once it's known that passt can keep to it.
pub(super) fn check(only: &OnlyThrough) -> io::Result<u32> {
    let name = &only.interface;
    let number = interface_number(name)
        .ok_or_else(|| io::Error::other(format!("{name} is not there, so nothing may use the network")))?;
    can_bind(name).map_err(|error| io::Error::other(format!("a socket can't be kept to {name}: {error}")))?;
    Ok(number)
}

fn filter() -> Filter {
    Filter { rules: vec![Rule::always(libc::SYS_connect, Action::Ask)], otherwise: Action::Allow }
}

/// Sends one descriptor over a unix socket. It makes system calls and nothing else.
fn send_descriptor(socket: RawFd, sent: RawFd) -> io::Result<()> {
    let mut byte = [0u8];
    let mut part = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    let mut control = [0u64; ONE_DESCRIPTOR / 8];
    // SAFETY: all zeroes is an empty message, and the lengths below are those of what it points at.
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &raw mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    // SAFETY: the control buffer holds a header and one descriptor, which is what's written.
    unsafe {
        message.msg_controllen = libc::CMSG_SPACE(mem::size_of::<RawFd>() as u32) as usize;
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(mem::size_of::<RawFd>() as u32) as usize;
        libc::CMSG_DATA(header).cast::<RawFd>().write_unaligned(sent);
        checked(libc::sendmsg(socket, &raw const message, 0) as libc::c_int)?;
    }
    Ok(())
}

/// The descriptor the other end sent, or an error if it closed without sending one.
fn receive_descriptor(socket: RawFd) -> io::Result<OwnedFd> {
    let mut byte = [0u8];
    let mut part = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    let mut control = [0u64; ONE_DESCRIPTOR / 8];
    // SAFETY: as in `send_descriptor`.
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &raw mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = mem::size_of_val(&control);
    // SAFETY: the kernel writes within the lengths given, and a header it returns lies in the buffer.
    unsafe {
        checked(libc::recvmsg(socket, &raw mut message, libc::MSG_CMSG_CLOEXEC) as libc::c_int)?;
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        if header.is_null() || (*header).cmsg_type != libc::SCM_RIGHTS {
            return Err(io::Error::other("passt's filter was not put in place"));
        }
        Ok(OwnedFd::from_raw_fd(libc::CMSG_DATA(header).cast::<RawFd>().read_unaligned()))
    }
}

/// What the launcher holds between starting passt and starting its guard.
pub(super) struct Listening {
    ours: OwnedFd,
    theirs: OwnedFd,
}

/// Has what `command` starts ask before each connect, and send the descriptor it asks on back here.
pub(super) fn listen(command: &mut Command) -> io::Result<Listening> {
    let (ours, theirs) = socket_pair()?;
    let mut ready = seccomp::ready(&filter())?;
    let send_to = theirs.as_raw_fd();
    // SAFETY: between fork and exec the closure makes only system calls, which are async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            let listener = ready.install_listened()?;
            let sent = send_descriptor(send_to, listener);
            libc::close(listener);
            sent
        });
    }
    Ok(Listening { ours, theirs })
}

impl Listening {
    /// The descriptor the started program's connects are answered on.
    pub(super) fn listener(self) -> io::Result<OwnedFd> {
        drop(self.theirs);
        receive_descriptor(self.ours.as_raw_fd())
    }
}

/// Starts the guard, as a child of this process like passt, so it goes when QEMU's group is stopped.
pub(super) fn start(only: &OnlyThrough, number: u32, listener: &OwnedFd) -> io::Result<()> {
    match fs::remove_file(&only.cut_file) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut guard = Command::new(SELF);
    guard.arg(GUARD).arg(&only.interface).arg(number.to_string());
    guard.arg(listener.as_raw_fd().to_string()).arg(&only.cut_file);
    keep_only(&mut guard, listener.as_raw_fd());
    guard.stdin(Stdio::null()).spawn().map(drop)
}

/// What the guard was told.
#[derive(Debug, PartialEq, Eq)]
pub struct Guard {
    only: OnlyThrough,
    number: u32,
    listener: RawFd,
}

impl Guard {
    /// The guard's words, or `None` if these are the launcher's.
    pub fn parse(arguments: &[OsString]) -> Option<Result<Self, String>> {
        let (first, rest) = arguments.split_first()?;
        if first != GUARD {
            return None;
        }
        let [interface, number, listener, cut_file] = rest else {
            return Some(Err(format!("{GUARD} takes an interface, its number, a descriptor and a file")));
        };
        let text = |word: &OsString| word.to_str().map(str::to_owned);
        let found = (|| {
            Some(Self {
                only: OnlyThrough { interface: text(interface)?, cut_file: cut_file.into() },
                number: text(number)?.parse().ok()?,
                listener: text(listener)?.parse().ok()?,
            })
        })();
        Some(found.ok_or_else(|| format!("{GUARD} can't take {rest:?}")))
    }

    /// Answers passt's connects until passt is gone. It only comes back early if something failed.
    pub fn run(&self) -> io::Result<()> {
        // SAFETY: a flag the kernel keeps for this thread: a signal when the parent, QEMU, has gone.
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) };
        answer_until_gone(self.listener, &self.only, self.number)
    }
}

fn interrupted(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Interrupted
}

/// Whether there's a connect to answer, or `false` once nothing that has the filter is left.
fn wait_for_one(listener: RawFd) -> io::Result<bool> {
    loop {
        let mut waited = libc::pollfd { fd: listener, events: libc::POLLIN, revents: 0 };
        // SAFETY: one entry, as said.
        match checked(unsafe { libc::poll(&raw mut waited, 1, -1) }) {
            Ok(_) => return Ok(waited.revents & libc::POLLIN != 0),
            Err(error) if interrupted(&error) => {}
            Err(error) => return Err(error),
        }
    }
}

fn answer_until_gone(listener: RawFd, only: &OnlyThrough, number: u32) -> io::Result<()> {
    let mut cut = false;
    while wait_for_one(listener)? {
        // SAFETY: all zeroes is what the kernel wants to be handed.
        let mut asked: libc::seccomp_notif = unsafe { mem::zeroed() };
        // SAFETY: the kernel fills in the one structure it's given.
        if let Err(error) =
            checked(unsafe { libc::ioctl(listener, libc::SECCOMP_IOCTL_NOTIF_RECV, &raw mut asked) })
        {
            // The one that asked may have been ended before it was heard.
            if interrupted(&error) || error.raw_os_error() == Some(libc::ENOENT) {
                continue;
            }
            return Err(error);
        }
        if !cut && interface_number(&only.interface) != Some(number) {
            cut = true;
            say_cut(only);
        }
        let mut answer = libc::seccomp_notif_resp { id: asked.id, val: 0, error: -REFUSAL, flags: 0 };
        if !cut {
            answer.error = 0;
            answer.flags = libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32;
        }
        // An answer to one that has ended meanwhile is refused, and there's nothing to do about it.
        // SAFETY: the kernel reads the one structure it's given.
        unsafe { libc::ioctl(listener, libc::SECCOMP_IOCTL_NOTIF_SEND, &raw mut answer) };
    }
    Ok(())
}

fn say_cut(only: &OnlyThrough) {
    let said = format!(
        "{} went away, so this machine's network is cut until the machine is started again.",
        only.interface
    );
    eprintln!("guesthouse-jail: {said}");
    if let Err(error) = fs::write(&only.cut_file, format!("{said}\n")) {
        eprintln!("guesthouse-jail: {}: {error}", only.cut_file.display());
    }
}

/// The file's folder is the run folder, where passt's pid file is too.
pub fn cut_file_beside(pid_file: &Path) -> PathBuf {
    pid_file.with_file_name(CUT_FILE)
}

pub const CUT_FILE: &str = "network-cut";

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    const PORT: &str = "GUESTHOUSE_GUARD_TEST_PORT";
    const CHILD: &str = "launch::guard::tests::a_child_connects_and_says_how_it_went";
    const LOOPBACK: &str = "lo";

    fn words(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_guards_words_are_told_from_the_launchers() {
        assert_eq!(Guard::parse(&words(&["--open-only", "--", "true"])), None);
        let guard = Guard::parse(&words(&[GUARD, "wg0", "7", "5", "/run/x/network-cut"])).unwrap().unwrap();
        assert_eq!(guard.only.interface, "wg0");
        assert_eq!((guard.number, guard.listener), (7, 5));
        assert!(Guard::parse(&words(&[GUARD, "wg0", "seven", "5", "/x"])).unwrap().is_err());
        assert!(Guard::parse(&words(&[GUARD, "wg0"])).unwrap().is_err());
    }

    #[test]
    fn an_interface_that_is_there_has_a_number_and_can_be_kept_to() {
        assert!(interface_number(LOOPBACK).is_some());
        assert_eq!(interface_number("no-such-one"), None);
        let only = |name: &str| OnlyThrough { interface: name.to_owned(), cut_file: "/nowhere".into() };
        assert!(check(&only(LOOPBACK)).is_ok());
        assert!(check(&only("no-such-one")).unwrap_err().to_string().contains("is not there"));
    }

    /// Not a test of its own: the child the tests below start under the filter. It ends with the
    /// error number of its connect, or 0.
    #[test]
    #[ignore = "run by the tests of the guard"]
    fn a_child_connects_and_says_how_it_went() {
        let port: u16 = env::var(PORT).unwrap().parse().unwrap();
        let code = match TcpStream::connect(("127.0.0.1", port)) {
            Ok(_) => 0,
            Err(error) => error.raw_os_error().unwrap_or(-1),
        };
        std::process::exit(code);
    }

    /// How a child's connect to a listener here went, with a guard that takes the loopback interface
    /// to have this number.
    fn connect_guarded(number: u32, cut_file: &Path) -> i32 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut child = Command::new(env::current_exe().unwrap());
        child.args(["--exact", CHILD, "--ignored", "--test-threads", "1"]);
        child.env(PORT, listener.local_addr().unwrap().port().to_string()).stdout(Stdio::null());
        let listening = listen(&mut child).unwrap();
        let mut child = child.spawn().unwrap();
        let asked_on = listening.listener().unwrap();
        let only = OnlyThrough { interface: LOOPBACK.to_owned(), cut_file: cut_file.to_owned() };
        let guard = thread::spawn(move || answer_until_gone(asked_on.as_raw_fd(), &only, number));
        let code = child.wait().unwrap().code().unwrap();
        guard.join().unwrap().unwrap();
        code
    }

    #[test]
    fn a_connect_goes_through_while_the_interface_has_its_number() {
        let cut_file = env::temp_dir().join(format!("guard-open-{}", std::process::id()));
        assert_eq!(connect_guarded(interface_number(LOOPBACK).unwrap(), &cut_file), 0);
        assert!(!cut_file.exists());
    }

    #[test]
    fn a_connect_is_refused_once_the_number_is_another_and_the_file_says_so() {
        let cut_file = env::temp_dir().join(format!("guard-cut-{}", std::process::id()));
        let another = interface_number(LOOPBACK).unwrap() + 1000;
        assert_eq!(connect_guarded(another, &cut_file), REFUSAL);
        assert!(fs::read_to_string(&cut_file).unwrap().starts_with("lo went away"));
        fs::remove_file(&cut_file).unwrap();
    }
}
