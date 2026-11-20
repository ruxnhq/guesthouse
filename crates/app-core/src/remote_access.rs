//! Remote Desktop and SSH in a Windows guest. A machine is made with or without each, and the user can
//! change that later. Windows lets either in only from 10.0.2.2, the host's end of QEMU's user network.
//! Every port forward goes through that network in every network mode, and QEMU hands the guest a
//! connection to a forward as coming from that address, so a forward is the only way in. A bridged
//! network gives the guest an address on the LAN, and neither listens there.
//!
//! The choices are kept in a file of their own beside the machine's settings, so that an older build can
//! still read a machine made with them. A machine made before them has no file, and Guesthouse leaves its
//! Windows as it is until the user chooses. A choice is then put right at every start, and at once while
//! the machine runs, by the guest scripts in `guest/`.

use crate::agent::guest_script;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::{Kind, Machine};
use crate::settings::write_json;
use crate::tasks::{State, Status, status};
use crate::text::base64_encode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const ACCESS_FILE: &str = "remote-access.json";
/// The task that takes a change to a running guest.
pub const APPLY_TASK: &str = "remote_access";
/// Where the guest takes the installer when it comes through the agent.
pub const GUEST_INSTALLER: &str = r"C:\gpupv\OpenSSH-Win64.msi";
/// Its name on the install disc, beside the script.
pub const INSTALLER_ON_DISC: &str = "OpenSSH-Win64.msi";
/// The host user's public keys are `~/.ssh/id_` and the kind of key, then `.pub`.
const PUBLIC_KEY_START: &str = "id_";
const PUBLIC_KEY_END: &str = ".pub";
pub const NO_OPENSSH: &str = "This build of Guesthouse has no OpenSSH for Windows to put in the guest. A release \
                              has it.";
const ONLY_WINDOWS: &str = "is only for a Windows machine with GPU-PV.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    RemoteDesktop,
    Ssh,
}

impl Service {
    pub const ALL: [Self; 2] = [Self::RemoteDesktop, Self::Ssh];

    pub fn name(self) -> &'static str {
        match self {
            Self::RemoteDesktop => "Remote Desktop",
            Self::Ssh => "SSH",
        }
    }

    /// As `guesthouse-vm` names it.
    pub fn key(self) -> &'static str {
        match self {
            Self::RemoteDesktop => "remote-desktop",
            Self::Ssh => "ssh",
        }
    }

    /// Where it listens in the guest.
    pub fn port(self) -> u16 {
        match self {
            Self::RemoteDesktop => 3389,
            Self::Ssh => 22,
        }
    }

    /// The guest script that turns it on or off.
    pub fn script_name(self) -> &'static str {
        match self {
            Self::RemoteDesktop => "remote-desktop.ps1",
            Self::Ssh => "ssh.ps1",
        }
    }
}

/// Each is `None` until someone chooses, as on a machine made before the choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_desktop: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<bool>,
}

impl Access {
    /// What a new machine gets unless told otherwise. Remote Desktop is part of Windows and only a
    /// forward reaches it, so it's on. SSH puts a program into Windows that most people never use, so
    /// it's off, which also lets a build without its installer make a machine the usual way.
    pub const NEW: Self = Self { remote_desktop: Some(true), ssh: Some(false) };

    pub fn get(&self, service: Service) -> Option<bool> {
        match service {
            Service::RemoteDesktop => self.remote_desktop,
            Service::Ssh => self.ssh,
        }
    }

    pub fn set(&mut self, service: Service, on: bool) {
        match service {
            Service::RemoteDesktop => self.remote_desktop = Some(on),
            Service::Ssh => self.ssh = Some(on),
        }
    }

    pub fn is_on(&self, service: Service) -> bool {
        self.get(service) == Some(true)
    }

    /// The services someone chose for, with the choice.
    pub fn chosen(&self) -> impl Iterator<Item = (Service, bool)> + '_ {
        Service::ALL.into_iter().filter_map(|service| Some((service, self.get(service)?)))
    }
}

fn access_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(ACCESS_FILE)
}

/// What the machine's file says. A file that can't be read is taken as no choice at all, which leaves
/// Windows alone.
pub fn read_access(context: &Context, machine: &Machine) -> Access {
    let text = fs::read_to_string(access_file(context, machine)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write_access(context: &Context, machine: &Machine, access: &Access) -> Result<()> {
    write_json(&access_file(context, machine), access)
}

/// Why this PC can't give a guest SSH, if it can't.
pub fn ssh_problem(context: &Context) -> Option<&'static str> {
    (!context.paths.openssh_installer().is_file()).then_some(NO_OPENSSH)
}

/// Why a machine can't be made with these choices, if it can't.
pub fn access_problem(context: &Context, access: &Access) -> Option<&'static str> {
    if access.is_on(Service::Ssh) { ssh_problem(context) } else { None }
}

/// Changes one choice, as the settings page and `guesthouse-vm set` do. Turning SSH off needs nothing
/// from this PC, and a machine that has it can always lose it.
pub fn choose(context: &Context, machine: &Machine, service: Service, on: bool) -> Result<()> {
    if machine.kind != Kind::Windows {
        return Err(Failure(format!("{} {ONLY_WINDOWS}", service.key())));
    }
    if service == Service::Ssh
        && on
        && let Some(problem) = ssh_problem(context)
    {
        return Err(Failure::new(problem));
    }
    let mut access = read_access(context, machine);
    access.set(service, on);
    write_access(context, machine, &access)
}

/// When a change reaches Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaches {
    /// The machine is running with nothing else going on, so the apply task can take it there.
    Now,
    /// The install takes it there.
    AtInstall,
    AtNextStart,
}

impl Reaches {
    pub fn text(self) -> &'static str {
        match self {
            Self::Now => "Windows gets it now.",
            Self::AtInstall => "Windows gets it when it is installed.",
            Self::AtNextStart => "Windows gets it at the next start.",
        }
    }
}

pub fn when_it_reaches(context: &Context, machine: &Machine) -> Reaches {
    reaches_by(machine, &status(context, machine))
}

/// The same, from what is already known of the machine, as the settings page has it.
pub fn reaches_by(machine: &Machine, found: &Status) -> Reaches {
    if machine.installing {
        Reaches::AtInstall
    } else if found.state == State::Running && !found.busy {
        Reaches::Now
    } else {
        Reaches::AtNextStart
    }
}

/// A PowerShell word between single quotes.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The host user's public keys, one a line, as `~/.ssh/id_*.pub` has them.
pub fn host_public_keys(home: &Path) -> String {
    let Ok(entries) = fs::read_dir(home.join(".ssh")) else { return String::new() };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with(PUBLIC_KEY_START) && name.ends_with(PUBLIC_KEY_END) && path.is_file()
        })
        .collect();
    files.sort();
    let lines: Vec<String> = files
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .flat_map(|text| text.lines().map(str::trim).map(str::to_owned).collect::<Vec<_>>())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    lines.join("\n")
}

/// Where the SSH script finds the installer, as PowerShell reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installer {
    /// A guest without OpenSSH says so.
    NotGiven,
    /// Beside the script, on the install disc.
    OnDisc,
    /// Where the agent put it.
    Sent,
}

impl Installer {
    fn expression(self) -> String {
        match self {
            Self::NotGiven => "''".to_owned(),
            Self::OnDisc => format!("Join-Path $PSScriptRoot {}", quoted(INSTALLER_ON_DISC)),
            Self::Sent => quoted(GUEST_INSTALLER),
        }
    }
}

/// The guest's script for this service with the choice above it, as the agent runs it and as it goes
/// on the install disc.
pub fn script_for(context: &Context, service: Service, on: bool, installer: Installer) -> Result<String> {
    let script = guest_script(context, service.script_name())?;
    let mut settings = format!("$on = ${on}\n");
    if service == Service::Ssh {
        let keys = base64_encode(host_public_keys(&context.paths.home).as_bytes());
        settings += &format!("$package = {}\n$keys = '{keys}'\n", installer.expression());
    }
    Ok(settings + &script)
}

/// The scripts the install disc gets for the machine's choices, each with its file name. With SSH on,
/// OpenSSH's installer goes beside them.
pub fn disc_scripts(context: &Context, access: &Access) -> Result<Vec<(&'static str, String)>> {
    if let Some(problem) = access_problem(context, access) {
        return Err(Failure(format!(
            "SSH is on for this machine. {problem} Turn SSH off for it to install."
        )));
    }
    access
        .chosen()
        .map(|(service, on)| {
            Ok((service.script_name(), script_for(context, service, on, Installer::OnDisc)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    fn windows(context: &Context) -> Machine {
        let machine = Machine::new("win", "Win", "/d", "/v");
        fs::create_dir_all(machine.folder(context)).unwrap();
        machine.save(context).unwrap();
        machine
    }

    #[test]
    fn a_machine_from_before_the_choice_has_none() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let old = windows(&context);
        assert_eq!(read_access(&context, &old), Access::default());
        assert_eq!(read_access(&context, &old).chosen().count(), 0, "so nothing in Windows is touched");
    }

    #[test]
    fn a_choice_is_kept_and_the_other_stays_unchosen() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let old = windows(&context);
        choose(&context, &old, Service::RemoteDesktop, true).unwrap();
        let access = read_access(&context, &old);
        assert_eq!(access, Access { remote_desktop: Some(true), ssh: None });
        let text = fs::read_to_string(access_file(&context, &old)).unwrap();
        assert!(!text.contains("ssh"), "{text}");
        choose(&context, &old, Service::RemoteDesktop, false).unwrap();
        assert_eq!(read_access(&context, &old).remote_desktop, Some(false));
    }

    #[test]
    fn the_machines_own_file_stays_as_an_older_build_reads_it() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = windows(&context);
        let before = fs::read_to_string(machine.folder(&context).join("machine.json")).unwrap();
        write_access(&context, &machine, &Access::NEW).unwrap();
        let after = fs::read_to_string(machine.folder(&context).join("machine.json")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn ssh_needs_the_installer_to_go_on_but_never_to_go_off() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = windows(&context);
        let refused = choose(&context, &machine, Service::Ssh, true).unwrap_err();
        assert_eq!(refused.0, NO_OPENSSH);
        assert_eq!(read_access(&context, &machine).ssh, None);
        choose(&context, &machine, Service::Ssh, false).unwrap();
        scratch.write("program/openssh/OpenSSH-Win64.msi", "installer");
        choose(&context, &machine, Service::Ssh, true).unwrap();
        fs::remove_file(context.paths.openssh_installer()).unwrap();
        choose(&context, &machine, Service::Ssh, false).unwrap();
        assert_eq!(read_access(&context, &machine).ssh, Some(false));
    }

    #[test]
    fn only_windows_has_these() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let plain = Machine { kind: Kind::Plain, ..windows(&context) };
        let refused = choose(&context, &plain, Service::RemoteDesktop, true).unwrap_err();
        assert!(refused.0.starts_with("remote-desktop is only for"), "{}", refused.0);
    }

    #[test]
    fn the_host_users_public_keys_are_read_and_nothing_else() {
        let scratch = Scratch::new();
        scratch.write("home/.ssh/id_ed25519.pub", "ssh-ed25519 AAAA me@pc\n");
        scratch.write("home/.ssh/id_rsa.pub", "\n# old\nssh-rsa BBBB me@pc\n");
        scratch.write("home/.ssh/id_ed25519", "PRIVATE");
        scratch.write("home/.ssh/authorized_keys", "ssh-ed25519 CCCC other@pc\n");
        let keys = host_public_keys(&scratch.path("home"));
        assert_eq!(keys, "ssh-ed25519 AAAA me@pc\nssh-rsa BBBB me@pc");
        assert_eq!(host_public_keys(&scratch.path("nobody")), "");
    }

    #[test]
    fn each_script_carries_the_choice_and_ssh_its_installer_and_keys() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        scratch.write("program/guest/remote-desktop.ps1", "turn it");
        scratch.write("program/guest/ssh.ps1", "set it up");
        scratch.write("home/.ssh/id_ed25519.pub", "ssh-ed25519 AAAA me@pc\n");
        let remote = script_for(&context, Service::RemoteDesktop, false, Installer::NotGiven).unwrap();
        assert_eq!(remote, "$on = $false\nturn it");
        let keys = base64_encode(b"ssh-ed25519 AAAA me@pc");
        let sent = script_for(&context, Service::Ssh, true, Installer::Sent).unwrap();
        assert_eq!(
            sent,
            format!("$on = $true\n$package = 'C:\\gpupv\\OpenSSH-Win64.msi'\n$keys = '{keys}'\nset it up")
        );
        let on_disc = script_for(&context, Service::Ssh, true, Installer::OnDisc).unwrap();
        assert!(on_disc.contains("$package = Join-Path $PSScriptRoot 'OpenSSH-Win64.msi'\n"), "{on_disc}");
        let asking = script_for(&context, Service::Ssh, true, Installer::NotGiven).unwrap();
        assert!(asking.contains("$package = ''\n"), "{asking}");
    }

    #[test]
    fn the_disc_gets_a_script_for_each_choice_and_refuses_ssh_it_cannot_bring() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        scratch.write("program/guest/remote-desktop.ps1", "turn it");
        scratch.write("program/guest/ssh.ps1", "set it up");
        assert_eq!(disc_scripts(&context, &Access::default()).unwrap(), Vec::new());
        let scripts = disc_scripts(&context, &Access::NEW).unwrap();
        let names: Vec<&str> = scripts.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["remote-desktop.ps1", "ssh.ps1"]);
        assert!(scripts[0].1.starts_with("$on = $true\n"));
        assert!(scripts[1].1.starts_with("$on = $false\n"));
        let with_ssh = Access { ssh: Some(true), ..Access::NEW };
        assert!(disc_scripts(&context, &with_ssh).unwrap_err().0.contains("Turn SSH off"));
        scratch.write("program/openssh/OpenSSH-Win64.msi", "installer");
        assert_eq!(disc_scripts(&context, &with_ssh).unwrap().len(), 2);
    }
}
