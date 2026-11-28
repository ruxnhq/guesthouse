//! The jail around a machine's virtual TPM. swtpm reads the system's libraries and its crypto
//! settings, and writes only the machine's TPM folder and the run folder, where its socket and log
//! are. It has no network, and the same filter as QEMU. It starts when QEMU is jailed, by the same
//! launcher, and with QEMU unjailed it runs as it is.

use super::VmPlan;
use guesthouse_jail::launch::{Launch, UnixSocket};
use guesthouse_jail::{Grant, Plan, system};
use std::path::PathBuf;

const SWTPM: &str = "swtpm";
/// Besides the system's programs: the loader's cache and the crypto library's settings.
const SYSTEM: [(Grant, &str); 6] = [
    (Grant::Read, "/etc/ld.so.cache"),
    (Grant::Read, system::LINKED_SETTINGS),
    (Grant::Read, "/etc/ssl"),
    (Grant::Read, "/etc/pki"),
    (Grant::Read, "/etc/crypto-policies"),
    (Grant::Read, "/proc"),
];
const DEVICES: [&str; 3] = ["/dev/null", "/dev/random", "/dev/urandom"];
/// Where the launcher leaves the TPM's socket, made and listening, for swtpm to take its clients on.
const SOCKET_FD: i32 = 20;
const CONTROL: &str = "--ctrl";

impl VmPlan {
    /// What swtpm may reach.
    pub(super) fn tpm_jail_plan(&self) -> Option<Plan> {
        let state = self.tpm.as_ref()?;
        let mut plan = Plan::default();
        system::grant_programs(&mut plan);
        for (grant, path) in SYSTEM {
            plan.grant(grant, path);
        }
        for device in DEVICES {
            plan.grant(Grant::Device, device);
        }
        plan.grant(Grant::Own, state).grant(Grant::Own, &self.run_folder);
        Some(plan)
    }

    /// The words that start the virtual TPM: in the launcher's jail when QEMU has one.
    pub fn swtpm_command(&self) -> Option<Vec<String>> {
        let mut arguments = self.swtpm_arguments()?;
        let program = super::program_on_path(SWTPM).unwrap_or_else(|| PathBuf::from(SWTPM));
        let mut words = Vec::new();
        if let (Some(jail), Some(plan)) = (&self.jail, self.tpm_jail_plan()) {
            // swtpm sets the mode of a socket it makes by its path, which the jail's filter refuses,
            // so the launcher makes it, and swtpm takes it by its number.
            let control = arguments.iter().position(|word| word == CONTROL).map(|at| at + 1);
            if let Some(at) = control {
                arguments[at] = format!("type=unixio,fd={SOCKET_FD}");
            }
            let socket = UnixSocket {
                path: self.run_folder.join(super::qemu_jail::TPM_SOCKET),
                fd: SOCKET_FD,
                listen: true,
            };
            let jailing = Launch {
                plan: Some(plan),
                passt: None,
                tap: None,
                unix_sockets: vec![socket],
                no_new_unix_sockets: false,
                reads_own_memory: false,
                pings: false,
                program: Vec::new(),
            };
            words.push(jail.launcher.display().to_string());
            words.extend(jailing.words());
        }
        words.push(program.display().to_string());
        words.extend(arguments);
        Some(words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Machine;
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};
    use crate::vm_args::{Boot, QemuJail};

    fn with_tpm(jailed: bool) -> (Scratch, VmPlan) {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine =
            Machine { tpm: true, ..Machine::new("win", "Win", "/m/win/disk.qcow2", "/m/win/vars.fd") };
        let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::number(1), 0);
        let jail = jailed.then(|| QemuJail {
            launcher: "/p/guesthouse-jail".into(),
            namespaces: true,
            qemu_sandbox: false,
            sound_socket: None,
            backing_files: Vec::new(),
        });
        (scratch, VmPlan { jail, ..plan })
    }

    #[test]
    fn a_jailed_machines_tpm_starts_in_the_launcher_with_its_own_folder_and_the_run_folder() {
        let (_scratch, plan) = with_tpm(true);
        let command = plan.swtpm_command().unwrap();
        assert_eq!(command[0], "/p/guesthouse-jail");
        let end = command.iter().position(|word| word == "--").unwrap();
        assert!(command[end + 1].ends_with("swtpm") && command[end + 2] == "socket", "{command:?}");
        let owned: Vec<&String> =
            command.windows(2).filter(|two| two[0] == "--own").map(|two| &two[1]).collect();
        let state = plan.tpm.as_ref().unwrap().display().to_string();
        assert_eq!(owned, [&state, &plan.run_folder.display().to_string()]);
        assert!(
            !command.iter().any(|word| word == "/m/win/disk.qcow2" || word == "/m/win"),
            "no machine file"
        );
        let listen = command.iter().position(|word| word == "--unix-listen").unwrap();
        assert_eq!(command[listen + 1], plan.run_folder.join("tpm.sock").display().to_string());
        assert!(command.contains(&format!("type=unixio,fd={}", command[listen + 2])), "{command:?}");
    }

    #[test]
    fn an_unjailed_machines_tpm_runs_as_it_is() {
        let (_scratch, plan) = with_tpm(false);
        let command = plan.swtpm_command().unwrap();
        assert!(command[0].ends_with("swtpm") && command[1] == "socket");
    }
}
