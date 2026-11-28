//! The jail around a machine's virtual TPM. swtpm reads the system's libraries and its crypto
//! settings, and writes only the machine's TPM folder and the run folder, where it makes its socket
//! and log as it always has. It has no network, and QEMU's filter but for setting a file's mode by
//! its path, which it does to its socket. It starts when QEMU is jailed, by the same launcher, and
//! with QEMU unjailed it runs as it is.

use super::VmPlan;
use guesthouse_jail::launch::Launch;
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
        let arguments = self.swtpm_arguments()?;
        let program = super::program_on_path(SWTPM).unwrap_or_else(|| PathBuf::from(SWTPM));
        let mut words = Vec::new();
        if let (Some(jail), Some(plan)) = (&self.jail, self.tpm_jail_plan()) {
            let jailing = Launch {
                plan: Some(plan),
                passt: None,
                tap: None,
                unix_sockets: Vec::new(),
                no_new_unix_sockets: false,
                reads_own_memory: false,
                pings: false,
                tpm: true,
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
        assert!(command.contains(&"--tpm".to_owned()), "its own filter");
        let socket = format!("type=unixio,path={}", plan.run_folder.join("tpm.sock").display());
        assert!(command.contains(&socket), "its socket where it always was: {command:?}");
    }

    #[test]
    fn an_unjailed_machines_tpm_runs_as_it_is() {
        let (_scratch, plan) = with_tpm(false);
        let command = plan.swtpm_command().unwrap();
        assert!(command[0].ends_with("swtpm") && command[1] == "socket");
    }
}
