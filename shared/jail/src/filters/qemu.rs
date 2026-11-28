//! What QEMU may not call. It's a deny-list, unlike the daemon's: the calls QEMU makes depend on its
//! build, its devices, its display and sound, and the programs it starts, and an allow-list made
//! without a recorded run of each would stop machines in ways no test here sees. These are the calls
//! that reach beyond the process or change the system, and none of them is part of running a VM.

use super::all;
use crate::seccomp::{Action, Condition, Filter, Rule};
use libc::*;

const DENIED: [c_long; 39] = [
    SYS_ptrace,
    SYS_process_vm_readv,
    SYS_process_vm_writev,
    SYS_kexec_load,
    SYS_kexec_file_load,
    SYS_init_module,
    SYS_finit_module,
    SYS_delete_module,
    SYS_bpf,
    SYS_perf_event_open,
    SYS_userfaultfd,
    SYS_keyctl,
    SYS_add_key,
    SYS_request_key,
    SYS_swapon,
    SYS_swapoff,
    SYS_reboot,
    SYS_acct,
    SYS_quotactl,
    SYS_open_by_handle_at,
    SYS_name_to_handle_at,
    SYS_lookup_dcookie,
    SYS_iopl,
    SYS_ioperm,
    SYS_syslog,
    SYS_settimeofday,
    SYS_clock_settime,
    SYS_adjtimex,
    SYS_clock_adjtime,
    SYS_personality,
    SYS_vhangup,
    SYS_setdomainname,
    SYS_sethostname,
    SYS_pidfd_getfd,
    SYS_kcmp,
    SYS_fanotify_init,
    SYS_io_uring_setup,
    SYS_io_uring_enter,
    SYS_io_uring_register,
];

/// The kernel's numbers for calls newer than the `libc` crate knows, from its x86-64 table.
const SYS_SETXATTRAT: c_long = 463;
const SYS_REMOVEXATTRAT: c_long = 466;
const SYS_OPEN_TREE_ATTR: c_long = 467;
const SYS_FILE_SETATTR: c_long = 469;

/// Changing a file's mode, owner, times or attributes by its path, and cutting it short by its path,
/// which Landlock's rules don't cover. QEMU changes nothing of a file's but through a descriptor
/// it has open, which Landlock let it open.
const BY_PATH: [c_long; 18] = [
    SYS_chmod,
    SYS_fchmodat,
    SYS_fchmodat2,
    SYS_chown,
    SYS_lchown,
    SYS_fchownat,
    SYS_utime,
    SYS_utimes,
    SYS_utimensat,
    SYS_futimesat,
    SYS_setxattr,
    SYS_lsetxattr,
    SYS_removexattr,
    SYS_lremovexattr,
    SYS_SETXATTRAT,
    SYS_REMOVEXATTRAT,
    SYS_FILE_SETATTR,
    SYS_truncate,
];

/// System V's shared memory, queues and semaphores, and POSIX queues, which no rule of Landlock's
/// names. QEMU shares memory through descriptors.
const IPC: [c_long; 17] = [
    SYS_shmget,
    SYS_shmat,
    SYS_shmctl,
    SYS_msgget,
    SYS_msgsnd,
    SYS_msgrcv,
    SYS_msgctl,
    SYS_semget,
    SYS_semop,
    SYS_semtimedop,
    SYS_semctl,
    SYS_mq_open,
    SYS_mq_unlink,
    SYS_mq_timedsend,
    SYS_mq_timedreceive,
    SYS_mq_notify,
    SYS_mq_getsetattr,
];

/// The sockets QEMU's user network makes, TCP and UDP, each with its protocol left to the kernel or
/// named. Landlock holds their ports.
const INTERNET: [&[Condition]; 8] = [
    &[Condition::Equals(0, AF_INET as u32), STREAM, Condition::Equals(2, 0)],
    &[Condition::Equals(0, AF_INET as u32), STREAM, Condition::Equals(2, IPPROTO_TCP as u32)],
    &[Condition::Equals(0, AF_INET as u32), DATAGRAM, Condition::Equals(2, 0)],
    &[Condition::Equals(0, AF_INET as u32), DATAGRAM, Condition::Equals(2, IPPROTO_UDP as u32)],
    &[Condition::Equals(0, AF_INET6 as u32), STREAM, Condition::Equals(2, 0)],
    &[Condition::Equals(0, AF_INET6 as u32), STREAM, Condition::Equals(2, IPPROTO_TCP as u32)],
    &[Condition::Equals(0, AF_INET6 as u32), DATAGRAM, Condition::Equals(2, 0)],
    &[Condition::Equals(0, AF_INET6 as u32), DATAGRAM, Condition::Equals(2, IPPROTO_UDP as u32)],
];
/// The guest's pings, which QEMU's user network sends on NAT through the kernel's ping sockets.
const PINGS: [&[Condition]; 2] = [
    &[Condition::Equals(0, AF_INET as u32), DATAGRAM, Condition::Equals(2, IPPROTO_ICMP as u32)],
    &[Condition::Equals(0, AF_INET6 as u32), DATAGRAM, Condition::Equals(2, IPPROTO_ICMPV6 as u32)],
];
/// A socket's type, under the flags that can come with it.
const TYPE_MASK: u32 = 0xf;
const STREAM: Condition = Condition::Masked(1, TYPE_MASK, SOCK_STREAM as u32);
const DATAGRAM: Condition = Condition::Masked(1, TYPE_MASK, SOCK_DGRAM as u32);

/// Mounting, which only means something in namespaces of one's own.
const MOUNTS: [c_long; 10] = [
    SYS_mount,
    SYS_umount2,
    SYS_pivot_root,
    SYS_fsopen,
    SYS_fsmount,
    SYS_fsconfig,
    SYS_move_mount,
    SYS_open_tree,
    SYS_mount_setattr,
    SYS_OPEN_TREE_ATTR,
];

const NAMESPACE_FLAGS: c_int =
    CLONE_NEWUSER | CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWNET | CLONE_NEWIPC | CLONE_NEWUTS | CLONE_NEWCGROUP;

/// Typing into the terminal QEMU was started from, and the console's own requests.
const TERMINAL_REQUESTS: [c_ulong; 2] = [TIOCSTI, TIOCLINUX];

/// The sockets QEMU may make: the kernel's netlink, which reaches no other machine; unix sockets
/// unless it was handed every one it needs; and the user network's. A pair of unix sockets only
/// carries a stream, as an end of a datagram pair can send to any socket by its path.
fn sockets(no_new_unix_sockets: bool, pings: bool) -> Vec<Rule> {
    let mut rules = vec![Rule::when(SYS_socket, Condition::Equals(0, AF_NETLINK as u32), Action::Allow)];
    let unix = if no_new_unix_sockets { Action::Fail(EPERM) } else { Action::Allow };
    rules.push(Rule::when(SYS_socket, Condition::Equals(0, AF_UNIX as u32), unix));
    let pings = if pings { &PINGS[..] } else { &[] };
    for kind in INTERNET.iter().chain(pings) {
        rules.push(Rule::when(SYS_socket, Condition::All(kind), Action::Allow));
    }
    rules.push(Rule::always(SYS_socket, Action::Fail(EAFNOSUPPORT)));
    rules.push(Rule::when(SYS_socketpair, DATAGRAM, Action::Fail(EPERM)));
    rules
}

/// The filter for QEMU. passt, which makes namespaces of its own, is started before it, outside.
/// `pings` lets it send the guest's pings, for a QEMU whose user network is the guest's NAT.
pub fn qemu(no_new_unix_sockets: bool, reads_own_memory: bool, pings: bool) -> Filter {
    let mut rules = Vec::new();
    if reads_own_memory {
        // Reading its own memory this way gives QEMU nothing it couldn't read already, and the pid
        // stays its own across the exec. A macOS guest's device finds out which pages the kernel can
        // pin by reading a byte of each mapping so, and refused, it hands the GPU copies of the guest's
        // memory. Only a macOS guest's QEMU is let.
        let own_memory = Condition::Equals(0, std::process::id());
        rules.push(Rule::when(SYS_process_vm_readv, own_memory, Action::Allow));
    }
    rules.extend(all(&DENIED, Action::Fail(EPERM)));
    rules.extend(all(&BY_PATH, Action::Fail(EPERM)));
    rules.extend(all(&IPC, Action::Fail(EPERM)));
    rules.extend(
        TERMINAL_REQUESTS
            .iter()
            .map(|request| Rule::when(SYS_ioctl, Condition::Equals(1, *request as u32), Action::Fail(EPERM))),
    );
    rules.extend(sockets(no_new_unix_sockets, pings));
    rules.extend(all(&MOUNTS, Action::Fail(EPERM)));
    rules.extend(all(&[SYS_unshare, SYS_setns], Action::Fail(EPERM)));
    rules.push(Rule::when(SYS_clone, Condition::HasAny(0, NAMESPACE_FLAGS as u32), Action::Fail(EPERM)));
    rules.push(Rule::always(SYS_clone3, Action::Fail(ENOSYS)));
    Filter { rules, otherwise: Action::Allow }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn denies(filter: &Filter, call: c_long) -> bool {
        filter
            .rules
            .iter()
            .any(|rule| rule.call == call && rule.when.is_none() && rule.action != Action::Allow)
    }

    #[test]
    fn only_a_macos_qemu_may_read_its_own_memory_and_no_other_process() {
        assert!(denies(&qemu(false, false, false), SYS_process_vm_readv), "not let unless asked");
        let filter = qemu(false, true, false);
        let rules: Vec<&Rule> =
            filter.rules.iter().filter(|rule| rule.call == SYS_process_vm_readv).collect();
        assert_eq!(rules[0].when, Some(Condition::Equals(0, std::process::id())));
        assert_eq!(rules[0].action, Action::Allow);
        assert_eq!((rules[1].when, rules[1].action), (None, Action::Fail(EPERM)), "any other pid");
        assert!(denies(&filter, SYS_process_vm_writev));
    }

    #[test]
    fn no_namespaces_or_mounts_and_no_tracing() {
        for no_new_unix_sockets in [false, true] {
            let filter = qemu(no_new_unix_sockets, false, true);
            for call in [SYS_unshare, SYS_setns, SYS_mount, SYS_ptrace, SYS_clone3] {
                assert!(denies(&filter, call), "{call}");
            }
        }
    }
}
