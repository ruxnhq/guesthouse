//! seccomp: which system calls a process may make, as a list of rules the kernel runs on each one.
//!
//! Rules are tried in order and the first that matches decides, so the calls made most often go
//! first: a call costs a couple of instructions for each rule ahead of it. The filter only runs on
//! x86-64, and a call from any other architecture, or through the x32 numbers, ends the process.

mod bpf;
pub mod trap;

use crate::no_new_privileges;
use std::io;
use std::os::fd::RawFd;

const SET_MODE_FILTER: libc::c_uint = 1;
const FLAG_THREAD_SYNC: libc::c_ulong = 1;

/// What happens to a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    /// Refused with this error, as if the kernel had said it.
    Fail(i32),
    /// Refused with `SIGSYS`, which [`trap`] turns into a note in the log and `ENOSYS`.
    Trap,
    /// Ends the process.
    Kill,
    /// Held until whoever listens on the filter has answered it, which [`Ready::install_listened`]
    /// gives the descriptor for.
    Ask,
}

/// A test on one of a call's arguments. Only the low 32 bits are compared, which is all the flags
/// and request numbers tested here use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Equals(u8, u32),
    HasAll(u8, u32),
    HasAny(u8, u32),
    HasNone(u8, u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub call: libc::c_long,
    pub when: Option<Condition>,
    pub action: Action,
}

impl Rule {
    pub const fn always(call: libc::c_long, action: Action) -> Self {
        Self { call, when: None, action }
    }

    pub const fn when(call: libc::c_long, condition: Condition, action: Action) -> Self {
        Self { call, when: Some(condition), action }
    }
}

/// The rules, and what happens to a call none of them match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub rules: Vec<Rule>,
    pub otherwise: Action,
}

/// Which threads a filter covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threads {
    /// This one, and what it starts from now on.
    This,
    /// Every thread of the process, now and later.
    All,
}

/// Installs the filter. It stays for the life of the process and across `execve`.
pub fn install(filter: &Filter, threads: Threads) -> io::Result<()> {
    no_new_privileges()?;
    let mut program = bpf::compile(filter)?;
    let fprog = libc::sock_fprog { len: program.len() as libc::c_ushort, filter: program.as_mut_ptr() };
    let flags = if threads == Threads::All { FLAG_THREAD_SYNC } else { 0 };
    // SAFETY: the program lives across the call, and its length is its own.
    let done = unsafe { libc::syscall(libc::SYS_seccomp, SET_MODE_FILTER, flags, &raw const fprog) };
    match done {
        0 => Ok(()),
        // With thread sync, a positive answer is the id of a thread that couldn't be synced.
        thread if thread > 0 => Err(io::Error::other(format!("thread {thread} could not take the filter"))),
        _ => Err(io::Error::last_os_error()),
    }
}

/// A filter already turned into the kernel's form, for a place where nothing may be allocated: between
/// fork and exec.
pub struct Ready(Vec<libc::sock_filter>);

pub fn ready(filter: &Filter) -> io::Result<Ready> {
    bpf::compile(filter).map(Ready)
}

impl Ready {
    /// Installs the filter on this thread and gives the descriptor its [`Action::Ask`] calls are
    /// answered on. It makes system calls and nothing else.
    pub fn install_listened(&mut self) -> io::Result<RawFd> {
        no_new_privileges()?;
        let fprog = libc::sock_fprog { len: self.0.len() as libc::c_ushort, filter: self.0.as_mut_ptr() };
        let flags = libc::SECCOMP_FILTER_FLAG_NEW_LISTENER;
        // SAFETY: the program lives across the call, and its length is its own.
        let listener = unsafe { libc::syscall(libc::SYS_seccomp, SET_MODE_FILTER, flags, &raw const fprog) };
        if listener < 0 { Err(io::Error::last_os_error()) } else { Ok(listener as RawFd) }
    }
}
