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

/// Mounting, which only means something in namespaces of one's own.
const MOUNTS: [c_long; 9] = [
    SYS_mount,
    SYS_umount2,
    SYS_pivot_root,
    SYS_fsopen,
    SYS_fsmount,
    SYS_fsconfig,
    SYS_move_mount,
    SYS_open_tree,
    SYS_mount_setattr,
];

const NAMESPACE_FLAGS: c_int =
    CLONE_NEWUSER | CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWNET | CLONE_NEWIPC | CLONE_NEWUTS | CLONE_NEWCGROUP;

/// Typing into the terminal QEMU was started from, and the console's own requests.
const TERMINAL_REQUESTS: [c_ulong; 2] = [TIOCSTI, TIOCLINUX];

/// The filter for QEMU. passt, which makes namespaces of its own, is started before it, outside.
pub fn qemu(no_new_unix_sockets: bool, reads_own_memory: bool) -> Filter {
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
    rules.extend(
        TERMINAL_REQUESTS
            .iter()
            .map(|request| Rule::when(SYS_ioctl, Condition::Equals(1, *request as u32), Action::Fail(EPERM))),
    );
    if no_new_unix_sockets {
        // Pairs can't reach anything outside, and the ones it needs were opened before.
        rules.push(Rule::when(SYS_socket, Condition::Equals(0, AF_UNIX as u32), Action::Fail(EPERM)));
    }
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
        assert!(denies(&qemu(false, false), SYS_process_vm_readv), "not let unless asked");
        let filter = qemu(false, true);
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
            let filter = qemu(no_new_unix_sockets, false);
            for call in [SYS_unshare, SYS_setns, SYS_mount, SYS_ptrace, SYS_clone3] {
                assert!(denies(&filter, call), "{call}");
            }
        }
    }
}
