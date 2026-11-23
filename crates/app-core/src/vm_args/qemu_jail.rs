//! The jail around a GPU machine's QEMU, for Windows and for the forwarded GPU alike: what
//! `guesthouse-jail` lets it reach, and bubblewrap's namespaces around that where the PC allows them.
//!
//! The plan names this VM's files one by one, so the machine's folder, with its token and settings,
//! stays out, and so does every other VM. What the network may do follows the machine's mode: NAT
//! is the internet for the guest, so only binds are held to the forwarded ports.

use super::arguments::{MONITOR_SOCKETS, words};
use super::forwarding_jail::DEVICES as CARD_NODES;
use super::ubuntu::{FORWARDED_INPUT_SOCKET, FRAMES_SOCKET};
use super::{Boot, QemuJail, VmPlan};
use crate::disk_image::backing::backing_files;
use crate::failure::{Failure, Result};
use crate::network::{Mode, Protocol, bridge_helper};
use guesthouse_jail::launch::{Launch, Passt, Tap, UnixSocket};
use guesthouse_jail::{Grant, Network, Plan, Ports, system};
use std::path::{Path, PathBuf};

/// Where the launcher leaves QEMU's unix sockets, when it opens them: monitors first, then the rest.
const FIRST_UNIX_FD: i32 = 20;
/// The daemon's sockets, which a Windows guest's QEMU connects to.
pub(super) const GLOBAL_SOCKET: &str = "global.sock";
pub(super) const VGPU_SOCKET: &str = "vgpu.sock";
const DAEMON_SOCKETS: [&str; 2] = [GLOBAL_SOCKET, VGPU_SOCKET];
/// swtpm's, which QEMU connects to.
pub(super) const TPM_SOCKET: &str = "tpm.sock";
const RECONNECT_MS: u32 = 500;
/// QEMU's pid file, which names the run folder in its words when no monitor socket does.
pub const PID_FILE: &str = "qemu.pid";

/// Where a bridged network's device is open for QEMU. Out of the way of what QEMU opens itself.
pub const TAP_FD: i32 = 10;
/// Where QEMU's end of its connection to passt is open.
pub const PASST_FD: i32 = 11;
const PASST: &str = "passt";
const DNS_PORT: u16 = 53;
/// Any free port, which sending a datagram binds first.
const EPHEMERAL_PORT: u16 = 0;
/// QEMU's own filter, on top. Starting programs stays allowed, for the shared folder's.
/// Its privileges group stays out: it ends QEMU's user network's child at `setsid`, before the
/// shared folder's program runs, and no new privileges, which the launcher sets, already covers it.
const QEMU_SANDBOX: &str = "on,obsolete=deny,spawn=allow,resourcecontrol=deny";
/// The same with no programs started, for a QEMU that has none to start: an AMD card's on native
/// context, with no shared folder.
const QEMU_SANDBOX_NO_SPAWN: &str = "on,obsolete=deny,spawn=deny,resourcecontrol=deny";

/// What any QEMU reads, besides the system's programs. `/proc` and `/sys` give nothing away, as
/// Landlock doesn't let a jailed process look into one outside its jail.
const SYSTEM: [(Grant, &str); 11] = [
    (Grant::Read, "/etc/ld.so.cache"),
    (Grant::Read, system::LINKED_SETTINGS),
    (Grant::Read, "/etc/nsswitch.conf"),
    (Grant::Read, "/etc/passwd"),
    (Grant::Read, "/etc/group"),
    (Grant::Read, "/etc/hosts"),
    (Grant::Read, "/etc/resolv.conf"),
    (Grant::Read, "/etc/qemu"),
    (Grant::Read, "/run/systemd/resolve"),
    (Grant::Read, "/sys"),
    (Grant::Read, "/proc"),
];
const DEVICES: [&str; 5] = ["/dev/kvm", "/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"];
/// What bubblewrap's namespaces start from, before this VM's own files go in. The network stays, for
/// QEMU's user network. `--new-session` is left out so the app can still stop QEMU with its process
/// group, and the filter refuses typing into a terminal instead. The pid namespace is left out too:
/// in one, the viewer's pid reads as 0, and QEMU's D-Bus display then refuses to let it in. Landlock
/// still keeps QEMU from tracing what's outside its jail, and from signalling it where it has scopes.
const NAMESPACES: &str = "--unshare-user --unshare-ipc --unshare-uts --unshare-cgroup-try \
    --tmpfs /tmp --ro-bind /usr /usr --symlink usr/lib /lib --symlink usr/lib64 /lib64 --symlink usr/bin /bin \
    --symlink usr/sbin /sbin --proc /proc --dev /dev --ro-bind /sys /sys";
/// Given by its full path, see [`super::program_word`].
const BUBBLEWRAP: &str = "bwrap";
/// Already in the namespaces' base.
const IN_THE_BASE: [&str; 3] = ["/usr", "/proc", "/sys"];
/// A jailed QEMU's `PATH`, which `setarch`, `taskset` and `prlimit` in front of the launcher are also
/// found on. NixOS has them in its system's programs, which are among the folders a jail grants.
const PATH: &str = "/usr/bin:/usr/sbin:/run/current-system/sw/bin";

/// The folder above the one QEMU is in: a QEMU in its build folder finds its firmware and libraries
/// through links to there, and for the system's it's `/usr`.
fn qemu_tree(qemu: &Path) -> PathBuf {
    qemu.parent().and_then(Path::parent).unwrap_or(qemu).to_owned()
}

impl VmPlan {
    fn forwarded_ports(&self, protocol: Protocol) -> Vec<u16> {
        let forwards = self.network.forwards.iter().filter(|forward| forward.protocol == protocol);
        forwards.filter_map(|forward| u16::try_from(forward.host).ok()).collect()
    }

    /// Ubuntu's line is its own: SeaBIOS from QEMU's files, NAT with its SSH forward, and none of the
    /// machine's network or shared folder settings. Every other line has those, and boots OVMF, which
    /// keeps its settings in a file of the machine's.
    fn has_machine_line(&self) -> bool {
        !matches!(self.boot, Boot::Ubuntu(_) | Boot::UbuntuInstall(_))
    }

    /// What QEMU's user network listens on: the agent, the files helper and the forwards, or for
    /// Ubuntu its SSH.
    fn listened_ports(&self) -> Vec<u16> {
        if !self.has_machine_line() {
            return vec![self.ssh_port()];
        }
        let mut ports = vec![self.agent_port];
        ports.extend(self.files_port);
        ports.extend(self.forwarded_ports(Protocol::Tcp));
        ports
    }

    /// passt does the NAT for a machine on it, outside the jail, so QEMU's own network is the
    /// restricted private link, as for a bridged or restricted machine. Ubuntu's line is NAT.
    fn jail_network(&self) -> Network {
        let nat = !self.has_machine_line() || (self.network.mode == Mode::Nat && !self.network.passt);
        let mut tcp_bind = self.listened_ports();
        // The user network reaches the shared folder's program through a pair of loopback TCP
        // sockets it makes for each connection, on a port it can't know beforehand.
        if self.shared_folder().is_some() {
            tcp_bind.push(EPHEMERAL_PORT);
        }
        let tcp_bind = Ports::Only(tcp_bind);
        if nat {
            return Network { tcp_bind, tcp_connect: Ports::Any, udp_bind: Ports::Any, udp_send: Ports::Any };
        }
        // A restricted user network only answers forwards, and asks the host's resolver for names.
        let udp_forwards = self.forwarded_ports(Protocol::Udp);
        // A UDP forward answers to whatever port the program on this PC sent from, such as
        // Moonlight's for Sunshine's video, so its replies can go to any port. The guest still
        // can't start anything of its own: the restricted network drops it.
        let udp_send = if udp_forwards.is_empty() { Ports::Only(vec![DNS_PORT]) } else { Ports::Any };
        let mut udp_bind = vec![EPHEMERAL_PORT];
        udp_bind.extend(udp_forwards);
        let tcp_connect = if self.shared_folder().is_some() { Ports::Any } else { Ports::none() };
        Network { tcp_bind, tcp_connect, udp_bind: Ports::Only(udp_bind), udp_send }
    }

    fn firmware_code(&self) -> &Path {
        if let Some(boot) = self.macos_boot() {
            return &boot.firmware_code;
        }
        if self.tpm.is_some() { &self.firmware.code_secure } else { &self.firmware.code }
    }

    fn shared_folder(&self) -> Option<&String> {
        self.share.as_ref().filter(|_| self.has_machine_line())
    }

    /// Every path QEMU may reach, and its network.
    pub fn qemu_jail_plan(&self, jail: &QemuJail) -> Plan {
        self.qemu_jail_plan_under(jail, Path::new(CARD_NODES))
    }

    /// The same, with the card's nodes looked for under `devices`.
    pub(super) fn qemu_jail_plan_under(&self, jail: &QemuJail, devices: &Path) -> Plan {
        let mut plan = Plan { network: self.jail_network(), ..Plan::default() };
        system::grant_programs(&mut plan);
        for (grant, path) in SYSTEM {
            plan.grant(grant, path);
        }
        plan.grant(Grant::Run, qemu_tree(&self.qemu));
        if let Some(folder) = &self.qemu_files {
            plan.grant(Grant::Read, folder);
        }
        if self.has_machine_line() {
            plan.grant(Grant::Read, self.firmware_code()).grant(Grant::Write, &self.vars);
        }
        // A disc chosen while it runs is outside the jail, and the guest sees an empty drive.
        if let Boot::Plain { iso } | Boot::OwnLinux { iso, .. } = &self.boot
            && !iso.is_empty()
        {
            plan.grant(Grant::Read, iso);
        }
        for device in DEVICES {
            plan.grant(Grant::Device, device);
        }
        plan.grant(Grant::Write, &self.disk);
        if self.is_gpu()
            && let Some(disc) = &self.tools_disc
        {
            plan.grant(Grant::Read, disc);
        }
        self.grant_forwarding(&mut plan, devices);
        self.grant_amd(&mut plan);
        self.grant_macos(&mut plan);
        if let Some(folder) = self.shared_folder() {
            plan.grant(Grant::Run, &self.share_program).grant(Grant::Own, folder);
        }
        if let Some(socket) = &jail.sound_socket {
            plan.grant(Grant::Socket, socket);
        }
        plan.grant(Grant::Own, &self.run_folder);
        // Last, so in namespaces each is bound read-only over anything granted above.
        for file in &jail.backing_files {
            plan.grant(Grant::Read, file);
        }
        plan
    }

    /// The jail with the files the disk is built on. Refuses a chain QEMU couldn't open, and a
    /// backing file in the shared folder, where QEMU may write anything.
    pub fn with_backing_files(&self, jail: QemuJail) -> Result<QemuJail> {
        let backing_files = backing_files(Path::new(&self.disk))?;
        let shared = |file: &&PathBuf| self.shared_folder().is_some_and(|folder| file.starts_with(folder));
        if let Some(file) = backing_files.iter().find(shared) {
            return Err(Failure(format!(
                "The VM did not start, as its disk is built on {}, which is in the shared folder. \
                 Move it out, as the guest can write anything in there.",
                file.display()
            )));
        }
        Ok(QemuJail { backing_files, ..jail })
    }

    fn passt(&self) -> Option<Passt> {
        let wanted = self.has_machine_line() && self.network.mode == Mode::Nat && self.network.passt;
        wanted.then(|| Passt {
            program: super::program_on_path(PASST).unwrap_or_else(|| PASST.into()),
            fd: PASST_FD,
            pid_file: self.run_folder.join(super::PASST_PID_FILE),
            words: self.passt_words(),
            only_through: None,
        })
    }

    fn tap(&self) -> Option<Tap> {
        (self.has_machine_line() && self.network.mode == Mode::Bridged).then(|| Tap {
            helper: bridge_helper().into(),
            bridge: self.network.bridge.clone(),
            fd: TAP_FD,
        })
    }

    /// Whether this QEMU runs in bubblewrap's namespaces as well.
    pub fn in_namespaces(&self, jail: &QemuJail) -> bool {
        jail.namespaces
    }

    /// bubblewrap's words, with each path of the plan bound at its own place, so the plan means the
    /// same inside.
    fn namespace_words(plan: &Plan, launcher: &Path) -> Vec<String> {
        let mut words = vec![super::program_word(BUBBLEWRAP)];
        words.extend(NAMESPACES.split_whitespace().map(str::to_owned));
        let bind = |kind: &str, path: &Path| {
            [kind.to_owned(), path.display().to_string(), path.display().to_string()]
        };
        words.extend(bind("--ro-bind", launcher));
        for (grant, path) in &plan.paths {
            // A listed folder's devices are bound one at a time, and it's made around them.
            if *grant == Grant::List || IN_THE_BASE.iter().any(|base| path.starts_with(base)) {
                continue;
            }
            let kind = match grant {
                Grant::Read | Grant::Run => "--ro-bind-try",
                Grant::Write | Grant::Own | Grant::Socket => "--bind-try",
                Grant::Device => "--dev-bind-try",
                Grant::List => unreachable!("skipped above"),
            };
            words.extend(bind(kind, path));
        }
        words.push("--".to_owned());
        words
    }

    /// What goes in front of QEMU with no jail: nothing, but for a machine held to one interface, whose
    /// passt the launcher starts and guards, as it opens passt and runs QEMU as it is.
    fn unjailed_words(&self, before_jail: &[String]) -> Vec<String> {
        let Some(passt) = self.network.only_through.as_ref().and_then(|_| self.passt()) else {
            return before_jail.to_vec();
        };
        let opening = Launch {
            plan: None,
            passt: Some(passt),
            tap: None,
            unix_sockets: Vec::new(),
            no_new_unix_sockets: false,
            reads_own_memory: false,
            program: Vec::new(),
        };
        let mut words = vec![self.launcher.display().to_string()];
        words.extend(opening.words());
        words.extend(before_jail.iter().cloned());
        words
    }

    /// What goes in front of QEMU for a jailed GPU machine, up to the program's own words, with
    /// `before_jail` right before the launcher that jails it. In namespaces, passt and a bridged
    /// network's helper go before bubblewrap: passt has a sandbox of its own, and the helper has to
    /// gain its privileges, which it can't inside. So the launcher runs twice, once to start those
    /// and once inside to jail QEMU.
    pub(super) fn jail_words(&self, before_jail: &[String]) -> Vec<String> {
        let Some(jail) = &self.jail else { return self.unjailed_words(before_jail) };
        let plan = self.qemu_jail_plan(jail);
        let mut jailing = Launch {
            plan: Some(plan.clone()),
            passt: self.passt(),
            tap: self.tap(),
            unix_sockets: self.unix_sockets(),
            no_new_unix_sockets: self.passes_unix_sockets(),
            reads_own_memory: self.macos_boot().is_some(),
            program: Vec::new(),
        };
        let launcher = jail.launcher.display().to_string();
        let mut words = Vec::new();
        if self.in_namespaces(jail) {
            if jailing.passt.is_some() || jailing.tap.is_some() {
                let opening = Launch {
                    plan: None,
                    passt: jailing.passt.take(),
                    tap: jailing.tap.take(),
                    unix_sockets: Vec::new(),
                    no_new_unix_sockets: false,
                    reads_own_memory: false,
                    program: Vec::new(),
                };
                words.push(launcher.clone());
                words.extend(opening.words());
            }
            words.extend(Self::namespace_words(&plan, &jail.launcher));
        }
        words.extend(before_jail.iter().cloned());
        words.push(launcher);
        words.extend(jailing.words());
        words
    }

    /// The launcher opens every unix socket QEMU needs, and QEMU may make none, so a QEMU that's been
    /// taken over can't reach the session bus or any other socket of the desktop's, whatever the
    /// kernel's Landlock and with or without namespaces. Only with no sound, as PipeWire's client
    /// makes its own socket.
    pub(super) fn passes_unix_sockets(&self) -> bool {
        self.jail.as_ref().is_some() && self.can_pass_unix_sockets()
    }

    /// Whether a jail would hand this QEMU its unix sockets.
    pub fn can_pass_unix_sockets(&self) -> bool {
        self.sound.is_none()
    }

    /// The unix sockets the launcher opens for QEMU, if it opens them, each at its descriptor. All are
    /// in the run folder: the monitors, the daemon's for a Windows guest, the virtual TPM's, and for
    /// the forwarded GPU the guest's input and the socket its frames are offered on.
    fn unix_sockets(&self) -> Vec<UnixSocket> {
        if !self.passes_unix_sockets() {
            return Vec::new();
        }
        let listening = |file: &'static str| (file, true);
        let connecting = |file: &'static str| (file, false);
        let mut files: Vec<(&str, bool)> = MONITOR_SOCKETS.into_iter().map(listening).collect();
        if self.is_gpu() {
            files.extend(DAEMON_SOCKETS.map(connecting));
        }
        // Ubuntu's own line has no TPM.
        if self.tpm.is_some() && self.has_machine_line() {
            files.push(connecting(TPM_SOCKET));
        }
        if self.has_forwarded_gpu() {
            // A Linux the user installed keeps QEMU's own keyboard and pointer.
            if !self.has_machine_line() {
                files.push(listening(FORWARDED_INPUT_SOCKET));
            }
            files.push(listening(FRAMES_SOCKET));
        }
        let at_descriptors = files.into_iter().zip(FIRST_UNIX_FD..);
        at_descriptors
            .map(|((file, listen), fd)| UnixSocket { path: self.run_folder.join(file), fd, listen })
            .collect()
    }

    /// Where the launcher left this socket of QEMU's, if it opened it.
    pub(super) fn socket_fd(&self, file: &str) -> Option<i32> {
        let path = self.run_folder.join(file);
        self.unix_sockets().into_iter().find(|socket| socket.path == path).map(|socket| socket.fd)
    }

    /// QEMU's monitors: on the descriptors the launcher opened, or at their paths.
    pub(super) fn monitor_arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        for (at, file) in MONITOR_SOCKETS.into_iter().enumerate() {
            arguments.extend(match self.socket_fd(file) {
                Some(fd) => words(&[
                    "-chardev",
                    &format!("socket,id=monitor{at},fd={fd},server=on,wait=off"),
                    "-mon",
                    &format!("chardev=monitor{at},mode=control"),
                ]),
                None => words(&["-qmp", &format!("unix:{},server=on,wait=off", self.in_run_folder(file))]),
            });
        }
        arguments
    }

    /// A socket QEMU connects to: on the launcher's descriptor, or by its path.
    pub(super) fn client_socket(&self, id: &str, file: &str) -> String {
        let path = self.in_run_folder(file);
        match self.socket_fd(file) {
            Some(fd) => format!("socket,id={id},fd={fd}"),
            None if file == TPM_SOCKET => format!("socket,id={id},path={path}"),
            None => format!("socket,id={id},path={path},reconnect-ms={RECONNECT_MS}"),
        }
    }

    /// A socket QEMU listens on for a device of its own: on the launcher's descriptor, or at its path.
    pub(super) fn listening_socket(&self, id: &str, file: &str) -> String {
        match self.socket_fd(file) {
            Some(fd) => format!("socket,id={id},fd={fd},server=on,wait=off"),
            None => format!("socket,id={id},path={},server=on,wait=off", self.in_run_folder(file)),
        }
    }

    /// QEMU's own filter, if this QEMU has one.
    pub(super) fn qemu_sandbox_arguments(&self) -> Vec<String> {
        match &self.jail {
            Some(jail) if jail.qemu_sandbox => {
                let spawns =
                    !self.has_amd_gpu() || self.starts_render_server() || self.shared_folder().is_some();
                let sandbox = if spawns { QEMU_SANDBOX } else { QEMU_SANDBOX_NO_SPAWN };
                vec!["-sandbox".to_owned(), sandbox.to_owned()]
            }
            _ => Vec::new(),
        }
    }

    /// A jailed QEMU starts with only the environment it needs, as the user's may hold secrets. Its
    /// temporary files go in its run folder, as the PC's `/tmp` is out of its reach.
    pub(super) fn jailed_environment(&self, jail: &QemuJail) -> Vec<(String, String)> {
        let mut environment = vec![
            ("PATH".to_owned(), PATH.to_owned()),
            ("TMPDIR".to_owned(), self.run_folder.display().to_string()),
        ];
        if let Some(folder) = jail.sound_socket.as_deref().and_then(Path::parent) {
            environment.push(("XDG_RUNTIME_DIR".to_owned(), folder.display().to_string()));
        }
        environment
    }
}

#[cfg(test)]
mod tests;
